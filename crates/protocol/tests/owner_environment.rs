use cafe_space_protocol::*;
use serde_json::{Value, json};

fn fixture() -> Value {
    serde_json::from_slice(include_bytes!(
        "../../../fixtures/v2/owner-environment.json"
    ))
    .unwrap()
}

#[test]
fn owner_wire_fixtures_round_trip_without_joining_control_or_events() {
    let fixture = fixture();
    assert_eq!(fixture["contract"]["protocolVersion"], PROTOCOL_VERSION);
    assert_eq!(PROTOCOL_VERSION, 10);
    for request in fixture["requests"].as_array().unwrap() {
        let frame = serde_json::to_vec(request).unwrap();
        let decoded = decode_client_message(&frame).unwrap();
        assert_eq!(serde_json::to_value(&decoded).unwrap(), *request);
        let mut control_request = request.clone();
        control_request["protocolVersion"] = control::VERSION.into();
        let (_, method) =
            control::decode_request(&serde_json::to_vec(&control_request).unwrap()).unwrap();
        assert!(method.is_none());
        let mut old = request.clone();
        old["protocolVersion"] = 3.into();
        assert!(decode_client_message(&serde_json::to_vec(&old).unwrap()).is_err());
        assert!(!format!("{decoded:?}").contains("synthetic-password"));
    }
    for response in fixture["responses"].as_array().unwrap() {
        let decoded = decode_server_message(&serde_json::to_vec(response).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), *response);
    }
    let mut event = json!({"type":"event","protocolVersion":PROTOCOL_VERSION,"eventStreamId":"fixture","sequence":1,"event":{"kind":"owner_environment","data":{}}});
    assert!(decode_server_message(&serde_json::to_vec(&event).unwrap()).is_err());
    event["event"]["kind"] = "owner_environment_changed".into();
    assert!(decode_server_message(&serde_json::to_vec(&event).unwrap()).is_err());
}

#[test]
fn inputs_reject_unsupported_names_values_and_secret_fields() {
    let fixture = fixture();
    let request = fixture["requests"][2].clone();
    for name in [
        "NODE_OPTIONS",
        "OPENAI_API_KEY",
        "SSH_AUTH_SOCK",
        "SSL_CERT_FILE",
        "CUSTOM_VALUE",
    ] {
        let mut valid = request.clone();
        valid["request"]["params"]["name"] = name.into();
        assert!(decode_client_message(&serde_json::to_vec(&valid).unwrap()).is_ok());
    }
    for name in [
        "",
        "CAFE_SPACE_TOKEN",
        "cafe_space_token",
        "BAD=NAME",
        "bad\nname",
    ] {
        let mut invalid = request.clone();
        invalid["request"]["params"]["name"] = name.into();
        assert!(decode_client_message(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
    for value in [
        String::new(),
        "has\0nul".into(),
        "has\nline".into(),
        "x".repeat(65537),
    ] {
        let mut invalid = request.clone();
        invalid["request"]["params"]["change"]["value"] = value.into();
        assert!(decode_client_message(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
    for extra in ["access", "root", "detectedValue"] {
        let mut invalid = fixture["requests"][1].clone();
        invalid["request"]["params"][extra] = "synthetic-private".into();
        assert!(decode_client_message(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
    let mut response = fixture["responses"][0].clone();
    response["result"]["data"]["result"]["data"]["detected"][0]["value"] =
        "synthetic-private".into();
    assert!(decode_server_message(&serde_json::to_vec(&response).unwrap()).is_err());
    response = fixture["responses"][0].clone();
    response["result"]["data"]["result"]["data"]["configured"][0]["display"] =
        "/private/ca.pem".into();
    assert!(decode_server_message(&serde_json::to_vec(&response).unwrap()).is_err());
}
