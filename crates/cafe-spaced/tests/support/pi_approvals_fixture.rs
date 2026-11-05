//! Host confirmations share session fences, bounded snapshots and the actual owned fake process.
use super::pi_commands_fixture::{cancel, dispatched, prompt, receive, response, view};
use super::*;
use cafe_space_ipc::BuildFlavor;
use cafe_space_protocol::*;
use cafe_spaced::{host::Host, state::State, update::Access};

fn approve(session: &SessionSnapshot, id: &str, decision: ApprovalDecision) -> Request {
    Request::RespondToApproval(RespondToApprovalParams {
        session_id: session.id.clone(),
        expected_stream_id: session.stream_id.clone().unwrap(),
        expected_cwd_fence: session.cwd_fence.clone(),
        approval_id: id.into(),
        decision,
    })
}

pub fn run() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/pi-rpc/host-approvals.json"
    ))
    .unwrap();
    assert_eq!(fixture["budgetBytes"], 65536);
    for case in fixture["cases"].as_array().unwrap() {
        let case = case.as_str().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join(BuildFlavor::Dev.directory_name());
        let cwd = temp.path().join("project");
        std::fs::create_dir(&cwd).unwrap();
        let host = Host::new(
            State::open(&data, BuildFlavor::Dev).unwrap(),
            &data,
            "approval-boot".into(),
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
        let (node, mut args) = pi_fixture::command(
            &data.join("agent-sessions/pi-native-fixture-1"),
            if case == "crash_dialog" {
                "approval-crash"
            } else {
                "normal"
            },
        );
        args.extend(["--fake-command-file".into(), log.as_os_str().to_owned()]);
        host.launch_pi_session(&params, &runtime, &node, &args, "native-fixture-1".into())
            .unwrap();
        until(|| {
            host.poll_processes(Instant::now()).unwrap();
            view(&host).session.state == SessionState::Ready
        });
        let session = view(&host).session;
        let text = match case {
            "local_ui" => "local-ui",
            "large_dialog" => "large-approval",
            _ => "approval",
        };
        let mut started = host
            .command_pi(
                Access::Local,
                prompt(&session, text, DeliveryMode::FollowUp),
            )
            .unwrap();
        dispatched(receive(&host, &mut started, false));
        until(|| {
            host.poll_processes(Instant::now()).unwrap();
            let v = view(&host);
            !v.approvals.is_empty() || v.session.state == SessionState::WaitingForLocalUi
        });
        let current = view(&host);
        assert_eq!(
            current.session.capabilities.tool_approval,
            ToolApproval::NeverAsks
        );
        assert!(!current.session.capabilities.approval_for_session);
        if matches!(case, "local_ui" | "large_dialog") {
            assert!(current.approvals.is_empty());
            assert_eq!(current.session.state, SessionState::WaitingForLocalUi);
            assert!(
                !std::fs::read_to_string(&log)
                    .unwrap()
                    .contains("extension_ui_response")
            );
        } else {
            let approval = &current.approvals[0];
            assert_eq!(approval.id.len(), 32);
            assert_eq!(approval.summary, "Synthetic confirmation");
            let rendered = serde_json::to_string(&current).unwrap();
            assert!(
                !rendered.contains("private-confirm") && !rendered.contains("native-fixture-1")
            );
            let request = approve(&current.session, &approval.id, ApprovalDecision::AllowOnce);
            if case == "scope_rejected" {
                assert_eq!(
                    host.command_pi(
                        Access::Local,
                        approve(
                            &current.session,
                            &approval.id,
                            ApprovalDecision::AllowForSession
                        )
                    )
                    .unwrap_err()
                    .to_string(),
                    "PI_APPROVAL_SCOPE_UNSUPPORTED"
                );
            } else if case == "remote" {
                assert_eq!(
                    host.command_pi(Access::Remote, request.clone())
                        .unwrap_err()
                        .to_string(),
                    "LOCAL_ONLY"
                );
            } else if case == "stale_fences" {
                for stream in [true, false] {
                    let mut stale = current.session.clone();
                    if stream {
                        stale.stream_id = Some("stale-stream".into());
                    } else {
                        stale.cwd_fence = "stale-cwd".into();
                    }
                    assert_eq!(
                        host.command_pi(
                            Access::Local,
                            approve(&stale, &approval.id, ApprovalDecision::AllowOnce)
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
            } else if case == "wrong_session" {
                let mut other = params.clone();
                other.session_id = "other-session".into();
                let other_native = "other-private";
                let (node, mut args) =
                    pi_fixture::command(&data.join("agent-sessions/pi-other-private"), "normal");
                let flag = args.iter().position(|arg| arg == "--session-id").unwrap();
                args[flag + 1] = other_native.into();
                host.launch_pi_session(&other, &runtime, &node, &args, other_native.into())
                    .unwrap();
                until(|| {
                    host.poll_processes(Instant::now()).unwrap();
                    let Response::Session(value) = response(
                        &host,
                        Request::GetSession(GetSessionParams {
                            session_id: other.session_id.clone(),
                        }),
                    ) else {
                        panic!("session")
                    };
                    value.session.session.state == SessionState::Ready
                });
                let mut stale = current.session.clone();
                stale.id = other.session_id.clone();
                let Response::Session(other_view) = response(
                    &host,
                    Request::GetSession(GetSessionParams {
                        session_id: other.session_id,
                    }),
                ) else {
                    panic!("session")
                };
                stale.stream_id = other_view.session.session.stream_id;
                assert_eq!(
                    host.command_pi(
                        Access::Local,
                        approve(&stale, &approval.id, ApprovalDecision::AllowOnce)
                    )
                    .unwrap_err()
                    .to_string(),
                    "PI_APPROVAL_NOT_FOUND"
                );
            }
            if matches!(case, "cancel_dialog" | "close_dialog" | "crash_dialog") {
                if matches!(case, "cancel_dialog" | "crash_dialog") {
                    let mut stopped = host
                        .command_pi(
                            Access::Local,
                            cancel(
                                &current.session,
                                current.session.active_run_id.as_ref().unwrap(),
                            ),
                        )
                        .unwrap();
                    let result = receive(&host, &mut stopped, false);
                    if case == "crash_dialog" {
                        assert!(matches!(result, RequestResult::Unknown(_)));
                    } else {
                        dispatched(result);
                    }
                } else if case == "close_dialog" {
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
                until(|| {
                    host.poll_processes(Instant::now()).unwrap();
                    view(&host).approvals.is_empty()
                });
                assert!(
                    !std::fs::read_to_string(&log)
                        .unwrap()
                        .contains("extension_ui_response")
                );
            } else {
                let decision = if case == "deny" {
                    ApprovalDecision::Deny
                } else {
                    ApprovalDecision::AllowOnce
                };
                let mut replied = host
                    .command_pi(
                        Access::Local,
                        approve(&current.session, &approval.id, decision),
                    )
                    .unwrap();
                assert!(
                    matches!(
                        replied.try_recv(),
                        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
                    ),
                    "enqueue is not dispatch"
                );
                if case == "duplicate" {
                    assert_eq!(
                        host.command_pi(Access::Local, request)
                            .unwrap_err()
                            .to_string(),
                        "PI_APPROVAL_PENDING"
                    );
                }
                dispatched(receive(&host, &mut replied, false));
                until(|| {
                    host.poll_processes(Instant::now()).unwrap();
                    view(&host).session.state == SessionState::Ready
                });
                assert!(view(&host).approvals.is_empty());
                assert!(
                    host.command_pi(
                        Access::Local,
                        approve(&current.session, &approval.id, ApprovalDecision::AllowOnce)
                    )
                    .is_err()
                );
                let commands = std::fs::read_to_string(&log).unwrap();
                assert_eq!(
                    commands
                        .lines()
                        .filter(|line| *line == "extension_ui_response")
                        .count(),
                    1
                );
                assert!(commands.contains(if case == "deny" {
                    "confirmed=false"
                } else {
                    "confirmed=true"
                }));
            }
        }
        assert!(view(&host).approvals.is_empty());
        host.stop_owned_processes().unwrap();
        drop(host);
        let reopened = Host::new(
            State::open(&data, BuildFlavor::Dev).unwrap(),
            &data,
            "approval-restart".into(),
            "a".repeat(64),
        )
        .unwrap();
        assert!(view(&reopened).approvals.is_empty());
        println!(
            "M1_PI_HOST_APPROVALS_OK case={case} modelCalls=0 privateIds=true noInventedDecision=true noReplay=true productionDisabled=true"
        );
    }
}
