use super::{Action, checked, templates};
use crate::{BuildFlavor, windows_security::current_sid};
use std::{io, path::Path, process::Command};

// Fixed scripts: paths, SID, XML and action are data in the child environment, not PowerShell source.
const TASK_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$service = New-Object -ComObject 'Schedule.Service'
$service.Connect()
$folder = $service.GetFolder('\')
$task = $null
try { $task = $folder.GetTask($env:CAFE_STARTUP_NAME) }
catch {
    # PowerShell maps HRESULT_FILE_NOT_FOUND to FileNotFoundException, not COMException.
    if ($_.Exception.HResult -ne -2147024894) { throw }
}
if ($env:CAFE_STARTUP_ACTION -eq 'install') {
    if ($null -ne $task) { throw 'Task already exists; no overwrite permitted' }
    $definition = $service.NewTask(0)
    $definition.XmlText = $env:CAFE_STARTUP_XML
    $null = $folder.RegisterTaskDefinition($env:CAFE_STARTUP_NAME, $definition, 2, $env:CAFE_STARTUP_SID, $null, 3, $null)
    exit 0
}
if ($null -eq $task) { throw 'Task is not installed' }
$definition = $task.Definition
# The scheduler may canonicalize an input SID to an account name on registration.
$principalSid = $definition.Principal.UserId
if (-not $principalSid.StartsWith('S-1-')) {
    $account = New-Object System.Security.Principal.NTAccount($principalSid)
    $principalSid = $account.Translate([System.Security.Principal.SecurityIdentifier]).Value
}
# Fixed exit codes reveal which invariant failed without logging identities, paths or XML.
if ($definition.RegistrationInfo.Description -cne $env:CAFE_STARTUP_DESCRIPTION) { exit 20 }
if ($definition.Actions.Count -ne 1) { exit 21 }
if ($definition.Actions.Item(1).Path -cne $env:CAFE_STARTUP_PROGRAM) { exit 22 }
if ([string]$definition.Actions.Item(1).Arguments -ne '') { exit 23 }
if ($principalSid -cne $env:CAFE_STARTUP_SID) { exit 24 }
if ($definition.Principal.LogonType -ne 3) { exit 25 }
if ($definition.Principal.RunLevel -ne 0) { exit 26 }
switch ($env:CAFE_STARTUP_ACTION) {
    'verify' { }
    'start' { $null = $task.Run($null) }
    'stop' {
        if ($task.State -eq 4 -or $task.State -eq 2) { $task.Stop(0) }
        $deadline = [DateTime]::UtcNow.AddSeconds(15)
        while ($task.State -eq 4 -or $task.State -eq 2) {
            if ([DateTime]::UtcNow -ge $deadline) { throw 'Task stop timed out; inspect before retrying' }
            Start-Sleep -Milliseconds 50
        }
    }
    'uninstall' {
        if ($task.State -eq 4 -or $task.State -eq 2) { $task.Stop(0) }
        $folder.DeleteTask($env:CAFE_STARTUP_NAME, 0)
    }
    default { throw 'Unknown action' }
}
exit 0
"#;

const RUN_KEY_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$path = 'Software\Microsoft\Windows\CurrentVersion\Run'
$key = $null
try {
    if ($env:CAFE_STARTUP_ACTION -eq 'install') { $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($path, $true) }
    else { $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($path, $true) }
    if ($null -eq $key) { throw 'Run key is not installed' }
    $old = $key.GetValue($env:CAFE_STARTUP_NAME, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    $expected = '"' + $env:CAFE_STARTUP_PROGRAM + '"'
    if ($env:CAFE_STARTUP_ACTION -eq 'install') {
        if ($null -ne $old) { throw 'Run value already exists; no overwrite permitted' }
        $key.SetValue($env:CAFE_STARTUP_NAME, $expected, [Microsoft.Win32.RegistryValueKind]::String)
    } else {
        if ($old -cne $expected -or $key.GetValueKind($env:CAFE_STARTUP_NAME) -ne [Microsoft.Win32.RegistryValueKind]::String) {
            throw 'Run value differs; refusing to remove it'
        }
        $key.DeleteValue($env:CAFE_STARTUP_NAME, $true)
    }
} finally { if ($null -ne $key) { $key.Dispose() } }
exit 0
"#;

fn powershell(script: &str) -> io::Result<Command> {
    let windows = std::env::var_os("SystemRoot")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "SystemRoot unavailable"))?;
    let mut command =
        Command::new(Path::new(&windows).join("System32/WindowsPowerShell/v1.0/powershell.exe"));
    command.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        script,
    ]);
    Ok(command)
}

