use cafe_space_protocol::*;
use serde_json::{Value, json};

fn fixture() -> Value {
    serde_json::from_slice(include_bytes!(
        "../../../fixtures/pi-rpc/runtime-registration.json"
    ))
    .unwrap()
}

#[test]
fn registration_requires_current_wire_and_never_extends_frozen_control() {
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
        assert!(
            !serde_json::to_string(&decoded)
                .unwrap()
                .contains("synthetic-private")
        );
    }
}

#[test]
fn registration_paths_are_required_bounded_and_cannot_supply_access_or_launch_options() {
    let base = fixture()["requests"][0].clone();
    for field in ["node", "packageRoot"] {
        let mut missing = base.clone();
        missing["request"]["params"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(decode_client_message(&serde_json::to_vec(&missing).unwrap()).is_err());
        for value in [
            Value::Null,
            json!(""),
            json!("has\0nul"),
            json!("has\nline"),
            json!("x".repeat(65537)),
        ] {
            let mut invalid = base.clone();
            invalid["request"]["params"][field] = value;
            assert!(decode_client_message(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
    }
    for field in ["access", "args", "env", "credentials", "autoInstall"] {
        let mut invalid = base.clone();
        invalid["request"]["params"][field] = json!("synthetic-private");
        assert!(decode_client_message(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
    let mut event = json!({"type":"event","protocolVersion":PROTOCOL_VERSION,"eventStreamId":"fixture","sequence":1,"event":{"kind":"pi_runtime_registered","data":{"node":"/synthetic-private/node"}}});
    assert!(decode_server_message(&serde_json::to_vec(&event).unwrap()).is_err());
    event["event"]["kind"] = "pi_runtime_changed".into();
    assert!(decode_server_message(&serde_json::to_vec(&event).unwrap()).is_err());
}
