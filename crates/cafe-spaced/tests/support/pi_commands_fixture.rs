//! Native Rust Host command integration over an owned fake process; never a model or production profile.
use super::*;
use cafe_space_ipc::BuildFlavor;
use cafe_space_protocol::*;
use cafe_spaced::{host::Host, state::State, update::Access};
use tokio::sync::oneshot;

pub(super) fn response(host: &Host, request: Request) -> Response {
    match host.dispatch(Access::Local, 1, request).unwrap().result {
        RequestResult::Applied(value) => value.result,
        other => panic!("expected applied: {other:?}"),
    }
}
pub(super) fn view(host: &Host) -> SessionView {
    let Response::Session(value) = response(
        host,
        Request::GetSession(GetSessionParams {
            session_id: "host-commands".into(),
        }),
    ) else {
        panic!("session")
    };
    value.session
}
pub(super) fn prompt(session: &SessionSnapshot, content: &str, delivery: DeliveryMode) -> Request {
    Request::Prompt(PromptParams {
        session_id: session.id.clone(),
        expected_stream_id: session.stream_id.clone().unwrap(),
        expected_cwd_fence: session.cwd_fence.clone(),
        content: content.into(),
        delivery,
    })
}
pub(super) fn cancel(session: &SessionSnapshot, run: &str) -> Request {
    Request::Cancel(CancelParams {
        session_id: session.id.clone(),
        expected_stream_id: session.stream_id.clone().unwrap(),
        expected_cwd_fence: session.cwd_fence.clone(),
        run_id: run.into(),
    })
}
pub(super) fn receive(
    host: &Host,
    received: &mut oneshot::Receiver<RequestResult>,
    expired: bool,
) -> RequestResult {
    let mut result = None;
    until(|| {
        host.poll_processes(
            Instant::now()
                + if expired {
                    Duration::from_secs(31)
                } else {
                    Duration::ZERO
                },
        )
        .unwrap();
        match received.try_recv() {
            Ok(value) => {
                result = Some(value);
                true
            }
            Err(oneshot::error::TryRecvError::Empty) => false,
            Err(oneshot::error::TryRecvError::Closed) => panic!("command result lost"),
        }
    });
    result.unwrap()
}
pub(super) fn dispatched(result: RequestResult) {
    let RequestResult::Dispatched(value) = result else {
        panic!("expected dispatch: {result:?}")
    };
    assert_eq!(value.operation_id.len(), 32);
    assert!(value.operation_id.bytes().all(|b| b.is_ascii_hexdigit()));
}

