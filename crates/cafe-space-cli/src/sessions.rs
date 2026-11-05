//! Fenced session requests over the existing full API. No launch, inferred fences, or retries.
use cafe_space_ipc::{BuildFlavor, client::Client};
use cafe_space_protocol::*;
use std::{
    ffi::{OsStr, OsString},
    io,
};

const HELP: &str = "session list [--json]\nsession get <id> [--json]\nsession new <id> <projectId> <runtimeId> <cwdFence> [--resume <sourceId>] [--json]\nsession prompt <id> <streamId> <cwdFence> (--text <text> | --file <path|->) [--delivery steer|follow_up] [--json]\nsession cancel <id> <streamId> <cwdFence> <runId> [--json]\nsession close <id> <streamId> <cwdFence> [--json]\nCopy fences/runId from session get or list; they are never guessed. Choose a stable new session ID before sending. Unknown outcomes: read state, never resend automatically. Dispatched is not turn completion. Files/stdin: BOM UTF-8/UTF-16LE/BE, otherwise strict UTF-8. Avoid PowerShell 5.1 text pipelines; use --text or --file.";
fn error(code: &str) -> io::Error {
    io::Error::other(code.to_owned())
}
fn text(value: &OsStr) -> io::Result<String> {
    value
        .to_str()
        .map(String::from)
        .ok_or_else(|| error("CLI_ARGUMENT_NOT_UTF8"))
}

pub(super) struct Command {
    pub request: Request,
    pub json: bool,
}

pub(super) fn parse(args: &[OsString]) -> io::Result<Option<Command>> {
    if args.len() == 1 && matches!(args[0].to_str(), Some("--help" | "-h")) {
        return Ok(None);
    }
    let first = args
        .first()
        .and_then(|arg| arg.to_str())
        .ok_or_else(|| error(HELP))?;
    let count = match first {
        "list" => 1,
        "get" => 2,
        "new" | "cancel" => 5,
        "prompt" | "close" => 4,
        _ => return Err(error(HELP)),
    };
    if args.len() < count {
        return Err(error(HELP));
    }
    let mut json = false;
    let mut source = None;
    let mut input = None;
    let mut delivery = None;
    let mut index = count;
    while index < args.len() {
        let flag = args[index]
            .to_str()
            .ok_or_else(|| error("CLI_ARGUMENT_NOT_UTF8"))?;
        if flag == "--json" && !json {
            json = true;
            index += 1;
            continue;
        }
        let value = args.get(index + 1).ok_or_else(|| error(HELP))?;
        match flag {
            "--resume" if first == "new" && source.is_none() => source = Some(text(value)?),
            "--text" | "--file" if first == "prompt" && input.is_none() => {
                input = Some((flag, value))
            }
            "--delivery" if first == "prompt" && delivery.is_none() => {
                delivery = Some(match value.to_str() {
                    Some("steer") => DeliveryMode::Steer,
                    Some("follow_up") => DeliveryMode::FollowUp,
                    _ => return Err(error(HELP)),
                })
            }
            _ => return Err(error(HELP)),
        }
        index += 2;
    }
    let request = match first {
        "list" => Request::ListSessions,
        "get" => Request::GetSession(GetSessionParams {
            session_id: text(&args[1])?,
        }),
        "new" => Request::CreateSession(CreateSessionParams {
            session_id: text(&args[1])?,
            project_id: text(&args[2])?,
            runtime_id: text(&args[3])?,
            expected_cwd_fence: text(&args[4])?,
            start: source.map_or(SessionStart::New, |source_session_id| {
                SessionStart::Resume(ResumeStartParams { source_session_id })
            }),
        }),
        "prompt" => {
            let (kind, value) = input.ok_or_else(|| error(HELP))?;
            Request::Prompt(PromptParams {
                session_id: text(&args[1])?,
                expected_stream_id: text(&args[2])?,
                expected_cwd_fence: text(&args[3])?,
                content: if kind == "--text" {
                    text(value)?
                } else {
                    super::input::file(value)?
                },
                delivery: delivery.unwrap_or(DeliveryMode::Steer),
            })
        }
        "cancel" => Request::Cancel(CancelParams {
            session_id: text(&args[1])?,
            expected_stream_id: text(&args[2])?,
            expected_cwd_fence: text(&args[3])?,
            run_id: text(&args[4])?,
        }),
        "close" => Request::CloseSession(CloseSessionParams {
            session_id: text(&args[1])?,
            expected_stream_id: text(&args[2])?,
            expected_cwd_fence: text(&args[3])?,
        }),
        _ => unreachable!(),
    };
    encode_client_message(&ClientMessage::Request(ClientRequest {
        message_type: RequestMessageType::Request,
        protocol_version: PROTOCOL_VERSION,
        request_id: "cli-validation".into(),
        request: request.clone(),
    }))
    .map_err(|_| error("CLI_ARGUMENT_INVALID"))?;
    Ok(Some(Command { request, json }))
}

