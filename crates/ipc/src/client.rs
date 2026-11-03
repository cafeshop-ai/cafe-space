//! Strict local request client bound to an owner-only data directory and per-boot identity.
use crate::{
    BuildFlavor,
    framing::{read_frame, write_frame},
    installation as files, local,
};
use cafe_space_protocol::{
    ClientHello, ClientMessage, ClientRequest, HealthResponse, HelloMessageType, PROTOCOL_VERSION,
    Request, RequestMessageType, RequestResult, Response, ServerMessage, decode_server_message,
    encode_client_message,
};
use std::{path::Path, time::Duration};
type Result<T> = std::result::Result<T, &'static str>;
const DEADLINE: Duration = Duration::from_secs(5);

pub struct Client {
    stream: local::ClientStream,
    next: u64,
}
impl Client {
    pub async fn connect(flavor: BuildFlavor, root: &Path) -> Result<(Self, HealthResponse)> {
        let endpoint = local::endpoint(flavor).map_err(|_| "INSTALL_PATH_UNAVAILABLE")?;
        Self::connect_at(&endpoint, root).await
    }
    async fn connect_at(endpoint: &Path, root: &Path) -> Result<(Self, HealthResponse)> {
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
            supported_protocol_versions: vec![PROTOCOL_VERSION],
            client_version: env!("CARGO_PKG_VERSION").into(),
        });
        let bytes = encode_client_message(&hello).map_err(|_| "INSTALL_CODEC_FAILED")?;
        tokio::time::timeout(DEADLINE, write_frame(&mut stream, &bytes))
            .await
            .map_err(|_| "INSTALL_HELLO_TIMEOUT")?
            .map_err(|_| "INSTALL_HELLO_FAILED")?;
        let mut client = Self { stream, next: 0 };
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
        decode_server_message(&bytes).map_err(|_| "INSTALL_CODEC_FAILED")
    }
    pub async fn request(&mut self, request: Request) -> Result<Response> {
        self.next = self.next.checked_add(1).ok_or("INSTALL_REQUEST_LIMIT")?;
        let id = format!("native-install-{}", self.next);
        let request = ClientMessage::Request(ClientRequest {
            message_type: RequestMessageType::Request,
            protocol_version: PROTOCOL_VERSION,
            request_id: id.clone(),
            request,
        });
        let bytes = encode_client_message(&request).map_err(|_| "INSTALL_CODEC_FAILED")?;
        tokio::time::timeout(DEADLINE, write_frame(&mut self.stream, &bytes))
            .await
            .map_err(|_| "INSTALL_WRITE_TIMEOUT")?
            .map_err(|_| "INSTALL_WRITE_FAILED")?;
        match self.read().await? {
            ServerMessage::Response(response) if response.request_id == id => match response.result
            {
                RequestResult::Applied(data) => Ok(data.result),
                _ => Err("INSTALL_REQUEST_REJECTED"),
            },
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
    async fn client_refuses_another_boot_before_any_mutation() {
        for matching in [false, true] {
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
                let _hello = read_frame(&mut stream).await.unwrap().unwrap();
                write_frame(
                    &mut stream,
                    br#"{"type":"welcome","protocolVersion":2,"daemonVersion":"0.1.0"}"#,
                )
                .await
                .unwrap();
                let request = cafe_space_protocol::decode_client_message(
                    &read_frame(&mut stream).await.unwrap().unwrap(),
                )
                .unwrap();
                let ClientMessage::Request(request) = request else {
                    panic!("expected health request")
                };
                assert_eq!(request.request, Request::GetStatus);
                let id = if matching { "a" } else { "b" }.repeat(32);
                let response = format!(
                    r#"{{"type":"response","protocolVersion":2,"requestId":"{}","result":{{"status":"applied","data":{{"result":{{"kind":"health","data":{{"daemonVersion":"0.1.0","ready":true,"binaryHash":"{}","eventStreamId":"{id}"}}}}}}}}}}"#,
                    request.request_id,
                    "c".repeat(64)
                );
                write_frame(&mut stream, response.as_bytes()).await.unwrap();
                assert!(
                    read_frame(&mut stream).await.unwrap().is_none(),
                    "no prepare_update may be sent before identity validation"
                );
            });
            let result = Client::connect_at(&endpoint, &root).await;
            assert_eq!(result.is_ok(), matching);
            drop(result);
            tokio::time::timeout(DEADLINE, serving)
                .await
                .unwrap()
                .unwrap();
        }
    }
}
