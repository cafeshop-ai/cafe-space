//! Owned offline Pi processes only; update markers remain inside temporary fixture roots.
use super::*;
use cafe_space_adapters::pi::install::Installation;
use cafe_space_ipc::{BuildFlavor, installation::Handover};
use cafe_space_protocol::*;
use cafe_spaced::{host::Host, state::State, update::Access};
use pi_commands_fixture::{cancel, dispatched, prompt, receive, response};

fn snapshot(host: &Host, id: &str) -> SessionSnapshot {
    let Response::Session(value) = response(
        host,
        Request::GetSession(GetSessionParams {
            session_id: id.into(),
        }),
    ) else {
        panic!("session")
    };
    value.session.session
}
fn update(host: &Host) -> UpdateState {
    let Response::Health(value) = response(host, Request::GetStatus) else {
        panic!("health")
    };
    value.update
}
fn prepare(host: &Host, mode: UpdateMode) -> UpdateState {
    let Response::Update(value) = response(
        host,
        Request::PrepareUpdate(PrepareUpdateParams {
            mode,
            target_hash: "b".repeat(64),
        }),
    ) else {
        panic!("update")
    };
    value.update
}
fn close(host: &Host, session: &SessionSnapshot) {
    dispatched(
        host.dispatch(
            Access::Local,
            1,
            Request::CloseSession(CloseSessionParams {
                session_id: session.id.clone(),
                expected_stream_id: session.stream_id.clone().unwrap(),
                expected_cwd_fence: session.cwd_fence.clone(),
            }),
        )
        .unwrap()
        .result,
    );
}

