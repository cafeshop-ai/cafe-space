use cafe_space_ipc::{BuildFlavor, client::Client};
use cafe_space_protocol::*;
use std::{
    ffi::OsString,
    io::{self, BufRead, IsTerminal, Write},
};

const HELP: &str = "recovery status [--json]\nrecovery restore\nRestore requires confirmation in a terminal. Unknown outcomes must be checked with recovery status, never replayed.";

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Command {
    Status { json: bool },
    Restore,
}

pub(super) fn parse(args: &[OsString]) -> io::Result<Option<Command>> {
    match args {
        [arg] if arg == "--help" || arg == "-h" => Ok(None),
        [arg] if arg == "status" => Ok(Some(Command::Status { json: false })),
        [arg, flag] if arg == "status" && flag == "--json" => {
            Ok(Some(Command::Status { json: true }))
        }
        [arg] if arg == "restore" => Ok(Some(Command::Restore)),
        _ => Err(io::Error::other(HELP)),
    }
}

fn confirm(mut input: impl BufRead, mut output: impl Write) -> io::Result<bool> {
    write!(
        output,
        "恢复会保留失败的数据和原备份，但会把当前状态换回升级前。确认后仍需回到上一版。\n输入 restore 确认，其他输入取消："
    )?;
    output.flush()?;
    let mut answer = String::new();
    std::io::Read::take(&mut input, 64).read_line(&mut answer)?;
    Ok(matches!(answer.as_str(), "restore\n" | "restore\r\n"))
}

pub(super) async fn run(flavor: BuildFlavor, command: Option<Command>) -> io::Result<()> {
    let Some(command) = command else {
        println!("{HELP}");
        return Ok(());
    };
    let root = flavor
        .data_dir()
        .ok_or_else(|| io::Error::other("INSTALL_PATH_UNAVAILABLE"))?;
    let (mut client, health) = Client::connect(flavor, &root)
        .await
        .map_err(io::Error::other)?;
    let details = if health.recovery.is_some() {
        Some(
            client
                .request_outcome(Request::GetRecoveryDetails)
                .await
                .map_err(io::Error::other)?,
        )
    } else {
        None
    };
    if let Command::Status { json } = command {
        println!(
            "{}",
            super::output::json(
                &RequestResult::Applied(Box::new(AppliedResult {
                    result: Response::Health(health)
                })),
                json
            )?
        );
        if let Some(details) = details {
            println!("{}", super::output::json(&details, json)?);
        }
        return Ok(());
    }
    let recovery = health
        .recovery
        .ok_or_else(|| io::Error::other("RESTORE_NOT_AVAILABLE"))?;
    if recovery.code != RecoveryCode::SchemaMigrationFailed || !recovery.backup {
        return Err(io::Error::other("RESTORE_NOT_AVAILABLE"));
    }
    let Some(RequestResult::Applied(applied)) = &details else {
        return Err(io::Error::other("RECOVERY_DETAILS_UNAVAILABLE"));
    };
    let Response::RecoveryDetails(files) = &applied.result else {
        return Err(io::Error::other("RECOVERY_DETAILS_UNAVAILABLE"));
    };
    let Some(backup) = &files.backup_file else {
        return Err(io::Error::other("RESTORE_NOT_AVAILABLE"));
    };
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other("CLI_CONFIRMATION_REQUIRES_TERMINAL"));
    }
    println!(
        "数据目录：{}\n恢复来源：{}\n恢复 schema：{}",
        files.data_directory, backup, recovery.from_schema
    );
    if !confirm(io::stdin().lock(), io::stdout().lock())? {
        return Err(io::Error::other("RESTORE_CANCELLED"));
    }
    // Confirmation may take minutes; reject a changed recovery episode rather than guess its backup.
    let current = client
        .request(Request::GetStatus)
        .await
        .map_err(io::Error::other)?;
    if !matches!(current, Response::Health(h) if h.recovery.as_ref() == Some(&recovery))
        || client
            .request_outcome(Request::GetRecoveryDetails)
            .await
            .map_err(io::Error::other)?
            != details.unwrap()
    {
        return Err(io::Error::other("RECOVERY_CHANGED"));
    }
    let result = client
        .request_outcome(Request::RestoreBackup(RestoreBackupParams {
            confirm: true,
            code: recovery.code,
        }))
        .await
        .unwrap_or_else(|_| {
            RequestResult::Unknown(ErrorResult {
                error: ApiError {
                    code: "RECOVERY_OUTCOME_UNKNOWN".into(),
                    message: "Outcome unknown; read recovery status without retrying.".into(),
                },
            })
        });
    println!("{}", super::output::json(&result, false)?);
    match result {
        RequestResult::Applied(value) if value.result == Response::Empty => {
            println!(
                "备份已恢复；未重新迁移。请回到上一版守护进程（有交接记录时：{} update rollback）。",
                flavor.cli_name()
            );
            Ok(())
        }
        RequestResult::Rejected(value) | RequestResult::Unknown(value) => {
            Err(io::Error::other(value.error.code))
        }
        _ => Err(io::Error::other("RECOVERY_OUTCOME_UNKNOWN")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_exact_recovery_commands_are_accepted() {
        let args = |words: &[&str]| words.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(
            parse(&args(&["status"])).unwrap(),
            Some(Command::Status { json: false })
        );
        assert_eq!(
            parse(&args(&["status", "--json"])).unwrap(),
            Some(Command::Status { json: true })
        );
        assert_eq!(parse(&args(&["restore"])).unwrap(), Some(Command::Restore));
        for words in [
            &[][..],
            &["restore", "--yes"],
            &["restore", "/backup"],
            &["status", "--force"],
        ] {
            assert!(parse(&args(words)).is_err());
        }
    }
    #[test]
    fn confirmation_is_explicit_bounded_and_cancellation_never_restores() {
        for text in ["restore\n", "restore\r\n"] {
            assert!(confirm(io::Cursor::new(text), Vec::new()).unwrap());
        }
        for text in [
            "",
            "yes\n",
            "restore",
            "restore extra\n",
            "\n",
            &"x".repeat(1000),
        ] {
            assert!(!confirm(io::Cursor::new(text), Vec::new()).unwrap());
        }
    }
}
