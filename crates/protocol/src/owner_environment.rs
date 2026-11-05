//! Owner-only settings contract; secret-bearing inputs deliberately redact Debug.
use super::*;

pub const OWNER_ENVIRONMENT_NAMES: &[&str] = &[
    "PATH",
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "https_proxy",
    "http_proxy",
    "no_proxy",
    "all_proxy",
    "NODE_EXTRA_CA_CERTS",
    "SSL_CERT_FILE",
    "SSH_AUTH_SOCK",
];

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
    if !OWNER_ENVIRONMENT_NAMES.contains(&params.name.as_str()) {
        return Err(ProtocolError::InvalidMessage);
    }
    if let OwnerEnvironmentChange::Set { value } = &params.change {
        if matches!(
            params.name.as_str(),
            "NODE_EXTRA_CA_CERTS" | "SSL_CERT_FILE" | "SSH_AUTH_SOCK"
        ) {
            return Err(ProtocolError::InvalidMessage);
        }
        check_id_max(value, MAX_TEXT_BYTES)?;
    }
    Ok(())
}

pub(crate) fn validate_status(status: &OwnerEnvironment) -> Result<(), ProtocolError> {
    for (entries, source) in [
        (&status.detected, OwnerEnvironmentSource::DaemonEnvironment),
        (&status.configured, OwnerEnvironmentSource::OwnerOverride),
    ] {
        check_limit(entries.len(), OWNER_ENVIRONMENT_NAMES.len())?;
        let mut seen = std::collections::BTreeSet::new();
        for entry in entries {
            if !OWNER_ENVIRONMENT_NAMES.contains(&entry.name.as_str())
                || entry.source != source
                || !seen.insert(&entry.name)
                || (source == OwnerEnvironmentSource::OwnerOverride && entry.adoptable)
            {
                return Err(ProtocolError::InvalidMessage);
            }
            check_id_max(&entry.display, MAX_LABEL_BYTES)?;
            if matches!(
                entry.name.as_str(),
                "PATH"
                    | "NO_PROXY"
                    | "no_proxy"
                    | "NODE_EXTRA_CA_CERTS"
                    | "SSL_CERT_FILE"
                    | "SSH_AUTH_SOCK"
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
