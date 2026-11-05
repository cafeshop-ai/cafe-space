//! Owned offline fake process through the production codec/API and real private local socket/pipe.
use super::*;
use cafe_space_ipc::{
    BuildFlavor,
    framing::{read_frame, write_frame},
    local,
};
use cafe_space_protocol::*;
use cafe_spaced::{api, host::Host, state::State, update::Access};
use pi_commands_fixture::{response, view};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

async fn send(stream: &mut local::ClientStream, id: &str, request: Request) {
    let frame = encode_client_message(&ClientMessage::Request(ClientRequest {
        message_type: RequestMessageType::Request,
        protocol_version: PROTOCOL_VERSION,
        request_id: id.into(),
        request,
    }))
    .unwrap();
    write_frame(stream, &frame).await.unwrap();
}

async fn receive(stream: &mut local::ClientStream, host: &Host, expired: bool) -> ServerMessage {
    let incoming = read_frame(stream);
    tokio::pin!(incoming);
    let deadline = tokio::time::sleep(DEADLINE);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            frame = &mut incoming => {
                let bytes = frame.unwrap().expect("connection closed before response");
                assert!(!String::from_utf8_lossy(&bytes).contains("never-forward"));
                return decode_server_message(&bytes).unwrap();
            }
            _ = tokio::time::sleep(Duration::from_millis(5)) => {
                host.poll_processes(Instant::now() + if expired { Duration::from_secs(31) } else { Duration::ZERO }).unwrap();
            }
            _ = &mut deadline => panic!("model wire fixture timed out"),
        }
    }
}

fn result(frame: ServerMessage, id: &str) -> RequestResult {
    let ServerMessage::Response(frame) = frame else {
        panic!("expected response {id}: {frame:?}")
    };
    assert_eq!(frame.request_id, id);
    frame.result
}

fn error(result: RequestResult, expected: &str, unknown: bool) {
    let value = match result {
        RequestResult::Rejected(value) if !unknown => value,
        RequestResult::Unknown(value) if unknown => value,
        other => panic!("wrong query outcome: {other:?}"),
    };
    assert_eq!(value.error.code, expected);
}

pub fn run() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(cases());
}

