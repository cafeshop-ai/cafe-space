use super::{
    Error, MAX_MESSAGE_BYTES, MAX_PARTS, MAX_PENDING, Observation, Result, Session, UserPreview,
    preview, string,
};
use cafe_space_protocol::*;
use serde_json::Value;

fn size(value: &impl serde::Serialize) -> Result<usize> {
    Ok(serde_json::to_vec(value)
        .map_err(|_| Error("PI_MESSAGE_INVALID"))?
        .len())
}
fn clipped(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.into();
    }
    let mut end = limit;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[Truncated: {} UTF-8 bytes]",
        &value[..end],
        value.len()
    )
}
fn text(value: String) -> MessagePart {
    MessagePart::Text(TextPart { text: value })
}
fn placeholder(part: &Value) -> Result<String> {
    Ok(format!(
        "[{} not displayed: {} encoded JSON bytes]",
        if part["type"] == "image" {
            "Image"
        } else {
            "Unsupported content"
        },
        size(part)?
    ))
}
fn bounded(message: &mut TranscriptMessage) -> Result<bool> {
    let bytes = size(message)?;
    if bytes <= MAX_MESSAGE_BYTES && message.parts.len() <= MAX_PARTS {
        return Ok(false);
    }
    // shortcut: oversized displays use explicit excerpts, add paginated attachments for full output later.
    if let [MessagePart::ToolCall(call)] = &mut message.parts[..] {
        call.id = clipped(&call.id, 1024);
        call.name = clipped(&call.name, 1024);
        call.arguments = clipped(&call.arguments, 1024);
        call.output = call.output.as_ref().map(|output| clipped(output, 4096));
    } else {
        let excerpt = message
            .parts
            .iter()
            .find_map(|part| match part {
                MessagePart::Text(part) | MessagePart::Thinking(part) => {
                    Some(clipped(&part.text, 4096))
                }
                _ => None,
            })
            .unwrap_or_default();
        message.parts = vec![text(format!(
            "{excerpt}\n[Message display truncated: {bytes} encoded JSON bytes]"
        ))];
    }
    Ok(true)
}
fn text_parts(content: &Value) -> Result<Vec<MessagePart>> {
    if let Some(text) = content.as_str() {
        return Ok(vec![MessagePart::Text(TextPart { text: text.into() })]);
    }
    let content = content.as_array().ok_or(Error("PI_MESSAGE_INVALID"))?;
    let mut parts: Vec<MessagePart> = content
        .iter()
        .take(if content.len() > MAX_PARTS {
            MAX_PARTS - 1
        } else {
            MAX_PARTS
        })
        .map(|part| match string(part, "type")? {
            "text" => Ok(MessagePart::Text(TextPart {
                text: string(part, "text")?.into(),
            })),
            "thinking" if part["redacted"] == true => Ok(MessagePart::Thinking(TextPart {
                text: "[Redacted thinking]".into(),
            })),
            "thinking" => Ok(MessagePart::Thinking(TextPart {
                text: string(part, "thinking")?.into(),
            })),
            "toolCall" => Ok(MessagePart::ToolCall(ToolCallPart {
                id: string(part, "id")?.into(),
                name: string(part, "name")?.into(),
                arguments: serde_json::to_string(
                    part.get("arguments").ok_or(Error("PI_MESSAGE_INVALID"))?,
                )
                .map_err(|_| Error("PI_MESSAGE_INVALID"))?,
                output: None,
                status: ToolStatus::Running,
            })),
            _ => Ok(text(placeholder(part)?)),
        })
        .collect::<Result<_>>()?;
    if content.len() > MAX_PARTS {
        parts.push(text(format!(
            "[{} content blocks omitted: {} encoded JSON bytes in full content]",
            content.len() - MAX_PARTS + 1,
            size(&content)?
        )));
    }
    Ok(parts)
}
fn output(result: &Value) -> Result<String> {
    if let Some(parts) = result["content"].as_array() {
        let mut text = String::new();
        for part in parts {
            if !text.is_empty() {
                text.push('\n');
            }
            if part["type"] == "text" {
                text.push_str(string(part, "text")?);
            } else {
                text.push_str(&placeholder(part)?);
            }
            if text.len() > MAX_MESSAGE_BYTES {
                return Ok(format!(
                    "{}\n[Tool result truncated: {} encoded JSON bytes]",
                    clipped(&text, 4096),
                    size(result)?
                ));
            }
        }
        Ok(text)
    } else {
        Err(Error("PI_MESSAGE_INVALID"))
    }
}
/// Mark a cached display incomplete without inferring the native execution outcome.
pub fn incomplete_message(message: &mut TranscriptMessage) -> Result<()> {
    message.status = TranscriptStatus::Error;
    message.parts.push(text("[Transcript incomplete: native message completion was not observed; execution outcome is not inferred]".into()));
    bounded(message)?;
    Ok(())
}

