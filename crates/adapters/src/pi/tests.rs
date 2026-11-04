use super::*;
use std::io::{BufReader, Cursor, Write};

fn startup() -> Session {
    let mut session = Session::new("native-fixture-1".into());
    let command = session.command(Command::GetState).unwrap();
    session.observe(json!({"type":"response","id":command.id,"command":"get_state","success":true,"data":{"sessionId":"native-fixture-1","sessionFile":"/private/do-not-forward.jsonl","isStreaming":false,"isCompacting":false,"pendingMessageCount":0}})).unwrap();
    session
}

#[test]
fn shared_traces_match_observed_states_and_authoritative_messages() {
    let traces: Value =
        serde_json::from_str(include_str!("../../../../fixtures/pi-rpc/traces.json")).unwrap();
    for trace in traces.as_array().unwrap() {
        let mut session = Session::new("native-fixture-1".into());
        let mut messages = Vec::new();
        for step in trace["steps"].as_array().unwrap() {
            if let Some(send) = step.get("send") {
                let outbound = session
                    .command(serde_json::from_value(send.clone()).unwrap())
                    .unwrap();
                let decoded = framing::Records::new(Cursor::new(outbound.bytes))
                    .next_record()
                    .unwrap()
                    .unwrap();
                assert_eq!(decoded["type"], send["type"]);
            } else {
                let observations = session.observe(step["receive"].clone()).unwrap();
                assert_eq!(
                    serde_json::to_value(session.state()).unwrap()["state"],
                    step["state"],
                    "{}",
                    trace["name"]
                );
                let rendered = format!("{observations:?}");
                assert!(
                    !rendered.contains("never-forward")
                        && !rendered.contains("native-fixture")
                        && !rendered.contains("private-dialog")
                );
                messages.extend(observations.into_iter().filter_map(
                    |observation| match observation {
                        Observation::Message(message) => Some(message),
                        _ => None,
                    },
                ));
            }
        }
        if trace["name"] == "first_turn_and_retry" {
            let complete = messages.last().unwrap();
            assert_eq!(
                complete.parts,
                vec![MessagePart::Text(TextPart {
                    text: "Final 茶".into()
                })]
            );
            assert_eq!(complete.status, TranscriptStatus::Complete);
            assert!(messages.iter().any(|message| matches!(&message.parts[..], [MessagePart::Text(TextPart {text})] if text.contains('\u{2028}') && text.contains('\u{2029}'))));
            assert!(messages.iter().all(|message| message.id == complete.id));
        }
        if trace["name"] == "tool_execution" {
            assert_eq!(messages.len(), 3);
            assert!(messages.iter().all(|message| message.id == messages[0].id));
            assert!(
                matches!(&messages.last().unwrap().parts[..], [MessagePart::ToolCall(ToolCallPart {output:Some(text),status:ToolStatus::Complete,..})] if text == "complete")
            );
        }
    }
}

#[test]
fn lf_framing_survives_each_byte_split_utf8_crlf_and_unicode_separators() {
    let record = json!({"type":"prompt","message":"Tea 茶\nquoted\"\u{2028}a\u{2029}b"});
    let mut bytes = framing::encode(&record).unwrap();
    bytes.insert(bytes.len() - 1, b'\r');
    bytes.extend(framing::encode(&json!({"type":"agent_settled","aborted":false})).unwrap());
    for capacity in 1..=bytes.len() {
        let mut records =
            framing::Records::new(BufReader::with_capacity(capacity, Cursor::new(&bytes)));
        assert_eq!(records.next_record().unwrap(), Some(record.clone()));
        assert_eq!(
            records.next_record().unwrap().unwrap()["type"],
            "agent_settled"
        );
        assert_eq!(records.next_record().unwrap(), None);
    }
}

