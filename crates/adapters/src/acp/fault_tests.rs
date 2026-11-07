use super::tests::*;
use super::*;
use futures::FutureExt;
use serde_json::{Value, json};

#[tokio::test]
async fn failed_resume_never_opens_a_replacement_session() {
    let result = exercise(Some("private-synthetic-session".into()), |_, _| async { panic!("not ready") }, |mut peer| async move {
        let init = peer.next("initialize").await;
        peer.respond(&init, fixture()["initialized"]["result"].clone()).await;
        let load = peer.next("session/load").await;
        peer.send(json!({"jsonrpc":"2.0", "id":load["id"], "error":{"code":-32000, "message":"private error must not escape"}})).await;
        assert!(peer.receive.recv().await.is_none());
    }).await;
    assert_eq!(result, Err(Error("ACP_REQUEST_FAILED")));
}

#[tokio::test]
async fn missing_load_capability_never_sends_load_or_new() {
    let result = exercise(
        Some("private-synthetic-session".into()),
        |_, _| async { panic!("not ready") },
        |mut peer| async move {
            let init = peer.next("initialize").await;
            peer.respond(&init, json!({"protocolVersion":1, "agentCapabilities":{}}))
                .await;
            assert!(peer.receive.recv().await.is_none());
        },
    )
    .await;
    assert_eq!(result, Err(Error("ACP_RESUME_UNSUPPORTED")));
}

#[tokio::test(start_paused = true)]
async fn incorrect_response_id_expires_without_retry() {
    let result = exercise(None, |_, _| async { panic!("not ready") }, |mut peer| async move {
        peer.next("initialize").await;
        peer.send(json!({"jsonrpc":"2.0", "id":"unrelated", "result":fixture()["initialized"]["result"]})).await;
        assert!(peer.receive.recv().await.is_none());
    }).await;
    assert_eq!(result, Err(Error("ACP_RESPONSE_TIMEOUT")));
}

#[tokio::test]
async fn crash_after_prompt_is_unknown_and_never_replayed() {
    let result = exercise(
        None,
        |session, mut events| async move {
            opened(&mut events).await;
            session.prompt("synthetic".into()).await?;
            panic!("EOF cannot settle a prompt")
        },
        |mut peer| async move {
            peer.open(false).await;
            peer.next("session/prompt").await;
            drop(peer.send);
            assert!(peer.receive.recv().await.is_none());
        },
    )
    .await;
    assert_eq!(result, Err(Error("ACP_COMMAND_OUTCOME_UNKNOWN")));
}

#[tokio::test]
async fn dropped_prompt_poison_cannot_be_swallowed_by_successful_callback() {
    let result = exercise(
        None,
        |session, mut events| async move {
            opened(&mut events).await;
            assert!(session.prompt("synthetic".into()).now_or_never().is_none());
            assert_eq!(
                session.prompt("never replay".into()).await.err(),
                Some(Error("ACP_COMMAND_OUTCOME_UNKNOWN"))
            );
            Ok(())
        },
        |mut peer| async move {
            peer.open(false).await;
            peer.drain().await;
        },
    )
    .await;
    assert_eq!(result, Err(Error("ACP_COMMAND_OUTCOME_UNKNOWN")));
}

#[tokio::test(start_paused = true)]
async fn cancel_without_correlated_completion_expires_unknown() {
    let result = exercise(
        None,
        |session, mut events| async move {
            opened(&mut events).await;
            let prompt = session.prompt("synthetic".into());
            let cancel = async {
                assert!(matches!(events.recv().await, Some(Event::Update(_))));
                session.cancel().await?;
                Ok::<_, Error>(())
            };
            tokio::try_join!(prompt, cancel)?;
            panic!("cancel delivery cannot settle")
        },
        |mut peer| async move {
            peer.open(false).await;
            peer.next("session/prompt").await;
            peer.send(fixture()["update"].clone()).await;
            peer.next("session/cancel").await;
            assert!(peer.receive.recv().await.is_none());
        },
    )
    .await;
    assert_eq!(result, Err(Error("ACP_CANCEL_OUTCOME_UNKNOWN")));
}

