//! Opt-in debug probe only. The fixture endpoint never enters JavaScript.
use cafe_space_protocol::{
    MessagePart, RequestResult, Response, ServerMessage, SessionState, SessionStopReason,
    TranscriptRole, TranscriptStatus,
};
use std::path::{Path, PathBuf};

pub const SESSION: &str = "host-commands";
pub const TEXT: &str = "Final synthetic 茶";

pub fn enabled() -> bool {
    cfg!(debug_assertions) && std::env::var("CAFE_SPACE_M1_PROBE").as_deref() == Ok("1")
}

fn valid_endpoint(endpoint: &Path) -> bool {
    #[cfg(windows)]
    {
        endpoint
            .to_str()
            .and_then(|value| value.strip_prefix(r"\\.\pipe\cafe-desktop-fixture-"))
            .is_some_and(|suffix| {
                !suffix.is_empty()
                    && suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
            })
    }
    #[cfg(unix)]
    {
        endpoint
            .file_name()
            .is_some_and(|name| name == "desktop.sock")
            && endpoint.parent().is_some_and(|parent| {
                parent.file_name().is_some_and(|name| name == "private")
                    && parent.parent().is_some_and(|root| {
                        root.parent() == Some(Path::new("/tmp"))
                            && root
                                .file_name()
                                .and_then(|name| name.to_str())
                                .is_some_and(|name| name.starts_with(".tmp") && name.len() > 4)
                    })
            })
    }
}

pub fn endpoint() -> Result<Option<PathBuf>, &'static str> {
    if !enabled() {
        return Ok(None);
    }
    let path = PathBuf::from(
        std::env::var_os("CAFE_SPACE_M1_PROBE_ENDPOINT").ok_or("M1_PROBE_ENDPOINT_REQUIRED")?,
    );
    if !valid_endpoint(&path) {
        return Err("M1_PROBE_ENDPOINT_INVALID");
    }
    Ok(Some(path))
}

pub fn confirms_chat(message: &ServerMessage) -> bool {
    let ServerMessage::Response(frame) = message else {
        return false;
    };
    let RequestResult::Applied(value) = &frame.result else {
        return false;
    };
    let Response::Session(response) = &value.result else {
        return false;
    };
    let view = &response.session;
    view.session.id == SESSION
        && view.session.state == SessionState::Stopped
        && view.session.stop_reason == Some(SessionStopReason::SessionClosed)
        && view.session.active_run_id.is_none()
        && view.messages.iter().any(|message| {
            message.role == TranscriptRole::Assistant
                && message.status == TranscriptStatus::Complete
                && message
                    .parts
                    .iter()
                    .any(|part| matches!(part, MessagePart::Text(value) if value.text == TEXT))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_endpoint_cannot_select_a_production_endpoint() {
        #[cfg(windows)]
        let (valid, invalid) = (
            r"\\.\pipe\cafe-desktop-fixture-123-.tmpExample",
            r"\\.\pipe\cafe-spaced-dev-S-1-5-21",
        );
        #[cfg(unix)]
        let (valid, invalid) = (
            "/tmp/.tmpExample/private/desktop.sock",
            "/tmp/cafe-space-dev/cafe-spaced-dev.sock",
        );
        assert!(valid_endpoint(Path::new(valid)));
        assert!(!valid_endpoint(Path::new(invalid)));
        assert!(!valid_endpoint(Path::new("relative/desktop.sock")));
        assert!(!valid_endpoint(Path::new(
            "/tmp/.tmpExample/../private/desktop.sock"
        )));
    }

    #[test]
    fn chat_proof_requires_a_complete_stopped_authoritative_snapshot() {
        let contract: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/pi-rpc/desktop-host-v5.json"
        ))
        .unwrap();
        assert_eq!(contract["sessionId"], SESSION);
        assert_eq!(contract["expectedText"], TEXT);
        let mut frame: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/v2/session-response.json"))
                .unwrap();
        let view = &mut frame["result"]["data"]["result"]["data"]["session"];
        view["session"]["id"] = SESSION.into();
        view["session"]["state"] = serde_json::json!({"state":"stopped"});
        view["session"]["stopReason"] = "SESSION_CLOSED".into();
        view["session"]
            .as_object_mut()
            .unwrap()
            .remove("activeRunId");
        view["messages"] = serde_json::json!([{"id":"synthetic", "role":"assistant", "status":"complete", "parts":[{"kind":"text", "data":{"text":TEXT}}]}]);
        let check = |value: &serde_json::Value| {
            cafe_space_protocol::decode_server_message(&serde_json::to_vec(value).unwrap())
                .is_ok_and(|message| confirms_chat(&message))
        };
        assert!(check(&frame));
        for (key, value) in [
            ("state", serde_json::json!({"state":"ready"})),
            ("stopReason", "AGENT_EXITED".into()),
            ("activeRunId", "unfinished".into()),
            ("id", "another-session".into()),
        ] {
            let mut wrong = frame.clone();
            wrong["result"]["data"]["result"]["data"]["session"]["session"][key] = value;
            assert!(!check(&wrong));
        }
        let mut partial = frame.clone();
        partial["result"]["data"]["result"]["data"]["session"]["messages"][0]["status"] =
            "streaming".into();
        assert!(!check(&partial));
        partial["result"]["data"]["result"]["data"]["session"]["messages"] = serde_json::json!([]);
        assert!(!check(&partial));
    }
}
