//! Wire-shape checks only: owned-process dispatch tests live in cafe-spaced.
use cafe_space_protocol::*;
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_slice(include_bytes!(
        "../../../fixtures/pi-rpc/commands-wire-v5.json"
    ))
    .unwrap()
}

#[test]
fn command_contract_round_trips_existing_v5_shapes_on_full_wire_nine() {
    let fixture = fixture();
    assert_eq!(fixture["status"], "local_wire_v9");
    assert_eq!(fixture["fullProtocolVersion"], PROTOCOL_VERSION);
    assert_eq!(PROTOCOL_VERSION, 9);
    assert_eq!(fixture["unknownNeverReplay"], true);
    assert_eq!(fixture["closedReceiverCode"], "PI_CONNECTION_CLOSED");
    assert_eq!(fixture["explicitCloseCode"], "PI_COMMAND_OUTCOME_UNKNOWN");
    let requests = fixture["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 3);
    for request in requests {
        let decoded = decode_client_message(&serde_json::to_vec(request).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), *request);
        let mut control = request.clone();
        control["protocolVersion"] = control::VERSION.into();
        let (_, method) = control::decode_request(&serde_json::to_vec(&control).unwrap()).unwrap();
        assert!(method.is_none());
    }
    let responses = fixture["responses"].as_array().unwrap();
    assert_eq!(responses.len(), 4);
    assert_eq!(
        responses
            .iter()
            .map(|r| r["result"]["status"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["dispatched", "rejected", "unknown", "applied"]
    );
    for response in responses {
        let decoded = decode_server_message(&serde_json::to_vec(response).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), *response);
        assert!(
            requests
                .iter()
                .any(|r| r["requestId"] == response["requestId"])
        );
    }
}

#[test]
fn command_shapes_require_fences_run_identity_and_valid_input() {
    let fixture = fixture();
    for request in fixture["requests"].as_array().unwrap() {
        let mut fields = vec!["sessionId", "expectedStreamId", "expectedCwdFence"];
        if request["request"]["method"] == "cancel" {
            fields.push("runId");
        }
        for field in fields {
            let mut missing = request.clone();
            missing["request"]["params"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(decode_client_message(&serde_json::to_vec(&missing).unwrap()).is_err());
            let mut empty = request.clone();
            empty["request"]["params"][field] = "".into();
            assert!(decode_client_message(&serde_json::to_vec(&empty).unwrap()).is_err());
        }
    }
    let prompt = &fixture["requests"][0];
    for content in ["".to_string(), "x".repeat(65_537)] {
        let mut invalid = prompt.clone();
        invalid["request"]["params"]["content"] = content.into();
        assert!(decode_client_message(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
    let mut invalid = prompt.clone();
    invalid["request"]["params"]["delivery"] = "queue".into();
    assert!(decode_client_message(&serde_json::to_vec(&invalid).unwrap()).is_err());
    let mut invalid = prompt.clone();
    invalid["request"]["params"]["nativeSessionId"] = "private-id".into();
    assert!(decode_client_message(&serde_json::to_vec(&invalid).unwrap()).is_err());
}
