use super::*;

fn fixture() -> Value {
    serde_json::from_str(include_str!("../../../../fixtures/pi-rpc/history.json")).unwrap()
}
fn query(session: &mut Session, messages: Value) -> Result<Vec<Observation>> {
    let query = session.command(Command::GetMessages)?;
    session.observe(json!({"type":"response", "id":query.id, "command":"get_messages", "success":true, "data":{"messages":messages}}))
}

#[test]
fn current_history_is_correlated_private_bounded_and_cannot_collide_with_live_ids() {
    let fixture = fixture();
    assert_eq!(fixture["limits"]["messages"], 128);
    assert_eq!(fixture["limits"]["encodedBytes"], 128 * 1024);
    let mut session = tests::startup();
    let observations = query(
        &mut session,
        fixture["response"]["data"]["messages"].clone(),
    )
    .unwrap();
    let Observation::HistoryLoaded {
        messages,
        truncated,
        user_preview,
    } = &observations[0]
    else {
        panic!("history expected")
    };
    assert_eq!(*truncated, fixture["truncated"].as_bool().unwrap());
    let preview = user_preview.as_ref().unwrap();
    assert_eq!(preview.message_id, messages[0].id);
    assert_eq!(preview.insertion_order, 1);
    assert_eq!(preview.text.as_deref(), Some("Remember 茶"));
    assert_eq!(
        serde_json::to_value(
            messages
                .iter()
                .map(|message| &message.role)
                .collect::<Vec<_>>()
        )
        .unwrap(),
        fixture["expectedRoles"]
    );
    let encoded = serde_json::to_string(messages).unwrap();
    assert!(!encoded.contains("synthetic-never-forward"));
    assert!(encoded.contains("Prior answer 茶") && encoded.contains("Prior tool result"));
    assert!(
        matches!(&messages[2].parts[0], MessagePart::ToolCall(call) if call.status == ToolStatus::Complete && call.arguments == "{\"synthetic\":true}")
    );
    let previous = messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    let live = session
        .observe(
            json!({"type":"message_end", "message":{"role":"assistant", "content":"Live answer"}}),
        )
        .unwrap();
    assert!(
        matches!(&live[0], Observation::Message(message, None) if !previous.contains(&message.id))
    );
    assert_eq!(session.state(), &SessionState::Ready);
}

#[test]
fn history_bounds_keep_the_newest_messages_without_claiming_completeness() {
    let mut session = tests::startup();
    let messages = (0..140)
        .map(|index| json!({"role":"user", "content":index.to_string()}))
        .collect::<Vec<_>>();
    let observed = query(&mut session, json!(messages)).unwrap();
    let Observation::HistoryLoaded {
        messages,
        truncated,
        user_preview,
    } = &observed[0]
    else {
        panic!("history expected")
    };
    assert!(*truncated);
    assert_eq!(messages.len(), 128);
    assert!(matches!(&messages[0].parts[0], MessagePart::Text(text) if text.text == "12"));
    assert_eq!(user_preview.as_ref().unwrap().text.as_deref(), Some("139"));
    let observed = query(
        &mut session,
        json!(vec![
            json!({"role":"user", "content":"茶".repeat(10000)});
            8
        ]),
    )
    .unwrap();
    let Observation::HistoryLoaded {
        messages,
        truncated,
        ..
    } = &observed[0]
    else {
        panic!("history expected")
    };
    assert!(*truncated);
    assert_eq!(messages.len(), 4);
    assert!(
        messages
            .iter()
            .map(|message| serde_json::to_vec(message).unwrap().len())
            .sum::<usize>()
            <= 131072
    );
    let observed = query(&mut session, json!([{"role":"assistant","stopReason":"pending","content":[{"type":"text","text":"unfinished"}]}])).unwrap();
    assert!(
        matches!(&observed[0], Observation::HistoryLoaded { messages, truncated:true, user_preview:None } if messages[0].status == TranscriptStatus::Error)
    );
    let observed = query(&mut session, json!([])).unwrap();
    assert!(
        matches!(&observed[0], Observation::HistoryLoaded { messages, truncated:false, user_preview:None } if messages.is_empty())
    );
}

#[test]
fn history_preview_survives_display_eviction_and_empty_latest_user_clears_it() {
    let mut entries = vec![json!({"role":"user", "content":"retained preview"})];
    entries.extend((0..129).map(|_| json!({"role":"assistant", "content":"reply"})));
    let mut session = tests::startup();
    let observed = query(&mut session, json!(entries)).unwrap();
    let Observation::HistoryLoaded {
        messages,
        user_preview,
        ..
    } = &observed[0]
    else {
        panic!("history expected")
    };
    assert_eq!(messages.len(), 128);
    assert_eq!(user_preview.as_ref().unwrap().insertion_order, 1);
    assert!(
        messages
            .iter()
            .all(|message| message.role == TranscriptRole::Assistant)
    );
    assert_eq!(
        user_preview.as_ref().unwrap().text.as_deref(),
        Some("retained preview")
    );
    assert!(
        !messages
            .iter()
            .any(|message| message.id == user_preview.as_ref().unwrap().message_id)
    );
    let live = session
        .observe(
            json!({"type":"message_end", "message":{"role":"user", "content":"after history"}}),
        )
        .unwrap();
    assert!(
        matches!(&live[0], Observation::Message(_, Some(preview)) if preview.insertion_order == 131)
    );
    let observed = query(
        &mut tests::startup(),
        json!([
            {"role":"user", "content":"old"},
            {"role":"user", "content":" \t\n "}
        ]),
    )
    .unwrap();
    assert!(matches!(
        &observed[0],
        Observation::HistoryLoaded {
            user_preview: Some(UserPreview {
                text: None,
                insertion_order: 2,
                ..
            }),
            ..
        }
    ));
}

#[test]
fn malformed_history_fails_closed_and_unsupported_roles_or_orphan_tools_are_explicit_gaps() {
    for invalid in [
        Value::Null,
        json!([{}]),
        json!([{"role":"assistant","content":null}]),
        json!(vec![json!({"role":"user","content":"x"}); 4097]),
    ] {
        assert!(query(&mut tests::startup(), invalid).is_err());
    }
    let observed = query(
        &mut tests::startup(),
        json!([
            {"role":"toolResult", "toolCallId":"missing", "content":[], "isError":false},
            {"role":"custom", "display":false, "content":"synthetic-never-forward"}
        ]),
    )
    .unwrap();
    assert!(
        matches!(&observed[0], Observation::HistoryLoaded { messages, truncated:true, user_preview:None } if messages.is_empty())
    );
    let mut session = tests::startup();
    let pending = session.command(Command::GetMessages).unwrap();
    session.expire(&pending.id);
    let observed = session.observe(json!({"type":"response","id":pending.id,"command":"get_messages","success":true,"data":{"messages":[{"role":"user","content":"must not appear"}]}})).unwrap();
    assert_eq!(observed, vec![Observation::Notice("PI_LATE_RESPONSE")]);
}