#[tokio::test]
async fn unresolved_permissions_prevent_ready_and_duplicate_options_are_rejected() {
    for duplicate in [false, true] {
        let result = exercise(
            None,
            |session, mut events| async move {
                opened(&mut events).await;
                session.prompt("synthetic".into()).await?;
                panic!("unresolved or ambiguous permission cannot succeed")
            },
            |mut peer| async move {
                peer.open(false).await;
                let prompt = peer.next("session/prompt").await;
                let mut permission = fixture()["permission"].clone();
                if duplicate {
                    permission["params"]["options"][0]["optionId"] = "once".into();
                }
                peer.send(permission).await;
                peer.respond(&prompt, fixture()["completed"]["result"].clone())
                    .await;
                peer.drain().await;
            },
        )
        .await;
        assert_eq!(
            result,
            Err(Error(if duplicate {
                "ACP_PERMISSION_INVALID"
            } else {
                "ACP_PERMISSION_UNRESOLVED"
            }))
        );
    }
}

#[tokio::test]
async fn incomplete_config_ack_cannot_be_reported_applied() {
    let result = exercise(
        None,
        |session, mut events| async move {
            opened(&mut events).await;
            session
                .set_config("model".into(), "synthetic".into())
                .await?;
            panic!("unconfirmed config must fail")
        },
        |mut peer| async move {
            peer.open(false).await;
            let request = peer.next("session/set_config_option").await;
            peer.respond(&request, json!({"configOptions":[]})).await;
            peer.drain().await;
        },
    )
    .await;
    assert_eq!(result, Err(Error("ACP_CONFIG_UNCONFIRMED")));
}

#[tokio::test]
async fn one_bad_config_does_not_remove_valid_choices() {
    exercise(
        None,
        |session, mut events| async move {
            opened(&mut events).await;
            session
                .set_config("model".into(), "synthetic".into())
                .await?;
            Ok(())
        },
        |mut peer| async move {
            let init = peer.next("initialize").await;
            peer.respond(&init, fixture()["initialized"]["result"].clone())
                .await;
            let new = peer.next("session/new").await;
            let mut created = fixture()["created"]["result"].clone();
            created["configOptions"]
                .as_array_mut()
                .unwrap()
                .insert(0, json!({"bad":"item"}));
            peer.respond(&new, created.clone()).await;
            let config = peer.next("session/set_config_option").await;
            peer.respond(&config, json!({"configOptions":created["configOptions"]}))
                .await;
            peer.drain().await;
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn slow_observer_backpressures_sdk_input_without_unbounded_growth() {
    let (sdk, transport) = Channel::duplex();
    let queued = transport.tx.clone();
    let (send, input) = mpsc::channel(QUEUE);
    let (output, _receive) = mpsc::channel(QUEUE);
    let bridge = tokio::spawn(transport::bridge(transport, input, output));
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let sent = count.clone();
    let writer = tokio::spawn(async move {
        for _ in 0..1000 {
            if send
                .send(Ok(fixture()["update"].to_string()))
                .await
                .is_err()
            {
                break;
            }
            sent.fetch_add(1, Ordering::Release);
        }
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(queued.len(), QUEUE);
    assert!(count.load(Ordering::Acquire) <= QUEUE * 2);
    bridge.abort();
    writer.abort();
    let _ = tokio::join!(bridge, writer);
    drop(sdk);
}

#[test]
fn grouped_choices_and_duplicate_ids_are_not_guessed() {
    let group: SessionConfigOption = serde_json::from_value(json!({"id":"model", "name":"Model", "type":"select", "currentValue":"a", "options":[{"group":"g", "name":"Group", "options":[{"value":"a", "name":"A"}]}]})).unwrap();
    assert_eq!(
        config::validate(std::slice::from_ref(&group), "model", "a"),
        Ok(())
    );
    assert_eq!(
        config::validate(&[group.clone(), group], "model", "a"),
        Err(Error("ACP_CONFIG_AMBIGUOUS"))
    );
    let _typed: InitializeRequest =
        serde_json::from_value(fixture()["initialize"]["params"].clone()).unwrap();
    assert_eq!(fixture()["sdkVersion"], "5.0.0");
    assert_eq!(fixture()["packageVersion"], PACKAGE_VERSION);
    let _: Value = fixture();
}
