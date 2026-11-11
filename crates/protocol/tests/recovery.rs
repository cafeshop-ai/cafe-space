use cafe_space_protocol::*;
use serde_json::{Value, json};

fn fixture() -> Value {
    serde_json::from_slice(include_bytes!("../../../fixtures/v2/recovery.json")).unwrap()
}

#[test]
fn recovery_fixtures_round_trip_only_on_full_wire_eight() {
    let fixture = fixture();
    assert_eq!(PROTOCOL_VERSION, 8);
    assert_eq!(fixture["fullProtocolVersion"], PROTOCOL_VERSION);
    assert_eq!(fixture["controlVersion"], control::VERSION);
    for frame in fixture["requests"].as_array().unwrap() {
        let bytes = serde_json::to_vec(frame).unwrap();
        let decoded = decode_client_message(&bytes).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&encode_client_message(&decoded).unwrap()).unwrap(),
            *frame
        );
        let mut old = frame.clone();
        old["protocolVersion"] = 7.into();
        assert!(decode_client_message(&serde_json::to_vec(&old).unwrap()).is_err());
        old["protocolVersion"] = control::VERSION.into();
        let (_, request) = control::decode_request(&serde_json::to_vec(&old).unwrap()).unwrap();
        assert_eq!(
            request.is_some(),
            frame["request"]["method"] == "get_status"
        );
    }
    for frame in fixture["responses"].as_array().unwrap() {
        let decoded = decode_server_message(&serde_json::to_vec(frame).unwrap()).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&encode_server_message(&decoded).unwrap()).unwrap(),
            *frame
        );
        assert!(!format!("{decoded:?}").contains("synthetic-private"));
    }
}

#[test]
fn recovery_health_is_path_free_not_ready_and_not_update_confirmation() {
    let base = fixture()["responses"][0].clone();
    for (field, value) in [
        ("fromSchema", json!(0)),
        ("fromSchema", json!(7)),
        ("toSchema", json!(4)),
        ("code", json!("UNRECOGNIZED")),
        ("backup", json!("yes")),
        ("path", json!("/private/state.sqlite3")),
    ] {
        let mut invalid = base.clone();
        invalid["result"]["data"]["result"]["data"]["recovery"][field] = value;
        assert!(
            decode_server_message(&serde_json::to_vec(&invalid).unwrap()).is_err(),
            "{field}"
        );
    }
    for field in ["ready", "awaitingUpdateConfirmation"] {
        let mut invalid = base.clone();
        invalid["result"]["data"]["result"]["data"][field] = true.into();
        assert!(decode_server_message(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
    let mut control_health = base;
    control_health["protocolVersion"] = control::VERSION.into();
    assert!(control::decode_server_message(&serde_json::to_vec(&control_health).unwrap()).is_err());
    control_health["result"]["data"]["result"]["data"]["recovery"] = Value::Null;
    assert!(control::decode_server_message(&serde_json::to_vec(&control_health).unwrap()).is_err());
    control_health["result"]["data"]["result"]["data"]
        .as_object_mut()
        .unwrap()
        .remove("recovery");
    assert!(control::decode_server_message(&serde_json::to_vec(&control_health).unwrap()).is_ok());
}

#[test]
fn restore_requires_explicit_fields_and_cannot_supply_files_or_authority() {
    let base = fixture()["requests"][2].clone();
    for field in ["confirm", "code"] {
        let mut invalid = base.clone();
        invalid["request"]["params"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(decode_client_message(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
    for field in ["path", "backupFile", "access", "force"] {
        let mut invalid = base.clone();
        invalid["request"]["params"][field] = json!("caller-controlled");
        assert!(decode_client_message(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
    let mut invalid = base;
    invalid["request"]["params"]["code"] = json!("UNRECOGNIZED");
    assert!(decode_client_message(&serde_json::to_vec(&invalid).unwrap()).is_err());
}

#[test]
fn local_details_have_bounded_paths_and_nullable_backup() {
    let base = fixture()["responses"][2].clone();
    for field in ["dataDirectory", "backupFile"] {
        for value in ["".into(), "bad\0path".into(), "x".repeat(65537)] {
            let mut invalid = base.clone();
            invalid["result"]["data"]["result"]["data"][field] = json!(value);
            assert!(decode_server_message(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
    }
    let mut invalid = base.clone();
    invalid["result"]["data"]["result"]["data"]["stateFiles"] = json!(["bad\npath"]);
    assert!(decode_server_message(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid["result"]["data"]["result"]["data"]["stateFiles"] = json!(vec!["/state"; 1001]);
    assert!(decode_server_message(&serde_json::to_vec(&invalid).unwrap()).is_err());
}
