use cafe_space_protocol::*;
use serde_json::{Value, json};

fn fixture() -> Value {
    serde_json::from_slice(include_bytes!(
        "../../../fixtures/v2/local-session-folder.json"
    ))
    .unwrap()
}

#[test]
fn local_folder_frames_are_v11_only_bounded_and_redacted() {
    let fixture = fixture();
    assert_eq!(PROTOCOL_VERSION, 11);
    assert_eq!(fixture["fullProtocolVersion"], PROTOCOL_VERSION);
    assert_eq!(fixture["controlVersion"], control::VERSION);
    assert_eq!(fixture["remoteMayResolve"], false);
    assert_eq!(fixture["resolverStartsAgent"], false);
    assert_eq!(fixture["resolverCalledOn"], "start");
    for frame in fixture["requests"].as_array().unwrap() {
        let bytes = serde_json::to_vec(frame).unwrap();
        let decoded = decode_client_message(&bytes).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&encode_client_message(&decoded).unwrap()).unwrap(),
            *frame
        );
        assert!(!format!("{decoded:?}").contains("synthetic-private"));
        for version in 1..PROTOCOL_VERSION {
            let mut old = frame.clone();
            old["protocolVersion"] = version.into();
            let bytes = serde_json::to_vec(&old).unwrap();
            assert!(decode_client_message(&bytes).is_err());
            if version == control::VERSION {
                assert!(control::decode_request(&bytes).unwrap().1.is_none());
            }
        }
    }
    for frame in fixture["responses"].as_array().unwrap() {
        let decoded = decode_server_message(&serde_json::to_vec(frame).unwrap()).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&encode_server_message(&decoded).unwrap()).unwrap(),
            *frame
        );
        assert!(!format!("{decoded:?}").contains("synthetic-private"));
        for version in 1..PROTOCOL_VERSION {
            let mut old = frame.clone();
            old["protocolVersion"] = version.into();
            assert!(decode_server_message(&serde_json::to_vec(&old).unwrap()).is_err());
        }
    }
}

#[test]
fn local_folder_requests_and_responses_reject_invalid_paths_and_extra_authority() {
    let fixture = fixture();
    let base = &fixture["requests"][0];
    for value in [
        json!(""),
        json!("bad\npath"),
        json!("bad\0path"),
        json!("x".repeat(65_537)),
        json!(17),
        json!([]),
    ] {
        let mut invalid = base.clone();
        invalid["request"]["params"]["root"] = value;
        assert!(decode_client_message(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
    for field in [
        "access",
        "projectId",
        "expectedCwdFence",
        "env",
        "args",
        "runtimeId",
    ] {
        let mut invalid = base.clone();
        invalid["request"]["params"][field] = "synthetic-private".into();
        assert!(decode_client_message(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
    let mut boundary = base.clone();
    boundary["request"]["params"]["root"] = "x"
        .repeat(fixture["maxRootBytes"].as_u64().unwrap() as usize)
        .into();
    assert!(decode_client_message(&serde_json::to_vec(&boundary).unwrap()).is_ok());
    let base = &fixture["responses"][0];
    for value in [
        Value::Null,
        json!(""),
        json!("bad\npath"),
        json!("bad\0path"),
        json!("x".repeat(65_537)),
    ] {
        let mut invalid = base.clone();
        invalid["result"]["data"]["result"]["data"]["root"] = value;
        assert!(decode_server_message(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
    for field in ["id", "name", "cwdFence"] {
        let mut invalid = base.clone();
        invalid["result"]["data"]["result"]["data"]["project"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(decode_server_message(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
    let mut event = json!({"type":"event", "protocolVersion":PROTOCOL_VERSION, "eventStreamId":"fixture", "sequence":1,
        "event":{"kind":"project_changed", "data":{"project":{"id":"fixture","name":"Fixture","cwdFence":"cwd"},"root":"/synthetic-private"}}});
    assert!(decode_server_message(&serde_json::to_vec(&event).unwrap()).is_err());
    event["event"]["kind"] = "local_session_folder".into();
    assert!(decode_server_message(&serde_json::to_vec(&event).unwrap()).is_err());
}
