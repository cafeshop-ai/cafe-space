use super::*;
use std::io::{BufReader, Cursor};

fn running() -> Session {
    let mut session = tests::startup();
    session.observe(json!({"type":"agent_start"})).unwrap();
    session
}
fn display(observations: &[Observation]) -> String {
    let mut rendered = String::new();
    for observation in observations {
        if let Observation::Message(message, _) = observation {
            let bytes = serde_json::to_vec(message).unwrap();
            assert!(bytes.len() <= MAX_MESSAGE_BYTES);
            assert!(message.parts.len() <= MAX_PARTS);
            rendered.push_str(std::str::from_utf8(&bytes).unwrap());
        }
    }
    rendered
}

#[test]
fn oversized_record_is_drained_with_a_count_and_never_completes_a_command() {
    let contract: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/pi-rpc/display-limits.json"
    ))
    .unwrap();
    let bytes = contract["cases"][0]["bytes"].as_u64().unwrap() as usize;
    assert_eq!(bytes, framing::MAX_RECORD_BYTES + 1);
    let mut input = vec![b'x'; bytes];
    input.extend_from_slice(b"\n{\"type\":\"agent_settled\",\"aborted\":false}\n");
    for capacity in [1, 4096, bytes + 100] {
        let mut reader =
            framing::Records::new(BufReader::with_capacity(capacity, Cursor::new(&input)));
        let mut session = running();
        let pending = session.command(Command::GetState).unwrap();
        let omitted = reader.next_record().unwrap().unwrap();
        assert_eq!(
            omitted,
            framing::Record::Omitted {
                bytes: bytes as u64
            }
        );
        let observed = session.observe(omitted).unwrap();
        assert!(observed.contains(&Observation::HistoryTruncated));
        assert!(display(&observed).contains(&bytes.to_string()));
        assert!(
            !observed
                .iter()
                .any(|value| matches!(value, Observation::Outcome { .. }))
        );
        assert_eq!(session.state(), &SessionState::Running);
        assert_eq!(
            session.expire(&pending.id),
            Some(Observation::Outcome {
                id: pending.id,
                outcome: Outcome::Unknown("PI_RESPONSE_TIMEOUT")
            })
        );
        session
            .observe(reader.next_record().unwrap().unwrap())
            .unwrap();
        assert_eq!(session.state(), &SessionState::Ready);
        assert_eq!(reader.next_record().unwrap(), None);
    }
    let mut exact = b"{\"type\":\"ignored\"}".to_vec();
    exact.resize(framing::MAX_RECORD_BYTES, b' ');
    exact.push(b'\n');
    assert!(matches!(
        framing::Records::new(Cursor::new(exact))
            .next_record()
            .unwrap(),
        Some(framing::Record::Json(_))
    ));
}

#[test]
fn text_escape_expansion_and_streamed_parts_have_nonfatal_display_bounds() {
    let mut session = running();
    session
        .observe(json!({"type":"message_start","message":{"role":"assistant","content":[]}}))
        .unwrap();
    session.observe(json!({"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":0}})).unwrap();
    let delta = "茶\u{0}\"".repeat(5000);
    let mut rendered = String::new();
    for _ in 0..4 {
        rendered.push_str(&display(&session.observe(json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":delta}})).unwrap()));
    }
    assert!(rendered.contains("truncated"));
    assert_eq!(session.state(), &SessionState::Running);
    let observed = session.observe(json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"茶".repeat(MAX_MESSAGE_BYTES)}]}})).unwrap();
    assert!(display(&observed).contains("truncated"));
    assert!(
        matches!(&observed[0],Observation::Message(message, None) if message.status == TranscriptStatus::Complete)
    );
    session
        .observe(json!({"type":"message_start","message":{"role":"assistant","content":[]}}))
        .unwrap();
    assert!(display(&session.observe(json!({"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":MAX_PARTS}})).unwrap()).contains("block index"));
    let parts = vec![json!({"type":"text","text":"x"}); MAX_PARTS + 1];
    assert!(
        display(
            &session
                .observe(
                    json!({"type":"message_end","message":{"role":"assistant","content":parts}})
                )
                .unwrap()
        )
        .contains("content blocks omitted")
    );
    assert_eq!(session.state(), &SessionState::Running);
}