impl Session {
    /// Project the runtime's current conversation, never append-order native file entries.
    pub(super) fn history(&mut self, value: &Value) -> Result<Observation> {
        let entries = value.as_array().ok_or(Error("PI_HISTORY_INVALID"))?;
        if entries.len() > 4096 || self.active_message.is_some() || !self.tools.is_empty() {
            return Err(Error("PI_HISTORY_INVALID"));
        }
        let mut retained = std::collections::VecDeque::new();
        let mut bytes = 0;
        let mut truncated = false;
        let mut user_preview = None;
        let mut calls = std::collections::BTreeMap::new();
        for entry in entries {
            truncated |= size(entry)? > MAX_MESSAGE_BYTES;
            let mut observations = Vec::new();
            match string(entry, "role")? {
                "system" => continue,
                "user" | "assistant" => {
                    self.transcript(
                        &serde_json::json!({"type":"message_end", "message":entry}),
                        &mut observations,
                    )?;
                    if self.active_truncated {
                        truncated = true;
                    }
                    if let Some(content) = entry["content"].as_array() {
                        for call in content.iter().filter(|part| part["type"] == "toolCall") {
                            let id = string(call, "id")?;
                            if calls.len() < MAX_PENDING && id.len() <= 256 && !id.is_empty() {
                                if calls.insert(id, call).is_some() {
                                    return Err(Error("PI_HISTORY_INVALID"));
                                }
                            } else {
                                truncated = true;
                            }
                        }
                    }
                }
                "toolResult" => {
                    let id = string(entry, "toolCallId")?;
                    let Some(call) = calls.remove(id) else {
                        truncated = true;
                        continue;
                    };
                    if string(call, "name")? != string(entry, "toolName")? {
                        return Err(Error("PI_HISTORY_INVALID"));
                    }
                    self.transcript(&serde_json::json!({"type":"tool_execution_start", "toolCallId":id, "toolName":call["name"], "args":call["arguments"]}), &mut Vec::new())?;
                    self.transcript(&serde_json::json!({"type":"tool_execution_end", "toolCallId":id, "isError":entry["isError"], "result":entry}), &mut observations)?;
                }
                _ => {
                    truncated = true;
                    continue;
                }
            }
            for observation in observations {
                if let Observation::Message(mut message, preview) = observation {
                    if preview.is_some() {
                        user_preview = preview;
                    }
                    if matches!(entry["stopReason"].as_str(), Some("pending" | "deferred")) {
                        incomplete_message(&mut message)?;
                        truncated = true;
                    }
                    encode_server_message(&ServerMessage::Event(ServerEvent {
                        message_type: EventMessageType::Event,
                        protocol_version: PROTOCOL_VERSION,
                        event_stream_id: "validation".into(),
                        sequence: 1,
                        event: Event::MessageChanged(MessageChangedData {
                            session_id: "validation".into(),
                            message: message.clone(),
                        }),
                    }))
                    .map_err(|_| Error("PI_HISTORY_INVALID"))?;
                    let encoded = size(&message)?;
                    bytes += encoded;
                    retained.push_back((message, encoded));
                    while retained.len() > 128 || bytes > 128 * 1024 {
                        bytes -= retained.pop_front().expect("retained message").1;
                        truncated = true;
                    }
                }
            }
        }
        truncated |= !calls.is_empty();
        self.active_truncated = false;
        Ok(Observation::HistoryLoaded {
            messages: retained.into_iter().map(|(message, _)| message).collect(),
            truncated,
            user_preview,
        })
    }