pub fn run() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/pi-rpc/host-commands.json"
    ))
    .unwrap();
    assert_eq!(
        fixture["cancel"],
        serde_json::json!([
            "validate owned run",
            "clear_queue acknowledged",
            "recheck same run",
            "abort"
        ])
    );
    let models: serde_json::Value =
        serde_json::from_str(include_str!("../../../../fixtures/pi-rpc/models.json")).unwrap();
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .chain(models["cases"].as_array().unwrap())
    {
        let case = case.as_str().unwrap();
        let mode = match case {
            value if value.starts_with("models") => value,
            "crash" | "malformed" | "rejected" | "unowned" => case,
            "no_response" | "close_pending" => "no-response",
            "settle_during_clear" => "settle-on-clear",
            "delayed_start" => "delayed-start",
            "start_without_ack" => "start-no-ack",
            "configuration_readback_rejected" => "readback-rejected",
            "configuration_readback_missing" => "readback-missing",
            "initial_configuration" => "initial-config",
            _ => "normal",
        };
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join(BuildFlavor::Dev.directory_name());
        let cwd = temp.path().join("project");
        std::fs::create_dir(&cwd).unwrap();
        let host = Host::new(
            State::open(&data, BuildFlavor::Dev).unwrap(),
            &data,
            "commands-boot".into(),
            "a".repeat(64),
        )
        .unwrap();
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
        let pid_file = data.join("agent.pid");
        let (node, mut args) =
            pi_fixture::command(&data.join("agent-sessions/pi-native-fixture-1"), mode);
        args.extend([
            "--fake-command-file".into(),
            log.as_os_str().to_owned(),
            "--fake-pid-file".into(),
            pid_file.as_os_str().to_owned(),
        ]);
        host.launch_pi_session(&params, &runtime, &node, &args, "native-fixture-1".into())
            .unwrap();
        if case == "unowned" {
            until(|| {
                host.poll_processes(Instant::now()).unwrap();
                matches!(view(&host).session.state, SessionState::Failed(ref failure) if failure.code == "PI_UNOWNED_RUN")
            });
            assert!(view(&host).session.active_run_id.is_none());
            let pid: u32 = std::fs::read_to_string(&pid_file).unwrap().parse().unwrap();
            until(|| !running(pid));
            println!(
                "M1_PI_HOST_COMMANDS_OK case=unowned modelCalls=0 unownedRunRejected=true processReaped=true productionDisabled=true"
            );
            continue;
        }
        until(|| {
            host.poll_processes(Instant::now()).unwrap();
            view(&host).session.state == SessionState::Ready
        });
        let session = view(&host).session;
        assert!(
            session.capabilities.delivery_control
                && session.capabilities.model_control
                && session.capabilities.thinking_control
        );
        match case {
            value if value.starts_with("models") => {
                let params = OpenSessionParams {
                    session_id: session.id.clone(),
                    expected_stream_id: session.stream_id.clone().unwrap(),
                    expected_cwd_fence: session.cwd_fence.clone(),
                };
                assert_eq!(
                    host.list_pi_models(Access::Remote, params.clone())
                        .unwrap_err()
                        .to_string(),
                    "LOCAL_ONLY"
                );
                let mut stale = params.clone();
                stale.expected_stream_id = "stale".into();
                assert!(host.list_pi_models(Access::Local, stale).is_err());
                let mut stale = params.clone();
                stale.expected_cwd_fence = "stale".into();
                assert!(host.list_pi_models(Access::Local, stale).is_err());
                assert_eq!(std::fs::read_to_string(&log).unwrap(), "get_state\n");
                let mut received = host.list_pi_models(Access::Local, params.clone()).unwrap();
                assert_eq!(
                    host.list_pi_models(Access::Local, params)
                        .unwrap_err()
                        .to_string(),
                    "PI_COMMAND_PENDING"
                );
                assert_eq!(
                    host.command_pi(
                        Access::Local,
                        prompt(&session, "must not run", DeliveryMode::FollowUp)
                    )
                    .unwrap_err()
                    .to_string(),
                    "PI_COMMAND_PENDING"
                );
                let before = view(&host).session;
                let before_snapshot = response(&host, Request::ListSessions);
                if case == "models-close" {
                    host.dispatch(
                        Access::Local,
                        1,
                        Request::CloseSession(CloseSessionParams {
                            session_id: session.id.clone(),
                            expected_stream_id: session.stream_id.clone().unwrap(),
                            expected_cwd_fence: session.cwd_fence.clone(),
                        }),
                    )
                    .unwrap();
                }
                let mut result = None;
                until(|| {
                    host.poll_processes(
                        Instant::now()
                            + if case == "models-timeout" {
                                Duration::from_secs(31)
                            } else {
                                Duration::ZERO
                            },
                    )
                    .unwrap();
                    match received.try_recv() {
                        Ok(value) => {
                            result = Some(value);
                            true
                        }
                        Err(oneshot::error::TryRecvError::Closed) => {
                            result = Some(Err(std::io::Error::other("PI_CONNECTION_CLOSED")));
                            true
                        }
                        Err(oneshot::error::TryRecvError::Empty) => false,
                    }
                });
                let result = result.unwrap();
                if case == "models" {
                    assert_eq!(
                        serde_json::to_value(result.unwrap()).unwrap(),
                        models["expected"]
                    );
                    assert_eq!(view(&host).session, before);
                    assert_eq!(response(&host, Request::ListSessions), before_snapshot);
                } else {
                    let error = result.unwrap_err().to_string();
                    assert!(!error.contains("never-forward"));
                    assert_eq!(
                        error,
                        match case {
                            "models-rejected" => "PI_COMMAND_REJECTED",
                            "models-timeout" => "PI_RESPONSE_TIMEOUT",
                            "models-invalid" => "PI_COMMAND_OUTCOME_UNKNOWN",
                            _ => "PI_CONNECTION_CLOSED",
                        }
                    );
                }
                assert!(
                    !serde_json::to_string(&view(&host))
                        .unwrap()
                        .contains("never-forward")
                );
                let log = std::fs::read_to_string(&log).unwrap();
                assert!(
                    log.lines()
                        .filter(|line| *line == "get_available_models")
                        .count()
                        <= 1
                );
                assert!(!log.contains("prompt"));
            }
            "initial_configuration" => {
                assert_eq!(
                    session.model,
                    Some(ModelInfo {
                        provider: "synthetic".into(),
                        id: "initial".into()
                    })
                );
                assert_eq!(session.thinking_level, Some(ThinkingLevel::Off));
                assert_eq!(std::fs::read_to_string(&log).unwrap(), "get_state\n");
            }
            "remote" | "stale_fences" => {
                let request = prompt(&session, "must not be sent", DeliveryMode::FollowUp);
                if case == "remote" {
                    assert_eq!(
                        host.command_pi(Access::Remote, request)
                            .unwrap_err()
                            .to_string(),
                        "LOCAL_ONLY"
                    );
                } else {
                    for stream in [true, false] {
                        let mut stale = session.clone();
                        if stream {
                            stale.stream_id = Some("stale-stream".into());
                        } else {
                            stale.cwd_fence = "stale-cwd".into();
                        }
                        assert_eq!(
                            host.command_pi(
                                Access::Local,
                                prompt(&stale, "must not be sent", DeliveryMode::FollowUp)
                            )
                            .unwrap_err()
                            .to_string(),
                            if stream {
                                "STALE_STREAM"
                            } else {
                                "STALE_CWD_FENCE"
                            }
                        );
                    }
                }
                assert_eq!(std::fs::read_to_string(&log).unwrap(), "get_state\n");
            }
            "model_and_thinking"
            | "configuration_readback_rejected"
            | "configuration_readback_missing" => {
                let mut model = host
                    .command_pi(
                        Access::Local,
                        Request::SetModel(SetModelParams {
                            session_id: session.id.clone(),
                            expected_stream_id: session.stream_id.clone().unwrap(),
                            expected_cwd_fence: session.cwd_fence.clone(),
                            provider: "synthetic".into(),
                            model_id: "tea".into(),
                        }),
                    )
                    .unwrap();
                assert!(
                    view(&host).session.model.is_none(),
                    "enqueue is not applied"
                );
                assert!(
                    host.command_pi(
                        Access::Local,
                        prompt(&session, "blocked", DeliveryMode::FollowUp)
                    )
                    .is_err()
                );
                let result = receive(&host, &mut model, false);
                if case != "model_and_thinking" {
                    assert!(
                        matches!(result, RequestResult::Unknown(ref value) if value.error.code == "PI_CONFIGURATION_OUTCOME_UNKNOWN")
                    );
                    assert!(
                        view(&host).session.model.is_none(),
                        "never invent the applied model"
                    );
                    host.stop_owned_processes().unwrap();
                    println!(
                        "M1_PI_HOST_COMMANDS_OK case={case} modelCalls=0 readbackUnknown=true noReplay=true productionDisabled=true"
                    );
                    continue;
                }
                assert!(matches!(result, RequestResult::Applied(_)));
                assert_eq!(
                    view(&host).session.model,
                    Some(ModelInfo {
                        provider: "synthetic".into(),
                        id: "tea".into()
                    })
                );
                let mut thinking = host
                    .command_pi(
                        Access::Local,
                        Request::SetThinking(SetThinkingParams {
                            session_id: session.id.clone(),
                            expected_stream_id: session.stream_id.clone().unwrap(),
                            expected_cwd_fence: session.cwd_fence.clone(),
                            level: ThinkingLevel::High,
                        }),
                    )
                    .unwrap();
                assert!(matches!(
                    receive(&host, &mut thinking, false),
                    RequestResult::Applied(_)
                ));
                assert_eq!(view(&host).session.thinking_level, Some(ThinkingLevel::Low));
            }
            _ => {
                let busy = matches!(
                    case,
                    "reordered_inputs" | "cancel" | "settle_during_clear" | "stale_run"
                );
                let text = if busy {
                    "busy"
                } else if case == "handled" {
                    "/handled"
                } else {
                    "synthetic"
                };
                let mut received = host
                    .command_pi(
                        Access::Local,
                        prompt(&session, text, DeliveryMode::FollowUp),
                    )
                    .unwrap();
                assert!(
                    matches!(
                        received.try_recv(),
                        Err(oneshot::error::TryRecvError::Empty)
                    ),
                    "enqueue does not acknowledge dispatch"
                );
                if case == "start_without_ack" {
                    until(|| {
                        host.poll_processes(Instant::now()).unwrap();
                        view(&host).session.state == SessionState::Running
                    });
                    assert!(
                        matches!(
                            received.try_recv(),
                            Err(oneshot::error::TryRecvError::Empty)
                        ),
                        "agent_start is not command acceptance"
                    );
                }
                if case == "close_pending" {
                    let result = host
                        .dispatch(
                            Access::Local,
                            1,
                            Request::CloseSession(CloseSessionParams {
                                session_id: session.id.clone(),
                                expected_stream_id: session.stream_id.clone().unwrap(),
                                expected_cwd_fence: session.cwd_fence.clone(),
                            }),
                        )
                        .unwrap()
                        .result;
                    assert!(matches!(result, RequestResult::Dispatched(_)));
                    assert!(matches!(
                        receive(&host, &mut received, false),
                        RequestResult::Unknown(_)
                    ));
                } else {
                    let result = receive(
                        &host,
                        &mut received,
                        matches!(case, "no_response" | "start_without_ack"),
                    );
                    if matches!(
                        case,
                        "crash" | "malformed" | "no_response" | "start_without_ack"
                    ) {
                        assert!(matches!(result, RequestResult::Unknown(_)));
                        assert!(matches!(view(&host).session.state, SessionState::Failed(_)));
                        assert!(view(&host).session.active_run_id.is_none());
                    } else if case == "rejected" {
                        assert!(
                            matches!(result, RequestResult::Rejected(ref value) if value.error.code == "PI_COMMAND_REJECTED")
                        );
                        assert!(
                            !serde_json::to_string(&result)
                                .unwrap()
                                .contains("synthetic-private")
                        );
                        assert_eq!(view(&host).session.state, SessionState::Ready);
                    } else {
                        dispatched(result);
                        if busy {
                            let running = view(&host).session;
                            assert_eq!(running.state, SessionState::Running);
                            let run = running.active_run_id.clone().unwrap();
                            if case == "reordered_inputs" {
                                let mut steer = host
                                    .command_pi(
                                        Access::Local,
                                        prompt(&running, "steer", DeliveryMode::Steer),
                                    )
                                    .unwrap();
                                let mut follow = host
                                    .command_pi(
                                        Access::Local,
                                        prompt(&running, "follow", DeliveryMode::FollowUp),
                                    )
                                    .unwrap();
                                dispatched(receive(&host, &mut follow, false));
                                dispatched(receive(&host, &mut steer, false));
                            }
                            if case == "stale_run" {
                                assert_eq!(
                                    host.command_pi(
                                        Access::Local,
                                        cancel(&running, "not-this-run")
                                    )
                                    .unwrap_err()
                                    .to_string(),
                                    "STALE_RUN"
                                );
                            }
                            let mut cancelled = host
                                .command_pi(Access::Local, cancel(&running, &run))
                                .unwrap();
                            assert_eq!(
                                host.command_pi(
                                    Access::Local,
                                    prompt(&running, "blocked", DeliveryMode::FollowUp)
                                )
                                .unwrap_err()
                                .to_string(),
                                "PI_CANCEL_PENDING"
                            );
                            let result = receive(&host, &mut cancelled, false);
                            let already_settled = matches!(result, RequestResult::Applied(_));
                            if already_settled {
                                assert_eq!(case, "settle_during_clear");
                            } else {
                                dispatched(result);
                            }
                            until(|| {
                                host.poll_processes(Instant::now()).unwrap();
                                view(&host).session.state == SessionState::Ready
                            });
                            assert!(view(&host).session.active_run_id.is_none());
                            let commands = std::fs::read_to_string(&log).unwrap();
                            if already_settled {
                                assert!(
                                    commands.ends_with("clear_queue\n"),
                                    "settled run must not be aborted"
                                );
                            } else {
                                assert!(commands.ends_with("clear_queue\nabort\n"));
                            }
                        } else if case == "first_turn" {
                            until(|| {
                                host.poll_processes(Instant::now()).unwrap();
                                view(&host).session.state == SessionState::Ready
                            });
                            assert!(
                                serde_json::to_string(&view(&host))
                                    .unwrap()
                                    .contains("Final synthetic")
                            );
                        } else if case == "handled" {
                            assert!(view(&host).session.active_run_id.is_none());
                        } else if case == "delayed_start" {
                            assert_eq!(
                                host.command_pi(
                                    Access::Local,
                                    prompt(&session, "duplicate", DeliveryMode::FollowUp)
                                )
                                .unwrap_err()
                                .to_string(),
                                "PI_COMMAND_PENDING"
                            );
                        }
                    }
                }
            }
        }
        let Response::Runtimes(runtimes) = response(&host, Request::ListRuntimes) else {
            panic!("runtimes")
        };
        assert!(runtimes.items.is_empty());
        host.stop_owned_processes().unwrap();
        assert!(
            std::fs::read_to_string(&log)
                .unwrap()
                .lines()
                .filter(|s| *s == "prompt")
                .count()
                <= 1,
            "no replay"
        );
        println!(
            "M1_PI_HOST_COMMANDS_OK case={case} modelCalls=0 nativeRustOnly=true correlated=true fences=true noReplay=true productionDisabled=true"
        );
    }
}