#[test]
fn images_unknown_blocks_and_large_tools_remain_bounded_without_leaking_payloads() {
    let mut session = running();
    let observed = session.observe(json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"image","data":"synthetic-hidden-image"},{"type":"future-block","payload":"synthetic-hidden-opaque"}]}})).unwrap();
    let rendered = display(&observed);
    assert!(rendered.contains("Image not displayed") && rendered.contains("Unsupported content"));
    assert!(rendered.contains("encoded JSON bytes") && !rendered.contains("synthetic-hidden"));
    let large = "\u{0}茶".repeat(MAX_MESSAGE_BYTES / 4);
    let start = session.observe(json!({"type":"tool_execution_start","toolCallId":"tool","toolName":"test","args":{"text":large}})).unwrap();
    assert!(display(&start).contains("Truncated"));
    let update = session.observe(json!({"type":"tool_execution_update","toolCallId":"tool","partialResult":{"content":[{"type":"image","data":"synthetic-hidden-image"},{"type":"text","text":large}]}})).unwrap();
    assert!(display(&update).contains("Truncated"));
    let end = session.observe(json!({"type":"tool_execution_end","toolCallId":"tool","isError":false,"result":{"content":[{"type":"image","data":"synthetic-hidden-image"},{"type":"text","text":"complete"}]}})).unwrap();
    assert!(display(&end).contains("complete"));
    assert!(
        matches!(&end[0],Observation::Message(message, None) if message.status==TranscriptStatus::Complete)
    );
    assert_eq!(session.state(), &SessionState::Running);
}

#[test]
fn user_previews_distinguish_identical_live_and_history_displays() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/session-activity/source-provenance.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let expected = vec![MessagePart::Text(TextPart {
            text: case["sameDisplayText"].as_str().unwrap().into(),
        })];
        assert!(case["opaquePreview"].is_null());
        assert_eq!(case["literalPreview"], case["sameDisplayText"]);
        for key in ["nativeOpaque", "nativeLiteral"] {
            let mut session = running();
            let live = session
                .observe(json!({"type":"message_end", "message":case[key]}))
                .unwrap();
            assert_eq!(live.len(), 1);
            let Observation::Message(message, Some(preview)) = &live[0] else {
                panic!("message and user preview expected together")
            };
            assert_eq!(message.role, TranscriptRole::User);
            assert_eq!(message.parts, expected);
            assert_eq!(preview.message_id, message.id);
            assert_eq!(preview.insertion_order, 1);
            let expected_preview = if key == "nativeOpaque" {
                &case["opaquePreview"]
            } else {
                &case["literalPreview"]
            };
            assert_eq!(
                serde_json::to_value(&preview.text).unwrap(),
                *expected_preview
            );
            let mut session = tests::startup();
            let request = session.command(Command::GetMessages).unwrap();
            let history = session
                .observe(json!({"type":"response", "id":request.id, "command":"get_messages", "success":true, "data":{"messages":[case[key]]}}))
                .unwrap();
            let Observation::HistoryLoaded {
                messages,
                user_preview,
                ..
            } = &history[0]
            else {
                panic!("history expected")
            };
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].role, TranscriptRole::User);
            assert_eq!(messages[0].parts, expected);
            let preview = user_preview.as_ref().unwrap();
            assert_eq!(preview.message_id, messages[0].id);
            assert_eq!(preview.insertion_order, 1);
            assert_eq!(
                serde_json::to_value(&preview.text).unwrap(),
                *expected_preview
            );
        }
    }
}

