//! Frozen wire-shape checks, not daemon dispatch or cross-version update acceptance tests.
use cafe_space_protocol::{
    PROTOCOL_VERSION, ProtocolError, decode_client_message, decode_server_message,
    encode_client_message, encode_server_message,
};
use serde_json::Value;

fn frames() -> Vec<Value> {
    let trace: Value =
        serde_json::from_slice(include_bytes!("../../../fixtures/control-v2/trace.json")).unwrap();
    trace["frames"].as_array().unwrap().clone()
}

fn round_trip(from: &str, frame: &Value) -> Result<Vec<u8>, ProtocolError> {
    let bytes = serde_json::to_vec(frame).unwrap();
    match from {
        "controller" => encode_client_message(&decode_client_message(&bytes)?),
        "daemon" => encode_server_message(&decode_server_message(&bytes)?),
        _ => panic!("unknown fixture direction: {from}"),
    }
}

#[test]
fn control_v2_frames_round_trip_and_requests_match_responses() {
    // A full-protocol bump must move these checks to the dedicated control codec, not edit the fixture.
    assert_eq!(PROTOCOL_VERSION, 2);
    let frames = frames();
    assert_eq!(frames.len(), 14);
    assert_eq!(frames[0]["frame"]["type"], "hello");
    assert_eq!(
        frames[0]["frame"]["supportedProtocolVersions"],
        serde_json::json!([2])
    );
    assert_eq!(frames[1]["frame"]["type"], "welcome");
    for (index, item) in frames.iter().enumerate() {
        let from = if index % 2 == 0 {
            "controller"
        } else {
            "daemon"
        };
        assert_eq!(item["from"], from, "frame {index}");
        let frame = &item["frame"];
        let encoded = round_trip(from, frame).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&encoded).unwrap(),
            *frame,
            "frame {index}"
        );
        if index > 0 {
            assert_eq!(frame["protocolVersion"], 2, "frame {index}");
        }
        if [10, 11].contains(&index) {
            assert_eq!(item["appliesWhen"], "daemon PROTOCOL_VERSION > 2");
        } else {
            assert!(item.get("appliesWhen").is_none(), "frame {index}");
        }
    }
    for pair in frames[2..].chunks_exact(2) {
        let request = &pair[0]["frame"];
        let response = &pair[1]["frame"];
        assert_eq!(request["type"], "request");
        assert_eq!(response["type"], "response");
        assert!(request["requestId"].is_string());
        assert_eq!(request["requestId"], response["requestId"]);
    }
}

#[test]
fn control_v2_frames_reject_unknown_fields_and_other_versions() {
    for (index, item) in frames().iter().enumerate() {
        let from = item["from"].as_str().unwrap();
        let frame = &item["frame"];
        let mut invalid = frame.clone();
        invalid["unknownField"] = Value::Bool(true);
        assert_eq!(
            round_trip(from, &invalid),
            Err(ProtocolError::InvalidFrame),
            "frame {index}"
        );
        if frame.get("protocolVersion").is_some() {
            for version in [1, 3] {
                let mut invalid = frame.clone();
                invalid["protocolVersion"] = Value::from(version);
                assert_eq!(
                    round_trip(from, &invalid),
                    Err(ProtocolError::UnsupportedVersion(version as u16)),
                    "frame {index}, version {version}"
                );
            }
        }
    }
}
