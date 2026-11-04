//! Lifecycle-only Host attachment with a fake process; never enables wire runtime admission.
use super::*;
use cafe_space_ipc::{BuildFlavor, installation::Handover};
use cafe_space_protocol::*;
use cafe_spaced::{host::Host, state::State, update::Access};

fn applied(host: &Host, request: Request) -> Response {
    match host.dispatch(Access::Local, 1, request).unwrap().result {
        RequestResult::Applied(value) => value.result,
        other => panic!("expected applied Host response: {other:?}"),
    }
}
fn snapshot(host: &Host) -> SessionSnapshot {
    match applied(
        host,
        Request::GetSession(GetSessionParams {
            session_id: "host-pi".into(),
        }),
    ) {
        Response::Session(value) => value.session.session,
        _ => panic!("session expected"),
    }
}

pub fn run() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/pi-rpc/host-lifecycle.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let mode = case["mode"].as_str().unwrap();
        let endings = if mode == "normal" {
            &['c', 'u', 's', 'd'][..]
        } else {
            &['f'][..]
        };
        for &ending in endings {
            let temp = tempfile::tempdir().unwrap();
            let data = temp.path().join(BuildFlavor::Dev.directory_name());
            let cwd = temp.path().join("project");
            std::fs::create_dir(&cwd).unwrap();
            let host = Host::new(
                State::open(&data, BuildFlavor::Dev).unwrap(),
                &data,
                "host-boot".into(),
                "a".repeat(64),
            )
            .unwrap();
            let Response::Runtimes(runtimes) = applied(&host, Request::ListRuntimes) else {
                panic!("runtimes")
            };
            assert!(
                runtimes.items.is_empty(),
                "production runtime must remain disabled"
            );
            let Response::Project(project) = applied(
                &host,
                Request::AddProject(AddProjectParams {
                    name: Some("Synthetic".into()),
                    root: cwd.to_str().unwrap().into(),
                }),
            ) else {
                panic!("project")
            };
            let params = CreateSessionParams {
                session_id: "host-pi".into(),
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
            let pid_file = temp.path().join("owned-pi.pid");
            let (node, args) = if mode == "startup-eof" {
                (
                    std::env::current_exe().unwrap(),
                    vec!["--fixture-pi-eof".into(), pid_file.as_os_str().to_owned()],
                )
            } else {
                let (node, mut args) =
                    pi_fixture::command(&data.join("agent-sessions/pi-native-fixture-1"), mode);
                args.extend(["--fake-pid-file".into(), pid_file.as_os_str().to_owned()]);
                (node, args)
            };
            let session = host
                .launch_pi_session(&params, &runtime, &node, &args, "native-fixture-1".into())
                .unwrap();
            assert_eq!(session.state, SessionState::Starting);
            assert_eq!(
                host.launch_pi_session(&params, &runtime, &node, &args, "native-fixture-1".into())
                    .unwrap(),
                session
            );
            until(|| pid_file.exists());
            let pid = read_pid(&pid_file);
            if mode == "normal" && ending == 'c' {
                let mut competing = params.clone();
                competing.session_id = "competing-pi".into();
                assert_eq!(
                    host.launch_pi_session(
                        &competing,
                        &runtime,
                        &node,
                        &args,
                        "native-fixture-1".into()
                    )
                    .unwrap_err()
                    .to_string(),
                    "NATIVE_SESSION_IN_USE"
                );
                assert!(
                    matches!(host.dispatch(Access::Local, 1, Request::GetSession(GetSessionParams { session_id: competing.session_id })).unwrap().result, RequestResult::Rejected(ref value) if value.error.code == "SESSION_NOT_FOUND")
                );
            }
            until(|| {
                let now = Instant::now()
                    + if mode == "startup-timeout" {
                        Duration::from_secs(31)
                    } else {
                        Duration::ZERO
                    };
                host.poll_processes(now).unwrap();
                let state = snapshot(&host).state;
                if mode == "normal" {
                    state == SessionState::Ready
                } else {
                    matches!(state, SessionState::Failed(_))
                }
            });
            if ending == 'd' {
                drop(host);
                until(|| !running(pid));
                let reopened = Host::new(
                    State::open(&data, BuildFlavor::Dev).unwrap(),
                    &data,
                    "drop-restart".into(),
                    "a".repeat(64),
                )
                .unwrap();
                let recovered = snapshot(&reopened);
                assert_eq!(recovered.state, SessionState::Stopped);
                assert_eq!(
                    recovered.stop_reason,
                    Some(SessionStopReason::DaemonRestarted)
                );
                assert!(recovered.active_run_id.is_none());
                println!(
                    "M1_PI_HOST_DROP_OK modelCalls=0 owned=true processGoneBeforeReopen=true restartedStopped=true noReplay=true productionDisabled=true"
                );
                continue;
            }
            if mode == "normal" {
                assert!(running(pid));
                match ending {
                    'c' => {
                        assert!(matches!(
                            host.dispatch(
                                Access::Local,
                                1,
                                Request::CloseSession(CloseSessionParams {
                                    session_id: session.id.clone(),
                                    expected_stream_id: session.stream_id.clone().unwrap(),
                                    expected_cwd_fence: session.cwd_fence.clone(),
                                })
                            )
                            .unwrap()
                            .result,
                            RequestResult::Dispatched(_)
                        ));
                    }
                    'u' => {
                        applied(
                            &host,
                            Request::PrepareUpdate(PrepareUpdateParams {
                                mode: UpdateMode::Now,
                                target_hash: "b".repeat(64),
                            }),
                        );
                    }
                    's' => {
                        assert_eq!(
                            applied(&host, Request::Shutdown),
                            Response::ShutdownAccepted
                        );
                    }
                    _ => unreachable!(),
                }
            }
            until(|| !running(pid));
            for _ in 0..3 {
                host.poll_processes(Instant::now()).unwrap();
                std::thread::sleep(Duration::from_millis(10));
            }
            // Shutdown rejects ordinary reads; release Host before inspecting the durable record.
            if ending != 's' {
                let dispatch = host
                    .dispatch(
                        Access::Local,
                        1,
                        Request::SubscribeEvents(SubscribeEventsParams {
                            after: Some(EventCursor {
                                event_stream_id: "host-boot".into(),
                                sequence: 0,
                            }),
                        }),
                    )
                    .unwrap();
                let (_, replay) = dispatch.subscription.unwrap();
                let states: Vec<_> = replay
                    .iter()
                    .filter_map(|event| match &event.event {
                        Event::SessionChanged(data) => Some(&data.session.state),
                        _ => None,
                    })
                    .collect();
                assert!(states.iter().any(|state| **state == SessionState::Starting));
                if mode == "normal" {
                    assert!(states.iter().any(|state| **state == SessionState::Ready));
                }
                let last = states.last().unwrap();
                assert!(matches!(
                    last,
                    SessionState::Stopped | SessionState::Failed(_)
                ));
            }
            if ending == 'f' {
                let Response::Update(update) = applied(
                    &host,
                    Request::PrepareUpdate(PrepareUpdateParams {
                        mode: UpdateMode::WhenIdle,
                        target_hash: "b".repeat(64),
                    }),
                ) else {
                    panic!("update")
                };
                assert_eq!(update.update, UpdateState::Draining);
                assert!(Handover::read(&data).unwrap().is_some());
            }
            drop(host);
            let reopened = Host::new(
                State::open(&data, BuildFlavor::Dev).unwrap(),
                &data,
                "restart-boot".into(),
                "a".repeat(64),
            )
            .unwrap();
            let persisted = snapshot(&reopened);
            assert!(persisted.active_run_id.is_none());
            if mode == "normal" {
                assert_eq!(persisted.state, SessionState::Stopped);
                assert_eq!(
                    persisted.stop_reason,
                    Some(if ending == 'c' {
                        SessionStopReason::SessionClosed
                    } else {
                        SessionStopReason::AgentExited
                    })
                );
            } else {
                assert_eq!(
                    persisted.state,
                    SessionState::Failed(FailureState {
                        code: case["code"].as_str().unwrap().into(),
                    })
                );
                assert!(persisted.stop_reason.is_none());
            }
            println!(
                "M1_PI_HOST_LIFECYCLE_OK case={mode} ending={ending} modelCalls=0 owned=true terminalPersisted=true latePollPreserved=true processReaped=true noReplay=true productionDisabled=true"
            );
        }
    }
}