async fn cases() {
    let contract: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/pi-rpc/model-list-wire-v5.json"
    ))
    .unwrap();
    for case in contract["wireCases"].as_array().unwrap() {
        let case = case.as_str().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join(BuildFlavor::Dev.directory_name());
        let cwd = temp.path().join("project");
        std::fs::create_dir(&cwd).unwrap();
        let host = Arc::new(
            Host::new(
                State::open(&data, BuildFlavor::Dev).unwrap(),
                &data,
                "models-wire".into(),
                "a".repeat(64),
            )
            .unwrap(),
        );
        let Response::Project(project) = response(
            &host,
            Request::AddProject(AddProjectParams {
                name: Some("Synthetic".into()),
                root: cwd.to_str().unwrap().into(),
            }),
        ) else {
            panic!("project")
        };
        let params = CreateSessionParams {
            session_id: "host-commands".into(),
            project_id: project.id,
            runtime_id: "pi".into(),
            expected_cwd_fence: project.cwd_fence,
            start: SessionStart::New,
        };
        let runtime = RuntimeSummary {
            id: "pi".into(),
            name: "Synthetic".into(),
            version: None,
            available: true,
            tool_approval: ToolApproval::NeverAsks,
        };
        let log = data.join("commands.log");
        let release = data.join("release-models");
        let mode = match case {
            "disconnect" | "event-before" | "root-replaced" => "models-held",
            "pending-command" => "no-read",
            _ => case,
        };
        let (node, mut args) =
            pi_fixture::command(&data.join("agent-sessions/pi-native-fixture-1"), mode);
        args.extend([
            "--fake-command-file".into(),
            log.as_os_str().to_owned(),
            "--fake-model-release".into(),
            release.as_os_str().to_owned(),
        ]);
        if case == "root-replaced" {
            args.push("--fake-release-cwd".into());
        }
        host.launch_pi_session(&params, &runtime, &node, &args, "native-fixture-1".into())
            .unwrap();
        until(|| {
            host.poll_processes(Instant::now()).unwrap();
            view(&host).session.state == SessionState::Ready
        });
        let session = view(&host).session;
        let query = OpenSessionParams {
            session_id: session.id.clone(),
            expected_stream_id: session.stream_id.clone().unwrap(),
            expected_cwd_fence: session.cwd_fence.clone(),
        };
        let before = response(&host, Request::ListSessions);
        #[cfg(windows)]
        let endpoint = std::path::PathBuf::from(format!(
            r"\\.\pipe\cafe-models-test-{}-{case}",
            std::process::id()
        ));
        #[cfg(unix)]
        let socket_dir = tempfile::tempdir_in("/tmp").unwrap();
        // TempDir need not be 0700; let Listener create its private child directory.
        #[cfg(unix)]
        let endpoint = socket_dir.path().join("private/models.sock");
        let mut listener = local::Listener::bind(&endpoint).await.unwrap();
        let mut client = local::connect(&endpoint).await.unwrap();
        let server = listener.accept().await.unwrap();
        let server_host = host.clone();
        let serving =
            tokio::spawn(async move { api::serve_connection(server, &server_host).await });
        write_frame(
            &mut client,
            &encode_client_message(&ClientMessage::Hello(ClientHello {
                message_type: HelloMessageType::Hello,
                supported_protocol_versions: vec![PROTOCOL_VERSION],
                client_version: "fixture".into(),
            }))
            .unwrap(),
        )
        .await
        .unwrap();
        assert!(matches!(
            receive(&mut client, &host, false).await,
            ServerMessage::Welcome(_)
        ));
        // Admission failures must not write native stdin or hide a valid local session behind remote access.
        error(
            host.dispatch(Access::Remote, 0, Request::ListModels(query.clone()))
                .unwrap()
                .result,
            "LOCAL_ONLY",
            false,
        );
        for (field, code) in [
            ("stream", "STALE_STREAM"),
            ("cwd", "STALE_CWD_FENCE"),
            ("session", "SESSION_NOT_FOUND"),
        ] {
            let mut stale = query.clone();
            match field {
                "stream" => stale.expected_stream_id = "stale".into(),
                "cwd" => stale.expected_cwd_fence = "stale".into(),
                _ => stale.session_id = "missing".into(),
            }
            send(&mut client, "stale", Request::ListModels(stale)).await;
            error(
                result(receive(&mut client, &host, false).await, "stale"),
                code,
                false,
            );
        }
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "get_state\n");
        if matches!(case, "busy" | "pending-command") {
            let command = if case == "busy" {
                pi_commands_fixture::prompt(&session, "busy", DeliveryMode::FollowUp)
            } else {
                Request::SetModel(SetModelParams {
                    session_id: session.id.clone(),
                    expected_stream_id: query.expected_stream_id.clone(),
                    expected_cwd_fence: query.expected_cwd_fence.clone(),
                    provider: "synthetic".into(),
                    model_id: "tea".into(),
                })
            };
            let mut pending = host.command_pi(Access::Local, command).unwrap();
            if case == "busy" {
                pi_commands_fixture::dispatched(pi_commands_fixture::receive(
                    &host,
                    &mut pending,
                    false,
                ));
            }
            send(&mut client, "models", Request::ListModels(query)).await;
            error(
                result(receive(&mut client, &host, false).await, "models"),
                if case == "busy" {
                    "PI_NOT_READY"
                } else {
                    "PI_COMMAND_PENDING"
                },
                false,
            );
            assert!(
                !std::fs::read_to_string(&log)
                    .unwrap()
                    .contains("get_available_models")
            );
            client.shutdown().await.unwrap();
            drop(client);
            tokio::time::timeout(DEADLINE, serving)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            host.stop_owned_processes().unwrap();
            println!(
                "M1_PI_MODELS_WIRE_OK case={case} nativeIpc=true modelCalls=0 productionDisabled=true"
            );
            continue;
        }
        let events = matches!(case, "event-before" | "event-after");
        if events {
            send(
                &mut client,
                "subscribe",
                Request::SubscribeEvents(SubscribeEventsParams { after: None }),
            )
            .await;
            assert!(matches!(
                result(receive(&mut client, &host, false).await, "subscribe"),
                RequestResult::Applied(_)
            ));
        }
        send(&mut client, "models", Request::ListModels(query.clone())).await;
        if matches!(
            case,
            "models-timeout" | "models-close" | "disconnect" | "event-before" | "root-replaced"
        ) {
            send(&mut client, "pending", Request::ListModels(query.clone())).await;
            error(
                result(receive(&mut client, &host, false).await, "pending"),
                "PI_COMMAND_PENDING",
                false,
            );
            send(&mut client, "health", Request::GetStatus).await;
            assert!(matches!(
                result(receive(&mut client, &host, false).await, "health"),
                RequestResult::Applied(_)
            ));
            // A held native query still permits the connection to receive unrelated frames.
            until(|| {
                host.poll_processes(Instant::now()).unwrap();
                std::fs::read_to_string(&log)
                    .unwrap()
                    .contains("get_available_models\n")
            });
        }
        if events {
            // Both orders are forced, not left to scheduler luck. A partial inbound frame survives either delivery.
            if case == "event-after" {
                let RequestResult::Applied(value) =
                    result(receive(&mut client, &host, false).await, "models")
                else {
                    panic!("models")
                };
                assert!(matches!(value.result, Response::Models(_)));
            }
            let frame = encode_client_message(&ClientMessage::Request(ClientRequest {
                message_type: RequestMessageType::Request,
                protocol_version: PROTOCOL_VERSION,
                request_id: "partial".into(),
                request: Request::GetStatus,
            }))
            .unwrap();
            let mut bytes = (frame.len() as u32).to_be_bytes().to_vec();
            bytes.extend(frame);
            client.write_all(&bytes[..2]).await.unwrap();
            response(
                &host,
                Request::UpdateProject(UpdateProjectParams {
                    project_id: params.project_id.clone(),
                    expected_cwd_fence: params.expected_cwd_fence.clone(),
                    name: Some("Renamed synthetic".into()),
                    root: None,
                }),
            );
            assert!(matches!(
                receive(&mut client, &host, false).await,
                ServerMessage::Event(ServerEvent {
                    event: Event::ProjectChanged(_),
                    ..
                })
            ));
            if case == "event-before" {
                std::fs::write(&release, b"release").unwrap();
                let RequestResult::Applied(value) =
                    result(receive(&mut client, &host, false).await, "models")
                else {
                    panic!("models")
                };
                assert!(matches!(value.result, Response::Models(_)));
            }
            client.write_all(&bytes[2..]).await.unwrap();
            assert!(matches!(
                result(receive(&mut client, &host, false).await, "partial"),
                RequestResult::Applied(_)
            ));
        } else if case == "disconnect" {
            client.shutdown().await.unwrap();
            drop(client);
            tokio::time::timeout(DEADLINE, serving)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            // Disconnect drops the reply, not the in-flight native query, and does not replay it.
            assert_eq!(
                host.list_pi_models(Access::Local, query.clone())
                    .unwrap_err()
                    .to_string(),
                "PI_COMMAND_PENDING"
            );
            std::fs::write(&release, b"release").unwrap();
            until(|| {
                host.poll_processes(Instant::now()).unwrap();
                host.command_pi(
                    Access::Local,
                    pi_commands_fixture::prompt(&session, "/handled", DeliveryMode::FollowUp),
                )
                .is_ok()
            });
            assert_eq!(
                std::fs::read_to_string(&log)
                    .unwrap()
                    .lines()
                    .filter(|v| *v == "get_available_models")
                    .count(),
                1
            );
            host.stop_owned_processes().unwrap();
            println!(
                "M1_PI_MODELS_WIRE_OK case={case} nativeIpc=true modelCalls=0 productionDisabled=true"
            );
            continue;
        } else {
            if case == "root-replaced" {
                // Windows pins a process's cwd; the fake agent releases it before this fault injection.
                until(|| data.join("release-models.waiting").exists());
                std::fs::rename(&cwd, temp.path().join("old-project")).unwrap();
                std::fs::create_dir(&cwd).unwrap();
                std::fs::write(&release, b"release").unwrap();
            }
            if case == "models-close" {
                send(
                    &mut client,
                    "close",
                    Request::CloseSession(CloseSessionParams {
                        session_id: session.id.clone(),
                        expected_stream_id: query.expected_stream_id.clone(),
                        expected_cwd_fence: query.expected_cwd_fence.clone(),
                    }),
                )
                .await;
                assert!(matches!(
                    result(receive(&mut client, &host, false).await, "close"),
                    RequestResult::Dispatched(_)
                ));
            }
            let outcome = result(
                receive(&mut client, &host, case == "models-timeout").await,
                "models",
            );
            match case {
                "models" | "models-empty" => {
                    let RequestResult::Applied(value) = outcome else {
                        panic!("models: {outcome:?}")
                    };
                    let Response::Models(list) = value.result else {
                        panic!("model list")
                    };
                    let fixture: serde_json::Value = serde_json::from_str(include_str!(
                        "../../../../fixtures/pi-rpc/models.json"
                    ))
                    .unwrap();
                    assert_eq!(
                        serde_json::to_value(list.items).unwrap(),
                        if case == "models-empty" {
                            serde_json::json!([])
                        } else {
                            fixture["expected"].clone()
                        }
                    );
                    assert_eq!(response(&host, Request::ListSessions), before);
                    assert_eq!(view(&host).session, session);
                }
                "models-rejected" => error(outcome, "PI_COMMAND_REJECTED", false),
                "models-invalid" => error(outcome, "PI_COMMAND_OUTCOME_UNKNOWN", true),
                "models-timeout" => error(outcome, "PI_RESPONSE_TIMEOUT", true),
                "models-close" => error(outcome, "PI_CONNECTION_CLOSED", true),
                "root-replaced" => error(outcome, "PROJECT_ROOT_CHANGED", false),
                _ => unreachable!(),
            }
        }
        assert_eq!(
            std::fs::read_to_string(&log)
                .unwrap()
                .lines()
                .filter(|v| *v == "get_available_models")
                .count(),
            1
        );
        client.shutdown().await.unwrap();
        drop(client);
        tokio::time::timeout(DEADLINE, serving)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        host.stop_owned_processes().unwrap();
        println!(
            "M1_PI_MODELS_WIRE_OK case={case} nativeIpc=true modelCalls=0 productionDisabled=true"
        );
    }
}
