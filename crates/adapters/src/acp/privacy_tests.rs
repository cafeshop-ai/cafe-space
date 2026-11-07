use super::tests::*;
use super::*;
use std::sync::atomic::AtomicUsize;
use tracing::{Subscriber, instrument::WithSubscriber};

struct CountEvents(Arc<AtomicUsize>);
impl Subscriber for CountEvents {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, _: &tracing::Event<'_>) {
        self.0.fetch_add(1, Ordering::Release);
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

#[tokio::test]
async fn sdk_diagnostics_cannot_leak_native_ids_into_host_tracing() {
    let count = Arc::new(AtomicUsize::new(0));
    async {
        tracing::info!("synthetic collector self-check");
        exercise(
            None,
            |session, mut events| async move {
                opened(&mut events).await;
                let prompt = session.prompt("synthetic-private-text".into());
                let approve = async {
                    let Some(Event::Permission { id, .. }) = events.recv().await else {
                        panic!("missing permission")
                    };
                    session
                        .approve(id, cafe_space_protocol::ApprovalDecision::Deny)
                        .await
                };
                tokio::try_join!(prompt, approve)?;
                Ok(())
            },
            |mut peer| async move {
                peer.open(false).await;
                let prompt = peer.next("session/prompt").await;
                peer.send(fixture()["permission"].clone()).await;
                peer.receive.recv().await.unwrap();
                peer.respond(&prompt, fixture()["completed"]["result"].clone())
                    .await;
                peer.drain().await;
            },
        )
        .await
        .unwrap();
    }
    .with_subscriber(CountEvents(count.clone()))
    .await;
    assert_eq!(count.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn incompatible_acp_version_never_starts_a_session() {
    let result = exercise(
        None,
        |_, _| async { panic!("incompatible peer cannot be ready") },
        |mut peer| async move {
            let init = peer.next("initialize").await;
            let mut response = fixture()["initialized"]["result"].clone();
            response["protocolVersion"] = 2.into();
            peer.respond(&init, response).await;
            assert!(peer.receive.recv().await.is_none());
        },
    )
    .await;
    assert!(matches!(
        result,
        Err(Error("ACP_VERSION_UNSUPPORTED" | "ACP_REQUEST_FAILED"))
    ));
}

#[tokio::test]
async fn permissions_received_after_cancel_are_cancelled_without_new_ui() {
    exercise(
        None,
        |session, mut events| async move {
            opened(&mut events).await;
            let prompt = session.prompt("synthetic".into());
            let cancel = async {
                assert!(matches!(events.recv().await, Some(Event::Update(_))));
                session.cancel().await
            };
            let (response, ()) = tokio::try_join!(prompt, cancel)?;
            assert_eq!(response.stop_reason, StopReason::Cancelled);
            assert!(events.try_recv().is_err());
            Ok(())
        },
        |mut peer| async move {
            peer.open(false).await;
            let prompt = peer.next("session/prompt").await;
            peer.send(fixture()["update"].clone()).await;
            peer.next("session/cancel").await;
            peer.send(fixture()["permission"].clone()).await;
            let response: serde_json::Value =
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
