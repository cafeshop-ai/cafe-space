//! Explicit local project administration. JSON output is ASCII even on legacy Windows consoles.
use cafe_space_ipc::{BuildFlavor, client::Client};
use cafe_space_protocol::*;
use std::{
    ffi::{OsStr, OsString},
    io,
    path::PathBuf,
};

fn error(code: &str) -> io::Error {
    io::Error::other(code.to_owned())
}
const HELP: &str = "project list\nproject add <directory> [--name <label>]\nproject update <id> <cwdFence> [--root <directory>] [--name <label>]\nproject remove <id> <cwdFence>\nproject root <id>\nMutations require local owner-only IPC. Copy id/cwdFence from project list. No request is automatically retried. This cwd allowlist is not a sandbox.";
fn text(value: &OsStr) -> io::Result<String> {
    value
        .to_str()
        .map(String::from)
        .ok_or_else(|| error("PROJECT_ARGUMENT_NOT_UTF8"))
}
fn path(value: &OsStr) -> io::Result<String> {
    let value = value
        .to_str()
        .ok_or_else(|| error("PROJECT_ROOT_NOT_UTF8"))?;
    if value.is_empty() || value.len() > 65536 || value.chars().any(char::is_control) {
        return Err(error("PROJECT_ROOT_UNSUPPORTED"));
    }
    let path = PathBuf::from(value);
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|_| error("PROJECT_ROOT_UNAVAILABLE"))?
            .join(path)
    };
    absolute
        .to_str()
        .map(String::from)
        .ok_or_else(|| error("PROJECT_ROOT_NOT_UTF8"))
}

pub(super) fn parse(args: &[OsString]) -> io::Result<Option<Request>> {
    if args.len() == 1 && matches!(args[0].to_str(), Some("--help" | "-h")) {
        return Ok(None);
    }
    let first = args
        .first()
        .and_then(|a| a.to_str())
        .ok_or_else(|| error(HELP))?;
    let request = match first {
        "list" if args.len() == 1 => Request::ListProjects,
        "root" if args.len() == 2 => Request::GetProjectRoot(GetProjectRootParams {
            project_id: text(&args[1])?,
        }),
        "remove" if args.len() == 3 => Request::RemoveProject(RemoveProjectParams {
            project_id: text(&args[1])?,
            expected_cwd_fence: text(&args[2])?,
        }),
        "add" | "update" => {
            let begin = if first == "add" { 2 } else { 3 };
            if args.len() < begin {
                return Err(error(HELP));
            }
            let mut root = if first == "add" {
                Some(path(&args[1])?)
            } else {
                None
            };
            let mut name = None;
            for pair in args[begin..].chunks(2) {
                if pair.len() != 2 {
                    return Err(error(HELP));
                }
                match pair[0].to_str() {
                    Some("--name") if name.is_none() => name = Some(text(&pair[1])?),
                    Some("--root") if first == "update" && root.is_none() => {
                        root = Some(path(&pair[1])?)
                    }
                    _ => return Err(error(HELP)),
                }
            }
            if first == "add" {
                Request::AddProject(AddProjectParams {
                    root: root.unwrap(),
                    name,
                })
            } else {
                if root.is_none() && name.is_none() {
                    return Err(error(HELP));
                }
                Request::UpdateProject(UpdateProjectParams {
                    project_id: text(&args[1])?,
                    expected_cwd_fence: text(&args[2])?,
                    root,
                    name,
                })
            }
        }
        _ => return Err(error(HELP)),
    };
    // Validate all protocol limits before connecting or writing to the daemon.
    encode_client_message(&ClientMessage::Request(ClientRequest {
        message_type: RequestMessageType::Request,
        protocol_version: PROTOCOL_VERSION,
        request_id: "project-validation".into(),
        request: request.clone(),
    }))
    .map_err(|_| error("PROJECT_ARGUMENT_INVALID"))?;
    Ok(Some(request))
}