#[test]
fn user_preview_keeps_its_message_id_across_start_end_and_excludes_assistants() {
    let mut session = running();
    let mut id = None;
    for kind in ["message_start", "message_end"] {
        let observed = session
            .observe(json!({"type":kind, "message":{"role":"user", "content":"Original 茶"}}))
            .unwrap();
        assert_eq!(observed.len(), 1);
        let Observation::Message(message, Some(preview)) = &observed[0] else {
            panic!("message and user preview expected together")
        };
        assert_eq!(preview.message_id, message.id);
        assert_eq!(preview.insertion_order, 1);
        assert_eq!(preview.text.as_deref(), Some("Original 茶"));
        if let Some(id) = &id {
            assert_eq!(&message.id, id);
        } else {
            id = Some(message.id.clone());
        }
    }
    let observed = session.observe(json!({"type":"message_end", "message":{"role":"assistant", "content":"not a user preview"}})).unwrap();
    assert_eq!(observed.len(), 1);
    assert!(matches!(&observed[0], Observation::Message(_, None)));
}

#[test]
fn preview_order_survives_interleaved_display_allocation_and_is_not_lexical_id_order() {
    let mut session = running();
    let start = session
        .observe(json!({"type":"message_start", "message":{"role":"user", "content":"first"}}))
        .unwrap();
    let Observation::Message(first, Some(first_preview)) = &start[0] else {
        panic!("paired user expected")
    };
    assert_eq!(first_preview.insertion_order, 1);
    session.observe(json!({"type":"tool_execution_start", "toolCallId":"interleaved", "toolName":"read", "args":{}})).unwrap();
    let end = session
        .observe(json!({"type":"message_end", "message":{"role":"user", "content":"first final"}}))
        .unwrap();
    let Observation::Message(completed, Some(completed_preview)) = &end[0] else {
        panic!("paired user expected")
    };
    assert_eq!(completed.id, first.id);
    assert_eq!(
        completed_preview.insertion_order,
        first_preview.insertion_order
    );
    let third = session
        .observe(json!({"type":"message_end", "message":{"role":"user", "content":"third"}}))
        .unwrap();
    let Observation::Message(third, Some(third_preview)) = &third[0] else {
        panic!("paired user expected")
    };
    assert_eq!(third_preview.insertion_order, 3);
    for _ in 0..6 {
        session
            .observe(
                json!({"type":"message_end", "message":{"role":"assistant", "content":"reply"}}),
            )
            .unwrap();
    }
    let tenth = session
        .observe(json!({"type":"message_end", "message":{"role":"user", "content":"tenth"}}))
        .unwrap();
    let Observation::Message(tenth, Some(tenth_preview)) = &tenth[0] else {
        panic!("paired user expected")
    };
    assert_eq!(tenth_preview.insertion_order, 10);
    assert!(tenth_preview.insertion_order > third_preview.insertion_order);
    assert!(tenth.id.as_str() < third.id.as_str());
}

#[test]
fn insertion_counter_fails_closed_before_signed_sqlite_overflow() {
    let mut session = running();
    session.message_counter = session_activity::MAX_INSERTION_ORDER - 1;
    let last = session
        .observe(json!({"type":"message_end", "message":{"role":"user", "content":"last valid"}}))
        .unwrap();
    assert!(
        matches!(&last[0], Observation::Message(_, Some(preview)) if preview.insertion_order == session_activity::MAX_INSERTION_ORDER)
    );
    let error = session
        .observe(
            json!({"type":"message_end", "message":{"role":"user", "content":"must not wrap"}}),
        )
        .unwrap_err();
    assert_eq!(error, Error("PI_COUNTER_EXHAUSTED"));
    assert_eq!(
        session.message_counter,
        session_activity::MAX_INSERTION_ORDER
    );
}

