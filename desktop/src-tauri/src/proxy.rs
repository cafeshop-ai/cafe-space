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
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tauri::ipc::Channel;
use tokio::sync::{Mutex, mpsc, oneshot};
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
    confirmed_empty: Arc<AtomicBool>,
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
        let endpoint = local::endpoint(flavor).map_err(|_| "IPC_ENDPOINT_UNAVAILABLE")?;
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
            supported_protocol_versions: vec![2],
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
        let confirmed_empty = Arc::new(AtomicBool::new(false));
        let proof = confirmed_empty.clone();
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
                        proof.store(true, Ordering::Release);
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
            let _ = events.send(DaemonDelivery::Closed { connection_id });
        });
        *slot = Some(Connection {
            id: id.clone(),
            outgoing,
            acknowledgments,
            _stop: stop,
            confirmed_empty,
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
            .is_some_and(|c| c.confirmed_empty.load(Ordering::Acquire))
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
