use super::{Error, Result, UserPreview, string};
use cafe_space_protocol::session_activity::{
    MAX_INSERTION_ORDER, MAX_PREVIEW_BYTES as MAX_BYTES, preview_separator as separator,
    validate_preview,
};
use serde_json::Value;

impl UserPreview {
    pub fn validate(&self) -> Result<()> {
        if self.message_id.is_empty()
            || self.message_id.len() > 128
            || self.message_id.chars().any(char::is_control)
            || self.insertion_order == 0
            || self.insertion_order > MAX_INSERTION_ORDER
            || self
                .text
                .as_ref()
                .is_some_and(|text| validate_preview(text).is_err())
        {
            return Err(Error("PI_PREVIEW_INVALID"));
        }
        Ok(())
    }
}

pub(super) fn extract(content: &Value) -> Result<Option<String>> {
    if !content.is_string() && !content.is_array() {
        return Err(Error("PI_MESSAGE_INVALID"));
    }
    let texts = content.as_str().map(Ok).into_iter().chain(
        content
            .as_array()
            .into_iter()
            .flatten()
            .filter(|part| part["type"] == "text")
            .map(|part| string(part, "text")),
    );
    let mut output = String::new();
    let mut full = false;
    for text in texts {
        let text = text?;
        if full {
            continue;
        }
        let mut space = !output.is_empty();
        for ch in text.chars() {
            if separator(ch) {
                space = !output.is_empty();
                continue;
            }
            if output.len() + usize::from(space) + ch.len_utf8() > MAX_BYTES {
                full = true;
                break;
            }
            if space {
                output.push(' ');
            }
            output.push(ch);
            space = false;
        }
    }
    Ok((!output.is_empty()).then_some(output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn raw_user_text_matches_shared_unicode_and_byte_boundary_examples() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../fixtures/session-activity/proposal.json"
        ))
        .unwrap();
        assert_eq!(fixture["fields"]["preview"]["maxUtf8Bytes"], MAX_BYTES);
        for case in fixture["previewExamples"].as_array().unwrap() {
            let content = case["textParts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|text| json!({"type":"text", "text":text}))
                .collect::<Vec<_>>();
            let text = extract(&json!(content)).unwrap();
            UserPreview {
                message_id: "message-fixture".into(),
                insertion_order: 1,
                text: text.clone(),
            }
            .validate()
            .unwrap();
            assert_eq!(serde_json::to_value(text).unwrap(), case["expected"]);
        }
        for case in fixture["boundaryExamples"].as_array().unwrap() {
            let scalar = case["scalar"].as_str().unwrap();
            let text = scalar.repeat(case["count"].as_u64().unwrap() as usize);
            let expected = scalar.repeat(case["expectedCount"].as_u64().unwrap() as usize);
            let preview = extract(&json!(text)).unwrap().unwrap();
            assert_eq!(preview, expected);
            assert_eq!(
                preview.len(),
                case["expectedUtf8Bytes"].as_u64().unwrap() as usize
            );
        }
    }

    #[test]
    fn native_source_distinguishes_opaque_blocks_from_literal_placeholders() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../fixtures/session-activity/source-provenance.json"
        ))
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            for (source, expected) in [
                ("nativeOpaque", "opaquePreview"),
                ("nativeLiteral", "literalPreview"),
            ] {
                assert_eq!(
                    serde_json::to_value(extract(&case[source]["content"]).unwrap()).unwrap(),
                    case[expected]
                );
            }
        }
        assert_eq!(extract(&json!([{"type":"thinking", "thinking":"hidden"}, {"type":"toolCall", "arguments":{"hidden":true}}, {"type":"text", "text":"visible"}])).unwrap().as_deref(), Some("visible"));
        assert_eq!(extract(&json!([])).unwrap(), None);
    }

    #[test]
    fn provenance_validation_rejects_invalid_ids_and_noncanonical_text() {
        for message_id in [String::new(), "x".repeat(129), "bad\nidentifier".into()] {
            assert_eq!(
                UserPreview {
                    message_id,
                    insertion_order: 1,
                    text: None
                }
                .validate(),
                Err(Error("PI_PREVIEW_INVALID"))
            );
        }
        for text in [
            String::new(),
            "x".repeat(MAX_BYTES + 1),
            " leading".into(),
            "trailing ".into(),
            "two  spaces".into(),
            "line\nbreak".into(),
            "tab\tbreak".into(),
            "unicode\u{2028}separator".into(),
            "bidi\u{202e}format".into(),
            "null\0byte".into(),
        ] {
            assert_eq!(
                UserPreview {
                    message_id: "message-fixture".into(),
                    insertion_order: 1,
                    text: Some(text)
                }
                .validate(),
                Err(Error("PI_PREVIEW_INVALID"))
            );
        }
        for text in [
            None,
            Some("x".repeat(MAX_BYTES)),
            Some("plain ☕ 👩‍💻".into()),
        ] {
            UserPreview {
                message_id: "message-fixture".into(),
                insertion_order: 1,
                text,
            }
            .validate()
            .unwrap();
        }
    }

    #[test]
    fn insertion_order_is_positive_and_fits_the_private_sqlite_integer() {
        for insertion_order in [0, MAX_INSERTION_ORDER + 1, u64::MAX] {
            assert_eq!(
                UserPreview {
                    message_id: "message-fixture".into(),
                    insertion_order,
                    text: None
                }
                .validate(),
                Err(Error("PI_PREVIEW_INVALID"))
            );
        }
        UserPreview {
            message_id: "message-fixture".into(),
            insertion_order: MAX_INSERTION_ORDER,
            text: None,
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn invalid_text_fails_even_after_the_preview_is_full() {
        for content in [
            Value::Null,
            json!({"text":"invalid container"}),
            json!([{"type":"text", "text":3}]),
            json!([{"type":"text", "text":"x".repeat(MAX_BYTES + 1)}, {"type":"text", "text":false}]),
        ] {
            assert!(extract(&content).is_err());
        }
        assert_eq!(
            extract(&json!(format!("{} \u{1f600}", "x".repeat(MAX_BYTES - 1))))
                .unwrap()
                .unwrap(),
            "x".repeat(MAX_BYTES - 1)
        );
    }
}
