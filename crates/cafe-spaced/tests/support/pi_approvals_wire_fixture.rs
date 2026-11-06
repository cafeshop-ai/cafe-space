//! Confirmation selection uses local IPC and native writer observations, never a tool policy.
use super::*;
use cafe_space_ipc::{framing::read_frame, local};
use cafe_space_protocol::*;
use cafe_spaced::{host::Host, update::Access};
use pi_approvals_fixture::approve;
use pi_commands_fixture::{cancel, dispatched, prompt};
use pi_models_wire_fixture::{error, receive, result, send};
use tokio::io::AsyncWriteExt;

async fn frame(client: &mut local::ClientStream, host: &Host, poll: bool) -> ServerMessage {
    let frame = if poll {
        receive(client, host, false).await
    } else {
        let bytes = tokio::time::timeout(DEADLINE, read_frame(client))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        decode_server_message(&bytes).unwrap()
    };
    let encoded = serde_json::to_string(&frame).unwrap();
    for private in [
        "private-confirm",
        "native-fixture-1",
        "synthetic-private",
        "never-forward",
    ] {
        assert!(
            !encoded.contains(private),
            "private runtime detail reached wire"
        );
    }
    frame
}

async fn outcome(
    client: &mut local::ClientStream,
    host: &Host,
    id: &str,
    poll: bool,
) -> RequestResult {
    result(frame(client, host, poll).await, id)
}

async fn snapshot(
    client: &mut local::ClientStream,
    host: &Host,
    session: &SessionSnapshot,
) -> SessionView {
    send(
        client,
        "snapshot",
        Request::GetSession(GetSessionParams {
            session_id: session.id.clone(),
        }),
    )
    .await;
    let RequestResult::Applied(value) = outcome(client, host, "snapshot", false).await else {
        panic!("snapshot")
    };
    let Response::Session(value) = value.result else {
        panic!("session view")
    };
    value.session
}