pub fn run() {
    let contract: serde_json::Value =
        serde_json::from_str(include_str!("../../../../fixtures/pi-rpc/update-gate.json")).unwrap();
    for case in contract["cases"].as_array().unwrap() {
        let case = case.as_str().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join(BuildFlavor::Dev.directory_name());
        let cwd = temp.path().join("project");
        std::fs::create_dir(&cwd).unwrap();
        let host = Host::new(
            State::open(&data, BuildFlavor::Dev).unwrap(),
            &data,
            "update-boot".into(),
            "a".repeat(64),
        )
        .unwrap();
        let Response::Project(project) = response(
            &host,
            Request::AddProject(AddProjectParams {
                name: None,
                root: cwd.to_str().unwrap().into(),
            }),
        ) else {
            panic!("project")
        };
        let runtime = RuntimeSummary {
            id: "pi".into(),
            name: "Synthetic".into(),
            version: None,
            available: true,
            tool_approval: ToolApproval::NeverAsks,
        };
        let mut sessions = Vec::new();
        let mut pids = Vec::new();
        let mut params = CreateSessionParams {
            session_id: String::new(),
            project_id: project.id,
            runtime_id: runtime.id.clone(),
            expected_cwd_fence: project.cwd_fence,
            start: SessionStart::New,
        };
        for index in 0..if matches!(case, "two_busy" | "now") {
            2
        } else {
            1
        } {
            params.session_id = format!("update-session-{index}");
            let native = format!("native-update-{index}");
            let mode = match case {
                "starting" => "startup-timeout",
                "reserved_prompt" => "delayed-start",
                "pending_models" => "models-timeout",
                "ready_approval_reply" | "ready_approval_close" => "ready-confirmation",
                _ => "normal",
            };
            let (node, mut args) =
                pi_fixture::command(&data.join(format!("agent-sessions/pi-{native}")), mode);
            let id = args.iter().position(|arg| arg == "--session-id").unwrap() + 1;
            args[id] = native.clone().into();
            let pid = data.join(format!("agent-{index}.pid"));
            let log = data.join(format!("commands-{index}.log"));
            // A starting Agent may stop before consuming input; an empty command log is valid.
            std::fs::write(&log, "").unwrap();
            args.extend([
                "--fake-pid-file".into(),
                pid.as_os_str().to_owned(),
                "--fake-command-file".into(),
                log.into_os_string(),
            ]);
            let session = host
                .launch_pi_session(&params, &runtime, &node, &args, native)
                .unwrap();
            until(|| pid.exists());
            pids.push(read_pid(&pid));
            if case != "starting" {
                until(|| {
                    host.poll_processes(Instant::now()).unwrap();
                    snapshot(&host, &session.id).state == SessionState::Ready
                });
            }
            sessions.push(snapshot(&host, &session.id));
        }
        let session = &sessions[0];
        let mut pending_models = None;
        let mut pending_configuration = None;
        if case == "idle" {
            // Native file created with the fixture user's owner even on elevated Windows runners.
            drop(
                cafe_space_ipc::private::file(
                    &data
                        .join("agent-sessions/pi-native-update-0/2000-01-01_native-update-0.jsonl"),
                )
                .unwrap(),
            );
            let mut reply = host
                .command_pi(
                    Access::Local,
                    prompt(session, "remember", DeliveryMode::Steer),
                )
                .unwrap();
            dispatched(receive(&host, &mut reply, false));
            until(|| {
                host.poll_processes(Instant::now()).unwrap();
                snapshot(&host, &session.id).state == SessionState::Ready
            });
        } else if case == "pending_models" {
            pending_models = Some(
                host.list_pi_models(
                    Access::Local,
                    OpenSessionParams {
                        session_id: session.id.clone(),
                        expected_stream_id: session.stream_id.clone().unwrap(),
                        expected_cwd_fence: session.cwd_fence.clone(),
                    },
                )
                .unwrap(),
            );
        } else if case == "pending_configuration" {
            pending_configuration = Some(
                host.command_pi(
                    Access::Local,
                    Request::SetModel(SetModelParams {
                        session_id: session.id.clone(),
                        expected_stream_id: session.stream_id.clone().unwrap(),
                        expected_cwd_fence: session.cwd_fence.clone(),
                        provider: "synthetic".into(),
                        model_id: "update-model".into(),
                    }),
                )
                .unwrap(),
            );
        } else if case != "starting" && !case.starts_with("ready_approval") {
            for session in &sessions {
                let mut reply = host
                    .command_pi(
                        Access::Local,
                        prompt(
                            session,
                            match case {
                                "approval" => "approval",
                                "local_ui" => "local-ui",
                                _ => "busy",
                            },
                            DeliveryMode::Steer,
                        ),
                    )
                    .unwrap();
                dispatched(receive(&host, &mut reply, false));
                let expected = match case {
                    "reserved_prompt" => SessionState::Ready,
                    "local_ui" => SessionState::WaitingForLocalUi,
                    _ => SessionState::Running,
                };
                until(|| {
                    host.poll_processes(Instant::now()).unwrap();
                    snapshot(&host, &session.id).state == expected
                });
            }
        }
        if case == "approval" || case.starts_with("ready_approval") {
            until(|| {
                host.poll_processes(Instant::now()).unwrap();
                let Response::Session(value) = response(
                    &host,
                    Request::GetSession(GetSessionParams {
                        session_id: session.id.clone(),
                    }),
                ) else {
                    panic!("session")
                };
                !value.session.approvals.is_empty()
            });
        }
        if case.starts_with("ready_approval") {
            assert_eq!(snapshot(&host, &session.id).state, SessionState::Ready);
            assert!(snapshot(&host, &session.id).active_run_id.is_none());
        }
        assert!(pids.iter().all(|pid| running(*pid)));
        assert_eq!(
            prepare(
                &host,
                if case == "now" {
                    UpdateMode::Now
                } else {
                    UpdateMode::WhenIdle
                }
            ),
            if matches!(case, "idle" | "now") {
                UpdateState::Draining
            } else {
                UpdateState::Pending
            },
            "{case}"
        );
        if !matches!(case, "idle" | "now") {
            assert!(Handover::read(&data).unwrap().is_none());
            if case == "disconnect" {
                host.disconnected(1);
                assert_eq!(update(&host), UpdateState::None);
            }
            match case {
                "starting"
                | "reserved_prompt"
                | "pending_models"
                | "ready_approval_reply"
                | "ready_approval_close" => {
                    for _ in 0..3 {
                        host.poll_processes(Instant::now()).unwrap();
                        assert_eq!(update(&host), UpdateState::Pending);
                        assert!(Handover::read(&data).unwrap().is_none());
                        assert!(running(pids[0]));
                    }
                    if case == "ready_approval_reply" {
                        let Response::Session(value) = response(
                            &host,
                            Request::GetSession(GetSessionParams {
                                session_id: session.id.clone(),
                            }),
                        ) else {
                            panic!("session")
                        };
                        let mut reply = host
                            .command_pi(
                                Access::Local,
                                Request::RespondToApproval(RespondToApprovalParams {
                                    session_id: session.id.clone(),
                                    expected_stream_id: session.stream_id.clone().unwrap(),
                                    expected_cwd_fence: session.cwd_fence.clone(),
                                    approval_id: value.session.approvals[0].id.clone(),
                                    decision: ApprovalDecision::Deny,
                                }),
                            )
                            .unwrap();
                        dispatched(receive(&host, &mut reply, false));
                    } else {
                        close(&host, session);
                    }
                    if let Some(reply) = &mut pending_models {
                        assert!(matches!(
                            reply.try_recv(),
                            Err(tokio::sync::oneshot::error::TryRecvError::Closed)
                        ));
                    }
                }
                "pending_configuration" => {
                    assert!(matches!(
                        receive(&host, pending_configuration.as_mut().unwrap(), false),
                        RequestResult::Applied(_)
                    ));
                    let current = snapshot(&host, &session.id);
                    assert_eq!(current.model.unwrap().id, "update-model");
                }
                _ => {
                    for (index, session) in sessions.iter().enumerate() {
                        let current = snapshot(&host, &session.id);
                        let mut reply = host
                            .command_pi(
                                Access::Local,
                                cancel(&current, current.active_run_id.as_deref().unwrap()),
                            )
                            .unwrap();
                        dispatched(receive(&host, &mut reply, false));
                        if case == "two_busy" && index == 0 {
                            assert_eq!(update(&host), UpdateState::Pending);
                            assert!(pids.iter().all(|pid| running(*pid)));
                            assert!(Handover::read(&data).unwrap().is_none());
                        }
                    }
                }
            }
        }
        if case == "disconnect" {
            assert_eq!(update(&host), UpdateState::None);
            assert!(Handover::read(&data).unwrap().is_none());
            assert!(running(pids[0]));
            assert_eq!(snapshot(&host, &session.id).state, SessionState::Ready);
            close(&host, session);
        } else {
            until(|| {
                host.poll_processes(Instant::now()).unwrap();
                update(&host) == UpdateState::Draining
            });
            let marker = Handover::read(&data).unwrap().unwrap();
            assert_eq!(marker.old_hash, "a".repeat(64));
            assert_eq!(marker.target_hash, "b".repeat(64));
            assert_eq!(
                marker.phase,
                cafe_space_ipc::installation::HandoverPhase::Draining
            );
            assert_eq!(
                host.command_pi(
                    Access::Local,
                    prompt(session, "never-send", DeliveryMode::Steer)
                )
                .unwrap_err()
                .to_string(),
                "UPDATE_IN_PROGRESS"
            );
            params.session_id = "new-during-drain".into();
            let (node, args) = pi_fixture::command(&cwd, "normal");
            assert_eq!(
                host.launch_pi_session(&params, &runtime, &node, &args, "never-start".into())
                    .unwrap_err()
                    .to_string(),
                "UPDATE_IN_PROGRESS"
            );
            host.disconnected(1);
            assert_eq!(update(&host), UpdateState::Draining);
        }
        until(|| pids.iter().all(|pid| !running(*pid)));
        for session in &sessions {
            let current = snapshot(&host, &session.id);
            assert_eq!(current.state, SessionState::Stopped);
            assert!(current.active_run_id.is_none());
        }
        let (_, events) = host
            .dispatch(
                Access::Local,
                1,
                Request::SubscribeEvents(SubscribeEventsParams {
                    after: Some(EventCursor {
                        event_stream_id: "update-boot".into(),
                        sequence: 0,
                    }),
                }),
            )
            .unwrap()
            .subscription
            .unwrap();
        if case != "disconnect" {
            let draining = events.iter().position(|event| matches!(&event.event, Event::UpdateChanged(value) if value.update == UpdateState::Draining)).unwrap();
            let mut previous_stop = None;
            for session in &sessions {
                let stopped = events.iter().position(|event| matches!(&event.event, Event::SessionChanged(value) if value.session.id == session.id && value.session.state == SessionState::Stopped)).unwrap();
                assert!(stopped < draining);
                assert!(previous_stop.is_none_or(|previous| previous < stopped));
                previous_stop = Some(stopped);
            }
        }
        let logs: Vec<_> = (0..sessions.len())
            .map(|index| {
                std::fs::read_to_string(data.join(format!("commands-{index}.log"))).unwrap()
            })
            .collect();
        for log in &logs {
            let expected = if matches!(
                case,
                "starting"
                    | "pending_models"
                    | "pending_configuration"
                    | "ready_approval_reply"
                    | "ready_approval_close"
            ) {
                0
            } else {
                1
            };
            assert_eq!(
                log.lines().filter(|line| *line == "prompt").count(),
                expected
            );
        }
        drop(host);
        let host = Host::new(
            State::open(&data, BuildFlavor::Dev).unwrap(),
            &data,
            "update-restart".into(),
            if case == "disconnect" { "a" } else { "b" }.repeat(64),
        )
        .unwrap();
        if case != "disconnect" {
            let Response::Health(health) = response(&host, Request::GetStatus) else {
                panic!("health")
            };
            assert!(!health.ready);
            assert_eq!(health.awaiting_update_confirmation, Some(true));
            assert_eq!(update(&host), UpdateState::Draining);
            // Synthetic controller confirmation in this disposable root, not an OS installation.
            let mut marker = Handover::read(&data).unwrap().unwrap();
            marker.phase = cafe_space_ipc::installation::HandoverPhase::Committed;
            marker.write(&data).unwrap();
            let Response::Health(health) = response(&host, Request::GetStatus) else {
                panic!("health")
            };
            assert!(health.ready);
            assert_eq!(health.update, UpdateState::None);
            assert!(health.awaiting_update_confirmation.is_none());
        }
        for session in &sessions {
            assert_eq!(snapshot(&host, &session.id).state, SessionState::Stopped);
        }
        if case == "idle" {
            let file =
                data.join("agent-sessions/pi-native-update-0/2000-01-01_native-update-0.jsonl");
            let before = std::fs::read(&file).unwrap();
            let (node, _) = pi_fixture::command(&cwd, "normal");
            let installation = Installation {
                node,
                cli: Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../test/fake-pi.mjs")
                    .canonicalize()
                    .unwrap(),
            };
            params.session_id = "resumed-after-update".into();
            params.start = SessionStart::Resume(ResumeStartParams {
                source_session_id: session.id.clone(),
            });
            host.start_installed_pi(Access::Local, &params, &installation)
                .unwrap();
            until(|| {
                host.poll_processes(Instant::now()).unwrap();
                snapshot(&host, &params.session_id).state == SessionState::Ready
            });
            assert_eq!(
                std::fs::read(&file).unwrap(),
                before,
                "Resume must not prompt"
            );
            let Response::Session(value) = response(
                &host,
                Request::GetSession(GetSessionParams {
                    session_id: params.session_id.clone(),
                }),
            ) else {
                panic!("resumed")
            };
            assert!(format!("{:?}", value.session.messages).contains("Final synthetic 茶"));
            close(&host, &snapshot(&host, &params.session_id));
        }
        println!(
            "M1_PI_HOST_UPDATE_OK case={case} modelCalls=0 busyWorkTracked=true terminalBeforeMarker=true processReaped=true noReplay=true productionDisabled=true"
        );
    }
}
