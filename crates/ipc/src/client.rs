//! Strict local request client bound to an owner-only data directory and per-boot identity.
use crate::{
    BuildFlavor,
    framing::{read_frame, write_frame},
    installation as files, local,
};
use cafe_space_protocol::{
    ClientHello, ClientMessage, ClientRequest, HealthResponse, HelloMessageType, PROTOCOL_VERSION,
    Request, RequestMessageType, RequestResult, Response, ServerMessage, control,
    decode_server_message, encode_client_message,
};
use std::{path::Path, time::Duration};
type Result<T> = std::result::Result<T, &'static str>;
const DEADLINE: Duration = Duration::from_secs(5);

pub struct Client {
    stream: local::ClientStream,
    next: u64,
    control_only: bool,
}

/// Frozen generation-2 update transport; never exposes the general CLI request channel.
pub struct ControlClient(Client);
impl ControlClient {
    pub async fn connect(flavor: BuildFlavor, root: &Path) -> Result<(Self, HealthResponse)> {
        let endpoint = local::endpoint(flavor).map_err(|_| "INSTALL_PATH_UNAVAILABLE")?;
        let (client, health) = Client::connect_mode(&endpoint, root, true).await?;
        Ok((Self(client), health))
    }

    pub async fn request(&mut self, request: Request) -> Result<Response> {
        self.0.request(request).await
    }
}

impl Client {
    pub async fn connect(flavor: BuildFlavor, root: &Path) -> Result<(Self, HealthResponse)> {
        let endpoint = local::endpoint(flavor).map_err(|_| "INSTALL_PATH_UNAVAILABLE")?;
        Self::connect_at(&endpoint, root).await
    }
    async fn connect_at(endpoint: &Path, root: &Path) -> Result<(Self, HealthResponse)> {
        Self::connect_mode(endpoint, root, false).await
    }
    async fn connect_mode(
        endpoint: &Path,
        root: &Path,
        control_only: bool,
    ) -> Result<(Self, HealthResponse)> {
        let identity = files::daemon_identity(root).map_err(|_| "INSTALL_IDENTITY_UNAVAILABLE")?;
        let mut stream = tokio::time::timeout(DEADLINE, local::connect(endpoint))
            .await
            .map_err(|_| "INSTALL_CONNECT_TIMEOUT")?
            .map_err(|_| "INSTALL_CONNECT_FAILED")?;
        #[cfg(windows)]
        if local::server_process_id(&stream).map_err(|_| "INSTALL_IDENTITY_UNAVAILABLE")?
            != identity.pid
        {
            return Err("INSTALL_PEER_MISMATCH");
        }
        let hello = ClientMessage::Hello(ClientHello {
            message_type: HelloMessageType::Hello,
            supported_protocol_versions: vec![if control_only {
                control::VERSION
            } else {
                PROTOCOL_VERSION
            }],
            client_version: env!("CARGO_PKG_VERSION").into(),
        });
        let bytes = if control_only {
            control::encode_client_message(&hello)
        } else {
            encode_client_message(&hello)
        }
        .map_err(|_| "INSTALL_CODEC_FAILED")?;
        tokio::time::timeout(DEADLINE, write_frame(&mut stream, &bytes))
            .await
            .map_err(|_| "INSTALL_HELLO_TIMEOUT")?
            .map_err(|_| "INSTALL_HELLO_FAILED")?;
        let mut client = Self {
            stream,
            next: 0,
            control_only,
        };
        if !matches!(client.read().await?, ServerMessage::Welcome(_)) {
            return Err("INSTALL_PROTOCOL_MISMATCH");
        }
        let Response::Health(health) = client.request(Request::GetStatus).await? else {
            return Err("INSTALL_PROTOCOL_MISMATCH");
        };
        if health.event_stream_id.as_ref() != Some(&identity.event_stream_id)
            || health.binary_hash.is_none()
        {
            return Err("INSTALL_PEER_MISMATCH");
        }
        Ok((client, health))
    }
    async fn read(&mut self) -> Result<ServerMessage> {
        let bytes = tokio::time::timeout(DEADLINE, read_frame(&mut self.stream))
            .await
            .map_err(|_| "INSTALL_READ_TIMEOUT")?
            .map_err(|_| "INSTALL_READ_FAILED")?
            .ok_or("INSTALL_CONNECTION_CLOSED")?;
        if self.control_only {
            control::decode_server_message(&bytes)
        } else {
            decode_server_message(&bytes)
        }
        .map_err(|_| "INSTALL_CODEC_FAILED")
    }
    /// Installer compatibility contract; general CLI callers use request_outcome to preserve codes.
    pub async fn request(&mut self, request: Request) -> Result<Response> {
        match self.request_outcome(request).await? {
            RequestResult::Applied(data) => Ok(data.result),
            _ => Err("INSTALL_REQUEST_REJECTED"),
        }
    }

