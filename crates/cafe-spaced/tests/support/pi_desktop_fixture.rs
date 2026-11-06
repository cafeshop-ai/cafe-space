//! Test-only, one-connection Host server. The only executable target is the existing fake Pi.
use super::*;
use cafe_space_ipc::{
    BuildFlavor,
    framing::{read_frame, write_frame},
    local,
};
use cafe_space_protocol::*;
use cafe_spaced::{api, host::Host, state::State};
use pi_commands_fixture::{dispatched, prompt, response, view};
use pi_models_wire_fixture::{result, send};
use std::sync::Arc;

struct Fixture {
    // Host must release its owned process and database before temporary paths disappear.
    host: Arc<Host>,
    _temp: tempfile::TempDir,
    #[cfg(unix)]
    _socket_dir: tempfile::TempDir,
    listener: local::Listener,
    endpoint: std::path::PathBuf,
    log: std::path::PathBuf,
    pid: u32,
    contract: serde_json::Value,
}

impl Fixture {
    async fn new() -> Self {
        let contract: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../fixtures/pi-rpc/desktop-host-v5.json"
        ))
        .unwrap();
        assert_eq!(contract["fullProtocolVersion"], PROTOCOL_VERSION);
        assert_eq!(contract["maxConnections"], 1);
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join(BuildFlavor::Dev.directory_name());
        let cwd = temp.path().join("project");
        std::fs::create_dir(&cwd).unwrap();
        let host = Arc::new(
            Host::new(
                State::open(&data, BuildFlavor::Dev).unwrap(),
                &data,
                "desktop-fixture".into(),
                "a".repeat(64),
            )
            .unwrap(),
        );
        let Response::Project(project) = response(
            &host,
            Request::AddProject(AddProjectParams {
                name: Some("Synthetic desktop".into()),
                root: cwd.to_str().unwrap().into(),
            }),
        ) else {
            panic!("project")
        };
        let params = CreateSessionParams {
            session_id: contract["sessionId"].as_str().unwrap().into(),
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
        let pid_file = data.join("agent.pid");
        let (node, mut args) =
            pi_fixture::command(&data.join("agent-sessions/pi-native-fixture-1"), "normal");
        args.extend([
            "--fake-command-file".into(),
            log.as_os_str().to_owned(),
            "--fake-pid-file".into(),
            pid_file.as_os_str().to_owned(),
        ]);
        host.launch_pi_session(&params, &runtime, &node, &args, "native-fixture-1".into())
            .unwrap();
        until(|| {
            host.poll_processes(Instant::now()).unwrap();
            view(&host).session.state == SessionState::Ready
        });
        #[cfg(windows)]
        let endpoint = std::path::PathBuf::from(format!(
            r"\\.\pipe\cafe-desktop-fixture-{}-{}",
            std::process::id(),
            temp.path().file_name().unwrap().to_str().unwrap()
        ));
        #[cfg(unix)]
        let socket_dir = tempfile::tempdir_in("/tmp").unwrap();
        #[cfg(unix)]
        let endpoint = socket_dir.path().join("private/desktop.sock");
        let listener = local::Listener::bind(&endpoint).await.unwrap();
        Self {
            host,
            _temp: temp,
            #[cfg(unix)]
            _socket_dir: socket_dir,
            listener,
            endpoint,
            log,
            pid: read_pid(&pid_file),
            contract,
        }
    }

    async fn serve(&mut self) -> std::io::Result<()> {
        let served = tokio::time::timeout(
            Duration::from_secs(self.contract["deadlineSeconds"].as_u64().unwrap()),
            async {
                let stream = self.listener.accept().await?;
                let connection = api::serve_connection(stream, &self.host);
                tokio::pin!(connection);
                let mut tick = tokio::time::interval(Duration::from_millis(5));
                loop {
                    tokio::select! {
                        result = &mut connection => return result.map(|_| ()),
                        _ = tick.tick() => { self.host.poll_processes(Instant::now())?; }
                    }
                }
            },
        )
        .await;
        let cleanup = self.host.stop_owned_processes();
        served.map_err(|_| std::io::Error::other("DESKTOP_FIXTURE_TIMEOUT"))??;
        cleanup
    }
}

async fn receive(client: &mut local::ClientStream) -> ServerMessage {
    let bytes = tokio::time::timeout(DEADLINE, read_frame(client))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    decode_server_message(&bytes).unwrap()
}

