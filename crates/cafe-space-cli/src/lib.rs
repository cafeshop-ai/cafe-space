mod projects;

use cafe_space_ipc::{
    BuildFlavor,
    client::Client,
    installation::{Handover, HandoverPhase},
    startup::{self, Action},
    updater::{self, UpdateRecoveryAction},
};
use std::{ffi::OsString, io};

pub fn run(flavor: BuildFlavor) -> io::Result<()> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "project") {
        let request = projects::parse(&args[1..])?;
        return tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(projects::run(flavor, request));
    }
    match parse(&args)? {
        None => {
            println!(
                "{0} autostart <install|uninstall|start|stop>\n{0} status\n{0} update <finish|rollback>\n{0} project list|add|update|remove|root (use project --help)\nExplicit per-user login registration; install requires the daemon in the private data directory's bin directory.\nInstall does not start it now. Uninstall stops the daemon; stored data is preserved.\nUpdate recovery is explicit, never replaces SQLite and never replays session commands.",
                flavor.cli_name()
            );
            #[cfg(windows)]
            println!(
                "Windows fallback: autostart <install-run-key|uninstall-run-key> (explicit only; not automatic)."
            );
            Ok(())
        }
        Some(command) => {
            match command {
                Command::Status => return run_async(show_status(flavor)),
                Command::Recover(action) => {
                    run_async(updater::recover(flavor, action))?;
                    println!(
                        "{}",
                        if action == UpdateRecoveryAction::Finish {
                            "更新已完成。"
                        } else {
                            "已回到上一版；数据库未被覆盖。"
                        }
                    );
                    return Ok(());
                }
                Command::Native(action) => startup::execute(flavor, action)?,
                #[cfg(windows)]
                Command::RunKey(install) => startup::run_key(flavor, install)?,
            }
            println!("Startup operation completed. No session commands were replayed.");
            Ok(())
        }
    }
}

fn run_async(future: impl std::future::Future<Output = updater::Result<()>>) -> io::Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(future)
        .map_err(io::Error::other)
}

async fn show_status(flavor: BuildFlavor) -> updater::Result<()> {
    let root = flavor.data_dir().ok_or("INSTALL_PATH_UNAVAILABLE")?;
    if let Some(marker) = Handover::read(&root).map_err(|_| "INSTALL_MARKER_INVALID")?
        && !updater::terminal(marker.phase)
    {
        if marker.phase == HandoverPhase::RecoveryRequired {
            println!(
                "需要明确恢复。涉及 state.sqlite3、handover.json 和 bin 下的 .prev 文件；不会自动降级或覆盖数据。\n旧版必须先确认能读取当前数据。由你选择恢复：{} update rollback",
                flavor.cli_name()
            );
        } else {
            println!(
                "更新未完成，守护进程尚未恢复接收新工作。\n完成更新：{0} update finish\n回到上一版：{0} update rollback",
                flavor.cli_name()
            );
        }
        return Ok(());
    }
    let (_, health) = Client::connect(flavor, &root).await?;
    if health.awaiting_update_confirmation == Some(true) {
        println!("更新未完成；请先检查交接标记。")
    } else if health.update == cafe_space_ipc::updater::UpdateState::Pending {
        println!("更新等待空闲；当前工作会继续。")
    } else if health.update == cafe_space_ipc::updater::UpdateState::Draining {
        println!("守护进程正在交接，暂不接受新工作。");
    } else if health.ready {
        println!("本机守护进程已就绪。")
    } else {
        println!("守护进程尚未就绪。")
    }
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
enum Command {
    Status,
    Recover(UpdateRecoveryAction),
    Native(Action),
    #[cfg(windows)]
    RunKey(bool),
}

fn parse(args: &[OsString]) -> io::Result<Option<Command>> {
    if args.is_empty() || args.len() == 1 && matches!(args[0].to_str(), Some("--help" | "-h")) {
        return Ok(None);
    }
    if args.len() == 1 && args[0] == "status" {
        return Ok(Some(Command::Status));
    }
    if args.len() == 2 && args[0] == "update" {
        return match args[1].to_str() {
            Some("finish") => Ok(Some(Command::Recover(UpdateRecoveryAction::Finish))),
            Some("rollback") => Ok(Some(Command::Recover(UpdateRecoveryAction::Rollback))),
            _ => Err(usage()),
        };
    }
    if args.len() != 2 || args[0] != "autostart" {
        return Err(usage());
    }
    let command = match args[1].to_str() {
        Some("install") => Command::Native(Action::Install),
        Some("uninstall") => Command::Native(Action::Uninstall),
        Some("start") => Command::Native(Action::Start),
        Some("stop") => Command::Native(Action::Stop),
        #[cfg(windows)]
        Some("install-run-key") => Command::RunKey(true),
        #[cfg(windows)]
        Some("uninstall-run-key") => Command::RunKey(false),
        _ => return Err(usage()),
    };
    Ok(Some(command))
}

fn usage() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "expected status, update finish|rollback or autostart install|uninstall|start|stop; use --help",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_recovery_requires_an_explicit_direction() {
        let args = |words: &[&str]| words.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(parse(&args(&["status"])).unwrap(), Some(Command::Status));
        for (word, action) in [
            ("finish", UpdateRecoveryAction::Finish),
            ("rollback", UpdateRecoveryAction::Rollback),
        ] {
            assert_eq!(
                parse(&args(&["update", word])).unwrap(),
                Some(Command::Recover(action))
            );
        }
        for words in [
            &["update"][..],
            &["update", "auto"],
            &["update", "rollback", "--force"],
            &["status", "--force"],
        ] {
            assert!(parse(&args(words)).is_err());
        }
    }

    #[test]
    fn only_explicit_exact_commands_can_register_startup() {
        let args = |words: &[&str]| words.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(parse(&[]).unwrap(), None);
        assert_eq!(parse(&args(&["--help"])).unwrap(), None);
        for (word, action) in [
            ("install", Action::Install),
            ("uninstall", Action::Uninstall),
            ("start", Action::Start),
            ("stop", Action::Stop),
        ] {
            assert_eq!(
                parse(&args(&["autostart", word])).unwrap(),
                Some(Command::Native(action))
            );
        }
        for words in [
            &["install"][..],
            &["autostart"],
            &["autostart", "install", "--force"],
            &["autostart", "wat"],
        ] {
            assert!(parse(&args(words)).is_err());
        }
    }
}