pub(super) fn execute(
    flavor: BuildFlavor,
    root: &Path,
    program: &Path,
    action: Action,
) -> io::Result<()> {
    let sid = current_sid()?;
    let xml = templates::windows_task(flavor, program, &sid)?;
    let mut command = powershell(TASK_SCRIPT)?;
    command
        .env(
            "CAFE_STARTUP_NAME",
            format!("{}-{sid}", flavor.daemon_name()),
        )
        .env(
            "CAFE_STARTUP_DESCRIPTION",
            format!("Cafe Space managed startup v1: {}", flavor.daemon_name()),
        )
        .env("CAFE_STARTUP_SID", sid)
        .env("CAFE_STARTUP_PROGRAM", program)
        .env("CAFE_STARTUP_XML", xml)
        .env(
            "CAFE_STARTUP_ACTION",
            match action {
                Action::Install => "install",
                Action::Uninstall => "uninstall",
                Action::Start => "start",
                Action::Stop => "stop",
                Action::Verify => "verify",
            },
        );
    if matches!(action, Action::Stop | Action::Uninstall) {
        command.env("CAFE_STARTUP_ACTION", "verify");
        checked(&mut command)?;
        let root = root.to_owned();
        // A separate thread also permits synchronous startup calls made from an async runtime.
        let normal = std::thread::Builder::new()
            .name("cafe-owned-stop".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| "INSTALL_STOP_RUNTIME_FAILED")?;
                runtime.block_on(crate::updater::request_shutdown(flavor, &root))
            })?
            .join()
            .map_err(|_| {
                io::Error::other("normal shutdown worker failed; inspect before retrying")
            })?;
        if let Err(code) = normal {
            use std::io::Write;
            let _ = writeln!(std::io::stderr(), "{code} hard_stop_fallback=true");
        }
        command.env(
            "CAFE_STARTUP_ACTION",
            if action == Action::Stop {
                "stop"
            } else {
                "uninstall"
            },
        );
    }
    checked(&mut command)
}

