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
        if let Observation::Message(message) = observation {
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
        matches!(&observed[0],Observation::Message(message) if message.status == TranscriptStatus::Complete)
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
        matches!(&end[0],Observation::Message(message) if message.status==TranscriptStatus::Complete)
    );
    assert_eq!(session.state(), &SessionState::Running);
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
    assert_eq!(gap.iter().filter(|value| matches!(value,Observation::Message(message) if message.status==TranscriptStatus::Error)).count(), 2);
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
