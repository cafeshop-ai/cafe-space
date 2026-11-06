//! Existing command receiver through authenticated private IPC, with no runnable production profile.
use super::*;
use cafe_space_ipc::local;
use cafe_space_protocol::*;
use cafe_spaced::{host::Host, update::Access};
use pi_commands_fixture::{cancel, dispatched, prompt, response, view};
use pi_models_wire_fixture::{error, receive, result, send};
use tokio::io::AsyncWriteExt;

async fn outcome(
    client: &mut local::ClientStream,
    host: &Host,
    id: &str,
    expired: bool,
) -> RequestResult {
    loop {
        match receive(client, host, expired).await {
            frame @ ServerMessage::Response(_) => return result(frame, id),
            ServerMessage::Event(_) => (),
            other => panic!("unexpected command frame: {other:?}"),
        }
    }
}

fn count(log: &Path, command: &str) -> usize {
    std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .filter(|line| *line == command)
        .count()
}

fn close(session: &SessionSnapshot) -> Request {
    Request::CloseSession(CloseSessionParams {
        session_id: session.id.clone(),
        expected_stream_id: session.stream_id.clone().unwrap(),
        expected_cwd_fence: session.cwd_fence.clone(),
    })
}

pub(super) async fn case(
    client: &mut local::ClientStream,
    host: &Host,
    session: &SessionSnapshot,
    case: &str,
    log: &Path,
) {
    if case == "command-fences" {
        for request in [
            prompt(session, "not sent", DeliveryMode::Steer),
            cancel(session, "not-owned"),
        ] {
            assert_eq!(
                host.command_pi(Access::Remote, request.clone())
                    .unwrap_err()
                    .to_string(),
                "LOCAL_ONLY"
            );
            error(
                host.dispatch(Access::Remote, 0, request).unwrap().result,
                "LOCAL_ONLY",
                false,
            );
        }
        for field in ["stream", "cwd", "session"] {
            let mut stale = session.clone();
            let code = match field {
                "stream" => {
                    stale.stream_id = Some("stale".into());
                    "STALE_STREAM"
                }
                "cwd" => {
                    stale.cwd_fence = "stale".into();
                    "STALE_CWD_FENCE"
                }
                _ => {
                    stale.id = "missing".into();
                    "SESSION_NOT_FOUND"
                }
            };
            for request in [
                prompt(&stale, "not sent", DeliveryMode::Steer),
                cancel(&stale, "not-owned"),
            ] {
                send(client, "stale", request).await;
                error(outcome(client, host, "stale", false).await, code, false);
            }
        }
        send(client, "cancel", cancel(session, "not-owned")).await;
        error(
            outcome(client, host, "cancel", false).await,
            "STALE_RUN",
            false,
        );
        assert_eq!(std::fs::read_to_string(log).unwrap(), "get_state\n");
        return;
    }
    let pending = case == "command-cancel-pending";
    if pending {
        send(
            client,
            "subscribe",
            Request::SubscribeEvents(SubscribeEventsParams { after: None }),
        )
        .await;
        assert!(matches!(
            outcome(client, host, "subscribe", false).await,
            RequestResult::Applied(_)
        ));
    }
    let busy = matches!(case, "command-reordered" | "command-settle");
    send(
        client,
        "prompt",
        prompt(
            session,
            if case == "command-handled" {
                "/handled"
            } else if busy {
                "busy"
            } else {
                "synthetic"
            },
            DeliveryMode::Steer,
        ),
    )
    .await;
    if matches!(
        case,
        "command-close" | "command-disconnect" | "command-timeout"
    ) {
        // A native log entry makes the missing acknowledgement deterministic, not a scheduler delay.
        send(client, "health", Request::GetStatus).await;
        assert!(matches!(
            outcome(client, host, "health", false).await,
            RequestResult::Applied(_)
        ));
        until(|| {
            host.poll_processes(Instant::now()).unwrap();
            count(log, "prompt") == 1
        });
        if case == "command-disconnect" {
            client.shutdown().await.unwrap();
            return;
        }
        if case == "command-close" {
            send(client, "close", close(session)).await;
            dispatched(outcome(client, host, "close", false).await);
            error(
                outcome(client, host, "prompt", false).await,
                "PI_COMMAND_OUTCOME_UNKNOWN",
                true,
            );
            assert_eq!(view(host).session.state, SessionState::Stopped);
        } else {
            error(
                outcome(client, host, "prompt", true).await,
                "PI_RESPONSE_TIMEOUT",
                true,
            );
            assert!(matches!(view(host).session.state, SessionState::Failed(_)));
        }
        assert_eq!(count(log, "prompt"), 1);
        return;
    }
    if pending {
        let ServerMessage::Event(ServerEvent {
            event: Event::SessionChanged(data),
            ..
        }) = receive(client, host, false).await
        else {
            panic!("agent_start must precede the still missing prompt acknowledgement")
        };
        assert_eq!(data.session.state, SessionState::Running);
        let running = data.session;
        send(client, "health", Request::GetStatus).await;
        assert!(matches!(
            outcome(client, host, "health", false).await,
            RequestResult::Applied(_)
        ));
        // Event delivery must also remain independent of the unacknowledged prompt.
        response(
            host,
            Request::UpdateProject(UpdateProjectParams {
                project_id: session.project_id.clone(),
                expected_cwd_fence: session.cwd_fence.clone(),
                name: Some("Renamed during prompt".into()),
                root: None,
            }),
        );
        assert!(
            matches!(receive(client, host, false).await, ServerMessage::Event(ServerEvent { event: Event::ProjectChanged(value), .. }) if value.project.id == session.project_id && value.project.name == "Renamed during prompt")
        );
        send(
            client,
            "cancel",
            cancel(&running, running.active_run_id.as_ref().unwrap()),
        )
        .await;
        dispatched(outcome(client, host, "cancel", false).await);
        until(|| {
            host.poll_processes(Instant::now()).unwrap();
            view(host).session.state == SessionState::Ready
        });
        assert!(view(host).session.active_run_id.is_none());
        assert_eq!(count(log, "clear_queue"), 1);
        assert_eq!(count(log, "abort"), 1);
        // Close resolves the first still-unacknowledged prompt as unknown, never dispatched/replayed.
        send(client, "close", close(&running)).await;
        let (mut closed, mut prompt_unknown, mut stopped) = (false, false, false);
        while !closed || !prompt_unknown || !stopped {
            match receive(client, host, false).await {
                ServerMessage::Response(frame) if frame.request_id == "close" => {
                    dispatched(frame.result);
                    closed = true;
                }
                ServerMessage::Response(frame) if frame.request_id == "prompt" => {
                    error(frame.result, "PI_COMMAND_OUTCOME_UNKNOWN", true);
                    prompt_unknown = true;
                }
                ServerMessage::Event(ServerEvent {
                    event: Event::SessionChanged(data),
                    ..
                }) => {
                    stopped |= data.session.state == SessionState::Stopped;
                }
                other => panic!("unexpected close frame: {other:?}"),
            }
        }
        return;
    }
    let reply = outcome(client, host, "prompt", false).await;
    if case == "command-rejected" {
        error(reply, "PI_COMMAND_REJECTED", false);
        assert_eq!(view(host).session.state, SessionState::Ready);
        return;
    }
    dispatched(reply);
    if busy {
        until(|| {
            host.poll_processes(Instant::now()).unwrap();
            view(host).session.state == SessionState::Running
        });
        let running = view(host).session;
        if case == "command-reordered" {
            send(
                client,
                "steer",
                prompt(&running, "steer", DeliveryMode::Steer),
            )
            .await;
            send(
                client,
                "follow",
                prompt(&running, "follow", DeliveryMode::FollowUp),
            )
            .await;
            // Native acknowledgement order is reversed; every wire reply retains its requestId.
            let mut ids = std::collections::BTreeSet::new();
            for _ in 0..2 {
                let ServerMessage::Response(frame) = receive(client, host, false).await else {
                    panic!("input response")
                };
                assert!(["steer", "follow"].contains(&frame.request_id.as_str()));
                assert!(ids.insert(frame.request_id));
                dispatched(frame.result);
            }
            assert_eq!(count(log, "steer"), 1);
            assert_eq!(count(log, "follow_up"), 1);
        }
        send(client, "stale-run", cancel(&running, "stale")).await;
        error(
            outcome(client, host, "stale-run", false).await,
            "STALE_RUN",
            false,
        );
        send(
            client,
            "cancel",
            cancel(&running, running.active_run_id.as_ref().unwrap()),
        )
        .await;
        let cancelled = outcome(client, host, "cancel", false).await;
        if case == "command-settle" {
            assert!(
                matches!(cancelled, RequestResult::Applied(value) if value.result == Response::Empty)
            );
            assert_eq!(count(log, "abort"), 0);
        } else {
            dispatched(cancelled);
            assert_eq!(count(log, "abort"), 1);
        }
        until(|| {
            host.poll_processes(Instant::now()).unwrap();
            view(host).session.state == SessionState::Ready
        });
        assert!(view(host).session.active_run_id.is_none());
        assert_eq!(count(log, "clear_queue"), 1);
    } else {
        until(|| {
            host.poll_processes(Instant::now()).unwrap();
            view(host).session.state == SessionState::Ready
        });
        let snapshot = view(host);
        assert!(snapshot.session.active_run_id.is_none());
        if case == "command-first" {
            assert!(
                serde_json::to_string(&snapshot)
                    .unwrap()
                    .contains("Final synthetic")
            );
        } else {
            assert_eq!(case, "command-handled");
            assert_eq!(count(log, "abort"), 0);
        }
    }
}