#[test]
fn malformed_large_incomplete_and_nonobject_records_fail_closed_without_echoing_bytes() {
    for (bytes, code) in [
        (b"{bad-secret}\n".to_vec(), "PI_RECORD_INVALID"),
        (b"[]\n".to_vec(), "PI_RECORD_INVALID"),
        (b"\n".to_vec(), "PI_RECORD_INVALID"),
        (b"{}".to_vec(), "PI_RECORD_INCOMPLETE"),
        (
            vec![b'x'; framing::MAX_RECORD_BYTES + 1],
            "PI_RECORD_TOO_LARGE",
        ),
        (
            vec![b'{', b'"', b'a', b'"', b':', b'"', 255, b'"', b'}', b'\n'],
            "PI_RECORD_INVALID",
        ),
    ] {
        let mut reader = framing::Records::new(Cursor::new(bytes));
        assert_eq!(reader.next_record().unwrap_err(), Error(code));
        assert_eq!(
            reader.next_record().unwrap_err(),
            Error("PI_CONNECTION_CLOSED")
        );
    }
}

#[test]
fn command_correlation_rejection_late_responses_deadlines_and_disconnect_are_honest() {
    let mut session = startup();
    let prompt = session
        .command(Command::Prompt {
            message: "synthetic".into(),
        })
        .unwrap();
    let outcomes = session.observe(json!({"type":"response","id":prompt.id,"command":"prompt","success":false,"error":"token=never-forward /private/path"})).unwrap();
    assert_eq!(
        outcomes,
        vec![Observation::Outcome {
            id: prompt.id.clone(),
            outcome: Outcome::Rejected("PI_COMMAND_REJECTED")
        }]
    );
    let pending = session
        .command(Command::Prompt {
            message: "synthetic".into(),
        })
        .unwrap();
    assert_eq!(
        session.expire(&pending.id),
        Some(Observation::Outcome {
            id: pending.id.clone(),
            outcome: Outcome::Unknown("PI_RESPONSE_TIMEOUT")
        })
    );
    assert_eq!(session.observe(json!({"type":"response","id":pending.id,"command":"prompt","success":true,"data":{"disposition":"started"}})).unwrap(),vec![Observation::Notice("PI_LATE_RESPONSE")]);
    assert_eq!(session.state(), &SessionState::Ready);
    let pending = session
        .command(Command::Prompt {
            message: "synthetic".into(),
        })
        .unwrap();
    let observations = session.disconnected("PI_CONNECTION_CLOSED");
    assert!(observations.contains(&Observation::Outcome {
        id: pending.id,
        outcome: Outcome::Unknown("PI_COMMAND_OUTCOME_UNKNOWN")
    }));
    assert!(matches!(session.state(), SessionState::Failed(_)));
    assert!(session.command(Command::GetState).is_err());
}

#[test]
fn responses_cannot_complete_a_run_or_validate_the_wrong_native_session() {
    let mut session = startup();
    session.observe(json!({"type":"agent_start"})).unwrap();
    let query = session.command(Command::GetState).unwrap();
    session.observe(json!({"type":"response","id":query.id,"command":"get_state","success":true,"data":{"sessionId":"native-fixture-1","isStreaming":false,"isCompacting":false,"pendingMessageCount":0}})).unwrap();
    assert_eq!(session.state(), &SessionState::Running);
    let mut session = Session::new("native-fixture-1".into());
    assert_eq!(
        session
            .command(Command::Prompt {
                message: "not-yet".into()
            })
            .err()
            .unwrap(),
        Error("PI_NOT_READY")
    );
    let query = session.command(Command::GetState).unwrap();
    assert_eq!(session.observe(json!({"type":"response","id":query.id,"command":"get_state","success":true,"data":{"sessionId":"wrong-private-id","isStreaming":false,"isCompacting":false,"pendingMessageCount":0}})).unwrap_err(), Error("PI_SESSION_IDENTITY_MISMATCH"));
}

