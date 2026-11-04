//! Frozen generation-2 update-control codec. Full API version checks remain independent.
//! The shared hello, health, update and result field types are part of this frozen contract.
use super::*;

pub const VERSION: u16 = 2;

pub fn allows(request: &Request) -> bool {
    matches!(
        request,
        Request::GetStatus | Request::PrepareUpdate(_) | Request::CancelUpdate | Request::Shutdown
    )
}

/// Decode a control connection's request header before any Host dispatch.
/// `None` means an unsupported method, including one unknown to this full-API build.
pub fn decode_request(frame: &[u8]) -> Result<(String, Option<Request>), ProtocolError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Envelope {
        #[serde(rename = "type")]
        _message_type: RequestMessageType,
        protocol_version: u16,
        request_id: String,
        request: serde_json::Value,
    }
    let envelope: Envelope = decode_json(frame)?;
    version(envelope.protocol_version)?;
    check_id(&envelope.request_id)?;
    let method = envelope
        .request
        .get("method")
        .and_then(serde_json::Value::as_str)
        .ok_or(ProtocolError::InvalidMessage)?;
    check_id(method)?;
    if !matches!(
        method,
        "get_status" | "prepare_update" | "cancel_update" | "shutdown"
    ) {
        return Ok((envelope.request_id, None));
    }
    let ClientMessage::Request(request) = decode_client_message(frame)? else {
        return Err(ProtocolError::InvalidMessage);
    };
    Ok((request.request_id, Some(request.request)))
}

pub fn decode_client_message(frame: &[u8]) -> Result<ClientMessage, ProtocolError> {
    let message = decode_json(frame)?;
    validate_client(&message)?;
    Ok(message)
}

pub fn encode_client_message(message: &ClientMessage) -> Result<Vec<u8>, ProtocolError> {
    validate_client(message)?;
    encode_json(message)
}

pub fn decode_server_message(frame: &[u8]) -> Result<ServerMessage, ProtocolError> {
    let message = decode_json(frame)?;
    validate_server(&message)?;
    Ok(message)
}

pub fn encode_server_message(message: &ServerMessage) -> Result<Vec<u8>, ProtocolError> {
    validate_server(message)?;
    encode_json(message)
}

fn version(version: u16) -> Result<(), ProtocolError> {
    if version == VERSION {
        Ok(())
    } else {
        Err(ProtocolError::UnsupportedVersion(version))
    }
}

fn validate_client(message: &ClientMessage) -> Result<(), ProtocolError> {
    match message {
        ClientMessage::Hello(hello) => {
            validate_protocol_versions(&hello.supported_protocol_versions)?;
            check_text(&hello.client_version, MAX_LABEL_BYTES, false)
        }
        ClientMessage::Request(request) => {
            version(request.protocol_version)?;
            check_id(&request.request_id)?;
            if !allows(&request.request) {
                return Err(ProtocolError::InvalidMessage);
            }
            if let Request::PrepareUpdate(params) = &request.request {
                check_binary_hash(&params.target_hash)?;
            }
            Ok(())
        }
    }
}

fn validate_server(message: &ServerMessage) -> Result<(), ProtocolError> {
    match message {
        ServerMessage::Welcome(welcome) => {
            version(welcome.protocol_version)?;
            check_text(&welcome.daemon_version, MAX_LABEL_BYTES, false)
        }
        ServerMessage::VersionMismatch(mismatch) => {
            validate_protocol_versions(&mismatch.supported_protocol_versions)?;
            check_text(&mismatch.daemon_version, MAX_LABEL_BYTES, false)
        }
        ServerMessage::Response(response) => {
            version(response.protocol_version)?;
            check_id(&response.request_id)?;
            match &response.result {
                RequestResult::Applied(result) => match &result.result {
                    Response::Health(_) | Response::Update(_) | Response::ShutdownAccepted => {
                        validate_response(&result.result)
                    }
                    _ => Err(ProtocolError::InvalidMessage),
                },
                RequestResult::Rejected(result) | RequestResult::Unknown(result) => {
                    validate_error(&result.error)
                }
                RequestResult::Dispatched(_) => Err(ProtocolError::InvalidMessage),
            }
        }
        ServerMessage::Event(_) => Err(ProtocolError::InvalidMessage),
    }
}