/// Opt-in fallback only: never switch backend automatically after a possibly successful task write.
pub(super) fn run_key(flavor: BuildFlavor, program: &Path, install: bool) -> io::Result<()> {
    if templates::path_text(program)?.contains('"')
        || program.as_os_str().to_string_lossy().encode_utf16().count() + 2 > 260
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Run key executable path contains quotes or exceeds the 260-character command limit",
        ));
    }
    let mut command = powershell(RUN_KEY_SCRIPT)?;
    command
        .env("CAFE_STARTUP_NAME", flavor.daemon_name())
        .env("CAFE_STARTUP_PROGRAM", program)
        .env(
            "CAFE_STARTUP_ACTION",
            if install { "install" } else { "uninstall" },
        );
    checked(&mut command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_task_parser_accepts_definition_without_registering_it() {
        let xml = templates::windows_task(
            BuildFlavor::Dev,
            &std::env::current_exe().unwrap(),
            &current_sid().unwrap(),
        )
        .unwrap();
        let script = r#"
$ErrorActionPreference = 'Stop'
$tokens = $null; $errors = $null
$null = [System.Management.Automation.Language.Parser]::ParseInput($env:CAFE_STARTUP_TASK_SCRIPT, [ref]$tokens, [ref]$errors)
if ($errors.Count -ne 0) { throw 'Invalid task script syntax' }
$null = [System.Management.Automation.Language.Parser]::ParseInput($env:CAFE_STARTUP_RUN_SCRIPT, [ref]$tokens, [ref]$errors)
if ($errors.Count -ne 0) { throw 'Invalid Run key script syntax' }
$service = New-Object -ComObject 'Schedule.Service'; $service.Connect()
$definition = $service.NewTask(0)
$definition.XmlText = $env:CAFE_STARTUP_XML
if ($definition.Actions.Count -ne 1 -or $definition.Principal.LogonType -ne 3) { throw 'Invalid parsed task' }
"#;
        let mut command = powershell(script).unwrap();
        command
            .env("CAFE_STARTUP_XML", xml)
            .env("CAFE_STARTUP_TASK_SCRIPT", TASK_SCRIPT)
            .env("CAFE_STARTUP_RUN_SCRIPT", RUN_KEY_SCRIPT);
        checked(&mut command).unwrap();
    }

    #[test]
    fn missing_task_is_handled_without_registering_anything() {
        let prefix = TASK_SCRIPT
            .split("if ($env:CAFE_STARTUP_ACTION")
            .next()
            .unwrap();
        let script = format!(
            "{prefix}\nif ($null -ne $task) {{ throw 'Unexpected existing task' }}\nexit 0"
        );
        let mut command = powershell(&script).unwrap();
        command.env(
            "CAFE_STARTUP_NAME",
            format!(
                "cafe-space-readonly-missing-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ),
        );
        checked(&mut command).unwrap();
    }

    #[test]
    fn task_validation_accepts_owner_account_name_or_sid_without_registering() {
        let sid = current_sid().unwrap();
        let program = std::env::current_exe().unwrap();
        let xml = templates::windows_task(BuildFlavor::Dev, &program, &sid).unwrap();
        let validation = TASK_SCRIPT
            .split_once("$definition = $task.Definition")
            .unwrap()
            .1
            .split_once("switch ($env:CAFE_STARTUP_ACTION)")
            .unwrap()
            .0;
        let script = format!(
            r#"
$ErrorActionPreference = 'Stop'
$service = New-Object -ComObject 'Schedule.Service'; $service.Connect()
$definition = $service.NewTask(0); $definition.XmlText = $env:CAFE_STARTUP_XML
foreach ($identity in @($env:CAFE_STARTUP_SID, [System.Security.Principal.WindowsIdentity]::GetCurrent().Name)) {{
    $definition.Principal.UserId = $identity
    {validation}
}}
exit 0
"#
        );
        let mut command = powershell(&script).unwrap();
        command
            .env("CAFE_STARTUP_XML", xml)
            .env("CAFE_STARTUP_SID", sid)
            .env("CAFE_STARTUP_PROGRAM", program)
            .env(
                "CAFE_STARTUP_DESCRIPTION",
                "Cafe Space managed startup v1: cafe-spaced-dev",
            );
        checked(&mut command).unwrap();
    }

    #[test]
    fn scripts_have_no_user_text_interpolation_or_automatic_fallback() {
        let command = powershell(TASK_SCRIPT).unwrap();
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args.last().unwrap(), &TASK_SCRIPT);
        assert!(
            TASK_SCRIPT.contains("RegisterTaskDefinition($env:CAFE_STARTUP_NAME, $definition, 2,")
        );
        assert!(TASK_SCRIPT.contains("no overwrite permitted"));
        assert!(!TASK_SCRIPT.contains("Invoke-Expression"));
        assert!(RUN_KEY_SCRIPT.contains("CurrentUser"));
        assert!(RUN_KEY_SCRIPT.contains("DoNotExpandEnvironmentNames"));
    }
}