    pub(super) fn incomplete_transcripts(&mut self) -> Result<Vec<Observation>> {
        let messages = self
            .active_message
            .take()
            .into_iter()
            .map(|(message, _)| message)
            .chain(std::mem::take(&mut self.tools).into_values());
        let mut observations = Vec::new();
        for mut message in messages {
            incomplete_message(&mut message)?;
            observations.push(Observation::Message(message, None));
        }
        Ok(observations)
    }
    pub(super) fn omitted_record(&mut self, bytes: u64) -> Result<Vec<Observation>> {
        self.transcript_gap = true;
        self.active_truncated = false;
        let mut observations = self.incomplete_transcripts()?;
        observations.push(Observation::HistoryTruncated);
        // Its type is unknown: never acknowledge a command or infer readiness from a discarded record.
        observations.push(Observation::Message(TranscriptMessage {
            id: self.message_id()?,
            role: TranscriptRole::System,
            status: TranscriptStatus::Complete,
            parts: vec![text(format!(
                "[RPC record omitted: {bytes} bytes exceeds display limit; transcript may be incomplete]"
            ))],
        }, None));
        Ok(observations)
    }
    fn message_id(&mut self) -> Result<String> {
        self.message_counter = self
            .message_counter
            .checked_add(1)
            .filter(|order| *order <= session_activity::MAX_INSERTION_ORDER)
            .ok_or(Error("PI_COUNTER_EXHAUSTED"))?;
        Ok(format!("message-{}", self.message_counter))
    }
    pub(super) fn transcript(
        &mut self,
        record: &Value,
        observations: &mut Vec<Observation>,
    ) -> Result<()> {
        match string(record, "type")? {
            "message_start" | "message_end" => {
                let native = &record["message"];
                let role = match string(native, "role")? {
                    "user" => TranscriptRole::User,
                    "assistant" => TranscriptRole::Assistant,
                    // Tool execution has its own stable lifecycle below, not a second transcript copy.
                    "toolResult" => return Ok(()),
                    _ => {
                        observations.push(Observation::Notice("PI_MESSAGE_ROLE_UNSUPPORTED"));
                        return Ok(());
                    }
                };
                let complete = record["type"] == "message_end";
                if self
                    .active_message
                    .as_ref()
                    .is_some_and(|(message, _)| !complete || message.role != role)
                {
                    return Err(Error("PI_MESSAGE_INVALID"));
                }
                let (id, insertion_order) = if complete {
                    match self.active_message.take() {
                        Some((message, order)) => (message.id, order),
                        None => (self.message_id()?, self.message_counter),
                    }
                } else {
                    (self.message_id()?, self.message_counter)
                };
                let user_preview = if role == TranscriptRole::User {
                    Some(UserPreview {
                        message_id: id.clone(),
                        insertion_order,
                        text: preview::extract(&native["content"])?,
                    })
                } else {
                    None
                };
                let mut message = TranscriptMessage {
                    id,
                    role,
                    parts: text_parts(&native["content"])?,
                    status: if matches!(native["stopReason"].as_str(), Some("error" | "aborted")) {
                        TranscriptStatus::Error
                    } else if complete {
                        TranscriptStatus::Complete
                    } else {
                        TranscriptStatus::Streaming
                    },
                };
                self.active_truncated = bounded(&mut message)?
                    || native["content"]
                        .as_array()
                        .is_some_and(|parts| parts.len() > MAX_PARTS);
                observations.push(Observation::Message(message.clone(), user_preview));
                if !complete {
                    self.active_message = Some((message, insertion_order));
                }
            }
            "message_update" => {
                if self.active_truncated || (self.transcript_gap && self.active_message.is_none()) {
                    return Ok(());
                }
                let event = &record["assistantMessageEvent"];
                let kind = string(event, "type")?;
                if !matches!(
                    kind,
                    "text_start"
                        | "text_delta"
                        | "text_end"
                        | "thinking_start"
                        | "thinking_delta"
                        | "thinking_end"
                        | "toolcall_start"
                        | "toolcall_delta"
                        | "toolcall_end"
                ) {
                    return Ok(());
                }
                let index = event["contentIndex"]
                    .as_u64()
                    .ok_or(Error("PI_MESSAGE_INVALID"))?;
                if index >= MAX_PARTS as u64 {
                    let message = self
                        .active_message
                        .as_mut()
                        .map(|(message, _)| message)
                        .ok_or(Error("PI_MESSAGE_INVALID"))?;
                    message.parts = vec![text(format!(
                        "[Streaming content omitted: block index {index} exceeds display limit]"
                    ))];
                    self.active_truncated = true;
                    observations.push(Observation::Message(message.clone(), None));
                    return Ok(());
                }
                let index = index as usize;
                let message = self
                    .active_message
                    .as_mut()
                    .map(|(message, _)| message)
                    .ok_or(Error("PI_MESSAGE_INVALID"))?;
                if message.role != TranscriptRole::Assistant || index > message.parts.len() {
                    return Err(Error("PI_MESSAGE_INVALID"));
                }
                let start = match kind {
                    "text_start" => Some(MessagePart::Text(TextPart {
                        text: String::new(),
                    })),
                    "thinking_start" => Some(MessagePart::Thinking(TextPart {
                        text: String::new(),
                    })),
                    "toolcall_start" => Some(MessagePart::ToolCall(ToolCallPart {
                        id: string(event, "id")?.into(),
                        name: string(event, "toolName")?.into(),
                        arguments: String::new(),
                        output: None,
                        status: ToolStatus::Running,
                    })),
                    _ => None,
                };
                if let Some(part) = start {
                    if index == message.parts.len() {
                        message.parts.push(part);
                    } else {
                        message.parts[index] = part;
                    }
                } else {
                    let part = message
                        .parts
                        .get_mut(index)
                        .ok_or(Error("PI_MESSAGE_INVALID"))?;
                    match (kind, part) {
                        ("text_delta", MessagePart::Text(TextPart { text }))
                        | ("thinking_delta", MessagePart::Thinking(TextPart { text })) => {
                            let delta = string(event, "delta")?;
                            text.push_str(delta);
                        }
                        ("text_end", MessagePart::Text(TextPart { text }))
                        | ("thinking_end", MessagePart::Thinking(TextPart { text })) => {
                            *text = string(event, "content")?.into()
                        }
                        ("toolcall_delta", MessagePart::ToolCall(call)) => {
                            let delta = string(event, "delta")?;
                            call.arguments.push_str(delta);
                        }
                        ("toolcall_end", part @ MessagePart::ToolCall(_)) => {
                            *part = text_parts(&Value::Array(vec![event["toolCall"].clone()]))?
                                .remove(0)
                        }
                        _ => return Err(Error("PI_MESSAGE_INVALID")),
                    }
                }
                self.active_truncated = bounded(message)?;
                observations.push(Observation::Message(message.clone(), None));
            }
            kind => {
                let native_id = string(record, "toolCallId")?;
                if native_id.len() > 256 || native_id.is_empty() {
                    return Err(Error("PI_MESSAGE_INVALID"));
                }
                if kind == "tool_execution_start" {
                    if self.tools.len() >= MAX_PENDING {
                        self.transcript_gap = true;
                        observations.push(Observation::Message(TranscriptMessage {
                            id: self.message_id()?,
                            role: TranscriptRole::System,
                            status: TranscriptStatus::Complete,
                            parts: vec![text(format!(
                                "[Tool display omitted: more than {MAX_PENDING} simultaneous tools]"
                            ))],
                        }, None));
                        return Ok(());
                    }
                    if self.tools.contains_key(native_id) {
                        return Err(Error("PI_MESSAGE_INVALID"));
                    }
                    let mut message = TranscriptMessage {
                        id: self.message_id()?,
                        role: TranscriptRole::Tool,
                        status: TranscriptStatus::Streaming,
                        parts: vec![MessagePart::ToolCall(ToolCallPart {
                            id: native_id.into(),
                            name: string(record, "toolName")?.into(),
                            arguments: serde_json::to_string(
                                record.get("args").ok_or(Error("PI_MESSAGE_INVALID"))?,
                            )
                            .map_err(|_| Error("PI_MESSAGE_INVALID"))?,
                            output: None,
                            status: ToolStatus::Running,
                        })],
                    };
                    bounded(&mut message)?;
                    self.tools.insert(native_id.into(), message);
                } else {
                    let complete = kind == "tool_execution_end";
                    let is_error = if complete {
                        record["isError"]
                            .as_bool()
                            .ok_or(Error("PI_MESSAGE_INVALID"))?
                    } else {
                        false
                    };
                    let rendered =
                        output(&record[if complete { "result" } else { "partialResult" }])?;
                    let Some(message) = self.tools.get_mut(native_id) else {
                        if self.transcript_gap {
                            return Ok(());
                        }
                        return Err(Error("PI_MESSAGE_INVALID"));
                    };
                    let MessagePart::ToolCall(call) = &mut message.parts[0] else {
                        return Err(Error("PI_MESSAGE_INVALID"));
                    };
                    call.output = Some(rendered);
                    if complete {
                        call.status = if is_error {
                            ToolStatus::Error
                        } else {
                            ToolStatus::Complete
                        };
                        message.status = if is_error {
                            TranscriptStatus::Error
                        } else {
                            TranscriptStatus::Complete
                        };
                    }
                    bounded(message)?;
                }
                observations.push(Observation::Message(self.tools[native_id].clone(), None));
                if kind == "tool_execution_end" {
                    self.tools.remove(native_id);
                }
            }
        }
        Ok(())
    }
}
