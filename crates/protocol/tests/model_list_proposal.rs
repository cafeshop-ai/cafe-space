//! A staged D18 proposal must not silently extend the negotiated full API or frozen control.
use cafe_space_protocol::*;
use serde_json::Value;

#[test]
fn model_list_proposal_is_not_admitted_before_the_full_version_upgrade() {
    let fixture: Value = serde_json::from_slice(include_bytes!(
        "../../../fixtures/pi-rpc/model-list-wire-v5.json"
    ))
    .unwrap();
    assert_eq!(fixture["status"], "proposal_only");
    assert_eq!(
        fixture["contract"]["previousFullProtocolVersion"],
        PROTOCOL_VERSION
    );
    assert_eq!(
        fixture["contract"]["fullProtocolVersion"],
        PROTOCOL_VERSION + 1
    );
    assert_eq!(fixture["contract"]["controlVersion"], control::VERSION);
    assert_eq!(fixture["contract"]["maxModels"], 256);
    assert_eq!(fixture["contract"]["automaticRetry"], false);
    assert_eq!(fixture["contract"]["productionRuntimeEnabled"], false);
    for request in fixture["requests"].as_array().unwrap() {
        assert!(decode_client_message(&serde_json::to_vec(request).unwrap()).is_err());
        let mut current = request.clone();
        current["protocolVersion"] = PROTOCOL_VERSION.into();
        assert!(decode_client_message(&serde_json::to_vec(&current).unwrap()).is_err());
        current["protocolVersion"] = control::VERSION.into();
        let (_, method) = control::decode_request(&serde_json::to_vec(&current).unwrap()).unwrap();
        assert!(method.is_none());
    }
    for response in fixture["responses"].as_array().unwrap() {
        assert!(decode_server_message(&serde_json::to_vec(response).unwrap()).is_err());
        if response["result"]["status"] == "applied" {
            let mut current = response.clone();
            current["protocolVersion"] = PROTOCOL_VERSION.into();
            assert!(decode_server_message(&serde_json::to_vec(&current).unwrap()).is_err());
            current["protocolVersion"] = control::VERSION.into();
            assert!(
                control::decode_server_message(&serde_json::to_vec(&current).unwrap()).is_err()
            );
            for model in response["result"]["data"]["result"]["data"]["items"]
                .as_array()
                .unwrap()
            {
                assert_eq!(model.as_object().unwrap().len(), 2);
                assert!(model["provider"].is_string() && model["id"].is_string());
            }
        }
    }
}
