//! Local-only projection. Raw detected values never leave this module except into private settings.
use super::*;
use cafe_space_protocol::{
    MAX_OWNER_ENVIRONMENT_ENTRIES, OwnerEnvironment, OwnerEnvironmentChange, OwnerEnvironmentEntry,
    OwnerEnvironmentSource, OwnerToolStatus, UpdateOwnerEnvironmentParams, owner_proxy_name,
    valid_owner_environment_name,
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
    if name == "PATH" {
        Environment::for_owner([("PATH".into(), value.into())])?.require_path()?;
    } else if matches!(
        owner_proxy_name(name),
        Some("HTTP_PROXY" | "HTTPS_PROXY" | "ALL_PROXY")
    ) {
        proxy_display(value).ok_or_else(|| error("OWNER_SETTINGS_INVALID"))?;
    }
    Ok(())
}

fn entry(key: &str, value: &str, source: OwnerEnvironmentSource) -> OwnerEnvironmentEntry {
    let proxy = matches!(
        owner_proxy_name(key),
        Some("HTTPS_PROXY" | "HTTP_PROXY" | "ALL_PROXY")
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
    inherited: impl IntoIterator<Item = (OsString, OsString)>,
) -> io::Result<OwnerEnvironment> {
    let detected: BTreeMap<_, _> = inherited
        .into_iter()
        .filter_map(|(key, value)| {
            let key = key.into_string().ok()?;
            #[cfg(windows)]
            let key = key.to_ascii_uppercase();
            let upper = key.to_ascii_uppercase();
            let shown = matches!(
                upper.as_str(),
                "PATH" | "NODE_EXTRA_CA_CERTS" | "SSL_CERT_FILE" | "SSH_AUTH_SOCK" | "NODE_OPTIONS"
            ) || owner_proxy_name(&key).is_some()
                || upper.ends_with("_API_KEY")
                || upper.ends_with("_TOKEN");
            if !shown || !valid_owner_environment_name(&key) {
                return None;
            }
            let text = value.to_str();
            let mut item = entry(
                &key,
                text.unwrap_or(""),
                OwnerEnvironmentSource::DaemonEnvironment,
            );
            if text.is_none() {
                item.adoptable = false;
            }
            Some((key, item))
        })
        .collect();
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
        detected: detected
            .into_values()
            .take(MAX_OWNER_ENVIRONMENT_ENTRIES)
            .collect(),
        configured: settings
            .overrides
            .iter()
            .map(|(key, value)| entry(key, value, OwnerEnvironmentSource::OwnerOverride))
            .collect(),
        path_ready,
        tools,
    })
}

pub(crate) fn wire_status(root: &Path) -> io::Result<OwnerEnvironment> {
    projection(&load(root)?, std::env::vars_os())
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
    if !valid_owner_environment_name(&key) {
        return Err(error("OWNER_SETTINGS_INVALID"));
    }
    let value = match params.change {
        OwnerEnvironmentChange::Remove => None,
        OwnerEnvironmentChange::AdoptDetected => {
            let value =
                lookup(&params.name).ok_or_else(|| error("OWNER_ENVIRONMENT_NOT_DETECTED"))?;
            let value = value
                .into_string()
                .map_err(|_| error("OWNER_SETTINGS_INVALID"))?;
            manual_value(&key, &value)?;
            Some(value)
        }
        OwnerEnvironmentChange::Set { value } => {
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
        let result = projection(
            &settings,
            [
                "OPENAI_API_KEY",
                "ANTHROPIC_API_KEY",
                "CUSTOM_TOKEN",
                "SSH_AUTH_SOCK",
                "NODE_OPTIONS",
                "IRRELEVANT",
                "CAFE_SPACE_TOKEN",
            ]
            .map(|key| (key.into(), "synthetic-detected-private".into())),
        )
        .unwrap();
        assert_eq!(result.detected.len(), 5);
        assert!(
            result
                .detected
                .iter()
                .any(|entry| entry.name == "OPENAI_API_KEY" && entry.display == "set")
        );
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
    fn status_limits_detection_without_hiding_valid_entries_or_exposing_values() {
        let settings = Settings {
            version: 1,
            overrides: BTreeMap::new(),
        };
        let inherited = (0..300)
            .map(|index| (format!("KEY_{index:03}_TOKEN").into(), "secret".into()))
            .chain([
                ("BAD\n_TOKEN".into(), "secret".into()),
                ("CAFE_SPACE_TOKEN".into(), "secret".into()),
            ]);
        let status = projection(&settings, inherited).unwrap();
        assert_eq!(status.detected.len(), MAX_OWNER_ENVIRONMENT_ENTRIES);
        assert!(
            status
                .detected
                .iter()
                .all(|entry| entry.display == "set" && entry.adoptable)
        );
        assert!(!serde_json::to_string(&status).unwrap().contains("secret"));
    }

    #[cfg(unix)]
    #[test]
    fn adopting_lowercase_proxy_pins_that_value_not_the_uppercase_value() {
        let (_temp, root) = super::super::tests::fixture();
        update_with_lookup(
            &root,
            UpdateOwnerEnvironmentParams {
                name: "https_proxy".into(),
                change: OwnerEnvironmentChange::AdoptDetected,
            },
            |key| {
                Some(
                    if key == "https_proxy" {
                        "http://lower.invalid:80"
                    } else {
                        "http://upper.invalid:80"
                    }
                    .into(),
                )
            },
        )
        .unwrap();
        assert_eq!(
            load(&root).unwrap().overrides["HTTPS_PROXY"],
            "http://lower.invalid:80"
        );
    }

    #[test]
    fn adopt_pins_inside_daemon_manual_secrets_are_private_and_remove_restores() {
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
        for key in [
            "SSH_AUTH_SOCK",
            "NODE_OPTIONS",
            "OPENAI_API_KEY",
            "CUSTOM_VALUE",
        ] {
            let response = update_with_lookup(
                &root,
                params(
                    key,
                    OwnerEnvironmentChange::Set {
                        value: "synthetic-private".into(),
                    },
                ),
                |_| None,
            )
            .unwrap();
            assert_eq!(load(&root).unwrap().overrides[key], "synthetic-private");
            assert!(
                !serde_json::to_string(&response)
                    .unwrap()
                    .contains("synthetic-private")
            );
        }
        let before = std::fs::read(root.join(FILE)).unwrap();
        for request in [
            params("CAFE_SPACE_TOKEN", OwnerEnvironmentChange::Remove),
            params(
                "BAD=NAME",
                OwnerEnvironmentChange::Set {
                    value: "synthetic-private".into(),
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
