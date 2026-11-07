//! Explicit local-owner registration over authenticated IPC; never installs or launches an Agent.
use super::sessions::Command;
use cafe_space_ipc::BuildFlavor;
use cafe_space_protocol::*;
use std::{ffi::OsString, io, path::PathBuf};

const HELP: &str = "runtime list\nruntime register pi <node-executable> <package-directory>\nruntime remove pi\nRegistration is local-only and passive: no download, shell, credentials or Agent execution. Uses the pinned Pi package version. Tools in created sessions run as you without asking. Removing registration blocks new sessions but does not stop existing ones. JSON outcomes are never automatically retried; after an unknown result, inspect runtime list.";

fn path(value: &OsString) -> io::Result<String> {
    let value = value
        .to_str()
        .ok_or_else(|| io::Error::other("CLI_ARGUMENT_NOT_UTF8"))?;
    if value.is_empty() || value.len() > 65536 || value.chars().any(char::is_control) {
        return Err(io::Error::other("CLI_PATH_INVALID"));
    }
    let path = PathBuf::from(value);
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|_| io::Error::other("CLI_CWD_UNAVAILABLE"))?
            .join(path)
    };
    path.to_str()
        .map(String::from)
        .ok_or_else(|| io::Error::other("CLI_ARGUMENT_NOT_UTF8"))
}

pub(super) fn parse(args: &[OsString]) -> io::Result<Option<Command>> {
    let request = match args {
        [help] if help == "--help" || help == "-h" => return Ok(None),
        [list] if list == "list" => Request::ListRuntimes,
        [remove, pi] if remove == "remove" && pi == "pi" => Request::UnregisterPiRuntime,
        [register, pi, node, package] if register == "register" && pi == "pi" => {
            Request::RegisterPiRuntime(RegisterPiRuntimeParams {
                node: path(node)?,
                package_root: path(package)?,
            })
        }
        _ => return Err(io::Error::other(HELP)),
    };
    encode_client_message(&ClientMessage::Request(ClientRequest {
        message_type: RequestMessageType::Request,
        protocol_version: PROTOCOL_VERSION,
        request_id: "runtime-validation".into(),
        request: request.clone(),
    }))
    .map_err(|_| io::Error::other("CLI_ARGUMENT_INVALID"))?;
    Ok(Some(Command {
        request,
        json: true,
    }))
}

pub(super) async fn run(flavor: BuildFlavor, command: Option<Command>) -> io::Result<()> {
    if command.is_none() {
        println!("{HELP}");
        return Ok(());
    }
    super::sessions::run(flavor, command).await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }
    #[test]
    fn registration_requires_explicit_paths_and_never_probes_or_guesses_an_installation() {
        assert!(parse(&args(&["--help"])).unwrap().is_none());
        assert_eq!(
            parse(&args(&["list"])).unwrap().unwrap().request,
            Request::ListRuntimes
        );
        assert_eq!(
            parse(&args(&["remove", "pi"])).unwrap().unwrap().request,
            Request::UnregisterPiRuntime
        );
        let command = parse(&args(&[
            "register",
            "pi",
            "missing-node",
            "missing-package",
        ]))
        .unwrap()
        .unwrap();
        assert!(command.json);
        let Request::RegisterPiRuntime(params) = command.request else {
            panic!("registration")
        };
        assert!(PathBuf::from(params.node).is_absolute());
        assert!(PathBuf::from(params.package_root).is_absolute());
        for words in [
            vec![],
            vec!["register", "pi"],
            vec!["register", "codex", "node", "package"],
            vec!["remove"],
            vec!["remove", "pi", "--force"],
            vec!["list", "extra"],
            vec!["register", "pi", "", "package"],
            vec!["register", "pi", "node", "a\nb"],
        ] {
            assert!(parse(&args(&words)).is_err());
        }
    }
    #[cfg(windows)]
    #[test]
    fn unpaired_utf16_path_is_rejected_before_connecting() {
        use std::os::windows::ffi::OsStringExt;
        let words = [
            "register".into(),
            "pi".into(),
            OsString::from_wide(&[67, 58, 92, 0xd800]),
            "package".into(),
        ];
        assert!(
            matches!(parse(&words), Err(error) if error.to_string() == "CLI_ARGUMENT_NOT_UTF8")
        );
    }
}
