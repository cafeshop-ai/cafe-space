//! Local-only projection. Raw detected values never leave this module except into private settings.
use super::*;
use cafe_space_protocol::{
    OWNER_ENVIRONMENT_NAMES, OwnerEnvironment, OwnerEnvironmentChange, OwnerEnvironmentEntry,
    OwnerEnvironmentSource, OwnerToolStatus, UpdateOwnerEnvironmentParams,
};

fn proxy_display(value: &str) -> Option<String> {
    let (scheme, rest) = value.split_once("://")?;
    if !["http", "https", "socks", "socks5", "socks5h"].contains(&scheme) {
        return None;
    }
    // Never echo path/query/fragment, ambiguous escapes or unvalidated authority characters.
    let authority = rest.split(['/', '?', '#']).next()?;
    let (credentials, host_port) = authority
        .rsplit_once('@')
        .map_or((false, authority), |(_, host)| (true, host));
    let (host, port) = if host_port.starts_with('[') {
        let end = host_port.find(']')?;
        let host = &host_port[..=end];
        host[1..host.len() - 1].parse::<std::net::Ipv6Addr>().ok()?;
        (host, host_port[end + 1..].strip_prefix(':').unwrap_or(""))
    } else {
        let (host, port) = host_port.split_once(':').unwrap_or((host_port, ""));
        if host.is_empty()
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        {
            return None;
        }
        (host, port)
    };
    if !port.is_empty()
        && (port.parse::<u16>().ok()? == 0 || !port.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    if host_port.starts_with('[')
        && host_port != host
        && !host_port.starts_with(&format!("{host}:"))
    {
        return None;
    }
    let display = format!(
        "{scheme}://{}{host}{}",
        if credentials {
            "•••:•••@"
        } else {
            ""
        },
        if port.is_empty() {
            String::new()
        } else {
            format!(":{port}")
        }
    );
    (display.len() <= 256).then_some(display)
}

fn manual_value(name: &str, value: &str) -> io::Result<()> {
    if value.is_empty() || value.len() > MAX_BYTES || value.chars().any(char::is_control) {
        return Err(error("OWNER_SETTINGS_INVALID"));
    }
    match name {
        "PATH" => {
            Environment::for_owner([("PATH".into(), value.into())])?.require_path()?;
        }
        "NO_PROXY" | "no_proxy" => {}
        "HTTPS_PROXY" | "HTTP_PROXY" | "ALL_PROXY" | "https_proxy" | "http_proxy" | "all_proxy" => {
            proxy_display(value).ok_or_else(|| error("OWNER_SETTINGS_INVALID"))?;
        }
        "NODE_EXTRA_CA_CERTS" | "SSL_CERT_FILE" | "SSH_AUTH_SOCK" => {}
        _ => return Err(error("OWNER_SETTINGS_INVALID")),
    }
    Ok(())
}

fn entry(key: &str, value: &str, source: OwnerEnvironmentSource) -> OwnerEnvironmentEntry {
    let proxy = matches!(
        key,
        "HTTPS_PROXY" | "HTTP_PROXY" | "ALL_PROXY" | "https_proxy" | "http_proxy" | "all_proxy"
    );
    OwnerEnvironmentEntry {
        name: key.into(),
        display: if proxy {
            proxy_display(value).unwrap_or_else(|| "set".into())
        } else {
            "set".into()
        },
        adoptable: source == OwnerEnvironmentSource::DaemonEnvironment
            && manual_value(key, value).is_ok(),
        source,
    }
}

fn projection(
    settings: &Settings,
    mut lookup: impl FnMut(&str) -> Option<OsString>,
) -> io::Result<OwnerEnvironment> {
    let mut detected = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for key in OWNER_ENVIRONMENT_NAMES {
        let key = name(key);
        if !seen.insert(key.clone()) {
            continue;
        }
        if let Some(value) = lookup(&key) {
            let value = value.to_str();
            let mut item = entry(
                &key,
                value.unwrap_or(""),
                OwnerEnvironmentSource::DaemonEnvironment,
            );
            if value.is_none() {
                item.adoptable = false;
            }
            detected.push(item);
        }
    }
    let environment = settings.environment()?;
    let path_ready = environment.require_path().is_ok();
    let tools = ["git", "node", "npm"]
        .into_iter()
        .chain(if cfg!(windows) { Some("pwsh") } else { None })
        .map(|tool| OwnerToolStatus {
            name: tool.into(),
            found: path_ready && environment.tool_found(tool),
        })
        .collect();
    Ok(OwnerEnvironment {
        detected,
        configured: settings
            .overrides
            .iter()
            .filter(|(key, _)| OWNER_ENVIRONMENT_NAMES.contains(&key.as_str()))
            .map(|(key, value)| entry(key, value, OwnerEnvironmentSource::OwnerOverride))
            .collect(),
        path_ready,
        tools,
    })
}

pub(crate) fn wire_status(root: &Path) -> io::Result<OwnerEnvironment> {
    projection(&load(root)?, |key| std::env::var_os(key))
}

pub(crate) fn update_wire(
    root: &Path,
    params: UpdateOwnerEnvironmentParams,
) -> io::Result<OwnerEnvironment> {
    update_with_lookup(root, params, |key| std::env::var_os(key))
}

fn update_with_lookup(
    root: &Path,
    params: UpdateOwnerEnvironmentParams,
    mut lookup: impl FnMut(&str) -> Option<OsString>,
) -> io::Result<OwnerEnvironment> {
    let key = name(&params.name);
    if !OWNER_ENVIRONMENT_NAMES.contains(&key.as_str()) {
        return Err(error("OWNER_SETTINGS_INVALID"));
    }
    let value = match params.change {
        OwnerEnvironmentChange::Remove => None,
        OwnerEnvironmentChange::AdoptDetected => {
            let value = lookup(&key).ok_or_else(|| error("OWNER_ENVIRONMENT_NOT_DETECTED"))?;
            let value = value
                .into_string()
                .map_err(|_| error("OWNER_SETTINGS_INVALID"))?;
            manual_value(&key, &value)?;
            Some(value)
        }
        OwnerEnvironmentChange::Set { value } => {
            if matches!(
                key.as_str(),
                "NODE_EXTRA_CA_CERTS" | "SSL_CERT_FILE" | "SSH_AUTH_SOCK"
            ) {
                return Err(error("OWNER_SETTINGS_INVALID"));
            }
            manual_value(&key, &value)?;
            Some(value)
        }
    };
    patch(root, [(key, value)].into())?;
    wire_status(root).map_err(|_| error("OWNER_SETTINGS_WRITE_UNKNOWN"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projection_masks_credentials_and_never_returns_paths_or_no_proxy_hosts() {
        let settings = Settings {
            version: 1,
            overrides: [
                (
                    "HTTPS_PROXY".into(),
                    "https://private-user:private-password@proxy.invalid:3128/private-query?secret"
                        .into(),
                ),
                ("SSH_AUTH_SOCK".into(), "/private/signing-socket".into()),
                ("NODE_EXTRA_CA_CERTS".into(), "/private/ca.pem".into()),
                ("NO_PROXY".into(), "private.internal".into()),
            ]
            .into(),
        };
        let result = projection(&settings, |_| Some("synthetic-detected-private".into())).unwrap();
        let json = serde_json::to_string(&result).unwrap();
        for secret in [
            "private-user",
            "private-password",
            "/private",
            "private.internal",
            "synthetic-detected-private",
        ] {
            assert!(!json.contains(secret), "secret leaked");
        }
        assert!(json.contains("https://•••:•••@proxy.invalid:3128"));
        assert!(result.configured.iter().all(|v| !v.adoptable));
        for value in [
            "http://user:password@evil.invalid%2fsecret",
            "file:///private/secret",
            "http://host.invalid:bad",
            "http://[::1]secret",
        ] {
            assert_eq!(proxy_display(value), None);
        }
        assert_eq!(
            proxy_display("http://[::1]:3128/path"),
            Some("http://[::1]:3128".into())
        );
    }
    #[test]
    fn adopt_copies_in_daemon_remove_restores_and_manual_signing_is_rejected() {
        let (_temp, root) = super::super::tests::fixture();
        let params = |name: &str, change| UpdateOwnerEnvironmentParams {
            name: name.into(),
            change,
        };
        let value = "http://synthetic-user:synthetic-password@proxy.invalid:3128";
        update_with_lookup(
            &root,
            params("HTTPS_PROXY", OwnerEnvironmentChange::AdoptDetected),
            |_| Some(value.into()),
        )
        .unwrap();
        assert_eq!(load(&root).unwrap().overrides["HTTPS_PROXY"], value);
        let before = std::fs::read(root.join(FILE)).unwrap();
        for request in [
            params("NODE_OPTIONS", OwnerEnvironmentChange::Remove),
            params(
                "SSH_AUTH_SOCK",
                OwnerEnvironmentChange::Set {
                    value: "/private/socket".into(),
                },
            ),
            params("HTTPS_PROXY", OwnerEnvironmentChange::AdoptDetected),
        ] {
            assert!(update_with_lookup(&root, request, |_| None).is_err());
            assert_eq!(std::fs::read(root.join(FILE)).unwrap(), before);
        }
        update_with_lookup(
            &root,
            params("HTTPS_PROXY", OwnerEnvironmentChange::Remove),
            |_| None,
        )
        .unwrap();
        assert!(!load(&root).unwrap().overrides.contains_key("HTTPS_PROXY"));
    }
}
