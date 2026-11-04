//! Frozen wire-shape checks, not daemon dispatch or cross-version update acceptance tests.
use cafe_space_protocol::{ClientMessage, ProtocolError, control};
use serde_json::Value;

fn frames() -> Vec<Value> {
    let trace: Value =
        serde_json::from_slice(include_bytes!("../../../fixtures/control-v2/trace.json")).unwrap();
    trace["frames"].as_array().unwrap().clone()
}

fn round_trip(from: &str, frame: &Value) -> Result<Vec<u8>, ProtocolError> {
    let bytes = serde_json::to_vec(frame).unwrap();
    match from {
        "controller" => control::encode_client_message(&control::decode_client_message(&bytes)?),
        "daemon" => control::encode_server_message(&control::decode_server_message(&bytes)?),
        _ => panic!("unknown fixture direction: {from}"),
    }
}

#[test]
fn control_v2_frames_round_trip_and_requests_match_responses() {
    assert_eq!(control::VERSION, 2);
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
        let encoded = if index == 10 {
            // This is a forbidden request's shape, not evidence that a daemon rejects it before dispatch.
            let request: ClientMessage = serde_json::from_value(frame.clone()).unwrap();
            assert_eq!(round_trip(from, frame), Err(ProtocolError::InvalidMessage));
            assert!(control::encode_client_message(&request).is_err());
            serde_json::to_vec(&request).unwrap()
        } else {
            round_trip(from, frame).unwrap()
        };
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
fn control_v2_codec_never_accepts_session_data_events_or_dispatched_results() {
    for bytes in [
        include_bytes!("../../../fixtures/v2/sessions-response.json").as_slice(),
        include_bytes!("../../../fixtures/v2/server-event.json").as_slice(),
        include_bytes!("../../../fixtures/v2/close-session-dispatched.json").as_slice(),
    ] {
        // These are valid full-API frames, but must never enter the update-control channel.
        assert!(cafe_space_protocol::decode_server_message(bytes).is_ok());
        assert!(control::decode_server_message(bytes).is_err());
    }
    let frames = frames();
    let mut invalid = frames[4]["frame"].clone();
    invalid["request"]["params"]["targetHash"] = "not-a-hash".into();
    assert_eq!(
        round_trip("controller", &invalid),
        Err(ProtocolError::InvalidMessage)
    );
    for outcome in ["rejected", "unknown"] {
        let mut response = frames[11]["frame"].clone();
        response["result"]["status"] = outcome.into();
        assert!(round_trip("daemon", &response).is_ok());
    }
}

#[test]
fn control_v2_request_gate_rejects_other_methods_before_payload_decoding() {
    let frames = frames();
    for (index, item) in frames.iter().enumerate().skip(2).step_by(2) {
        let (id, request) =
            control::decode_request(&serde_json::to_vec(&item["frame"]).unwrap()).unwrap();
        assert_eq!(id, item["frame"]["requestId"].as_str().unwrap());
        assert_eq!(request.is_none(), index == 10);
    }
    for method in ["add_project", "subscribe_events", "future_method"] {
        let mut frame = frames[10]["frame"].clone();
        frame["request"] = serde_json::json!({"method": method, "params": {"unrecognized": true}});
        assert_eq!(
            control::decode_request(&serde_json::to_vec(&frame).unwrap())
                .unwrap()
                .1,
            None
        );
        frame["extraEnvelopeField"] = true.into();
        assert_eq!(
            control::decode_request(&serde_json::to_vec(&frame).unwrap()),
            Err(ProtocolError::InvalidFrame)
        );
    }
    let mut frame = frames[4]["frame"].clone();
    frame["request"]["params"]["targetHash"] = "bad-hash".into();
    assert!(control::decode_request(&serde_json::to_vec(&frame).unwrap()).is_err());
    frame["protocolVersion"] = 3.into();
    assert_eq!(
        control::decode_request(&serde_json::to_vec(&frame).unwrap()),
        Err(ProtocolError::UnsupportedVersion(3))
    );
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
