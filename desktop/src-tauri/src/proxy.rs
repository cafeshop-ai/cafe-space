//! One window, one owner-only IPC connection. Queued commands are never replayed.
use cafe_space_ipc::{
    BuildFlavor,
    framing::{read_frame, write_frame},
    local,
};
use cafe_space_protocol::{
    ClientHello, ClientMessage, HelloMessageType, RequestResult, Response, ServerMessage,
    decode_client_message, decode_server_message, encode_client_message,
};
use serde::Serialize;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
    time::Duration,
};
use tauri::ipc::Channel;
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use ts_rs::TS;

const DEADLINE: Duration = Duration::from_secs(5);
const QUEUE_SIZE: usize = 16;

#[derive(Clone, Serialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum DaemonDelivery {
    Message {
        connection_id: String,
        sequence: u32,
        message: Box<ServerMessage>,
    },
    Closed {
        connection_id: String,
    },
}

struct Connection {
    id: String,
    outgoing: mpsc::Sender<Vec<u8>>,
    acknowledgments: mpsc::Sender<u32>,
    _stop: oneshot::Sender<()>,
    proof: Arc<AtomicU8>,
    chat: watch::Receiver<Option<u32>>,
}

#[derive(Default)]
pub struct Proxy {
    connection: Mutex<Option<Connection>>,
    next_id: AtomicU64,
}

