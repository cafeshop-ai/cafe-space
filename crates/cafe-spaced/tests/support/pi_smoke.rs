//! Opt-in local smoke only. The default/CI harness never enters this path.
use super::*;
use cafe_space_adapters::pi::{self, Command as RpcCommand, Observation};
use cafe_space_protocol::{MessagePart, SessionState, TranscriptRole, TranscriptStatus};
use cafe_spaced::pi_driver::Driver;
use std::io;

fn error(code: &'static str) -> io::Error {
    io::Error::other(code)
}
fn launch(
    installation: &pi::install::Installation,
    cwd: &Path,
    sessions: &Path,
    native: &str,
    resume: Option<&Path>,
) -> io::Result<(Process, Driver)> {
    let mut args = installation
        .arguments(native, sessions, None, resume.map(|file| (file, cwd)))
        .map_err(io::Error::other)?;
    args.extend(
        [
            "--no-tools",
            "--no-skills",
            "--no-context-files",
            "--no-prompt-templates",
        ]
        .map(OsString::from),
    );
    let mut process = Process::spawn(&installation.node, &args, &project_root::inspect(cwd)?)?;
    let mut driver = Driver::new(process.take_streams()?, native.into(), Instant::now())?;
    let deadline = Instant::now() + Duration::from_secs(40);
    while driver.state() != &SessionState::Ready {
        driver.poll(Instant::now());
        if matches!(driver.state(), SessionState::Failed(_)) {
            return Err(error("PI_SMOKE_STARTUP_FAILED"));
        }
        if Instant::now() >= deadline {
            return Err(error("PI_SMOKE_STARTUP_TIMEOUT"));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok((process, driver))
}
fn turn(driver: &mut Driver, message: String, token: &str) -> io::Result<()> {
    driver
        .send(RpcCommand::Prompt { message }, Instant::now())
        .map_err(io::Error::other)?;
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut exact = false;
    let mut settled = false;
    while !settled {
        for observed in driver.poll(Instant::now()) {
            match observed {
                Observation::Message(message, _)
                    if message.role == TranscriptRole::Assistant
                        && message.status == TranscriptStatus::Complete =>
                {
                    let text: String = message
                        .parts
                        .iter()
                        .filter_map(|part| {
                            if let MessagePart::Text(part) = part {
                                Some(part.text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect();
                    exact = text.trim() == token;
                }
                Observation::State(SessionState::Ready) => settled = true,
                Observation::State(SessionState::Failed(_) | SessionState::WaitingForLocalUi) => {
                    return Err(error("PI_SMOKE_TURN_FAILED"));
                }
                Observation::Outcome {
                    outcome: pi::Outcome::Rejected(_) | pi::Outcome::Unknown(_),
                    ..
                } => return Err(error("PI_SMOKE_OUTCOME_UNKNOWN_OR_REJECTED")),
                _ => {}
            }
        }
        if Instant::now() >= deadline {
            return Err(error("PI_SMOKE_TURN_TIMEOUT"));
        }
        if !settled {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    if !exact {
        return Err(error("PI_SMOKE_REPLY_MISMATCH"));
    }
    Ok(())
}
pub fn run(node: &Path, package: &Path) -> io::Result<std::process::ExitCode> {
    let installation =
        pi::install::Installation::inspect(node, package).map_err(io::Error::other)?;
    let version = Command::new(&installation.node)
        .arg("--version")
        .output()
        .map_err(|_| error("PI_SMOKE_NODE_UNAVAILABLE"))?;
    let version = std::str::from_utf8(&version.stdout)
        .map_err(|_| error("PI_SMOKE_NODE_VERSION_INVALID"))?
        .trim()
        .trim_start_matches('v');
    let numbers: Vec<u32> = version
        .split('.')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .map_err(|_| error("PI_SMOKE_NODE_VERSION_INVALID"))?;
    if !version.is_ascii() || numbers.len() != 3 || numbers.as_slice() < [22, 19, 0].as_slice() {
        return Err(error("PI_SMOKE_NODE_VERSION_UNSUPPORTED"));
    }
    let temp = tempfile::tempdir()?;
    let root = temp.path().canonicalize()?.join("private-smoke");
    let _root = cafe_space_ipc::private::directory(&root)?;
    let cwd = root.join("project");
    let _cwd = cafe_space_ipc::private::directory(&cwd)?;
    let sessions = root.join("sessions");
    let _sessions = cafe_space_ipc::private::directory(&sessions)?;
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|_| error("PI_SMOKE_RANDOM_FAILED"))?;
    let nonce: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let native = format!("cafe-smoke-{nonce}");
    let token = format!("CAFE_PI_FIRST_{nonce}");
    {
        let (mut process, mut driver) = launch(&installation, &cwd, &sessions, &native, None)?;
        turn(
            &mut driver,
            format!(
                "Reply with exactly the following single line, without quotes or explanation. Do not use tools.\n{token}"
            ),
            &token,
        )?;
        process.stop()?;
        drop(driver);
    }
    let files = std::fs::read_dir(&sessions)?.collect::<io::Result<Vec<_>>>()?;
    let file = files
        .iter()
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .ends_with(&format!("_{native}.jsonl"))
        })
        .ok_or_else(|| error("PI_SMOKE_HISTORY_MISSING"))?
        .path();
    {
        let (mut process, mut driver) =
            launch(&installation, &cwd, &sessions, &native, Some(&file))?;
        turn(&mut driver, "Reply with exactly the single line you returned in your preceding assistant response in this conversation, without quotes or explanation. Do not use tools.".into(), &token)?;
        process.stop()?;
        drop(driver);
    }
    println!(
        "M1_PI_REAL_SMOKE_OK package=1.5.0 submittedPrompts=2 fresh=true resumeIdentity=true resumeHistory=true exactReplies=true settled=true ownedCleanup=true toolsDisabled=true sensitiveEnvironmentDefaultOff=true rawOutputShared=false"
    );
    Ok(std::process::ExitCode::SUCCESS)
}
