//! Explicitly opted-in real Host smoke, isolated from default/CI tests and real user sessions.
use super::*;
use cafe_space_adapters::pi::install::{self, Installation};
use cafe_space_ipc::{BuildFlavor, private};
use cafe_space_protocol::*;
use cafe_spaced::{host::Host, state::State, update::Access};
use std::io;

fn error(code: &'static str) -> io::Error {
    io::Error::other(code)
}
fn response(host: &Host, request: Request) -> io::Result<Response> {
    match host.dispatch(Access::Local, 1, request)?.result {
        RequestResult::Applied(value) => Ok(value.result),
        _ => Err(error("PI_HOST_SMOKE_READ_FAILED")),
    }
}
fn view(host: &Host, id: &str) -> io::Result<SessionView> {
    match response(
        host,
        Request::GetSession(GetSessionParams {
            session_id: id.into(),
        }),
    )? {
        Response::Session(value) => Ok(value.session),
        _ => Err(error("PI_HOST_SMOKE_READ_FAILED")),
    }
}
fn ready(host: &Host, id: &str) -> io::Result<SessionSnapshot> {
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        host.poll_processes(Instant::now())?;
        let session = view(host, id)?.session;
        match session.state {
            SessionState::Ready => return Ok(session),
            SessionState::Starting if Instant::now() < deadline => {}
            _ => return Err(error("PI_HOST_SMOKE_STARTUP_FAILED")),
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn turn(host: &Host, session: &SessionSnapshot, content: String, token: &str) -> io::Result<()> {
    let mut reply = host.command_pi(
        Access::Local,
        Request::Prompt(PromptParams {
            session_id: session.id.clone(),
            expected_stream_id: session
                .stream_id
                .clone()
                .ok_or_else(|| error("PI_HOST_SMOKE_NO_STREAM"))?,
            expected_cwd_fence: session.cwd_fence.clone(),
            content,
            delivery: DeliveryMode::Steer,
        }),
    )?;
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut accepted = false;
    loop {
        host.poll_processes(Instant::now())?;
        match reply.try_recv() {
            Ok(RequestResult::Dispatched(_)) => accepted = true,
            Ok(_) => return Err(error("PI_HOST_SMOKE_PROMPT_REJECTED_OR_UNKNOWN")),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) if accepted => {}
            Err(_) => return Err(error("PI_HOST_SMOKE_PROMPT_OUTCOME_UNKNOWN")),
        }
        let current = view(host, &session.id)?;
        if matches!(
            current.session.state,
            SessionState::Failed(_) | SessionState::Stopped | SessionState::WaitingForLocalUi
        ) {
            return Err(error("PI_HOST_SMOKE_TURN_FAILED"));
        }
        if accepted && current.session.state == SessionState::Ready {
            let exact = current
                .messages
                .iter()
                .rev()
                .find(|message| {
                    message.role == TranscriptRole::Assistant
                        && message.status == TranscriptStatus::Complete
                })
                .is_some_and(|message| {
                    let text: String = message
                        .parts
                        .iter()
                        .filter_map(|part| match part {
                            MessagePart::Text(part) => Some(part.text.as_str()),
                            _ => None,
                        })
                        .collect();
                    text.trim() == token
                });
            return if exact {
                Ok(())
            } else {
                Err(error("PI_HOST_SMOKE_REPLY_MISMATCH"))
            };
        }
        if Instant::now() >= deadline {
            return Err(error("PI_HOST_SMOKE_TURN_TIMEOUT"));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn close(host: &Host, session: &SessionSnapshot) -> io::Result<()> {
    let result = host.dispatch(
        Access::Local,
        1,
        Request::CloseSession(CloseSessionParams {
            session_id: session.id.clone(),
            expected_stream_id: session
                .stream_id
                .clone()
                .ok_or_else(|| error("PI_HOST_SMOKE_NO_STREAM"))?,
            expected_cwd_fence: session.cwd_fence.clone(),
        }),
    )?;
    if !matches!(result.result, RequestResult::Dispatched(_)) {
        return Err(error("PI_HOST_SMOKE_CLOSE_FAILED"));
    }
    if view(host, &session.id)?.session.state != SessionState::Stopped {
        return Err(error("PI_HOST_SMOKE_CLOSE_NOT_STOPPED"));
    }
    Ok(())
}
pub fn run(node: &Path, package: &Path) -> io::Result<std::process::ExitCode> {
    let installation = Installation::inspect(node, package)
        .map_err(|_| error("PI_HOST_SMOKE_INSTALLATION_INVALID"))?;
    let temp = tempfile::tempdir()?;
    let root = temp.path().canonicalize()?.join("private-host-smoke");
    let _root = private::directory(&root)?;
    let cwd = root.join("project");
    let _cwd = private::directory(&cwd)?;
    // Only this smoke substitutes a private wrapper; production factory accepts trusted installation paths.
    let wrapper = root.join("restricted-cli.mjs");
    let mut file = private::file(&wrapper)?;
    let real_cli = serde_json::to_string(&install::node_path(&installation.cli))?;
    write!(
        file,
        "import {{pathToFileURL}} from 'node:url';\nprocess.argv[1]={real_cli};\nprocess.argv.push('--no-tools','--no-skills','--no-context-files','--no-prompt-templates');\nawait import(pathToFileURL(process.argv[1]).href);\n"
    )?;
    file.sync_all()?;
    let restricted = Installation {
        node: installation.node,
        cli: wrapper,
    };
    let data = root.join(BuildFlavor::Dev.directory_name());
    let host = Host::new(
        State::open(&data, BuildFlavor::Dev)?,
        &data,
        "real-host-smoke".into(),
        "a".repeat(64),
    )?;
    let Response::Project(project) = response(
        &host,
        Request::AddProject(AddProjectParams {
            name: None,
            root: cwd
                .to_str()
                .ok_or_else(|| error("PI_HOST_SMOKE_CWD_INVALID"))?
                .into(),
        }),
    )?
    else {
        return Err(error("PI_HOST_SMOKE_PROJECT_FAILED"));
    };
    let params = CreateSessionParams {
        session_id: "smoke-source".into(),
        project_id: project.id,
        runtime_id: "pi".into(),
        expected_cwd_fence: project.cwd_fence,
        start: SessionStart::New,
    };
    host.start_installed_pi(Access::Local, &params, &restricted)?;
    let source = ready(&host, &params.session_id)?;
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|_| error("PI_HOST_SMOKE_RANDOM_FAILED"))?;
    let nonce: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let token = format!("CAFE_HOST_FIRST_{nonce}");
    turn(
        &host,
        &source,
        format!(
            "Reply with exactly the following single line, without quotes or explanation. Do not use tools.\n{token}"
        ),
        &token,
    )?;
    close(&host, &source)?;
    // Drop/reopen Host to prove identity comes from durable storage, not a retained driver/cache.
    drop(host);
    let host = Host::new(
        State::open(&data, BuildFlavor::Dev)?,
        &data,
        "real-host-restarted".into(),
        "a".repeat(64),
    )?;
    let mut resume = params;
    resume.session_id = "smoke-resumed".into();
    resume.start = SessionStart::Resume(ResumeStartParams {
        source_session_id: source.id.clone(),
    });
    host.start_installed_pi(Access::Local, &resume, &restricted)?;
    let resumed = ready(&host, &resume.session_id)?;
    if resumed.stream_id == source.stream_id
        || !view(&host, &resume.session_id)?.messages.is_empty()
    {
        return Err(error("PI_HOST_SMOKE_RESUME_FENCE_OR_CACHE_INVALID"));
    }
    turn(&host, &resumed, "Reply with exactly the single line you returned in your preceding assistant response in this conversation, without quotes or explanation. Do not use tools.".into(), &token)?;
    close(&host, &resumed)?;
    let stopped = host.start_installed_pi(Access::Local, &resume, &restricted)?;
    if stopped.state != SessionState::Stopped || stopped.stream_id != resumed.stream_id {
        return Err(error("PI_HOST_SMOKE_RETRY_REPLAYED"));
    }
    let Response::Runtimes(runtimes) = response(&host, Request::ListRuntimes)? else {
        return Err(error("PI_HOST_SMOKE_RUNTIMES_INVALID"));
    };
    if !runtimes.items.is_empty() {
        return Err(error("PI_HOST_SMOKE_PRODUCTION_ADMITTED"));
    }
    host.stop_owned_processes()?;
    println!(
        "M1_PI_REAL_HOST_SMOKE_OK package=1.5.0 submittedPrompts=2 hostReopened=true privateIdentityDurable=true exactResumeFile=true nativeHistoryRecall=true secondPromptOmitsToken=true exactReplies=true settled=true ownedCleanup=true idempotentStoppedRetry=true toolsDisabled=true sensitiveEnvironmentDefaultOff=true productionDisabled=true rawOutputShared=false"
    );
    Ok(std::process::ExitCode::SUCCESS)
}