pub fn serve() -> std::io::Result<std::process::ExitCode> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let mut fixture = Fixture::new().await;
            // Local test runner metadata only; no native IDs, credentials or production endpoint.
            println!(
                "{}",
                serde_json::json!({
                    "marker": "M1_DESKTOP_HOST_FIXTURE_READY",
                    "endpoint": fixture.endpoint,
                    "session": view(&fixture.host).session,
                })
            );
            std::io::stdout().flush()?;
            fixture.serve().await?;
            until(|| !running(fixture.pid));
            let log = std::fs::read_to_string(&fixture.log)?;
            println!(
                "{}",
                serde_json::json!({
                    "marker": "M1_DESKTOP_HOST_FIXTURE_STOPPED",
                    "promptCount": log.lines().filter(|line| *line == "prompt").count(),
                })
            );
            Ok(std::process::ExitCode::SUCCESS)
        })
}

pub fn check() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut fixture = Fixture::new().await;
        let session = view(&fixture.host).session;
        let endpoint = fixture.endpoint.clone();
        let content = fixture.contract["prompt"].as_str().unwrap().to_owned();
        let expected_text = fixture.contract["expectedText"].as_str().unwrap().to_owned();
        let client = async {
            let mut client = local::connect(&endpoint).await.unwrap();
            write_frame(&mut client, &encode_client_message(&ClientMessage::Hello(ClientHello {
                message_type: HelloMessageType::Hello,
                supported_protocol_versions: vec![PROTOCOL_VERSION], client_version: "desktop-fixture-check".into(),
            })).unwrap()).await.unwrap();
            assert!(matches!(receive(&mut client).await, ServerMessage::Welcome(_)));
            send(&mut client, "runtimes", Request::ListRuntimes).await;
            assert!(matches!(result(receive(&mut client).await, "runtimes"), RequestResult::Applied(value) if matches!(&value.result, Response::Runtimes(list) if list.items.is_empty())));
            send(&mut client, "subscribe", Request::SubscribeEvents(SubscribeEventsParams { after: None })).await;
            let RequestResult::Applied(value) = result(receive(&mut client).await, "subscribe") else { panic!("subscription rejected") };
            let Response::Subscription(subscription) = value.result else { panic!("subscription") };
            assert_eq!(subscription.mode, SubscriptionMode::SnapshotRequired);
            let snapshot = subscription.snapshot.unwrap();
            assert_eq!(snapshot.sessions, vec![session.clone()]);
            assert!(snapshot.runtimes.is_empty());
            send(&mut client, "prompt", prompt(&session, &content, DeliveryMode::Steer)).await;
            let (mut accepted, mut text, mut ready) = (false, false, false);
            while !accepted || !text || !ready {
                match receive(&mut client).await {
                    frame @ ServerMessage::Response(_) => { dispatched(result(frame, "prompt")); accepted = true; }
                    ServerMessage::Event(ServerEvent { event: Event::SessionChanged(data), .. }) => {
                        assert_eq!(data.session.id, session.id);
                        ready |= data.session.state == SessionState::Ready && data.session.active_run_id.is_none();
                    }
                    ServerMessage::Event(ServerEvent { event: Event::MessageChanged(data), .. }) => {
                        assert_eq!(data.session_id, session.id);
                        text |= data.message.status == TranscriptStatus::Complete && data.message.parts.iter().any(|part| matches!(part, MessagePart::Text(value) if value.text == expected_text));
                    }
                    other => panic!("unexpected desktop fixture frame: {other:?}"),
                }
            }
            send(&mut client, "close", Request::CloseSession(CloseSessionParams {
                session_id: session.id, expected_stream_id: session.stream_id.unwrap(), expected_cwd_fence: session.cwd_fence,
            })).await;
            let (mut closed, mut stopped) = (false, false);
            while !closed || !stopped {
                match receive(&mut client).await {
                    frame @ ServerMessage::Response(_) => { dispatched(result(frame, "close")); closed = true; }
                    ServerMessage::Event(ServerEvent { event: Event::SessionChanged(data), .. }) => { stopped |= data.session.state == SessionState::Stopped; }
                    other => panic!("unexpected fixture close frame: {other:?}"),
                }
            }
        };
        let (served, ()) = tokio::join!(fixture.serve(), client);
        served.unwrap();
        let log = std::fs::read_to_string(&fixture.log).unwrap();
        assert_eq!(log.lines().filter(|line| *line == "prompt").count(), 1);
        until(|| !running(fixture.pid));
        drop(fixture);
        let mut idle = Fixture::new().await;
        tokio::time::pause();
        let expired = async { tokio::time::advance(Duration::from_secs(61)).await; };
        let (served, ()) = tokio::join!(idle.serve(), expired);
        assert_eq!(served.unwrap_err().to_string(), "DESKTOP_FIXTURE_TIMEOUT");
        until(|| !running(idle.pid));
        tokio::time::resume();
        println!("M1_DESKTOP_HOST_FIXTURE_CHECK_OK nativeIpc=true timeoutCleanup=true modelCalls=0 productionDisabled=true");
    });
}
