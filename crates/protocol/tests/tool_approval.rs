use cafe_space_protocol::{
    PROTOCOL_VERSION, RuntimeSummary, SessionSnapshot, ToolApproval, decode_server_message,
};
use serde_json::Value;

#[test]
fn approval_fixtures_are_explicit_and_old_records_never_gain_a_default() {
    assert_eq!(PROTOCOL_VERSION, 3);
    let runtimes: Value = serde_json::from_slice(include_bytes!(
        "../../../fixtures/v2/runtimes-response.json"
    ))
    .unwrap();
    decode_server_message(&serde_json::to_vec(&runtimes).unwrap()).unwrap();
    let items = runtimes["result"]["data"]["result"]["data"]["items"]
        .as_array()
        .unwrap();
    assert_eq!(items.len(), 3);
    let expected = [
        ToolApproval::NeverAsks,
        ToolApproval::MayAsk,
        ToolApproval::Unknown,
    ];
    for (item, expected) in items.iter().zip(expected) {
        let runtime: RuntimeSummary = serde_json::from_value(item.clone()).unwrap();
        assert_eq!(runtime.tool_approval, expected);
        let mut missing = item.clone();
        missing.as_object_mut().unwrap().remove("toolApproval");
        assert!(serde_json::from_value::<RuntimeSummary>(missing).is_err());
        let mut misleading = item.clone();
        misleading["toolApproval"] = "asks_per_tool".into();
        assert!(serde_json::from_value::<RuntimeSummary>(misleading).is_err());
    }
    let legacy: Value = serde_json::from_slice(include_bytes!(
        "../../../fixtures/migrations/v3-session.json"
    ))
    .unwrap();
    assert!(serde_json::from_value::<SessionSnapshot>(legacy.clone()).is_err());
    for approval in ["never_asks", "may_ask", "unknown"] {
        let mut migrated = legacy.clone();
        migrated["capabilities"]["toolApproval"] = approval.into();
        let snapshot: SessionSnapshot = serde_json::from_value(migrated).unwrap();
        assert_eq!(
            serde_json::to_value(snapshot.capabilities.tool_approval).unwrap(),
            approval
        );
    }
    decode_server_message(include_bytes!(
        "../../../fixtures/v2/session-tool-approval-event.json"
    ))
    .unwrap();
}
