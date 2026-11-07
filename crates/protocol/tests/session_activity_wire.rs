use cafe_space_protocol::*;
use serde_json::{Value, json};

fn fixture() -> Value {
    serde_json::from_slice(include_bytes!(
        "../../../fixtures/session-activity/wire-v6.json"
    ))
    .unwrap()
}

#[test]
fn activity_metadata_requires_current_full_wire_and_never_extends_frozen_control() {
    let fixture = fixture();
    assert_eq!(fixture["fullProtocolVersion"], PROTOCOL_VERSION);
    assert_eq!(PROTOCOL_VERSION, 7);
    assert_eq!(fixture["controlVersion"], control::VERSION);
    let event = fixture["event"].clone();
    let decoded = decode_server_message(&serde_json::to_vec(&event).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(&decoded).unwrap(), event);
    assert_eq!(
        serde_json::from_slice::<Value>(&encode_server_message(&decoded).unwrap()).unwrap(),
        event
    );
    for version in [2, 3, 4, 5, 6] {
        let mut old = event.clone();
        old["protocolVersion"] = version.into();
        assert!(decode_server_message(&serde_json::to_vec(&old).unwrap()).is_err());
        if version == control::VERSION {
            assert!(control::decode_server_message(&serde_json::to_vec(&old).unwrap()).is_err());
        }
    }
}

#[test]
fn metadata_bounds_unknown_fields_and_null_canonicalization_are_strict() {
    let fixture = fixture();
    let base = &fixture["event"];
    for (cases, valid) in [("validMetadata", true), ("invalidMetadata", false)] {
        for metadata in fixture[cases].as_array().unwrap() {
            let mut frame = base.clone();
            let session = frame["event"]["data"]["session"].as_object_mut().unwrap();
            session.remove("lastActivityAt");
            session.remove("preview");
            session.extend(metadata.as_object().unwrap().clone());
            let decoded = decode_server_message(&serde_json::to_vec(&frame).unwrap());
            assert_eq!(decoded.is_ok(), valid, "{metadata}");
            if let Ok(decoded) = decoded {
                assert_eq!(serde_json::to_value(&decoded).unwrap(), frame);
                encode_server_message(&decoded).unwrap();
            }
        }
    }
    let mut nullable = base.clone();
    nullable["event"]["data"]["session"]["lastActivityAt"] = Value::Null;
    nullable["event"]["data"]["session"]["preview"] = Value::Null;
    let decoded = decode_server_message(&serde_json::to_vec(&nullable).unwrap()).unwrap();
    let canonical = serde_json::to_value(decoded).unwrap();
    for field in ["lastActivityAt", "preview"] {
        assert!(canonical["event"]["data"]["session"].get(field).is_none());
    }
    let mut oversized = decode_server_message(&serde_json::to_vec(base).unwrap()).unwrap();
    let ServerMessage::Event(ServerEvent {
        event: Event::SessionChanged(data),
        ..
    }) = &mut oversized
    else {
        panic!("session event")
    };
    data.session.preview = Some("x".repeat(257));
    assert!(encode_server_message(&oversized).is_err());
}

#[test]
fn every_public_snapshot_surface_enforces_the_same_metadata_rules() {
    let fixture = fixture();
    for (cases, valid) in [("validMetadata", true), ("invalidMetadata", false)] {
        for metadata in fixture[cases].as_array().unwrap() {
            let mut session = fixture["event"]["event"]["data"]["session"].clone();
            session.as_object_mut().unwrap().remove("lastActivityAt");
            session.as_object_mut().unwrap().remove("preview");
            session
                .as_object_mut()
                .unwrap()
                .extend(metadata.as_object().unwrap().clone());
            for result in [
                json!({"kind":"sessions","data":{"items":[session.clone()],"eventStreamId":"fixture","sequence":1}}),
                json!({"kind":"session","data":{"session":{"session":session.clone(),"messages":[],"historyTruncated":false,"approvals":[]},"eventStreamId":"fixture","sequence":1}}),
                json!({"kind":"subscription","data":{"eventStreamId":"fixture","sequence":1,"mode":"snapshot_required","snapshot":{"update":"none","projects":[],"runtimes":[],"sessions":[session.clone()]}}}),
            ] {
                let frame = json!({"type":"response","protocolVersion":PROTOCOL_VERSION,"requestId":"fixture","result":{"status":"applied","data":{"result":result}}});
                let decoded = decode_server_message(&serde_json::to_vec(&frame).unwrap());
                assert_eq!(decoded.is_ok(), valid, "{frame}");
                if let Ok(decoded) = decoded {
                    let encoded = encode_server_message(&decoded).unwrap();
                    assert_eq!(serde_json::from_slice::<Value>(&encoded).unwrap(), frame);
                }
            }
        }
    }
}
