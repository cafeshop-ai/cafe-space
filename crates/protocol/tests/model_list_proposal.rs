//! The fixture-first model-list contract is full API only; frozen control stays unchanged.
use cafe_space_protocol::*;
use serde_json::{Value, json};

fn fixture() -> Value {
    serde_json::from_slice(include_bytes!(
        "../../../fixtures/pi-rpc/model-list-wire-v5.json"
    ))
    .unwrap()
}

#[test]
fn model_list_round_trips_only_in_full_v5_without_extending_control() {
    let fixture = fixture();
    assert_eq!(fixture["status"], "full_wire_v5");
    assert_eq!(fixture["contract"]["fullProtocolVersion"], PROTOCOL_VERSION);
    assert_eq!(PROTOCOL_VERSION, 5);
    assert_eq!(fixture["contract"]["previousFullProtocolVersion"], 4);
    assert_eq!(fixture["contract"]["controlVersion"], control::VERSION);
    assert_eq!(fixture["contract"]["maxModels"], MAX_MODELS);
    assert_eq!(fixture["contract"]["automaticRetry"], false);
    assert_eq!(fixture["contract"]["productionRuntimeEnabled"], false);
    for request in fixture["requests"].as_array().unwrap() {
        let decoded = decode_client_message(&serde_json::to_vec(request).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), *request);
        let mut old = request.clone();
        old["protocolVersion"] = 4.into();
        assert!(decode_client_message(&serde_json::to_vec(&old).unwrap()).is_err());
        old["protocolVersion"] = control::VERSION.into();
        let (_, method) = control::decode_request(&serde_json::to_vec(&old).unwrap()).unwrap();
        assert!(method.is_none());
    }
    for response in fixture["responses"].as_array().unwrap() {
        let decoded = decode_server_message(&serde_json::to_vec(response).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), *response);
        let mut old = response.clone();
        old["protocolVersion"] = 4.into();
        assert!(decode_server_message(&serde_json::to_vec(&old).unwrap()).is_err());
        if response["result"]["status"] == "applied" {
            old["protocolVersion"] = control::VERSION.into();
            assert!(control::decode_server_message(&serde_json::to_vec(&old).unwrap()).is_err());
        }
    }
}

#[test]
fn model_lists_reject_invalid_bounds_duplicates_private_metadata_and_missing_fences() {
    let fixture = fixture();
    let request = &fixture["requests"][0];
    for field in ["sessionId", "expectedStreamId", "expectedCwdFence"] {
        let mut bad = request.clone();
        bad["request"]["params"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(decode_client_message(&serde_json::to_vec(&bad).unwrap()).is_err());
        let mut bad = request.clone();
        bad["request"]["params"][field] = "".into();
        assert!(decode_client_message(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    let frame = &fixture["responses"][0];
    for items in [
        json!([{"provider":"", "id":"tea"}]),
        json!([{"provider":"p", "id":"bad\nname"}]),
        json!([{"provider":"p".repeat(129), "id":"tea"}]),
        json!([{"provider":"p", "id":"茶".repeat(86)}]),
        json!([{"provider":"p", "id":"tea"},{"provider":"p", "id":"tea"}]),
        json!([{"provider":"p", "id":"tea", "apiKey":"never-forward"}]),
        json!(vec![json!({"provider":"p", "id":"tea"}); MAX_MODELS + 1]),
    ] {
        let mut bad = frame.clone();
        bad["result"]["data"]["result"]["data"]["items"] = items;
        assert!(decode_server_message(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    let mut maximum = frame.clone();
    maximum["result"]["data"]["result"]["data"]["items"] = json!(
        (0..MAX_MODELS)
            .map(|n| json!({"provider":"p", "id":n.to_string()}))
            .collect::<Vec<_>>()
    );
    assert!(decode_server_message(&serde_json::to_vec(&maximum).unwrap()).is_ok());
    let event = json!({"type":"event","protocolVersion":PROTOCOL_VERSION,"eventStreamId":"fixture","sequence":1,"event":{"kind":"models","data":{"items":[]}}});
    assert!(decode_server_message(&serde_json::to_vec(&event).unwrap()).is_err());
}