#[test]
fn confirmations_are_explicit_and_dispatch_is_not_completion_or_session_permission() {
    let mut session = startup();
    let observations = session.observe(json!({"type":"extension_ui_request","id":"private-ui-id","method":"confirm","title":"Synthetic approval"})).unwrap();
    let Observation::Approval(approval) = &observations[0] else {
        panic!("approval required")
    };
    assert_ne!(approval.id, "private-ui-id");
    assert!(!capabilities().approval_for_session);
    assert_eq!(
        session
            .approval_response(&approval.id, ApprovalDecision::AllowForSession)
            .unwrap_err(),
        Error("PI_APPROVAL_SCOPE_UNSUPPORTED")
    );
    for decision in [ApprovalDecision::AllowOnce, ApprovalDecision::Deny] {
        let bytes = session
            .approval_response(&approval.id, decision.clone())
            .unwrap();
        let record: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            record,
            json!({"type":"extension_ui_response","id":"private-ui-id","confirmed":decision==ApprovalDecision::AllowOnce})
        );
    }
    assert_eq!(
        session.approval_written(&approval.id).unwrap(),
        Observation::ApprovalDispatched {
            id: approval.id.clone()
        }
    );
    assert!(
        session
            .approval_response(&approval.id, ApprovalDecision::AllowOnce)
            .is_err()
    );
    session.observe(json!({"type":"extension_ui_request","id":"private-choice","method":"select","title":"Choose","options":["Allow for session","Deny"]})).unwrap();
    assert_eq!(session.state(), &SessionState::WaitingForLocalUi);
    assert!(session.dialogs.is_empty());
}

#[test]
fn cancellation_requires_a_confirmed_clear_queue_and_new_delivery_invalidates_it() {
    let mut session = startup();
    session.observe(json!({"type":"agent_start"})).unwrap();
    assert_eq!(
        session.command(Command::Abort).err().unwrap(),
        Error("PI_QUEUE_NOT_CLEARED")
    );
    let clear = session.command(Command::ClearQueue).unwrap();
    assert!(session.command(Command::Abort).is_err());
    session.observe(json!({"type":"response","id":clear.id,"command":"clear_queue","success":true,"data":{"steering":[],"followUp":[]}})).unwrap();
    let steer = session
        .prompt("synthetic".into(), DeliveryMode::Steer)
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&steer.bytes).unwrap()["type"],
        "steer"
    );
    assert!(session.command(Command::Abort).is_err());
    let clear = session.command(Command::ClearQueue).unwrap();
    session
        .observe(json!({"type":"response","id":clear.id,"command":"clear_queue","success":true}))
        .unwrap();
    session.command(Command::Abort).unwrap();
    assert_eq!(session.state(), &SessionState::Running);
}

#[test]
fn pending_commands_dialogs_and_message_parts_have_explicit_bounds() {
    let mut session = startup();
    for _ in 0..MAX_PENDING {
        session.command(Command::GetState).unwrap();
    }
    assert_eq!(
        session.command(Command::GetState).err().unwrap(),
        Error("PI_COMMAND_LIMIT")
    );
    let mut session = startup();
    for id in 0..MAX_PENDING {
        session.observe(json!({"type":"extension_ui_request","id":format!("native-{id}"),"method":"confirm","title":"Synthetic"})).unwrap();
    }
    assert_eq!(session.observe(json!({"type":"extension_ui_request","id":"over-limit","method":"confirm","title":"Synthetic"})).unwrap_err(),Error("PI_DIALOG_LIMIT"));
    let mut session = startup();
    session
        .observe(json!({"type":"message_start","message":{"role":"assistant","content":[]}}))
        .unwrap();
    assert_eq!(session.observe(json!({"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":MAX_PARTS}})).unwrap_err(),Error("PI_MESSAGE_TOO_LARGE"));
    let mut session = startup();
    assert_eq!(session.observe(json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"x".repeat(MAX_MESSAGE_BYTES)}]}})).unwrap_err(),Error("PI_MESSAGE_TOO_LARGE"));
}