impl Proxy {
    pub async fn open(
        &self,
        flavor: BuildFlavor,
        events: Channel<DaemonDelivery>,
    ) -> Result<String, &'static str> {
        let endpoint = match crate::probe::endpoint()? {
            Some(endpoint) => endpoint,
            None => local::endpoint(flavor).map_err(|_| "IPC_ENDPOINT_UNAVAILABLE")?,
        };
        self.open_at(&endpoint, events).await
    }

    async fn open_at(
        &self,
        endpoint: &Path,
        events: Channel<DaemonDelivery>,
    ) -> Result<String, &'static str> {
        let mut slot = self.connection.lock().await;
        if slot.is_some() {
            return Err("IPC_ALREADY_CONNECTED");
        }
        let hello = encode_client_message(&ClientMessage::Hello(ClientHello {
            message_type: HelloMessageType::Hello,
            supported_protocol_versions: vec![cafe_space_protocol::PROTOCOL_VERSION],
            client_version: env!("CARGO_PKG_VERSION").into(),
        }))
        .map_err(|_| "IPC_INVALID_HELLO")?;
        let mut stream = tokio::time::timeout(DEADLINE, local::connect(endpoint))
            .await
            .map_err(|_| "IPC_CONNECT_TIMEOUT")?
            .map_err(|_| "IPC_CONNECT_FAILED")?;
        tokio::time::timeout(DEADLINE, write_frame(&mut stream, &hello))
            .await
            .map_err(|_| "IPC_HELLO_TIMEOUT")?
            .map_err(|_| "IPC_HELLO_FAILED")?;
        let previous = self
            .next_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| "IPC_CONNECTION_LIMIT")?;
        let id = (previous + 1).to_string();
        let (outgoing, mut queued) = mpsc::channel::<Vec<u8>>(QUEUE_SIZE);
        let (acknowledgments, mut acked) = mpsc::channel::<u32>(1);
        let (stop, stopped) = oneshot::channel::<()>();
        let proof = Arc::new(AtomicU8::new(0));
        let observed = proof.clone();
        let (chat_proof, chat) = watch::channel(None);
        let connection_id = id.clone();
        tokio::spawn(async move {
            let (mut reader, mut writer) = tokio::io::split(stream);
            let receiving = async {
                let mut sequence = 0u32;
                loop {
                    let frame = if sequence == 0 {
                        tokio::time::timeout(DEADLINE, read_frame(&mut reader))
                            .await
                            .map_err(|_| ())?
                    } else {
                        read_frame(&mut reader).await
                    };
                    let bytes = frame.map_err(|_| ())?.ok_or(())?;
                    let message = decode_server_message(&bytes).map_err(|_| ())?;
                    if sequence == 0
                        && !matches!(
                            message,
                            ServerMessage::Welcome(_) | ServerMessage::VersionMismatch(_)
                        )
                    {
                        return Err::<(), ()>(());
                    }
                    sequence = sequence.checked_add(1).ok_or(())?;
                    if let ServerMessage::Response(response) = &message
                        && matches!(&response.result, RequestResult::Applied(data) if matches!(&data.result, Response::Sessions(list) if list.items.is_empty()))
                    {
                        observed.fetch_or(1, Ordering::Release);
                    }
                    let confirms_chat = crate::probe::confirms_chat(&message);
                    if confirms_chat {
                        chat_proof.send_replace(None);
                    }
                    events
                        .send(DaemonDelivery::Message {
                            connection_id: connection_id.clone(),
                            sequence,
                            message: Box::new(message),
                        })
                        .map_err(|_| ())?;
                    // Tauri's internal channel queue has no application backpressure: allow only one unacknowledged frame.
                    let received = tokio::time::timeout(DEADLINE, acked.recv())
                        .await
                        .map_err(|_| ())?
                        .ok_or(())?;
                    if received != sequence {
                        return Err(());
                    }
                    if confirms_chat {
                        chat_proof.send_replace(Some(sequence));
                    }
                }
            };
            let sending = async {
                while let Some(frame) = queued.recv().await {
                    write_frame(&mut writer, &frame).await.map_err(|_| ())?;
                }
                Ok::<(), ()>(())
            };
            // Cancellation always drops both halves; a partial frame is never resumed.
            tokio::select! { _ = receiving => {}, _ = sending => {}, _ = stopped => {} }
            drop(chat_proof);
            let _ = events.send(DaemonDelivery::Closed { connection_id });
        });
        *slot = Some(Connection {
            id: id.clone(),
            outgoing,
            acknowledgments,
            _stop: stop,
            proof,
            chat,
        });
        Ok(id)
    }

    pub async fn send(&self, id: &str, frame: &str) -> Result<(), &'static str> {
        let message = decode_client_message(frame.as_bytes()).map_err(|_| "IPC_INVALID_REQUEST")?;
        if !matches!(message, ClientMessage::Request(_)) {
            return Err("IPC_REQUEST_REQUIRED");
        }
        let frame = encode_client_message(&message).map_err(|_| "IPC_INVALID_REQUEST")?;
        let slot = self.connection.lock().await;
        let connection = slot
            .as_ref()
            .filter(|c| c.id == id)
            .ok_or("IPC_STALE_CONNECTION")?;
        connection
            .outgoing
            .try_send(frame)
            .map_err(|_| "IPC_QUEUE_FULL_OR_CLOSED")
    }

    pub async fn ack(&self, id: &str, sequence: u32) -> Result<(), &'static str> {
        let slot = self.connection.lock().await;
        let connection = slot
            .as_ref()
            .filter(|c| c.id == id)
            .ok_or("IPC_STALE_CONNECTION")?;
        connection
            .acknowledgments
            .try_send(sequence)
            .map_err(|_| "IPC_ACK_QUEUE_FULL_OR_CLOSED")
    }

    pub async fn close(&self, id: &str) -> Result<(), &'static str> {
        let mut slot = self.connection.lock().await;
        if slot.as_ref().is_some_and(|c| c.id == id) {
            slot.take();
            Ok(())
        } else {
            Err("IPC_STALE_CONNECTION")
        }
    }

    pub async fn confirmed_empty(&self) -> bool {
        self.connection
            .lock()
            .await
            .as_ref()
            .is_some_and(|c| c.proof.load(Ordering::Acquire) & 1 != 0)
    }

    pub async fn confirmed_chat(&self) -> bool {
        tokio::time::timeout(DEADLINE, async {
            let (id, mut chat) = {
                let slot = self.connection.lock().await;
                let Some(connection) = slot.as_ref() else {
                    return false;
                };
                (connection.id.clone(), connection.chat.clone())
            };
            // Retain the receipt across both scheduling orders; queue admission is not ACK consumption.
            let sequence = match chat.wait_for(|value| value.is_some()).await {
                Ok(value) => *value,
                Err(_) => return false,
            };
            self.connection
                .lock()
                .await
                .as_ref()
                .is_some_and(|connection| {
                    connection.id == id
                        && connection.chat.has_changed().is_ok()
                        && *connection.chat.borrow() == sequence
                })
        })
        .await
        .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_endpoint(temp: &tempfile::TempDir) -> std::path::PathBuf {
        #[cfg(unix)]
        let path = temp.path().join("ipc/s");
        #[cfg(windows)]
        let path = std::path::PathBuf::from(format!(
            r"\\.\pipe\cafe-space-proxy-test-{}-{}",
            std::process::id(),
            temp.path().file_name().unwrap().to_str().unwrap()
        ));
        path
    }

    struct ChatFixture {
        _temp: tempfile::TempDir,
        id: String,
        deliveries: mpsc::UnboundedReceiver<tauri::ipc::InvokeResponseBody>,
        server: tokio::task::JoinHandle<()>,
        cut: oneshot::Sender<()>,
        repeat: mpsc::Sender<()>,
    }

    async fn chat_fixture(proxy: &Proxy) -> ChatFixture {
        let temp = tempfile::tempdir().unwrap();
        let endpoint = test_endpoint(&temp);
        let mut listener = local::Listener::bind(&endpoint).await.unwrap();
        let (received, mut deliveries) = mpsc::unbounded_channel();
        let channel = Channel::new(move |body| {
            received.send(body).unwrap();
            Ok(())
        });
        let mut frame: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/v2/session-response.json"))
                .unwrap();
        let view = &mut frame["result"]["data"]["result"]["data"]["session"];
        view["session"]["id"] = crate::probe::SESSION.into();
        view["session"]["state"] = serde_json::json!({"state":"stopped"});
        view["session"]["stopReason"] = "SESSION_CLOSED".into();
        view["session"]
            .as_object_mut()
            .unwrap()
            .remove("activeRunId");
        view["messages"] = serde_json::json!([{"id":"synthetic", "role":"assistant", "status":"complete", "parts":[{"kind":"text", "data":{"text":crate::probe::TEXT}}]}]);
        let bytes = serde_json::to_vec(&frame).unwrap();
        assert!(crate::probe::confirms_chat(
            &decode_server_message(&bytes).unwrap()
        ));
        let (cut, mut cutoff) = oneshot::channel();
        let (repeat, mut repeats) = mpsc::channel(1);
        let server = tokio::spawn(async move {
            let mut stream = listener.accept().await.unwrap();
            read_frame(&mut stream).await.unwrap().unwrap();
            write_frame(
                &mut stream,
                include_bytes!("../../../fixtures/v2/server-welcome.json"),
            )
            .await
            .unwrap();
            write_frame(&mut stream, &bytes).await.unwrap();
            loop {
                tokio::select! {
                    result = read_frame(&mut stream) => { assert!(result.unwrap().is_none()); break; },
                    _ = &mut cutoff => break,
                    Some(()) = repeats.recv() => write_frame(&mut stream, &bytes).await.unwrap(),
                }
            }
        });
        let id = proxy.open_at(&endpoint, channel).await.unwrap();
        for sequence in [1, 2] {
            let tauri::ipc::InvokeResponseBody::Json(body) =
                tokio::time::timeout(DEADLINE, deliveries.recv())
                    .await
                    .unwrap()
                    .unwrap()
            else {
                panic!("JSON delivery required")
            };
            let value: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(value["connectionId"], id);
            assert_eq!(value["sequence"], sequence);
            if sequence == 1 {
                proxy.ack(&id, sequence).await.unwrap();
            }
        }
        ChatFixture {
            _temp: temp,
            id,
            deliveries,
            server,
            cut,
            repeat,
        }
    }

    async fn assert_pending(mut future: std::pin::Pin<&mut impl std::future::Future>) {
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(future.as_mut().poll(cx)))
                .await
                .is_pending()
        );
    }

    #[tokio::test]
    async fn final_chat_ack_is_consumed_not_just_queued_and_survives_both_wait_orders() {
        for wait_first in [true, false] {
            let proxy = Proxy::default();
            let fixture = chat_fixture(&proxy).await;
            if wait_first {
                let confirmed = proxy.confirmed_chat();
                tokio::pin!(confirmed);
                assert_pending(confirmed.as_mut()).await;
                proxy.ack(&fixture.id, 2).await.unwrap();
                assert_pending(confirmed.as_mut()).await;
                assert!(confirmed.await);
            } else {
                proxy.ack(&fixture.id, 2).await.unwrap();
                let mut receipt = proxy.connection.lock().await.as_ref().unwrap().chat.clone();
                assert_eq!(
                    *tokio::time::timeout(DEADLINE, receipt.wait_for(|value| value.is_some()))
                        .await
                        .unwrap()
                        .unwrap(),
                    Some(2)
                );
                assert!(proxy.confirmed_chat().await);
            }
            proxy.close(&fixture.id).await.unwrap();
            assert!(!proxy.confirmed_chat().await);
            tokio::time::timeout(DEADLINE, fixture.server)
                .await
                .unwrap()
                .unwrap();
        }
    }

    #[tokio::test]
    async fn a_new_final_snapshot_cannot_reuse_the_previous_delivery_receipt() {
        let proxy = Proxy::default();
        let mut fixture = chat_fixture(&proxy).await;
        proxy.ack(&fixture.id, 2).await.unwrap();
        assert!(proxy.confirmed_chat().await);
        fixture.repeat.send(()).await.unwrap();
        let tauri::ipc::InvokeResponseBody::Json(body) =
            tokio::time::timeout(DEADLINE, fixture.deliveries.recv())
                .await
                .unwrap()
                .unwrap()
        else {
            panic!("JSON delivery required")
        };
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["sequence"], 3);
        let confirmed = proxy.confirmed_chat();
        tokio::pin!(confirmed);
        assert_pending(confirmed.as_mut()).await;
        proxy.ack(&fixture.id, 3).await.unwrap();
        assert!(confirmed.await);
        proxy.close(&fixture.id).await.unwrap();
        tokio::time::timeout(DEADLINE, fixture.server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn missing_or_wrong_final_chat_ack_fails_without_a_success_receipt() {
        for wrong in [false, true] {
            let proxy = Proxy::default();
            let mut fixture = chat_fixture(&proxy).await;
            let confirmed = proxy.confirmed_chat();
            tokio::pin!(confirmed);
            assert_pending(confirmed.as_mut()).await;
            if wrong {
                proxy.ack(&fixture.id, 3).await.unwrap();
            }
            assert!(!confirmed.await);
            assert!(
                matches!(tokio::time::timeout(DEADLINE, fixture.deliveries.recv()).await.unwrap().unwrap(), tauri::ipc::InvokeResponseBody::Json(body) if body.contains("closed"))
            );
            assert!(!proxy.confirmed_chat().await);
            proxy.close(&fixture.id).await.unwrap();
            tokio::time::timeout(DEADLINE, fixture.server)
                .await
                .unwrap()
                .unwrap();
        }
    }

    #[tokio::test]
    async fn replacing_a_connection_cannot_satisfy_its_previous_final_ack_waiter() {
        let proxy = Proxy::default();
        let old = chat_fixture(&proxy).await;
        let confirmed = proxy.confirmed_chat();
        tokio::pin!(confirmed);
        assert_pending(confirmed.as_mut()).await;
        proxy.close(&old.id).await.unwrap();
        let new = chat_fixture(&proxy).await;
        assert_ne!(old.id, new.id);
        assert!(proxy.ack(&old.id, 2).await.is_err());
        proxy.ack(&new.id, 2).await.unwrap();
        assert!(proxy.confirmed_chat().await);
        assert!(!confirmed.await);
        proxy.close(&new.id).await.unwrap();
        for server in [old.server, new.server] {
            tokio::time::timeout(DEADLINE, server)
                .await
                .unwrap()
                .unwrap();
        }
    }

    #[tokio::test]
    async fn receiver_exit_revokes_final_ack_even_while_the_connection_slot_remains() {
        let proxy = Proxy::default();
        let mut fixture = chat_fixture(&proxy).await;
        proxy.ack(&fixture.id, 2).await.unwrap();
        assert!(proxy.confirmed_chat().await);
        fixture.cut.send(()).unwrap();
        assert!(
            matches!(tokio::time::timeout(DEADLINE, fixture.deliveries.recv()).await.unwrap().unwrap(), tauri::ipc::InvokeResponseBody::Json(body) if body.contains("closed"))
        );
        assert!(proxy.connection.lock().await.is_some());
        assert!(!proxy.confirmed_chat().await);
        proxy.close(&fixture.id).await.unwrap();
        tokio::time::timeout(DEADLINE, fixture.server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn slow_ui_is_bounded_to_one_unacknowledged_channel_delivery() {
        let temp = tempfile::tempdir().unwrap();
        let endpoint = test_endpoint(&temp);
        let mut listener = local::Listener::bind(&endpoint).await.unwrap();
        let (received, mut deliveries) = mpsc::unbounded_channel();
        let channel = Channel::new(move |body| {
            received.send(body).unwrap();
            Ok(())
        });
        let server = async {
            let mut stream = listener.accept().await.unwrap();
            read_frame(&mut stream).await.unwrap().unwrap();
            for frame in [
                include_bytes!("../../../fixtures/v2/server-welcome.json").as_slice(),
                include_bytes!("../../../fixtures/v2/sessions-response.json").as_slice(),
            ] {
                write_frame(&mut stream, frame).await.unwrap();
            }
            // Unix may reset a connection closed with an unread response still buffered.
            match read_frame(&mut stream).await {
                Ok(None) => {}
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
                other => panic!("expected closed transport, got {other:?}"),
            }
        };
        let client = async {
            let proxy = Proxy::default();
            let id = proxy.open_at(&endpoint, channel).await.unwrap();
            assert!(
                matches!(deliveries.recv().await.unwrap(), tauri::ipc::InvokeResponseBody::Json(ref body) if body.contains("welcome"))
            );
            assert!(
                matches!(deliveries.recv().await.unwrap(), tauri::ipc::InvokeResponseBody::Json(ref body) if body.contains("closed"))
            );
            assert!(deliveries.try_recv().is_err());
            proxy.close(&id).await.unwrap();
        };
        tokio::time::timeout(DEADLINE + Duration::from_secs(3), async {
            tokio::join!(server, client)
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn channel_preserves_input_outcomes_and_events_while_prompt_is_pending() {
        let commands: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/pi-rpc/commands-wire-v5.json"
        ))
        .unwrap();
        let approvals: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/pi-rpc/approvals-wire-v5.json"
        ))
        .unwrap();
        let requests = [
            commands["requests"][0].clone(),
            commands["requests"][1].clone(),
            commands["requests"][2].clone(),
            approvals["requests"][0].clone(),
        ];
        // The first prompt stays unanswered while cancel, rejection and approval events arrive.
        let replies = [
            commands["responses"][2].clone(),
            commands["responses"][1].clone(),
            approvals["events"][0].clone(),
            approvals["responses"][0].clone(),
            approvals["events"][1].clone(),
            commands["responses"][0].clone(),
        ];
        let temp = tempfile::tempdir().unwrap();
        let endpoint = test_endpoint(&temp);
        let mut listener = local::Listener::bind(&endpoint).await.unwrap();
        let (received, mut deliveries) = mpsc::unbounded_channel();
        let channel = Channel::new(move |body| {
            received.send(body).unwrap();
            Ok(())
        });
        let server = async {
            let mut stream = listener.accept().await.unwrap();
            assert!(matches!(
                decode_client_message(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap(),
                ClientMessage::Hello(_)
            ));
            write_frame(
                &mut stream,
                include_bytes!("../../../fixtures/v2/server-welcome.json"),
            )
            .await
            .unwrap();
            for expected in &requests {
                let bytes = read_frame(&mut stream).await.unwrap().unwrap();
                decode_client_message(&bytes).unwrap();
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
                    *expected
                );
            }
            for reply in &replies {
                let bytes = serde_json::to_vec(reply).unwrap();
                decode_server_message(&bytes).unwrap();
                write_frame(&mut stream, &bytes).await.unwrap();
            }
            // Closing the proxy sends no extra request or replay of the unknown cancel.
            assert!(read_frame(&mut stream).await.unwrap().is_none());
        };
        let client = async {
            let proxy = Proxy::default();
            let id = proxy.open_at(&endpoint, channel).await.unwrap();
            let welcome = deliveries.recv().await.unwrap();
            assert!(
                matches!(welcome, tauri::ipc::InvokeResponseBody::Json(ref body) if body.contains("welcome"))
            );
            proxy.ack(&id, 1).await.unwrap();
            for request in &requests {
                proxy.send(&id, &request.to_string()).await.unwrap();
            }
            for (index, expected) in replies.iter().enumerate() {
                let tauri::ipc::InvokeResponseBody::Json(body) = deliveries.recv().await.unwrap()
                else {
                    panic!("expected JSON channel delivery")
                };
                let delivery: serde_json::Value = serde_json::from_str(&body).unwrap();
                let sequence = u32::try_from(index).unwrap() + 2;
                assert_eq!(delivery["kind"], "message");
                assert_eq!(delivery["connectionId"], id);
                assert_eq!(delivery["sequence"], sequence);
                assert_eq!(delivery["message"], *expected);
                if index + 1 < replies.len() {
                    proxy.ack(&id, sequence).await.unwrap();
                }
            }
            assert!(!proxy.confirmed_empty().await);
            proxy.close(&id).await.unwrap();
            assert!(
                matches!(deliveries.recv().await.unwrap(), tauri::ipc::InvokeResponseBody::Json(ref body) if body.contains("closed"))
            );
        };
        tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(server, client)
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn channel_forwards_real_ipc_and_fences_old_connections() {
        let temp = tempfile::tempdir().unwrap();
        let endpoint = test_endpoint(&temp);
        let mut listener = local::Listener::bind(&endpoint).await.unwrap();
        let (received, mut deliveries) = mpsc::unbounded_channel();
        let channel = Channel::new(move |body| {
            received.send(body).unwrap();
            Ok(())
        });
        let server = async {
            let mut stream = listener.accept().await.unwrap();
            assert!(matches!(
                decode_client_message(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap(),
                ClientMessage::Hello(_)
            ));
            write_frame(
                &mut stream,
                include_bytes!("../../../fixtures/v2/server-welcome.json"),
            )
            .await
            .unwrap();
            assert!(matches!(
                decode_client_message(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap(),
                ClientMessage::Request(_)
            ));
            write_frame(
                &mut stream,
                include_bytes!("../../../fixtures/v2/sessions-response.json"),
            )
            .await
            .unwrap();
            assert!(read_frame(&mut stream).await.unwrap().is_none());
        };
        let client = async {
            let proxy = Proxy::default();
            let id = proxy.open_at(&endpoint, channel.clone()).await.unwrap();
            assert_eq!(
                proxy.open_at(&endpoint, channel).await.unwrap_err(),
                "IPC_ALREADY_CONNECTED"
            );
            let welcome = deliveries.recv().await.unwrap();
            assert!(
                matches!(welcome, tauri::ipc::InvokeResponseBody::Json(ref body) if body.contains("welcome"))
            );
            assert!(proxy.close("stale").await.is_err());
            proxy.ack(&id, 1).await.unwrap();
            assert!(
                proxy
                    .send(
                        "stale",
                        include_str!("../../../fixtures/v2/client-request.json")
                    )
                    .await
                    .is_err()
            );
            assert!(
                proxy
                    .send(&id, include_str!("../../../fixtures/v2/client-hello.json"))
                    .await
                    .is_err()
            );
            assert!(
                proxy
                    .send(&id, "{\"type\":\"request\",\"unknown\":1}")
                    .await
                    .is_err()
            );
            proxy
                .send(
                    &id,
                    include_str!("../../../fixtures/v2/client-request.json"),
                )
                .await
                .unwrap();
            let response = deliveries.recv().await.unwrap();
            assert!(
                matches!(response, tauri::ipc::InvokeResponseBody::Json(ref body) if body.contains("sessions"))
            );
            proxy.close(&id).await.unwrap();
            assert!(proxy.ack(&id, 2).await.is_err());
            assert!(
                matches!(deliveries.recv().await.unwrap(), tauri::ipc::InvokeResponseBody::Json(ref body) if body.contains("closed"))
            );
        };
        tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(server, client)
        })
        .await
        .unwrap();
    }
}
