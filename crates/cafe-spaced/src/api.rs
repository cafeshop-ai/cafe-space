//! Local API after owner-only IPC authentication; pending updates are connection leases.

use crate::{host::Host, update::Access};
use cafe_space_ipc::framing::{read_frame, write_frame};
use cafe_space_protocol::{
    ClientMessage, PROTOCOL_VERSION, ResponseMessageType, ServerEvent, ServerMessage,
    ServerResponse, ServerWelcome, VersionMismatch, VersionMismatchMessageType, WelcomeMessageType,
    decode_client_message, encode_server_message,
};
use std::io;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::broadcast,
};

/// The listener establishes owner-only access; no wire field can turn a remote peer local.
pub async fn serve_connection(
    mut stream: impl AsyncRead + AsyncWrite + Unpin,
    host: &Host,
) -> io::Result<()> {
    let Some(frame) =
        tokio::time::timeout(std::time::Duration::from_secs(5), read_frame(&mut stream))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "hello deadline exceeded"))??
    else {
        return Ok(());
    };
    let ClientMessage::Hello(hello) = decode_client_message(&frame).map_err(invalid_message)?
    else {
        return Err(invalid_message("expected protocol hello"));
    };
    if !hello
        .supported_protocol_versions
        .contains(&PROTOCOL_VERSION)
    {
        send(
            &mut stream,
            ServerMessage::VersionMismatch(VersionMismatch {
                message_type: VersionMismatchMessageType::VersionMismatch,
                daemon_version: env!("CARGO_PKG_VERSION").into(),
                supported_protocol_versions: vec![PROTOCOL_VERSION],
            }),
        )
        .await?;
        return Ok(());
    }
    send(
        &mut stream,
        ServerMessage::Welcome(ServerWelcome {
            message_type: WelcomeMessageType::Welcome,
            protocol_version: PROTOCOL_VERSION,
            daemon_version: env!("CARGO_PKG_VERSION").into(),
        }),
    )
    .await?;
    let lease = ConnectionLease {
        host,
        owner: host.connection_id(),
    };
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut subscription: Option<broadcast::Receiver<ServerEvent>> = None;
    loop {
        // Retain a partially read frame while events are sent: read_frame is not cancellation-safe.
        let incoming = read_frame(&mut reader);
        tokio::pin!(incoming);
        loop {
            tokio::select! {
                frame = &mut incoming => {
                    let Some(frame) = frame? else { return Ok(()); };
                    let ClientMessage::Request(request) = decode_client_message(&frame).map_err(invalid_message)? else { return Err(invalid_message("hello is only valid at connection start")); };
                    let dispatch = host.dispatch(Access::Local, lease.owner, request.request)?;
                    let sent = send(&mut writer, ServerMessage::Response(ServerResponse { message_type: ResponseMessageType::Response, protocol_version: PROTOCOL_VERSION, request_id: request.request_id, result: dispatch.result })).await;
                    if dispatch.exit_after_reply { host.shutdown.notify_one(); }
                    sent?;
                    if let Some((receiver, replay)) = dispatch.subscription {
                        subscription = Some(receiver);
                        for event in replay { send(&mut writer, ServerMessage::Event(event)).await?; }
                    }
                    break;
                }
                event = async { subscription.as_mut().expect("enabled subscription exists").recv().await }, if subscription.is_some() => {
                    let event = event.map_err(|_| invalid_message("event subscription lagged or closed; obtain a fresh snapshot"))?;
                    send(&mut writer, ServerMessage::Event(event)).await?;
                }
            }
        }
    }
}

struct ConnectionLease<'a> {
    host: &'a Host,
    owner: u64,
}
impl Drop for ConnectionLease<'_> {
    fn drop(&mut self) {
        self.host.disconnected(self.owner);
    }
}