#[test]
fn partial_write_is_unknown_and_is_never_automatically_replayed() {
    struct Broken {
        count: usize,
    }
    impl Write for Broken {
        fn write(&mut self, _bytes: &[u8]) -> std::io::Result<usize> {
            self.count += 1;
            if self.count == 1 {
                Ok(1)
            } else {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = Broken { count: 0 };
    assert_eq!(
        framing::write_record(&mut output, b"{}\n").unwrap_err(),
        Error("PI_WRITE_OUTCOME_UNKNOWN")
    );
    assert_eq!(output.count, 2);
}

#[cfg(windows)]
#[test]
fn node_cli_arguments_use_regular_absolute_paths_after_canonical_validation() {
    use std::path::Path;
    for (verbatim, regular) in [
        (r"\\?\C:\synthetic 茶\cli.mjs", r"C:\synthetic 茶\cli.mjs"),
        (r"\\?\UNC\server\share\cli.mjs", r"\\server\share\cli.mjs"),
    ] {
        assert_eq!(install::node_path(Path::new(verbatim)), Path::new(regular));
    }
}

#[test]
fn malformed_correlated_responses_never_lose_the_unknown_outcome_or_failure_observation() {
    for record in [
        json!({"type":"response","id":"rpc-2","command":"abort","success":true}),
        json!({"type":"response","id":"rpc-2","command":"prompt","success":true,"data":{"disposition":"made-up"}}),
    ] {
        let mut session = startup();
        let sent = session
            .command(Command::Prompt {
                message: "synthetic".into(),
            })
            .unwrap();
        let error = session.observe(record).unwrap_err();
        let outcomes = session.disconnected(error.0);
        assert!(
            outcomes.contains(&Observation::State(SessionState::Failed(FailureState {
                code: error.0.into()
            })))
        );
        assert!(outcomes.contains(&Observation::Outcome {
            id: sent.id,
            outcome: Outcome::Unknown("PI_COMMAND_OUTCOME_UNKNOWN")
        }));
    }
}

#[test]
fn package_pin_launch_flags_and_resume_preflight_are_passive_and_identity_checked() {
    let contract: Value =
        serde_json::from_str(include_str!("../../../../fixtures/pi-rpc/contract.json")).unwrap();
    assert_eq!(contract["packageVersion"], install::VERSION);
    assert_eq!(
        contract["framing"]["maxRecordBytes"],
        framing::MAX_RECORD_BYTES
    );
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("package");
    std::fs::create_dir_all(root.join("dist/bundle")).unwrap();
    let node = temp.path().join("node-not-executed");
    std::fs::write(&node, b"not executable; passive detection only").unwrap();
    let metadata =
        json!({"name":install::PACKAGE,"version":install::VERSION,"bin":{"pi":install::CLI}});
    std::fs::write(
        root.join("package.json"),
        serde_json::to_vec(&metadata).unwrap(),
    )
    .unwrap();
    std::fs::write(
        root.join(install::CLI),
        "throw new Error('must never execute')",
    )
    .unwrap();
    let installation = install::Installation::inspect(&node, &root).unwrap();
    let sessions = temp.path().join("sessions");
    std::fs::create_dir(&sessions).unwrap();
    let args = installation
        .arguments("native-fixture-1", &sessions, Some("Synthetic 茶"), None)
        .unwrap();
    assert_eq!(
        args[1..7]
            .iter()
            .map(|arg| arg.to_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "--mode",
            "rpc",
            "--offline",
            "--no-extensions",
            "--no-approve",
            "--no-themes"
        ]
    );
    assert!(!args.iter().any(|arg| arg == "--continue" || arg == "-e"));
    let file = sessions.join("timestamp_native-fixture-1.jsonl");
    assert!(
        installation
            .arguments(
                "native-fixture-1",
                &sessions,
                None,
                Some((&file, temp.path()))
            )
            .is_err()
    );
    let header = json!({"type":"session","version":3,"id":"native-fixture-1","cwd":temp.path()});
    std::fs::write(&file, framing::encode(&header).unwrap()).unwrap();
    let resumed = installation
        .arguments(
            "native-fixture-1",
            &sessions,
            None,
            Some((&file, temp.path())),
        )
        .unwrap();
    assert!(!resumed.iter().any(|arg| arg == "--name"));
    assert!(
        installation
            .arguments("wrong", &sessions, None, Some((&file, temp.path())))
            .is_err()
    );
    assert!(
        installation
            .arguments("../escape", &sessions, None, None)
            .is_err()
    );
    let mut wrong = metadata;
    wrong["version"] = "9.9.9".into();
    std::fs::write(
        root.join("package.json"),
        serde_json::to_vec(&wrong).unwrap(),
    )
    .unwrap();
    assert_eq!(
        install::Installation::inspect(&node, &root).err().unwrap(),
        Error("PI_VERSION_UNSUPPORTED")
    );
}
