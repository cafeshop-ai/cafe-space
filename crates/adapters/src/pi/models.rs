//! Project configured model identifiers only, never provider endpoints, headers or credentials.
use super::*;

pub(super) fn project(value: &Value) -> Result<Vec<ModelInfo>> {
    let entries = value.as_array().ok_or(Error("PI_MODEL_LIST_INVALID"))?;
    if entries.len() > MAX_AVAILABLE_MODELS {
        return Err(Error("PI_MODEL_LIST_LIMIT"));
    }
    let mut seen = std::collections::BTreeSet::new();
    entries
        .iter()
        .map(|entry| {
            let provider = entry["provider"]
                .as_str()
                .ok_or(Error("PI_MODEL_LIST_INVALID"))?;
            let id = entry["id"].as_str().ok_or(Error("PI_MODEL_LIST_INVALID"))?;
            if provider.is_empty()
                || provider.len() > 128
                || id.is_empty()
                || id.len() > 256
                || provider.chars().any(char::is_control)
                || id.chars().any(char::is_control)
                || !seen.insert((provider, id))
            {
                return Err(Error("PI_MODEL_LIST_INVALID"));
            }
            Ok(ModelInfo {
                provider: provider.into(),
                id: id.into(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixture_projects_only_correlated_bounded_model_identifiers() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../../../fixtures/pi-rpc/models.json")).unwrap();
        let mut session = super::super::tests::startup();
        let outbound = session.command(Command::GetAvailableModels).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&outbound.bytes).unwrap()["type"],
            fixture["request"]["type"]
        );
        let mut response = fixture["response"].clone();
        response["id"] = outbound.id.clone().into();
        let observed = session.observe(response.clone()).unwrap();
        let models = observed
            .iter()
            .find_map(|o| match o {
                Observation::AvailableModels { id, models } if id == &outbound.id => Some(models),
                _ => None,
            })
            .unwrap();
        assert_eq!(serde_json::to_value(models).unwrap(), fixture["expected"]);
        assert!(!format!("{observed:?}").contains("never-forward"));
        assert_eq!(session.state(), &SessionState::Ready);
        assert_eq!(
            session.observe(response).unwrap(),
            vec![Observation::Notice("PI_LATE_RESPONSE")]
        );
        assert_eq!(project(&json!([])).unwrap(), vec![]);
    }
    #[test]
    fn invalid_or_oversized_model_metadata_never_yields_a_partial_list() {
        for value in [
            json!(null),
            json!({}),
            json!([{}]),
            json!([{ "provider":"", "id":"tea" }]),
            json!([{ "provider":"synthetic", "id":"bad\nname" }]),
            json!([{ "provider":"synthetic", "id":"茶".repeat(86) }]),
            json!([{ "provider":"synthetic", "id":"tea" },{ "provider":"synthetic", "id":"tea" }]),
        ] {
            assert_eq!(project(&value).unwrap_err(), Error("PI_MODEL_LIST_INVALID"));
        }
        assert_eq!(
            project(&json!(vec![
                json!({"provider":"synthetic","id":"tea"});
                MAX_AVAILABLE_MODELS + 1
            ]))
            .unwrap_err(),
            Error("PI_MODEL_LIST_LIMIT")
        );
        let mut session = super::super::tests::startup();
        let command = session.command(Command::GetAvailableModels).unwrap();
        let result = session.observe(json!({"type":"response","id":command.id,"command":"get_available_models","success":false,"error":"synthetic-never-forward"})).unwrap();
        assert_eq!(
            result,
            vec![Observation::Outcome {
                id: command.id,
                outcome: Outcome::Rejected("PI_COMMAND_REJECTED")
            }]
        );
    }
}