fn invalid_message(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

async fn send(stream: &mut (impl AsyncWrite + Unpin), message: ServerMessage) -> io::Result<()> {
    let frame = encode_server_message(&message).map_err(invalid_message)?;
    write_frame(stream, &frame).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use cafe_space_protocol::{
        ClientHello, ClientRequest, EventCursor, HelloMessageType, Request, RequestMessageType,
        RequestResult, Response, SubscribeEventsParams, SubscriptionMode, decode_server_message,
        encode_client_message,
    };
    use tokio::io::{AsyncWriteExt, DuplexStream};

    async fn serve_connection(stream: DuplexStream, stream_id: &str) -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp
            .path()
            .join(cafe_space_ipc::BuildFlavor::Dev.directory_name());
        let state = crate::state::State::open(&root, cafe_space_ipc::BuildFlavor::Dev)?;
        let host = Host::new(state, &root, stream_id.into(), "a".repeat(64))?;
        super::serve_connection(stream, &host).await
    }

    async fn send_client(stream: &mut DuplexStream, message: ClientMessage) {
        write_frame(stream, &encode_client_message(&message).unwrap())
            .await
            .unwrap();
    }

    async fn receive(stream: &mut DuplexStream) -> ServerMessage {
        decode_server_message(&read_frame(stream).await.unwrap().unwrap()).unwrap()
    }

    async fn hello(stream: &mut DuplexStream, versions: Vec<u16>) {
        send_client(
            stream,
            ClientMessage::Hello(ClientHello {
                message_type: HelloMessageType::Hello,
                supported_protocol_versions: versions,
                client_version: "fixture".into(),
            }),
        )
        .await;
    }

    fn request(id: &str, request: Request) -> ClientMessage {
        ClientMessage::Request(ClientRequest {
            message_type: RequestMessageType::Request,
            protocol_version: PROTOCOL_VERSION,
            request_id: id.into(),
            request,
        })
    }

    #[tokio::test]
    async fn event_delivery_does_not_cancel_a_partially_read_request_frame() {
        use cafe_space_protocol::{Event, PrepareUpdateParams, UpdateMode, UpdateState};
        let temp = tempfile::tempdir().unwrap();
        let root = temp
            .path()
            .join(cafe_space_ipc::BuildFlavor::Dev.directory_name());
        let host = Host::new(
            crate::state::State::open(&root, cafe_space_ipc::BuildFlavor::Dev).unwrap(),
            &root,
            "boot-fixture".into(),
            "a".repeat(64),
        )
        .unwrap();
        host.set_active_for_test(1).unwrap();
        let (mut client, server) = tokio::io::duplex(1024);
        let checking = async {
            hello(&mut client, vec![2]).await;
            receive(&mut client).await;
            send_client(
                &mut client,
                request(
                    "subscribe",
                    Request::SubscribeEvents(SubscribeEventsParams { after: None }),
                ),
            )
            .await;
            receive(&mut client).await;
            let body = encode_client_message(&request("partial", Request::GetStatus)).unwrap();
            let mut packet = (body.len() as u32).to_be_bytes().to_vec();
            packet.extend(body);
            client.write_all(&packet[..9]).await.unwrap();
            // Give both join branches a poll before publishing the competing event.
            tokio::task::yield_now().await;
            tokio::task::yield_now().await;
            host.dispatch(
                Access::Local,
                99,
                Request::PrepareUpdate(PrepareUpdateParams {
                    mode: UpdateMode::WhenIdle,
                    target_hash: "b".repeat(64),
                }),
            )
            .unwrap();
            let ServerMessage::Event(event) = receive(&mut client).await else {
                panic!("update event expected")
            };
            assert!(
                matches!(event.event, Event::UpdateChanged(value) if value.update == UpdateState::Pending)
            );
            client.write_all(&packet[9..]).await.unwrap();
            let ServerMessage::Response(response) = receive(&mut client).await else {
                panic!("response expected")
            };
            assert_eq!(response.request_id, "partial");
            assert!(
                matches!(response.result, RequestResult::Applied(ref data) if matches!(&data.result, Response::Health(health) if health.update == UpdateState::Pending))
            );
            client.shutdown().await.unwrap();
        };
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            tokio::join!(super::serve_connection(server, &host), checking)
        })
        .await
        .unwrap();
        result.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn hello_has_a_deadline() {
        let (_client, server) = tokio::io::duplex(64);
        assert_eq!(
            serve_connection(server, "boot-fixture")
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
    }

    #[tokio::test]
    async fn handshake_health_empty_sessions_and_subscriptions() {
        let (mut client, server) = tokio::io::duplex(64);
        let serving = serve_connection(server, "boot-fixture");
        let checking = async {
            hello(&mut client, vec![1, 2, 3]).await;
            assert!(matches!(
                receive(&mut client).await,
                ServerMessage::Welcome(_)
            ));
            for (id, command) in [
                ("health", Request::GetStatus),
                ("sessions", Request::ListSessions),
            ] {
                send_client(&mut client, request(id, command)).await;
                let ServerMessage::Response(response) = receive(&mut client).await else {
                    panic!("response expected")
                };
                assert_eq!(response.request_id, id);
                let RequestResult::Applied(data) = response.result else {
                    panic!("applied expected")
                };
                match data.result {
                    Response::Health(health) => assert!(health.ready),
                    Response::Sessions(list) => {
                        assert!(list.items.is_empty());
                        assert_eq!(list.sequence, 0);
                        assert_eq!(list.event_stream_id, "boot-fixture");
                    }
                    _ => panic!("unexpected result"),
                }
            }
            for (after, expected) in [
                (None, SubscriptionMode::SnapshotRequired),
                (
                    Some(EventCursor {
                        event_stream_id: "boot-fixture".into(),
                        sequence: 0,
                    }),
                    SubscriptionMode::Resumed,
                ),
                (
                    Some(EventCursor {
                        event_stream_id: "older-boot".into(),
                        sequence: 0,
                    }),
                    SubscriptionMode::SnapshotRequired,
                ),
                (
                    Some(EventCursor {
                        event_stream_id: "boot-fixture".into(),
                        sequence: 1,
                    }),
                    SubscriptionMode::SnapshotRequired,
                ),
            ] {
                send_client(
                    &mut client,
                    request(
                        "subscribe",
                        Request::SubscribeEvents(SubscribeEventsParams { after }),
                    ),
                )
                .await;
                let ServerMessage::Response(response) = receive(&mut client).await else {
                    panic!("response expected")
                };
                let RequestResult::Applied(data) = response.result else {
                    panic!("applied expected")
                };
                let Response::Subscription(subscription) = data.result else {
                    panic!("subscription expected")
                };
                assert_eq!(subscription.mode, expected);
                assert_eq!(
                    subscription.snapshot.is_some(),
                    expected == SubscriptionMode::SnapshotRequired
                );
            }
            client.shutdown().await.unwrap();
        };
        let (result, ()) = tokio::join!(serving, checking);
        result.unwrap();
    }

    #[tokio::test]
    async fn mismatch_returns_supported_versions_and_closes() {
        let (mut client, server) = tokio::io::duplex(64);
        let checking = async {
            hello(&mut client, vec![1, 3]).await;
            let ServerMessage::VersionMismatch(mismatch) = receive(&mut client).await else {
                panic!("mismatch expected")
            };
            assert_eq!(mismatch.supported_protocol_versions, vec![2]);
            assert!(read_frame(&mut client).await.unwrap().is_none());
        };
        let (result, ()) = tokio::join!(serve_connection(server, "boot-fixture"), checking);
        result.unwrap();
    }

    #[tokio::test]
    async fn requests_before_hello_and_duplicate_hello_are_rejected() {
        for initial in [true, false] {
            let (mut client, server) = tokio::io::duplex(64);
            let checking = async {
                if initial {
                    send_client(&mut client, request("before-hello", Request::GetStatus)).await;
                } else {
                    hello(&mut client, vec![2]).await;
                    assert!(matches!(
                        receive(&mut client).await,
                        ServerMessage::Welcome(_)
                    ));
                    hello(&mut client, vec![2]).await;
                }
            };
            let (result, ()) = tokio::join!(serve_connection(server, "boot-fixture"), checking);
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
        }
    }

    #[tokio::test]
    async fn session_controls_are_rejected_not_silently_applied() {
        let (mut client, server) = tokio::io::duplex(64);
        let checking = async {
            hello(&mut client, vec![2]).await;
            assert!(matches!(
                receive(&mut client).await,
                ServerMessage::Welcome(_)
            ));
            write_frame(
                &mut client,
                include_bytes!("../../../fixtures/v2/create-session-request.json"),
            )
            .await
            .unwrap();
            let ServerMessage::Response(response) = receive(&mut client).await else {
                panic!("response expected")
            };
            let RequestResult::Rejected(result) = response.result else {
                panic!("rejected expected")
            };
            assert_eq!(result.error.code, "NOT_IMPLEMENTED");
            client.shutdown().await.unwrap();
        };
        let (result, ()) = tokio::join!(serve_connection(server, "boot-fixture"), checking);
        result.unwrap();
    }
}
