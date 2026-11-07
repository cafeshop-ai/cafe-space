//! Shared metadata rules; adding these fields to snapshots requires the full-wire upgrade.
use crate::ProtocolError;

pub const MAX_ACTIVITY_MILLIS: u64 = 8_640_000_000_000_000;
pub const MAX_PREVIEW_BYTES: usize = 256;
pub const MAX_INSERTION_ORDER: u64 = i64::MAX as u64;

pub fn validate_activity(value: u64) -> Result<(), ProtocolError> {
    if value == 0 || value > MAX_ACTIVITY_MILLIS {
        return Err(ProtocolError::InvalidMessage);
    }
    Ok(())
}

pub fn advance_activity(
    previous: Option<u64>,
    observed: Option<u64>,
) -> Result<Option<u64>, ProtocolError> {
    if let Some(value) = previous {
        validate_activity(value)?;
    }
    // A bad clock is not permission to manufacture a new timestamp or repair corrupt storage.
    let observed = observed.filter(|value| validate_activity(*value).is_ok());
    Ok(previous.into_iter().chain(observed).max())
}

pub fn preview_separator(ch: char) -> bool {
    ch.is_whitespace()
        || ch.is_control()
        || matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

pub fn validate_preview(text: &str) -> Result<(), ProtocolError> {
    if text.is_empty()
        || text.len() > MAX_PREVIEW_BYTES
        || text.starts_with(' ')
        || text.ends_with(' ')
        || text.contains("  ")
        || text.chars().any(|ch| ch != ' ' && preview_separator(ch))
    {
        return Err(ProtocolError::InvalidMessage);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn clock_examples_are_monotonic_without_fabricating_unknown_times() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../fixtures/session-activity/clock-examples.json"
        ))
        .unwrap();
        assert_eq!(fixture["maximum"], MAX_ACTIVITY_MILLIS);
        for case in fixture["cases"].as_array().unwrap() {
            let previous = case["previous"].as_u64();
            let observed = case["observed"].as_u64();
            if case["invalidPrevious"] == true {
                assert!(
                    advance_activity(previous, observed).is_err(),
                    "{}",
                    case["name"]
                );
            } else {
                assert_eq!(
                    advance_activity(previous, observed).unwrap(),
                    case["expected"].as_u64(),
                    "{}",
                    case["name"]
                );
            }
        }
        assert!(validate_activity(0).is_err());
        assert!(validate_activity(MAX_ACTIVITY_MILLIS + 1).is_err());
        assert!(validate_activity(MAX_ACTIVITY_MILLIS).is_ok());
    }

    #[test]
    fn shared_preview_rule_rejects_byte_overflow_and_noncanonical_text() {
        for text in [
            "".into(),
            "x".repeat(257),
            "茶".repeat(86),
            " leading".into(),
            "trailing ".into(),
            "two  spaces".into(),
            "line\nbreak".into(),
            "bidi\u{202e}text".into(),
        ] {
            assert!(validate_preview(&text).is_err());
        }
        for text in ["x".repeat(256), "茶".repeat(85), "plain ☕ 👩‍💻".into()] {
            validate_preview(&text).unwrap();
        }
    }
}
