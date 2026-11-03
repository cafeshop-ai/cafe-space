//! Deterministic OS definitions, tested on every host without installing anything.

use crate::BuildFlavor;
use std::{io, path::Path};

pub(super) fn path_text(path: &Path) -> io::Result<&str> {
    let text = path
        .to_str()
        .filter(|s| !s.chars().any(char::is_control))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "startup paths must be Unicode without control characters",
            )
        })?;
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "startup path must be absolute",
        ));
    }
    Ok(text)
}

#[cfg(any(test, target_os = "macos", windows))]
fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(any(test, target_os = "linux"))]
pub(super) fn systemd(flavor: BuildFlavor, program: &Path) -> io::Result<String> {
    // ExecStart is not a shell: double %/$ to disable systemd specifier/env expansion.
    let program = path_text(program)?
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%")
        .replace('$', "$$");
    Ok(format!(
        "# Cafe Space managed startup v1\n[Unit]\nDescription={}\n[Service]\nType=simple\nExecStart=\"{program}\"\nRestart=no\nKillMode=mixed\nUMask=0077\n[Install]\nWantedBy=default.target\n",
        flavor.daemon_name()
    ))
}

#[cfg(any(test, target_os = "macos"))]
pub(super) fn launchd(flavor: BuildFlavor, program: &Path) -> io::Result<String> {
    let program = xml(path_text(program)?);
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!-- Cafe Space managed startup v1 -->\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>{}</string>\n<key>ProgramArguments</key><array><string>{program}</string></array>\n<key>RunAtLoad</key><true/>\n<key>KeepAlive</key><false/>\n<key>Umask</key><integer>63</integer>\n</dict></plist>\n",
        flavor.service_label()
    ))
}

#[cfg(any(test, windows))]
pub(super) fn windows_task(flavor: BuildFlavor, program: &Path, sid: &str) -> io::Result<String> {
    if !sid.starts_with("S-1-")
        || !sid
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'S' || b == b'-')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid user SID",
        ));
    }
    let program = xml(path_text(program)?);
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\n<Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\n<RegistrationInfo><Description>Cafe Space managed startup v1: {}</Description></RegistrationInfo>\n<Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{sid}</UserId></LogonTrigger></Triggers>\n<Principals><Principal id=\"Owner\"><UserId>{sid}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>\n<Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><AllowHardTerminate>true</AllowHardTerminate><StartWhenAvailable>true</StartWhenAvailable><RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable><Enabled>true</Enabled><Hidden>true</Hidden><ExecutionTimeLimit>PT0S</ExecutionTimeLimit></Settings>\n<Actions Context=\"Owner\"><Exec><Command>{program}</Command></Exec></Actions>\n</Task>\n",
        flavor.daemon_name()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tricky_path() -> std::path::PathBuf {
        std::env::temp_dir()
            .join("中文 & <'\"%$ dir")
            .join("cafe-spaced-dev")
    }

    #[test]
    fn definitions_preserve_paths_and_do_not_restart_old_bytes() {
        let path = tricky_path();
        let unit = systemd(BuildFlavor::Dev, &path).unwrap();
        assert!(unit.contains("Restart=no\n"));
        assert!(unit.contains("KillMode=mixed\n"));
        assert!(unit.contains("%%$$ dir"));
        assert!(unit.contains("\\\""));
        assert!(!unit.contains("User="));
        let plist = launchd(BuildFlavor::Dev, &path).unwrap();
        assert!(plist.contains("ai.cafeshop.cafe-spaced-dev"));
        assert!(plist.contains("<key>KeepAlive</key><false/>"));
        assert!(plist.contains("中文 &amp; &lt;&apos;&quot;%$ dir"));
        let task = windows_task(BuildFlavor::Dev, &path, "S-1-5-21-123").unwrap();
        assert!(task.contains("<LogonType>InteractiveToken</LogonType>"));
        assert!(task.contains("<RunLevel>LeastPrivilege</RunLevel>"));
        assert!(task.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(!task.contains("RestartOnFailure"));
        assert_eq!(task.matches("<UserId>S-1-5-21-123</UserId>").count(), 2);
        assert!(task.contains("中文 &amp; &lt;&apos;&quot;%$ dir"));
    }

    #[test]
    fn rejects_invalid_paths_and_sids() {
        for path in [Path::new("relative"), Path::new("/tmp/line\nbreak")] {
            assert!(systemd(BuildFlavor::Release, path).is_err());
            assert!(launchd(BuildFlavor::Release, path).is_err());
        }
        assert!(windows_task(BuildFlavor::Dev, &tricky_path(), "S-1-<bad>").is_err());
    }
}