    /// Preserve applied/rejected/dispatched/unknown; never retry after an uncertain outcome.
    pub async fn request_outcome(&mut self, request: Request) -> Result<RequestResult> {
        if self.control_only && !control::allows(&request) {
            return Err("CONTROL_ONLY");
        }
        self.next = self.next.checked_add(1).ok_or("INSTALL_REQUEST_LIMIT")?;
        let id = format!("native-install-{}", self.next);
        let request = ClientMessage::Request(ClientRequest {
            message_type: RequestMessageType::Request,
            protocol_version: if self.control_only {
                control::VERSION
            } else {
                PROTOCOL_VERSION
            },
            request_id: id.clone(),
            request,
        });
        let bytes = if self.control_only {
            control::encode_client_message(&request)
        } else {
            encode_client_message(&request)
        }
        .map_err(|_| "INSTALL_CODEC_FAILED")?;
        tokio::time::timeout(DEADLINE, write_frame(&mut self.stream, &bytes))
            .await
            .map_err(|_| "INSTALL_WRITE_TIMEOUT")?
            .map_err(|_| "INSTALL_WRITE_FAILED")?;
        match self.read().await? {
            ServerMessage::Response(response) if response.request_id == id => Ok(response.result),
            _ => Err("INSTALL_PROTOCOL_MISMATCH"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[cfg(windows)]
    use std::path::PathBuf;
    #[tokio::test]
    async fn general_cli_preserves_session_outcomes_and_sends_each_request_once() {
        use cafe_space_protocol::{
            ResponseMessageType, ServerResponse, decode_client_message, encode_server_message,
        };
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/cli/session.json")).unwrap();
        let requests = fixture["commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|case| serde_json::from_value::<Request>(case["request"].clone()).unwrap())
            .collect::<Vec<_>>();
        let outcomes = fixture["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| serde_json::from_value::<RequestResult>(value.clone()).unwrap())
            .collect::<Vec<_>>();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let _guard = crate::private::directory(&root).unwrap();
        #[cfg(unix)]
        let endpoint = root.join("s");
        #[cfg(windows)]
        let endpoint = PathBuf::from(format!(
            r"\\.\pipe\cafe-cli-fixture-{}-{}",
            std::process::id(),
            temp.path().file_name().unwrap().to_str().unwrap()
        ));
        let mut listener = local::Listener::bind(&endpoint).await.unwrap();
        let expected = requests.clone();
        let replies = outcomes.clone();
        let server = tokio::spawn(async move {
            let mut stream = listener.accept().await.unwrap();
            let mut seen = 0;
            for expected in expected {
                for result in &replies {
                    let ClientMessage::Request(request) =
                        decode_client_message(&read_frame(&mut stream).await.unwrap().unwrap())
                            .unwrap()
                    else {
                        panic!("request expected")
                    };
                    seen += 1;
                    assert_eq!(request.request, expected);
                    assert_eq!(request.request_id, format!("native-install-{seen}"));
                    let response = ServerMessage::Response(ServerResponse {
                        message_type: ResponseMessageType::Response,
                        protocol_version: PROTOCOL_VERSION,
                        request_id: request.request_id,
                        result: result.clone(),
                    });
                    write_frame(&mut stream, &encode_server_message(&response).unwrap())
                        .await
                        .unwrap();
                }
            }
            let _lost = read_frame(&mut stream).await.unwrap().unwrap();
            // EOF after receipt is unknown to the client, never an invitation to replay.
        });
        let stream = local::connect(&endpoint).await.unwrap();
        let mut client = Client {
            stream,
            next: 0,
            control_only: false,
        };
        for request in requests {
            for outcome in &outcomes {
                assert_eq!(
                    &client.request_outcome(request.clone()).await.unwrap(),
                    outcome
                );
            }
        }
        assert!(client.request_outcome(Request::ListSessions).await.is_err());
        assert_eq!(client.next, 29);
        tokio::time::timeout(DEADLINE, server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn client_refuses_another_boot_before_any_mutation() {
        for (matching, control_only) in [(false, false), (true, false), (false, true), (true, true)]
        {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("root");
            let _guard = crate::private::directory(&root).unwrap();
            let mut record = crate::private::file(&root.join("daemon.instance")).unwrap();
            writeln!(record, "{} {}", std::process::id(), "a".repeat(32)).unwrap();
            drop(record);
            #[cfg(unix)]
            let endpoint = root.join("s");
            #[cfg(windows)]
            let endpoint = PathBuf::from(format!(
                r"\\.\pipe\cafe-native-install-{}-{}",
                std::process::id(),
                temp.path().file_name().unwrap().to_string_lossy()
            ));
            let mut listener = local::Listener::bind(&endpoint).await.unwrap();
            let serving = tokio::spawn(async move {
                let mut stream = listener.accept().await.unwrap();
                let hello: ClientMessage =
                    serde_json::from_slice(&read_frame(&mut stream).await.unwrap().unwrap())
                        .unwrap();
                let ClientMessage::Hello(hello) = hello else {
                    panic!("hello required")
                };
                assert_eq!(
                    hello.supported_protocol_versions,
                    vec![if control_only {
                        control::VERSION
                    } else {
                        PROTOCOL_VERSION
                    }]
                );
                let version = if control_only {
                    control::VERSION
                } else {
                    PROTOCOL_VERSION
                };
                let welcome = format!(
                    r#"{{"type":"welcome","protocolVersion":{version},"daemonVersion":"0.1.0"}}"#
                );
                write_frame(&mut stream, welcome.as_bytes()).await.unwrap();
                let bytes = read_frame(&mut stream).await.unwrap().unwrap();
                let request = if control_only {
                    control::decode_client_message(&bytes)
                } else {
                    cafe_space_protocol::decode_client_message(&bytes)
                }
                .unwrap();
                let ClientMessage::Request(request) = request else {
                    panic!("expected health request")
                };
                assert_eq!(request.request, Request::GetStatus);
                let id = if matching { "a" } else { "b" }.repeat(32);
                let response = format!(
                    r#"{{"type":"response","protocolVersion":{version},"requestId":"{}","result":{{"status":"applied","data":{{"result":{{"kind":"health","data":{{"daemonVersion":"0.1.0","ready":true,"binaryHash":"{}","eventStreamId":"{id}"}}}}}}}}}}"#,
                    request.request_id,
                    "c".repeat(64)
                );
                write_frame(&mut stream, response.as_bytes()).await.unwrap();
                assert!(
                    read_frame(&mut stream).await.unwrap().is_none(),
                    "no prepare_update may be sent before identity validation"
                );
            });
            let result = Client::connect_mode(&endpoint, &root, control_only).await;
            assert_eq!(result.is_ok(), matching);
            if matching && control_only {
                let mut client = ControlClient(result.unwrap().0);
                assert_eq!(
                    client.request(Request::ListSessions).await,
                    Err("CONTROL_ONLY")
                );
            } else {
                drop(result);
            }
            tokio::time::timeout(DEADLINE, serving)
                .await
                .unwrap()
                .unwrap();
        }
    }
}