#[test]
fn redacted_thinking_accepts_missing_text_without_exporting_replay_payloads() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/pi-rpc/display-limits.json"
    ))
    .unwrap();
    let mut session = running();
    let observations = session
        .observe(json!({
            "type": "message_end",
            "message": {"role": "assistant", "content": fixture["redactedThinking"]["content"]}
        }))
        .unwrap();
    let rendered = display(&observations);
    assert!(rendered.contains("Visible answer 茶"));
    assert!(!rendered.contains("synthetic-never-forward"));
    let Observation::Message(message, None) = &observations[0] else {
        panic!("message expected")
    };
    for part in &message.parts[..2] {
        assert!(
            matches!(part, MessagePart::Thinking(value) if value.text == fixture["redactedThinking"]["placeholder"].as_str().unwrap())
        );
    }
    assert_eq!(session.state(), &SessionState::Running);
    session
        .observe(json!({"type":"agent_settled", "aborted":false}))
        .unwrap();
    assert_eq!(session.state(), &SessionState::Ready);
    // An unredacted block still requires real text, rather than silently hiding malformed data.
    assert!(running().observe(json!({"type":"message_end", "message":{"role":"assistant", "content":[{"type":"thinking", "redacted":false}]}})).is_err());
}

#[test]
fn dropped_transcript_records_do_not_poison_following_updates_or_the_next_turn() {
    let mut session = running();
    session
        .observe(json!({"type":"message_start","message":{"role":"assistant","content":[]}}))
        .unwrap();
    session.observe(json!({"type":"tool_execution_start","toolCallId":"lost-tool","toolName":"synthetic","args":{}})).unwrap();
    let gap = session
        .observe(framing::Record::Omitted { bytes: 2_000_000 })
        .unwrap();
    assert_eq!(gap.iter().filter(|value| matches!(value,Observation::Message(message, None) if message.status==TranscriptStatus::Error)).count(), 2);
    assert!(display(&gap).contains("Transcript incomplete"));
    assert!(session.observe(json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"lost-start"}})).unwrap().is_empty());
    assert!(session.observe(json!({"type":"tool_execution_end","toolCallId":"lost-tool","isError":false,"result":{"content":[]}})).unwrap().is_empty());
    session
        .observe(json!({"type":"message_end","message":{"role":"assistant","content":"survived"}}))
        .unwrap();
    session
        .observe(json!({"type":"agent_settled","aborted":false}))
        .unwrap();
    assert_eq!(session.state(), &SessionState::Ready);
    session.observe(json!({"type":"agent_start"})).unwrap();
    assert!(session.observe(json!({"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":0}})).is_err());
}

#[test]
fn overflowing_simultaneous_tools_keeps_the_registry_bounded_and_the_run_alive() {
    let mut session = running();
    for index in 0..=MAX_PENDING {
        let observed = session.observe(json!({"type":"tool_execution_start","toolCallId":format!("tool-{index}"),"toolName":"synthetic","args":{}})).unwrap();
        assert!(!display(&observed).is_empty());
    }
    assert_eq!(session.tools.len(), MAX_PENDING);
    assert_eq!(session.state(), &SessionState::Running);
    session
        .observe(json!({"type":"agent_settled","aborted":false}))
        .unwrap();
    assert!(session.tools.is_empty());
    assert_eq!(session.state(), &SessionState::Ready);
}

#[test]
fn oversized_confirmation_requires_local_ui_instead_of_approving_hidden_details() {
    let mut session = running();
    let observed = session.observe(json!({"type":"extension_ui_request","id":"dialog","method":"confirm","title":"x".repeat(MAX_MESSAGE_BYTES + 1)})).unwrap();
    assert_eq!(session.state(), &SessionState::WaitingForLocalUi);
    assert!(observed.contains(&Observation::LocalUiRequired));
    assert!(
        !observed
            .iter()
            .any(|value| matches!(value, Observation::Approval(_)))
    );
    assert_eq!(
        session
            .approval_response("dialog", ApprovalDecision::AllowOnce)
            .unwrap_err(),
        Error("PI_APPROVAL_NOT_FOUND")
    );
}
