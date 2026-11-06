//! Local API after owner-only IPC authentication; pending updates are connection leases.

use crate::{
    host::{
        Host, ModelQueryResult,
        pi_commands::{command_rejection, unknown},
        pi_models::wire_result,
    },
    update::Access,
};
use cafe_space_ipc::framing::{read_frame, write_frame};
use cafe_space_protocol::{
    ClientMessage, Event, PROTOCOL_VERSION, Request, RequestResult, ResponseMessageType,
    ServerEvent, ServerMessage, ServerResponse, ServerWelcome, UpdateState, VersionMismatch,
    VersionMismatchMessageType, WelcomeMessageType, control, decode_client_message,
    encode_server_message,
};
use std::{future::Future, io, pin::Pin, task::Poll};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::broadcast,
};

const MAX_PENDING_INPUT: usize = 32;
type PendingInput = (String, tokio::sync::oneshot::Receiver<RequestResult>);

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
        && hello
            .supported_protocol_versions
            .contains(&control::VERSION)
    {
        return serve_control_connection(stream, host).await;
    }
    if !hello
        .supported_protocol_versions
        .contains(&PROTOCOL_VERSION)
    {
        let mut supported = vec![PROTOCOL_VERSION];
        if PROTOCOL_VERSION != control::VERSION {
            supported.push(control::VERSION);
        }
        send(
            &mut stream,
            ServerMessage::VersionMismatch(VersionMismatch {
                message_type: VersionMismatchMessageType::VersionMismatch,
                daemon_version: env!("CARGO_PKG_VERSION").into(),
                supported_protocol_versions: supported,
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
    // One pending query per connection bounds responses without blocking reads, events or close.
    let mut models: Option<(String, tokio::sync::oneshot::Receiver<ModelQueryResult>)> = None;
    let mut configuration: Option<PendingInput> = None;
    let mut input: Vec<PendingInput> = Vec::new();
    loop {
        // Retain a partially read frame while events are sent: read_frame is not cancellation-safe.
        let incoming = read_frame(&mut reader);
        tokio::pin!(incoming);
        loop {
            tokio::select! {
                frame = &mut incoming => {
                    let Some(frame) = frame? else { return Ok(()); };
                    let ClientMessage::Request(request) = decode_client_message(&frame).map_err(invalid_message)? else { return Err(invalid_message("hello is only valid at connection start")); };
                    if let Request::ListModels(params) = request.request {
                        let query = if models.is_some() {
                            Err(io::Error::other("PI_COMMAND_PENDING"))
                        } else {
                            host.list_pi_models(Access::Local, params)
                        };
                        match query {
                            Ok(reply) => models = Some((request.request_id, reply)),
                            Err(error) => send(&mut writer, ServerMessage::Response(ServerResponse {
                                message_type: ResponseMessageType::Response,
                                protocol_version: PROTOCOL_VERSION,
                                request_id: request.request_id,
                                result: wire_result(ModelQueryResult::Rejected(error)),
                            })).await?,
                        }
                        break;
                    }
                    if matches!(&request.request, Request::Prompt(_) | Request::Cancel(_) | Request::RespondToApproval(_)) {
                        let reply = if !input_has_capacity(input.len(), &request.request) {
                            Err(io::Error::other("PI_COMMAND_PENDING"))
                        } else {
                            host.command_pi(Access::Local, request.request)
                        };
                        match reply {
                            Ok(reply) => input.push((request.request_id, reply)),
                            Err(error) => send(&mut writer, ServerMessage::Response(ServerResponse {
                                message_type: ResponseMessageType::Response,
                                protocol_version: PROTOCOL_VERSION,
                                request_id: request.request_id,
                                result: command_rejection(error, "PI_COMMAND_UNAVAILABLE"),
                            })).await?,
                        }
                        break;
                    }
                    if matches!(&request.request, Request::SetModel(_) | Request::SetThinking(_)) {
                        let reply = if configuration.is_some() {
                            Err(io::Error::other("PI_COMMAND_PENDING"))
                        } else {
                            host.command_pi(Access::Local, request.request)
                        };
                        match reply {
                            Ok(reply) => configuration = Some((request.request_id, reply)),
                            Err(error) => send(&mut writer, ServerMessage::Response(ServerResponse {
                                message_type: ResponseMessageType::Response,
                                protocol_version: PROTOCOL_VERSION,
                                request_id: request.request_id,
                                result: command_rejection(error, "PI_CONFIGURATION_UNAVAILABLE"),
                            })).await?,
                        }
                        break;
                    }
                    let normal_shutdown = matches!(&request.request, Request::Shutdown);
                    let dispatch = host.dispatch(Access::Local, lease.owner, request.request)?;
                    let sent: io::Result<()> = async {
                        send(&mut writer, ServerMessage::Response(ServerResponse { message_type: ResponseMessageType::Response, protocol_version: PROTOCOL_VERSION, request_id: request.request_id, result: dispatch.result })).await?;
                        if dispatch.exit_after_reply && !normal_shutdown && let Some(receiver) = subscription.as_mut() {
                            loop {
                                let event = receiver.try_recv().map_err(|_| invalid_message("draining event unavailable; reconcile handover marker"))?;
                                let draining = matches!(&event.event, Event::UpdateChanged(value) if value.update == UpdateState::Draining);
                                send(&mut writer, ServerMessage::Event(event)).await?;
                                if draining { break; }
                            }
                        }
                        Ok(())
                    }.await;
                    if dispatch.exit_after_reply { host.shutdown.notify_one(); }
                    sent?;
                    if dispatch.exit_after_reply { return Ok(()); }
                    if let Some((receiver, replay)) = dispatch.subscription {
                        subscription = Some(receiver);
                        for event in replay { send(&mut writer, ServerMessage::Event(event)).await?; }
                    }
                    break;
                }
                result = async { (&mut models.as_mut().expect("pending query exists").1).await }, if models.is_some() => {
                    let (request_id, _) = models.take().expect("completed query exists");
                    let result = result.unwrap_or(ModelQueryResult::Unknown("PI_CONNECTION_CLOSED"));
                    send(&mut writer, ServerMessage::Response(ServerResponse {
                        message_type: ResponseMessageType::Response,
                        protocol_version: PROTOCOL_VERSION,
                        request_id,
                        result: wire_result(result),
                    })).await?;
                }
                result = async { (&mut configuration.as_mut().expect("pending configuration exists").1).await }, if configuration.is_some() => {
                    let (request_id, _) = configuration.take().expect("completed configuration exists");
                    send_pi_command_result(&mut writer, request_id, result).await?;
                }
                (request_id, result) = next_input_result(&mut input), if !input.is_empty() => {
                    send_pi_command_result(&mut writer, request_id, result).await?;
                }
                event = async { subscription.as_mut().expect("enabled subscription exists").recv().await }, if subscription.is_some() => {
                    let event = event.map_err(|_| invalid_message("event subscription lagged or closed; obtain a fresh snapshot"))?;
                    let draining = matches!(&event.event, Event::UpdateChanged(value) if value.update == UpdateState::Draining);
                    send(&mut writer, ServerMessage::Event(event)).await?;
                    if draining { return Ok(()); }
                }
            }
        }
    }
}

/// A frozen control connection has no subscriptions and never decodes session/project payloads.
async fn serve_control_connection(
    mut stream: impl AsyncRead + AsyncWrite + Unpin,
    host: &Host,
) -> io::Result<()> {
    send_control(
        &mut stream,
        ServerMessage::Welcome(ServerWelcome {
            message_type: WelcomeMessageType::Welcome,
            protocol_version: control::VERSION,
            daemon_version: env!("CARGO_PKG_VERSION").into(),
        }),
    )
    .await?;
    let lease = ConnectionLease {
        host,
        owner: host.connection_id(),
    };
    while let Some(frame) = read_frame(&mut stream).await? {
        let (request_id, request) = control::decode_request(&frame).map_err(invalid_message)?;
        let (result, exit) = if let Some(request) = request {
            let dispatch = host.dispatch(Access::Local, lease.owner, request)?;
            (dispatch.result, dispatch.exit_after_reply)
        } else {
            (
                cafe_space_protocol::RequestResult::Rejected(cafe_space_protocol::ErrorResult {
                    error: cafe_space_protocol::ApiError {
                        code: "CONTROL_ONLY".into(),
                        message: "This connection only allows update control.".into(),
                    },
                }),
                false,
            )
        };
        let sent = send_control(
            &mut stream,
            ServerMessage::Response(ServerResponse {
                message_type: ResponseMessageType::Response,
                protocol_version: control::VERSION,
                request_id,
                result,
            }),
        )
        .await;
        if exit {
            host.shutdown.notify_one();
        }
        sent?;
        if exit {
            return Ok(());
        }
    }
    Ok(())
}

async fn send_control(
    stream: &mut (impl AsyncWrite + Unpin),
    message: ServerMessage,
) -> io::Result<()> {
    let frame = control::encode_server_message(&message).map_err(invalid_message)?;
    write_frame(stream, &frame).await
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

fn input_has_capacity(pending: usize, request: &Request) -> bool {
    // Reserve one reply for cancel so prompts/approval selections cannot fill the Stop slot.
    pending < MAX_PENDING_INPUT - usize::from(!matches!(request, Request::Cancel(_)))
}

async fn next_input_result(
    input: &mut Vec<PendingInput>,
) -> (
    String,
    Result<RequestResult, tokio::sync::oneshot::error::RecvError>,
) {
    std::future::poll_fn(|cx| {
        for index in 0..input.len() {
            if let Poll::Ready(result) = Pin::new(&mut input[index].1).poll(cx) {
                let (id, _) = input.swap_remove(index);
                return Poll::Ready((id, result));
            }
        }
        Poll::Pending
    })
    .await
}

async fn send_pi_command_result(
    stream: &mut (impl AsyncWrite + Unpin),
    request_id: String,
    result: Result<RequestResult, tokio::sync::oneshot::error::RecvError>,
) -> io::Result<()> {
    send(
        stream,
        ServerMessage::Response(ServerResponse {
            message_type: ResponseMessageType::Response,
            protocol_version: PROTOCOL_VERSION,
            request_id,
            result: result.unwrap_or_else(|_| unknown("PI_CONNECTION_CLOSED")),
        }),
    )
    .await
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
    async fn input_replies_do_not_wait_for_the_first_command_or_lose_closed_receivers() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/pi-rpc/commands-wire-v5.json"
        ))
        .unwrap();
        assert_eq!(fixture["maxPendingInputPerConnection"], MAX_PENDING_INPUT);
        assert_eq!(fixture["reservedCancelReplies"], 1);
        let approvals: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/pi-rpc/approvals-wire-v5.json"
        ))
        .unwrap();
        assert_eq!(approvals["maxPendingInputPerConnection"], MAX_PENDING_INPUT);
        assert_eq!(approvals["reservedCancelReplies"], 1);
        for frame in fixture["requests"]
            .as_array()
            .unwrap()
            .iter()
            .chain(approvals["requests"].as_array().unwrap())
        {
            let ClientMessage::Request(request) =
                decode_client_message(&serde_json::to_vec(frame).unwrap()).unwrap()
            else {
                panic!("command request")
            };
            assert!(input_has_capacity(MAX_PENDING_INPUT - 2, &request.request));
            assert_eq!(
                input_has_capacity(MAX_PENDING_INPUT - 1, &request.request),
                matches!(request.request, Request::Cancel(_))
            );
            assert!(!input_has_capacity(MAX_PENDING_INPUT, &request.request));
        }
        let (first, first_reply) = tokio::sync::oneshot::channel();
        let (cancel, cancel_reply) = tokio::sync::oneshot::channel();
        let mut input = vec![
            ("prompt".into(), first_reply),
            ("cancel".into(), cancel_reply),
        ];
        let waiting = async {
            let (id, result) = next_input_result(&mut input).await;
            assert_eq!(id, "cancel");
            assert!(matches!(result.unwrap(), RequestResult::Unknown(_)));
        };
        let completing = async {
            tokio::task::yield_now().await;
            cancel.send(unknown("PI_CANCEL_OUTCOME_UNKNOWN")).unwrap();
        };
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            tokio::join!(waiting, completing);
        })
        .await
        .unwrap();
        assert_eq!(input.len(), 1);
        drop(first);
        let (id, result) = next_input_result(&mut input).await;
        assert_eq!(id, "prompt");
        assert!(result.is_err());
        assert!(input.is_empty());
    }

    #[tokio::test]
    async fn dropped_configuration_sender_and_explicit_close_keep_distinct_unknown_frames() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/pi-rpc/configuration-wire-v5.json"
        ))
        .unwrap();
        for dropped in [true, false] {
            let (reply, received) = tokio::sync::oneshot::channel();
            if dropped {
                drop(reply);
            } else {
                reply.send(unknown("PI_COMMAND_OUTCOME_UNKNOWN")).unwrap();
            }
            let (mut client, mut server) = tokio::io::duplex(1024);
            let sending = async move {
                send_pi_command_result(&mut server, "configure".into(), received.await)
                    .await
                    .unwrap();
            };
            let checking = async {
                let ServerMessage::Response(response) = receive(&mut client).await else {
                    panic!("configuration response expected")
                };
                assert_eq!(response.request_id, "configure");
                assert_eq!(response.protocol_version, fixture["fullProtocolVersion"]);
                let RequestResult::Unknown(value) = response.result else {
                    panic!("a lost reply is never rejection or success")
                };
                assert_eq!(
                    value.error.code,
                    fixture[if dropped {
                        "closedReceiverCode"
                    } else {
                        "closeCode"
                    }]
                );
                assert!(value.error.message.contains("do not replay"));
                assert!(read_frame(&mut client).await.unwrap().is_none());
            };
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                tokio::join!(sending, checking)
            })
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn owner_settings_round_trip_over_framed_ipc_without_secret_echo() {
        let (mut client, server) = tokio::io::duplex(8192);
        let serving =
            tokio::spawn(async move { serve_connection(server, "owner-wire-fixture").await });
        hello(&mut client, vec![PROTOCOL_VERSION]).await;
        assert!(matches!(
            receive(&mut client).await,
            ServerMessage::Welcome(_)
        ));
        send_client(&mut client, request("owner-set", Request::UpdateOwnerEnvironment(cafe_space_protocol::UpdateOwnerEnvironmentParams {
            name: "HTTPS_PROXY".into(),
            change: cafe_space_protocol::OwnerEnvironmentChange::Set { value: "https://synthetic-user:synthetic-password@proxy.invalid:3128/private".into() },
        }))).await;
        let response = receive(&mut client).await;
        let encoded = serde_json::to_string(&response).unwrap();
        for secret in ["synthetic-user", "synthetic-password", "/private"] {
            assert!(!encoded.contains(secret));
        }
        assert!(encoded.contains("https://•••:•••@proxy.invalid:3128"));
        send_client(
            &mut client,
            request("owner-get", Request::GetOwnerEnvironment),
        )
        .await;
        assert!(
            matches!(receive(&mut client).await, ServerMessage::Response(response) if matches!(&response.result, RequestResult::Applied(value) if matches!(value.result, Response::OwnerEnvironment(ref status) if status.configured.iter().any(|entry| entry.name == "HTTPS_PROXY"))))
        );
        drop(client);
        serving.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn frozen_control_handler_matches_trace_without_subscribing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp
            .path()
            .join(cafe_space_ipc::BuildFlavor::Dev.directory_name());
        let host = Host::new(
            crate::state::State::open(&root, cafe_space_ipc::BuildFlavor::Dev).unwrap(),
            &root,
            "daemon-fixture".into(),
            "a".repeat(64),
        )
        .unwrap();
        host.set_active_for_test(1).unwrap();
        let trace: serde_json::Value =
            serde_json::from_slice(include_bytes!("../../../fixtures/control-v2/trace.json"))
                .unwrap();
        let (mut client, server) = tokio::io::duplex(1024);
        let checking = async {
            const { assert!(PROTOCOL_VERSION > control::VERSION) };
            for item in trace["frames"].as_array().unwrap() {
                if item["from"] == "controller" {
                    write_frame(&mut client, &serde_json::to_vec(&item["frame"]).unwrap())
                        .await
                        .unwrap();
                } else {
                    let bytes = read_frame(&mut client).await.unwrap().unwrap();
                    control::decode_server_message(&bytes).unwrap();
                    let mut expected = item["frame"].clone();
                    if expected["type"] == "welcome" {
                        expected["daemonVersion"] = env!("CARGO_PKG_VERSION").into();
                    }
                    if expected["result"]["data"]["result"]["kind"] == "health" {
                        expected["result"]["data"]["result"]["data"]["daemonVersion"] =
                            env!("CARGO_PKG_VERSION").into();
                    }
                    assert_eq!(
                        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
                        expected
                    );
                }
            }
            assert!(read_frame(&mut client).await.unwrap().is_none());
        };
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            tokio::join!(super::serve_connection(server, &host), checking)
        })
        .await
        .unwrap();
        result.unwrap();
        assert!(host.normal_shutdown_requested().unwrap());
    }

    #[tokio::test]
    async fn control_handler_refuses_unknown_methods_and_releases_pending_lease() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp
            .path()
            .join(cafe_space_ipc::BuildFlavor::Dev.directory_name());
        let host = Host::new(
            crate::state::State::open(&root, cafe_space_ipc::BuildFlavor::Dev).unwrap(),
            &root,
            "daemon-fixture".into(),
            "a".repeat(64),
        )
        .unwrap();
        host.set_active_for_test(1).unwrap();
        let (mut client, server) = tokio::io::duplex(1024);
        let checking = async {
            hello(&mut client, vec![control::VERSION]).await;
            let bytes = read_frame(&mut client).await.unwrap().unwrap();
            assert!(matches!(
                control::decode_server_message(&bytes).unwrap(),
                ServerMessage::Welcome(_)
            ));
            assert_eq!(
                decode_server_message(&bytes),
                Err(cafe_space_protocol::ProtocolError::UnsupportedVersion(
                    control::VERSION
                ))
            );
            for method in ["add_project", "subscribe_events", "future_method"] {
                let frame = serde_json::json!({"type":"request", "protocolVersion":2, "requestId":"denied", "request":{"method":method,"params":{"unrecognized":true}}});
                write_frame(&mut client, &serde_json::to_vec(&frame).unwrap())
                    .await
                    .unwrap();
                let bytes = read_frame(&mut client).await.unwrap().unwrap();
                let ServerMessage::Response(response) =
                    control::decode_server_message(&bytes).unwrap()
                else {
                    panic!("response required")
                };
                assert!(
                    matches!(response.result, RequestResult::Rejected(error) if error.error.code == "CONTROL_ONLY")
                );
            }
            let frame = serde_json::json!({"type":"request", "protocolVersion":2, "requestId":"pending", "request":{"method":"prepare_update", "params":{"mode":"when_idle","targetHash":"b".repeat(64)}}});
            write_frame(&mut client, &serde_json::to_vec(&frame).unwrap())
                .await
                .unwrap();
            let bytes = read_frame(&mut client).await.unwrap().unwrap();
            let ServerMessage::Response(response) = control::decode_server_message(&bytes).unwrap()
            else {
                panic!("response required")
            };
            assert!(
                matches!(response.result, RequestResult::Applied(data) if matches!(&data.result, Response::Update(value) if value.update == UpdateState::Pending))
            );
            drop(client);
        };
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            tokio::join!(super::serve_connection(server, &host), checking)
        })
        .await
        .unwrap();
        result.unwrap();
        let result = host
            .dispatch(Access::Local, 99, Request::GetStatus)
            .unwrap()
            .result;
        assert!(
            matches!(result, RequestResult::Applied(data) if matches!(&data.result, Response::Health(health) if health.update == UpdateState::None))
        );
        let result = host
            .dispatch(Access::Local, 99, Request::ListProjects)
            .unwrap()
            .result;
        assert!(
            matches!(result, RequestResult::Applied(data) if matches!(&data.result, Response::Projects(projects) if projects.items.is_empty()))
        );
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
            hello(&mut client, vec![PROTOCOL_VERSION]).await;
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
            hello(&mut client, vec![1, control::VERSION, PROTOCOL_VERSION]).await;
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
            hello(&mut client, vec![1, PROTOCOL_VERSION + 1]).await;
            let ServerMessage::VersionMismatch(mismatch) = receive(&mut client).await else {
                panic!("mismatch expected")
            };
            assert_eq!(
                mismatch.supported_protocol_versions,
                vec![PROTOCOL_VERSION, control::VERSION]
            );
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
                    hello(&mut client, vec![PROTOCOL_VERSION]).await;
                    assert!(matches!(
                        receive(&mut client).await,
                        ServerMessage::Welcome(_)
                    ));
                    hello(&mut client, vec![PROTOCOL_VERSION]).await;
                }
            };
            let (result, ()) = tokio::join!(serve_connection(server, "boot-fixture"), checking);
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
        }
    }

    #[tokio::test]
    async fn framed_registry_reconciles_create_and_reports_close_as_dispatched() {
        use cafe_space_protocol::{CloseSessionParams, CreateSessionParams, SessionStart};
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
        let params = CreateSessionParams {
            session_id: "session-fixture".into(),
            project_id: "project-fixture".into(),
            runtime_id: "pi".into(),
            expected_cwd_fence: "cwd-fixture".into(),
            start: SessionStart::New,
        };
        let first = host.create_for_test(&params).unwrap();
        let RequestResult::Applied(ref data) = first else {
            panic!("created")
        };
        let Response::Session(ref created) = data.result else {
            panic!("created")
        };
        let stream = created.session.session.stream_id.clone().unwrap();
        let (mut client, server) = tokio::io::duplex(64);
        let checking = async {
            hello(&mut client, vec![PROTOCOL_VERSION]).await;
            assert!(matches!(
                receive(&mut client).await,
                ServerMessage::Welcome(_)
            ));
            send_client(
                &mut client,
                request("reconcile-create", Request::CreateSession(params.clone())),
            )
            .await;
            let ServerMessage::Response(response) = receive(&mut client).await else {
                panic!("response")
            };
            assert_eq!(response.result, first);
            let close = Request::CloseSession(CloseSessionParams {
                session_id: params.session_id,
                expected_stream_id: stream.clone(),
                expected_cwd_fence: params.expected_cwd_fence,
            });
            for id in ["first-close", "reconcile-close"] {
                send_client(&mut client, request(id, close.clone())).await;
                let ServerMessage::Response(response) = receive(&mut client).await else {
                    panic!("response")
                };
                assert!(
                    matches!(response.result,RequestResult::Dispatched(data) if data.operation_id==stream)
                );
            }
            client.shutdown().await.unwrap();
        };
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(super::serve_connection(server, &host), checking)
        })
        .await
        .unwrap();
        result.unwrap();
    }

    #[tokio::test]
    async fn session_controls_are_rejected_not_silently_applied() {
        let (mut client, server) = tokio::io::duplex(64);
        let checking = async {
            hello(&mut client, vec![PROTOCOL_VERSION]).await;
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
            assert_eq!(result.error.code, "PROJECT_NOT_APPROVED");
            client.shutdown().await.unwrap();
        };
        let (result, ()) = tokio::join!(serve_connection(server, "boot-fixture"), checking);
        result.unwrap();
    }
}
