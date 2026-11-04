use super::{
    Error, MAX_MESSAGE_BYTES, MAX_PARTS, MAX_PENDING, Observation, Result, Session, string,
};
use cafe_space_protocol::*;
use serde_json::Value;

fn bounded(message: &TranscriptMessage) -> Result<()> {
    // shortcut: text-only v2 messages are capped at 64 KiB, add paginated attachment support before raising this.
    if message.parts.len() > MAX_PARTS
        || serde_json::to_vec(message)
            .map_err(|_| Error("PI_MESSAGE_INVALID"))?
            .len()
            > MAX_MESSAGE_BYTES
    {
        return Err(Error("PI_MESSAGE_TOO_LARGE"));
    }
    Ok(())
}
fn text_parts(content: &Value) -> Result<Vec<MessagePart>> {
    if let Some(text) = content.as_str() {
        return Ok(vec![MessagePart::Text(TextPart { text: text.into() })]);
    }
    let content = content.as_array().ok_or(Error("PI_MESSAGE_INVALID"))?;
    if content.len() > MAX_PARTS {
        return Err(Error("PI_MESSAGE_TOO_LARGE"));
    }
    content
        .iter()
        .map(|part| match string(part, "type")? {
            "text" => Ok(MessagePart::Text(TextPart {
                text: string(part, "text")?.into(),
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
            _ => Err(Error("PI_CONTENT_UNSUPPORTED")),
        })
        .collect()
}
fn output(result: &Value) -> Result<String> {
    if let Some(parts) = result["content"].as_array() {
        let mut text = String::new();
        for part in parts {
            if part["type"] != "text" {
                return Err(Error("PI_CONTENT_UNSUPPORTED"));
            }
            if !text.is_empty() {
                text.push('\n');
            }
            let next = string(part, "text")?;
            if text.len() + next.len() > MAX_MESSAGE_BYTES {
                return Err(Error("PI_MESSAGE_TOO_LARGE"));
            }
            text.push_str(next);
        }
        Ok(text)
    } else {
        Err(Error("PI_MESSAGE_INVALID"))
    }
}
impl Session {
    fn message_id(&mut self) -> Result<String> {
        self.message_counter = self
            .message_counter
            .checked_add(1)
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
                    .is_some_and(|message| !complete || message.role != role)
                {
                    return Err(Error("PI_MESSAGE_INVALID"));
                }
                let id = if complete {
                    match self.active_message.take() {
                        Some(message) => message.id,
                        None => self.message_id()?,
                    }
                } else {
                    self.message_id()?
                };
                let message = TranscriptMessage {
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
                bounded(&message)?;
                observations.push(Observation::Message(message.clone()));
                if !complete {
                    self.active_message = Some(message);
                }
            }
            "message_update" => {
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
                    return Err(Error("PI_MESSAGE_TOO_LARGE"));
                }
                let index = index as usize;
                let message = self
                    .active_message
                    .as_mut()
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
                            if text.len() + delta.len() > MAX_MESSAGE_BYTES {
                                return Err(Error("PI_MESSAGE_TOO_LARGE"));
                            }
                            text.push_str(delta);
                        }
                        ("text_end", MessagePart::Text(TextPart { text }))
                        | ("thinking_end", MessagePart::Thinking(TextPart { text })) => {
                            *text = string(event, "content")?.into()
                        }
                        ("toolcall_delta", MessagePart::ToolCall(call)) => {
                            let delta = string(event, "delta")?;
                            if call.arguments.len() + delta.len() > MAX_MESSAGE_BYTES {
                                return Err(Error("PI_MESSAGE_TOO_LARGE"));
                            }
                            call.arguments.push_str(delta);
                        }
                        ("toolcall_end", part @ MessagePart::ToolCall(_)) => {
                            *part = text_parts(&Value::Array(vec![event["toolCall"].clone()]))?
                                .remove(0)
                        }
                        _ => return Err(Error("PI_MESSAGE_INVALID")),
                    }
                }
                bounded(message)?;
                observations.push(Observation::Message(message.clone()));
            }
            kind => {
                let native_id = string(record, "toolCallId")?;
                if native_id.len() > 256 || native_id.is_empty() {
                    return Err(Error("PI_MESSAGE_INVALID"));
                }
                if kind == "tool_execution_start" {
                    if self.tools.len() >= MAX_PENDING {
                        return Err(Error("PI_TOOL_LIMIT"));
                    }
                    if self.tools.contains_key(native_id) {
                        return Err(Error("PI_MESSAGE_INVALID"));
                    }
                    let message = TranscriptMessage {
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
                    bounded(&message)?;
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
                    let message = self
                        .tools
                        .get_mut(native_id)
                        .ok_or(Error("PI_MESSAGE_INVALID"))?;
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
                observations.push(Observation::Message(self.tools[native_id].clone()));
                if kind == "tool_execution_end" {
                    self.tools.remove(native_id);
                }
            }
        }
        Ok(())
    }
}