fn finish(result: &RequestResult) -> io::Result<()> {
    match result {
        RequestResult::Applied(_) | RequestResult::Dispatched(_) => Ok(()),
        RequestResult::Rejected(value) | RequestResult::Unknown(value) => {
            Err(error(&value.error.code))
        }
    }
}

pub(super) async fn run(flavor: BuildFlavor, command: Option<Command>) -> io::Result<()> {
    let Some(command) = command else {
        println!("{HELP}");
        return Ok(());
    };
    let root = flavor
        .data_dir()
        .ok_or_else(|| error("INSTALL_PATH_UNAVAILABLE"))?;
    let (mut client, health) = Client::connect(flavor, &root).await.map_err(error)?;
    let result = if command.request == Request::GetStatus {
        RequestResult::Applied(Box::new(AppliedResult {
            result: Response::Health(health),
        }))
    } else {
        client
            .request_outcome(command.request)
            .await
            .unwrap_or_else(|_| {
                RequestResult::Unknown(ErrorResult {
                    error: ApiError {
                        code: "CLI_OUTCOME_UNKNOWN".into(),
                        message: "Outcome unknown; read state without retrying.".into(),
                    },
                })
            })
    };
    println!("{}", super::output::json(&result, command.json)?);
    finish(&result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }
    #[test]
    fn shared_commands_pin_fences_ids_and_explicit_delivery_before_connection() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/cli/session.json")).unwrap();
        for case in fixture["commands"].as_array().unwrap() {
            let words = case["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|word| OsString::from(word.as_str().unwrap()))
                .collect::<Vec<_>>();
            let command = parse(&words).unwrap().unwrap();
            assert!(!command.json);
            assert_eq!(
                serde_json::to_value(command.request).unwrap(),
                case["request"]
            );
            let mut words = words;
            words.push("--json".into());
            assert!(parse(&words).unwrap().unwrap().json);
        }
        assert!(parse(&args(&["--help"])).unwrap().is_none());
        for words in [
            vec!["new"],
            vec!["list", "--text", "x"],
            vec!["list", "--json", "--json"],
            vec!["prompt", "id", "s", "c"],
            vec!["prompt", "id", "s", "c", "--text", "x", "--file", "missing"],
            vec![
                "prompt",
                "id",
                "s",
                "c",
                "--text",
                "x",
                "--delivery",
                "queue",
            ],
            vec!["close", "id", "s"],
            vec!["cancel", "id", "s", "c"],
        ] {
            assert!(parse(&args(&words)).is_err());
        }
        let parsed = parse(&args(&["prompt", "id", "s", "c", "--text", "--json"]))
            .unwrap()
            .unwrap();
        assert!(!parsed.json);
        assert!(matches!(parsed.request, Request::Prompt(value) if value.content == "--json"));
        assert!(
            parse(&args(&[
                "prompt",
                "id",
                "s",
                "c",
                "--text",
                &"x".repeat(65537)
            ]))
            .is_err()
        );
    }
    #[test]
    fn outcomes_keep_unicode_exact_and_never_report_unknown_as_success() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/cli/session.json")).unwrap();
        for value in fixture["outcomes"].as_array().unwrap() {
            let result: RequestResult = serde_json::from_value(value.clone()).unwrap();
            let output = super::super::output::json(&result, true).unwrap();
            assert!(output.is_ascii());
            assert_eq!(
                serde_json::from_str::<RequestResult>(&output).unwrap(),
                result
            );
            assert_eq!(
                finish(&result).is_ok(),
                matches!(
                    result,
                    RequestResult::Applied(_) | RequestResult::Dispatched(_)
                )
            );
        }
    }
    #[cfg(windows)]
    #[test]
    fn invalid_utf16_argument_is_not_replaced_with_a_question_mark() {
        use std::os::windows::ffi::OsStringExt;
        let mut words = args(&["prompt", "id", "s", "c", "--text"]);
        words.push(OsString::from_wide(&[0xd800]));
        assert!(parse(&words).is_err());
    }
}
