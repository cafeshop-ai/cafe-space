//! Read-only M0 API for an already authenticated local IPC connection.

use std::io;

use cafe_space_ipc::framing::{read_frame, write_frame};
use cafe_space_protocol::{
    ApiError, AppliedResult, ClientMessage, ErrorResult, HealthResponse, PROTOCOL_VERSION,
    ProjectList, Request, RequestResult, Response, ResponseMessageType, RuntimeList, ServerMessage,
    ServerResponse, ServerWelcome, SessionList, SnapshotResponse, SubscriptionMode,
    SubscriptionResponse, VersionMismatch, VersionMismatchMessageType, WelcomeMessageType,
    decode_client_message, encode_server_message,
};
use tokio::io::{AsyncRead, AsyncWrite};

/// The listener must establish owner-only access before passing in a stream.
/// `event_stream_id` identifies this daemon boot, not an individual connection.
pub async fn serve_connection(
    mut stream: impl AsyncRead + AsyncWrite + Unpin,
    event_stream_id: &str,
) -> io::Result<()> {
    let Some(frame) = read_frame(&mut stream).await? else {
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

    while let Some(frame) = read_frame(&mut stream).await? {
        let ClientMessage::Request(request) =
            decode_client_message(&frame).map_err(invalid_message)?
        else {
            return Err(invalid_message("hello is only valid at connection start"));
        };
        // shortcut: M0 has no session engine; replace empty views when the M1 registry lands.
        let result = match request.request {
            Request::GetStatus => applied(Response::Health(HealthResponse {
                daemon_version: env!("CARGO_PKG_VERSION").into(),
                ready: true,
            })),
            Request::ListProjects => applied(Response::Projects(ProjectList { items: vec![] })),
            Request::ListRuntimes => applied(Response::Runtimes(RuntimeList { items: vec![] })),
            Request::ListSessions => applied(Response::Sessions(SessionList {
                items: vec![],
                event_stream_id: event_stream_id.into(),
                sequence: 0,
            })),
            Request::SubscribeEvents(params) => {
                let resumed = params.after.as_ref().is_some_and(|cursor| {
                    cursor.event_stream_id == event_stream_id && cursor.sequence == 0
                });
                applied(Response::Subscription(SubscriptionResponse {
                    event_stream_id: event_stream_id.into(),
                    sequence: 0,
                    mode: if resumed {
                        SubscriptionMode::Resumed
                    } else {
                        SubscriptionMode::SnapshotRequired
                    },
                    snapshot: (!resumed).then(|| SnapshotResponse {
                        projects: vec![],
                        runtimes: vec![],
                        sessions: vec![],
                    }),
                }))
            }
            Request::GetSession(_) => rejected("SESSION_NOT_FOUND", "No sessions are configured."),
            _ => rejected(
                "NOT_IMPLEMENTED",
                "This daemon skeleton does not execute session operations.",
            ),
        };
        send(
            &mut stream,
            ServerMessage::Response(ServerResponse {
                message_type: ResponseMessageType::Response,
                protocol_version: PROTOCOL_VERSION,
                request_id: request.request_id,
                result,
            }),
        )
        .await?;
    }
    Ok(())
}

fn applied(response: Response) -> RequestResult {
    RequestResult::Applied(Box::new(AppliedResult { result: response }))
}

fn rejected(code: &str, message: &str) -> RequestResult {
    RequestResult::Rejected(ErrorResult {
        error: ApiError {
            code: code.into(),
            message: message.into(),
        },
    })
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
        ClientHello, ClientRequest, EventCursor, HelloMessageType, RequestMessageType,
        SubscribeEventsParams, decode_server_message, encode_client_message,
    };
    use tokio::io::{AsyncWriteExt, DuplexStream};

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
