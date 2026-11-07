use super::*;
use cafe_space_protocol::ApprovalDecision;
use serde_json::{Value, json};
use std::future::Future;

pub(super) fn fixture() -> Value {
    serde_json::from_str(include_str!("../../../../fixtures/acp/contract.json")).unwrap()
}
pub(super) struct Peer {
    pub(super) send: mpsc::Sender<io::Result<String>>,
    pub(super) receive: mpsc::Receiver<String>,
}
impl Peer {
    pub(super) async fn next(&mut self, method: &str) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(2), self.receive.recv())
            .await
            .unwrap()
            .unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["method"], method);
        value
    }
    pub(super) async fn send(&self, value: Value) {
        self.send.send(Ok(value.to_string())).await.unwrap();
    }
    pub(super) async fn respond(&self, request: &Value, result: Value) {
        self.send(json!({"jsonrpc":"2.0", "id":request["id"], "result":result}))
            .await;
    }
    pub(super) async fn open(&mut self, resume: bool) {
        let request = self.next("initialize").await;
        let typed: InitializeRequest = serde_json::from_value(request["params"].clone()).unwrap();
        assert_eq!(typed.protocol_version, ProtocolVersion::V1);
        assert!(!typed.client_capabilities.terminal);
        assert!(!typed.client_capabilities.fs.read_text_file);
        assert!(!typed.client_capabilities.fs.write_text_file);
        self.respond(&request, fixture()["initialized"]["result"].clone())
            .await;
        let request = self
            .next(if resume {
                "session/load"
            } else {
                "session/new"
            })
            .await;
        assert_eq!(request["params"]["mcpServers"], json!([]));
        if resume {
            assert_eq!(request["params"]["sessionId"], "private-synthetic-session");
        }
        self.respond(&request, fixture()["created"]["result"].clone())
            .await;
    }
    pub(super) async fn drain(mut self) {
        while self.receive.recv().await.is_some() {}
    }
}
pub(super) async fn exercise<F, Fut, P, Pfut>(resume: Option<String>, run: F, peer: P) -> Result<()>
where
    F: FnOnce(Session, mpsc::Receiver<Event>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
    P: FnOnce(Peer) -> Pfut,
    Pfut: Future<Output = ()>,
{
    let (send, input) = mpsc::channel(QUEUE);
    let (output, receive) = mpsc::channel(QUEUE);
    let (events, observed) = mpsc::channel(QUEUE);
    let root = tempfile::tempdir().unwrap();
    let client = connect(
        input,
        output,
        events,
        root.path().into(),
        resume,
        Box::new(move |session| Box::pin(run(session, observed))),
    );
    let (result, ()) = tokio::time::timeout(Duration::from_secs(35), async {
        tokio::join!(client, peer(Peer { send, receive }))
    })
    .await
    .expect("bounded synthetic exchange");
    result
}
pub(super) async fn opened(events: &mut mpsc::Receiver<Event>) {
    match events.recv().await.unwrap() {
        Event::Opened {
            native_id,
            modes,
            config,
        } => {
            assert_eq!(native_id, "private-synthetic-session");
            assert_eq!(modes.unwrap().current_mode_id.to_string(), "read-only");
            assert_eq!(config[0].id.to_string(), "model");
        }
        _ => panic!("expected opened"),
    }
}

#[tokio::test]
async fn initialize_new_prompt_and_native_settlement() {
    exercise(
        None,
        |session, mut events| async move {
            opened(&mut events).await;
            let prompt = fixture()["prompt"]["params"]["prompt"][0]["text"]
                .as_str()
                .unwrap()
                .to_owned();
            let result = session.prompt(prompt).await?;
            assert_eq!(result.stop_reason, StopReason::EndTurn);
            assert!(matches!(
                events.recv().await,
                Some(Event::Update(SessionUpdate::AgentMessageChunk(_)))
            ));
            assert_eq!(session.cancel().await, Err(Error("ACP_NOT_RUNNING")));
            Ok(())
        },
        |mut peer| async move {
            peer.open(false).await;
            let prompt = peer.next("session/prompt").await;
            assert_eq!(prompt["params"], fixture()["prompt"]["params"]);
            peer.send(fixture()["update"].clone()).await;
            peer.respond(&prompt, fixture()["completed"]["result"].clone())
                .await;
            peer.drain().await;
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn exact_load_and_stable_model_mode_configuration() {
    exercise(
        Some("private-synthetic-session".into()),
        |session, mut events| async move {
            opened(&mut events).await;
            assert_eq!(
                session.set_mode("unknown".into()).await,
                Err(Error("ACP_MODE_UNSUPPORTED"))
            );
            assert_eq!(
                session
                    .set_config("model".into(), "unknown".into())
                    .await
                    .err(),
                Some(Error("ACP_CONFIG_VALUE_INVALID"))
            );
            session.set_mode("read-only".into()).await?;
            let config = session
                .set_config("model".into(), "synthetic".into())
                .await?;
            assert_eq!(config[0].id.to_string(), "model");
            Ok(())
        },
        |mut peer| async move {
            peer.open(true).await;
            let mode = peer.next("session/set_mode").await;
            assert_eq!(mode["params"]["modeId"], "read-only");
            peer.respond(&mode, json!({})).await;
            let config = peer.next("session/set_config_option").await;
            assert_eq!(config["params"]["configId"], "model");
            assert_eq!(config["params"]["value"], "synthetic");
            peer.respond(
                &config,
                json!({"configOptions":fixture()["created"]["result"]["configOptions"]}),
            )
            .await;
            peer.drain().await;
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn permissions_select_kind_not_position_and_never_guess_persistent_scope() {
    exercise(
        None,
        |session, mut events| async move {
            opened(&mut events).await;
            let prompt = session.prompt("synthetic".into());
            let controls = async {
                let Some(Event::Permission { id, .. }) = events.recv().await else {
                    panic!("expected permission")
                };
                assert_eq!(
                    session.approve(id, ApprovalDecision::AllowForSession).await,
                    Err(Error("ACP_APPROVAL_UNSUPPORTED"))
                );
                session.approve(id, ApprovalDecision::AllowOnce).await?;
                assert_eq!(
                    session.approve(id, ApprovalDecision::Deny).await,
                    Err(Error("ACP_APPROVAL_NOT_FOUND"))
                );
                assert!(matches!(
                    events.recv().await,
                    Some(Event::PermissionClosed(_))
                ));
                Ok::<_, Error>(())
            };
            let (response, ()) = tokio::try_join!(prompt, controls)?;
            assert_eq!(response.stop_reason, StopReason::EndTurn);
            Ok(())
        },
        |mut peer| async move {
            peer.open(false).await;
            let prompt = peer.next("session/prompt").await;
            peer.send(fixture()["permission"].clone()).await;
            let response: Value =
                serde_json::from_str(&peer.receive.recv().await.unwrap()).unwrap();
            assert_eq!(response["id"], "permission-private-1");
            assert_eq!(
                response["result"]["outcome"],
                json!({"outcome":"selected", "optionId":"once"})
            );
            peer.respond(&prompt, fixture()["completed"]["result"].clone())
                .await;
            peer.drain().await;
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn cancel_does_not_settle_and_closes_pending_permissions() {
    exercise(
        None,
        |session, mut events| async move {
            opened(&mut events).await;
            let prompt = session.prompt("synthetic".into());
            let controls = async {
                assert!(matches!(
                    events.recv().await,
                    Some(Event::Permission { .. })
                ));
                assert_eq!(
                    session.prompt("cannot steer".into()).await.err(),
                    Some(Error("ACP_BUSY"))
                );
                session.cancel().await?;
                assert_eq!(session.cancel().await, Err(Error("ACP_CANCEL_PENDING")));
                assert_eq!(
                    session.prompt("not ready".into()).await.err(),
                    Some(Error("ACP_BUSY"))
                );
                Ok::<_, Error>(())
            };
            let (response, ()) = tokio::try_join!(prompt, controls)?;
            assert_eq!(response.stop_reason, StopReason::Cancelled);
            Ok(())
        },
        |mut peer| async move {
            peer.open(false).await;
            let prompt = peer.next("session/prompt").await;
            peer.send(fixture()["permission"].clone()).await;
            peer.next("session/cancel").await;
            let response: Value =
                serde_json::from_str(&peer.receive.recv().await.unwrap()).unwrap();
            assert_eq!(response["result"]["outcome"]["outcome"], "cancelled");
            peer.respond(&prompt, fixture()["cancelled"]["result"].clone())
                .await;
            peer.drain().await;
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn wrong_session_fails_closed_without_forwarding_output() {
    let result = exercise(
        None,
        |session, mut events| async move {
            opened(&mut events).await;
            session.prompt("synthetic".into()).await?;
            panic!("wrong session must not succeed")
        },
        |mut peer| async move {
            peer.open(false).await;
            peer.next("session/prompt").await;
            let mut update = fixture()["update"].clone();
            update["params"]["sessionId"] = "wrong-private-id".into();
            peer.send(update).await;
            peer.drain().await;
        },
    )
    .await;
    assert_eq!(result, Err(Error("ACP_SESSION_MISMATCH")));
}

#[tokio::test]
async fn malformed_and_oversized_frames_are_not_exposed_as_error_text() {
    for (input, expected) in [
        ("private secret not json".to_owned(), "ACP_RECORD_INVALID"),
        ("x".repeat(MAX_RECORD_BYTES + 1), "ACP_RECORD_TOO_LARGE"),
    ] {
        let result = exercise(
            None,
            |_, _| async { panic!("not ready") },
            |peer| async move {
                peer.send.send(Ok(input)).await.unwrap();
                peer.drain().await;
            },
        )
        .await;
        assert_eq!(result, Err(Error(expected)));
    }
}
