//! Existing approval wire shapes; runtime dispatch tests live in cafe-spaced.
use cafe_space_protocol::*;
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_slice(include_bytes!(
        "../../../fixtures/pi-rpc/approvals-wire-v5.json"
    ))
    .unwrap()
}

#[test]
fn approvals_round_trip_existing_v5_shapes_without_extending_control_or_tool_policy() {
    let fixture = fixture();
    assert_eq!(fixture["status"], "local_wire_v8");
    assert_eq!(fixture["fullProtocolVersion"], PROTOCOL_VERSION);
    assert_eq!(fixture["toolApproval"], "never_asks");
    assert_eq!(fixture["allowForSession"], false);
    assert_eq!(fixture["unknownNeverReplay"], true);
    for request in fixture["requests"].as_array().unwrap() {
        let decoded = decode_client_message(&serde_json::to_vec(request).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), *request);
        let mut control = request.clone();
        control["protocolVersion"] = control::VERSION.into();
        assert!(
            control::decode_request(&serde_json::to_vec(&control).unwrap())
                .unwrap()
                .1
                .is_none()
        );
    }
    for frame in fixture["responses"]
        .as_array()
        .unwrap()
        .iter()
        .chain(fixture["events"].as_array().unwrap())
    {
        let decoded = decode_server_message(&serde_json::to_vec(frame).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), *frame);
    }
    assert_eq!(
        fixture["events"][0]["event"]["data"]["approval"]["id"],
        fixture["events"][1]["event"]["data"]["approvalId"]
    );
    assert_eq!(
        fixture["events"][1]["event"]["data"]["decision"],
        fixture["requests"][0]["request"]["params"]["decision"]
    );
}

#[test]
fn approval_selection_requires_fences_public_id_and_an_explicit_decision() {
    let fixture = fixture();
    let request = &fixture["requests"][0];
    for field in [
        "sessionId",
        "expectedStreamId",
        "expectedCwdFence",
        "approvalId",
        "decision",
    ] {
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
    let mut bad = request.clone();
    bad["request"]["params"]["nativeDialogId"] = "must-not-forward".into();
    assert!(decode_client_message(&serde_json::to_vec(&bad).unwrap()).is_err());
    let mut bad = request.clone();
    bad["request"]["params"]["decision"] = "allow_always".into();
    assert!(decode_client_message(&serde_json::to_vec(&bad).unwrap()).is_err());
}