pub(super) async fn case(
    client: &mut local::ClientStream,
    host: &Host,
    session: &SessionSnapshot,
    case: &str,
    log: &Path,
    release: &Path,
) {
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
    let local_ui = matches!(case, "approval-local-ui" | "approval-large");
    let text = match case {
        "approval-local-ui" => "local-ui",
        "approval-large" => "large-approval",
        _ => "approval",
    };
    send(client, "prompt", prompt(session, text, DeliveryMode::Steer)).await;
    let (mut started, mut offered, mut waiting) = (false, None, false);
    while !started
        || if local_ui {
            !waiting
        } else {
            offered.is_none()
        }
    {
        match frame(client, host, true).await {
            ServerMessage::Response(response) => {
                assert_eq!(response.request_id, "prompt");
                dispatched(response.result);
                started = true;
            }
            ServerMessage::Event(ServerEvent {
                event: Event::SessionChanged(data),
                ..
            }) => {
                waiting |= data.session.state == SessionState::WaitingForLocalUi;
            }
            ServerMessage::Event(ServerEvent {
                event: Event::ApprovalRequired(data),
                ..
            }) => {
                assert!(!local_ui);
                assert_eq!(data.session_id, session.id);
                assert!(offered.replace(data.approval).is_none());
            }
            other => panic!("unexpected confirmation startup: {other:?}"),
        }
    }
    let current = snapshot(client, host, session).await;
    assert_eq!(
        current.session.capabilities.tool_approval,
        ToolApproval::NeverAsks
    );
    assert!(!current.session.capabilities.approval_for_session);
    if local_ui {
        assert!(current.approvals.is_empty());
        assert_eq!(current.session.state, SessionState::WaitingForLocalUi);
        assert!(
            !std::fs::read_to_string(log)
                .unwrap()
                .contains("extension_ui_response")
        );
        return;
    }
    let approval = offered.unwrap();
    assert_eq!(approval.id.len(), 32);
    assert_eq!(current.approvals, std::slice::from_ref(&approval));
    assert_eq!(approval.summary, "Synthetic confirmation");
    let session = &current.session;
    let decision = if case == "approval-deny" {
        ApprovalDecision::Deny
    } else {
        ApprovalDecision::AllowOnce
    };
    let selection = approve(session, &approval.id, decision.clone());
    if case == "approval-fences" {
        assert_eq!(
            host.command_pi(Access::Remote, selection.clone())
                .unwrap_err()
                .to_string(),
            "LOCAL_ONLY"
        );
        error(
            host.dispatch(Access::Remote, 0, selection.clone())
                .unwrap()
                .result,
            "LOCAL_ONLY",
            false,
        );
        for (stream, code) in [(true, "STALE_STREAM"), (false, "STALE_CWD_FENCE")] {
            let mut stale = session.clone();
            if stream {
                stale.stream_id = Some("stale".into());
            } else {
                stale.cwd_fence = "stale".into();
            }
            send(
                client,
                "stale",
                approve(&stale, &approval.id, decision.clone()),
            )
            .await;
            error(outcome(client, host, "stale", false).await, code, false);
        }
        send(
            client,
            "missing",
            approve(session, "different-public-id", decision.clone()),
        )
        .await;
        error(
            outcome(client, host, "missing", false).await,
            "PI_APPROVAL_NOT_FOUND",
            false,
        );
        assert!(
            !std::fs::read_to_string(log)
                .unwrap()
                .contains("extension_ui_response")
        );
    }
    if case == "approval-scope" {
        send(
            client,
            "scope",
            approve(session, &approval.id, ApprovalDecision::AllowForSession),
        )
        .await;
        error(
            outcome(client, host, "scope", false).await,
            "PI_APPROVAL_SCOPE_UNSUPPORTED",
            false,
        );
        assert!(
            !std::fs::read_to_string(log)
                .unwrap()
                .contains("extension_ui_response")
        );
    }
    if case == "approval-cancel" {
        send(
            client,
            "cancel",
            cancel(session, session.active_run_id.as_ref().unwrap()),
        )
        .await;
        let (mut cancelled, mut ready) = (false, false);
        while !cancelled || !ready {
            match frame(client, host, true).await {
                ServerMessage::Response(response) => {
                    assert_eq!(response.request_id, "cancel");
                    dispatched(response.result);
                    cancelled = true;
                }
                ServerMessage::Event(ServerEvent {
                    event: Event::SessionChanged(data),
                    ..
                }) => ready |= data.session.state == SessionState::Ready,
                other => panic!("unselected dialog must not invent a decision: {other:?}"),
            }
        }
        assert!(snapshot(client, host, session).await.approvals.is_empty());
        send(client, "expired", selection).await;
        error(
            outcome(client, host, "expired", false).await,
            "PI_APPROVAL_NOT_FOUND",
            false,
        );
        assert!(
            !std::fs::read_to_string(log)
                .unwrap()
                .contains("extension_ui_response")
        );
        return;
    }
    send(client, "approval", selection.clone()).await;
    if case == "approval-duplicate" {
        send(client, "duplicate", selection).await;
        error(
            outcome(client, host, "duplicate", false).await,
            "PI_APPROVAL_PENDING",
            false,
        );
    }
    // No Host poll: writer observations cannot yet resolve the selection, even if I/O has completed.
    send(client, "health", Request::GetStatus).await;
    assert!(matches!(
        outcome(client, host, "health", false).await,
        RequestResult::Applied(_)
    ));
    if case == "approval-disconnect" {
        client.shutdown().await.unwrap();
        return;
    }
    if case == "approval-close-pending" {
        send(
            client,
            "close",
            Request::CloseSession(CloseSessionParams {
                session_id: session.id.clone(),
                expected_stream_id: session.stream_id.clone().unwrap(),
                expected_cwd_fence: session.cwd_fence.clone(),
            }),
        )
        .await;
        let (mut closed, mut unknown, mut stopped) = (false, false, false);
        while !closed || !unknown || !stopped {
            match frame(client, host, false).await {
                ServerMessage::Response(response) if response.request_id == "close" => {
                    dispatched(response.result);
                    closed = true;
                }
                ServerMessage::Response(response) if response.request_id == "approval" => {
                    error(response.result, "PI_COMMAND_OUTCOME_UNKNOWN", true);
                    unknown = true;
                }
                ServerMessage::Event(ServerEvent {
                    event: Event::SessionChanged(data),
                    ..
                }) => stopped |= data.session.state == SessionState::Stopped,
                other => panic!("pending close must not invent a decision: {other:?}"),
            }
        }
        assert!(snapshot(client, host, session).await.approvals.is_empty());
        assert!(
            std::fs::read_to_string(log)
                .unwrap()
                .lines()
                .filter(|line| *line == "extension_ui_response")
                .count()
                <= 1
        );
        return;
    }
    let (mut dispatched_reply, mut resolved, mut ready, mut released) =
        (false, false, false, false);
    while !dispatched_reply || !resolved || !ready {
        match frame(client, host, true).await {
            ServerMessage::Response(response) => {
                assert_eq!(response.request_id, "approval");
                dispatched(response.result);
                assert!(!dispatched_reply);
                dispatched_reply = true;
            }
            ServerMessage::Event(ServerEvent {
                event: Event::ApprovalResolved(data),
                ..
            }) => {
                assert_eq!(data.session_id, session.id);
                assert_eq!(data.approval_id, approval.id);
                assert_eq!(data.decision, decision);
                assert!(!resolved);
                resolved = true;
            }
            ServerMessage::Event(ServerEvent {
                event: Event::SessionChanged(data),
                ..
            }) => ready |= data.session.state == SessionState::Ready,
            other => panic!("unexpected selection outcome: {other:?}"),
        }
        if dispatched_reply && resolved && !released {
            assert!(!ready, "writer dispatch is not settlement");
            std::fs::write(release, b"settle after writer dispatch").unwrap();
            released = true;
        }
    }
    assert!(snapshot(client, host, session).await.approvals.is_empty());
    let commands = std::fs::read_to_string(log).unwrap();
    assert_eq!(
        commands
            .lines()
            .filter(|line| *line == "extension_ui_response")
            .count(),
        1
    );
    assert!(commands.contains(if case == "approval-deny" {
        "confirmed=false"
    } else {
        "confirmed=true"
    }));
}