pub(super) async fn run(flavor: BuildFlavor, request: Option<Request>) -> io::Result<()> {
    let Some(request) = request else {
        println!("{HELP}");
        return Ok(());
    };
    let root = flavor
        .data_dir()
        .ok_or_else(|| error("INSTALL_PATH_UNAVAILABLE"))?;
    let (mut client, _) = Client::connect(flavor, &root).await.map_err(error)?;
    // Share the authenticated transport, not the installer's rejection-normalizing request method.
    let result = client.request_outcome(request).await.map_err(error)?;
    let output = ascii_json(&result)?;
    println!("{output}");
    match result {
        RequestResult::Rejected(value) | RequestResult::Unknown(value) => {
            Err(error(&value.error.code))
        }
        RequestResult::Applied(_) => Ok(()),
        RequestResult::Dispatched(_) => Err(error("PROJECT_OUTCOME_UNCONFIRMED")),
    }
}

fn ascii_json(value: &RequestResult) -> io::Result<String> {
    use std::fmt::Write;
    let json = serde_json::to_string(value).map_err(|_| error("PROJECT_OUTPUT_INVALID"))?;
    let mut output = String::new();
    for ch in json.chars() {
        if ch.is_ascii() {
            output.push(ch);
        } else {
            for unit in ch.encode_utf16(&mut [0; 2]) {
                write!(&mut output, "\\u{unit:04x}")
                    .map_err(|_| error("PROJECT_OUTPUT_INVALID"))?;
            }
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }
    #[test]
    fn explicit_project_commands_validate_paths_fences_and_options_before_connecting() {
        assert_eq!(
            parse(&args(&["list"])).unwrap(),
            Some(Request::ListProjects)
        );
        assert!(matches!(
            parse(&args(&["add", ".", "--name", "茶"])).unwrap(),
            Some(Request::AddProject(AddProjectParams { name: Some(_), .. }))
        ));
        assert!(matches!(
            parse(&args(&["update", "id", "fence", "--name", "Tea"])).unwrap(),
            Some(Request::UpdateProject(UpdateProjectParams {
                root: None,
                ..
            }))
        ));
        assert!(matches!(
            parse(&args(&["remove", "id", "fence"])).unwrap(),
            Some(Request::RemoveProject(_))
        ));
        for values in [
            vec!["add"],
            vec!["update", "id", "fence"],
            vec!["remove", "id"],
            vec!["list", "extra"],
            vec!["add", ".", "--name", "a", "--name", "b"],
            vec!["add", ".", "--root", "."],
        ] {
            assert!(parse(&args(&values)).is_err());
        }
        assert_eq!(
            parse(&args(&["add", "a\nb"])).unwrap_err().to_string(),
            "PROJECT_ROOT_UNSUPPORTED"
        );
        assert_eq!(
            parse(&args(&["add", ".", "--name", "a\nb"]))
                .unwrap_err()
                .to_string(),
            "PROJECT_ARGUMENT_INVALID"
        );
    }
    #[test]
    fn ascii_json_preserves_unicode_and_exact_server_outcome_codes() {
        let value = RequestResult::Rejected(ErrorResult {
            error: ApiError {
                code: "STALE_CWD_FENCE".into(),
                message: "茶 ☕ 𐐀".into(),
            },
        });
        let output = ascii_json(&value).unwrap();
        assert!(output.is_ascii());
        assert!(output.contains("STALE_CWD_FENCE"));
        assert!(output.contains("\\ud801\\udc00"));
        assert_eq!(
            serde_json::from_str::<RequestResult>(&output).unwrap(),
            value
        );
    }
    #[cfg(windows)]
    #[test]
    fn unpaired_utf16_root_is_rejected_without_lossy_conversion() {
        use std::os::windows::ffi::OsStringExt;
        let args = vec![
            OsString::from("add"),
            OsString::from_wide(&[67, 58, 92, 0xd800]),
        ];
        assert_eq!(
            parse(&args).unwrap_err().to_string(),
            "PROJECT_ROOT_NOT_UTF8"
        );
    }
}
