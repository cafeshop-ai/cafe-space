use cafe_space_ipc::{
    BuildFlavor,
    startup::{self, Action},
};
use std::{ffi::OsString, io};

pub fn run(flavor: BuildFlavor) -> io::Result<()> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match parse(&args)? {
        None => {
            println!(
                "{} autostart <install|uninstall|start|stop>\nExplicit per-user login registration; install requires the daemon in the private data directory's bin directory.\nInstall does not start it now. Uninstall stops the daemon; stored data is preserved.",
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
                Command::Native(action) => startup::execute(flavor, action)?,
                #[cfg(windows)]
                Command::RunKey(install) => startup::run_key(flavor, install)?,
            }
            println!("Startup operation completed. No session commands were replayed.");
            Ok(())
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum Command {
    Native(Action),
    #[cfg(windows)]
    RunKey(bool),
}

fn parse(args: &[OsString]) -> io::Result<Option<Command>> {
    if args.is_empty() || args.len() == 1 && matches!(args[0].to_str(), Some("--help" | "-h")) {
        return Ok(None);
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
        "expected autostart install|uninstall|start|stop; use --help",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
