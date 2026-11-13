//! Owner-only settings contract; secret-bearing inputs deliberately redact Debug.
use super::*;

pub const MAX_OWNER_ENVIRONMENT_ENTRIES: usize = 256;

pub fn valid_owner_environment_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_LABEL_BYTES
        && !name.chars().any(|c| c.is_control() || c == '=')
        && !name
            .get(..11)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("CAFE_SPACE_"))
}

pub fn owner_proxy_name(name: &str) -> Option<&'static str> {
    ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"]
        .into_iter()
        .find(|candidate| name.eq_ignore_ascii_case(candidate))
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct UpdateOwnerEnvironmentParams {
    pub name: String,
    pub change: OwnerEnvironmentChange,
}
impl fmt::Debug for UpdateOwnerEnvironmentParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("UpdateOwnerEnvironmentParams([redacted])")
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[ts(export)]
pub enum OwnerEnvironmentChange {
    AdoptDetected,
    Remove,
    Set { value: String },
}
impl fmt::Debug for OwnerEnvironmentChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OwnerEnvironmentChange([redacted])")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct OwnerEnvironment {
    pub detected: Vec<OwnerEnvironmentEntry>,
    pub configured: Vec<OwnerEnvironmentEntry>,
    pub path_ready: bool,
    pub tools: Vec<OwnerToolStatus>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct OwnerEnvironmentEntry {
    pub name: String,
    pub display: String,
    pub source: OwnerEnvironmentSource,
    pub adoptable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum OwnerEnvironmentSource {
    DaemonEnvironment,
    OwnerOverride,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct OwnerToolStatus {
    pub name: String,
    pub found: bool,
}

pub(crate) fn validate_update(params: &UpdateOwnerEnvironmentParams) -> Result<(), ProtocolError> {
    if !valid_owner_environment_name(&params.name) {
        return Err(ProtocolError::InvalidMessage);
    }
    if let OwnerEnvironmentChange::Set { value } = &params.change {
        check_id_max(value, MAX_TEXT_BYTES)?;
    }
    Ok(())
}

pub(crate) fn validate_status(status: &OwnerEnvironment) -> Result<(), ProtocolError> {
    for (entries, source) in [
        (&status.detected, OwnerEnvironmentSource::DaemonEnvironment),
        (&status.configured, OwnerEnvironmentSource::OwnerOverride),
    ] {
        check_limit(entries.len(), MAX_OWNER_ENVIRONMENT_ENTRIES)?;
        let mut seen = std::collections::BTreeSet::new();
        for entry in entries {
            if !valid_owner_environment_name(&entry.name)
                || entry.source != source
                || !seen.insert(&entry.name)
                || (source == OwnerEnvironmentSource::OwnerOverride && entry.adoptable)
            {
                return Err(ProtocolError::InvalidMessage);
            }
            check_id_max(&entry.display, MAX_LABEL_BYTES)?;
            if !matches!(
                owner_proxy_name(&entry.name),
                Some("HTTP_PROXY" | "HTTPS_PROXY" | "ALL_PROXY")
            ) && entry.display != "set"
            {
                return Err(ProtocolError::InvalidMessage);
            }
        }
    }
    check_limit(status.tools.len(), 4)?;
    let mut seen = std::collections::BTreeSet::new();
    for tool in &status.tools {
        if !["git", "node", "npm", "pwsh"].contains(&tool.name.as_str()) || !seen.insert(&tool.name)
        {
            return Err(ProtocolError::InvalidMessage);
        }
    }
    Ok(())
}
