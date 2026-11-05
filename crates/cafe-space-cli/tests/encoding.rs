//! Cross-process/shell checks of the exact production parser and output codec; never connects a daemon.
// harness=false includes cfg(test) helpers but does not run their libtest functions.
#[allow(dead_code, unused_imports)]
#[path = "../src/input.rs"]
mod input;
#[path = "../src/output.rs"]
mod output;
#[allow(dead_code, unused_imports)]
#[path = "../src/sessions.rs"]
mod sessions;
use cafe_space_protocol::*;
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn invoke(mut command: Command, bytes: Option<&[u8]>, directory: &Path) -> String {
    let stdout = directory.join("stdout.json");
    let stderr = directory.join("stderr.txt");
    command
        .stdout(std::fs::File::create(&stdout).unwrap())
        .stderr(std::fs::File::create(&stderr).unwrap());
    command.stdin(if bytes.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let mut child = command.spawn().expect("start encoding fixture process");
    if let Some(bytes) = bytes {
        child.stdin.take().unwrap().write_all(bytes).unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(
                status.success(),
                "encoding process failed: {}",
                String::from_utf8_lossy(&std::fs::read(&stderr).unwrap())
            );
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("encoding process deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let bytes = std::fs::read(stdout).unwrap();
    assert!(
        bytes.is_ascii(),
        "--json stdout must be ASCII even through legacy code pages"
    );
    let RequestResult::Rejected(value) = serde_json::from_slice::<RequestResult>(&bytes).unwrap()
    else {
        panic!("encoding result expected")
    };
    assert_eq!(value.error.code, "CLI_ENCODING_FIXTURE");
    value.error.message
}
fn probe(program: &Path) -> Command {
    let mut command = Command::new(program);
    command.args([
        "--probe",
        "prompt",
        "session-fixture",
        "stream-fixture",
        "cwd-fixture",
    ]);
    command
}
fn main() {
    // Keep the production execution code typechecked here, but never execute an IPC connection.
    let _ = sessions::run;
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|arg| arg == "--probe") {
        let command = sessions::parse(&args[1..]).unwrap().unwrap();
        assert!(command.json);
        let Request::Prompt(params) = command.request else {
            panic!("prompt expected")
        };
        let result = RequestResult::Rejected(ErrorResult {
            error: ApiError {
                code: "CLI_ENCODING_FIXTURE".into(),
                message: params.content,
            },
        });
        println!("{}", output::json(&result, true).unwrap());
        return;
    }
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/cli/session.json")).unwrap();
    let text = fixture["text"].as_str().unwrap();
    let argument = fixture["argumentText"].as_str().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let program = std::env::current_exe().unwrap();
    let mut command = probe(&program);
    command.args(["--text", text, "--json"]);
    assert_eq!(invoke(command, None, root), text);
    let mut variants = vec![
        text.as_bytes().to_vec(),
        [vec![0xef, 0xbb, 0xbf], text.as_bytes().to_vec()].concat(),
    ];
    for little in [true, false] {
        let mut bytes = if little {
            vec![0xff, 0xfe]
        } else {
            vec![0xfe, 0xff]
        };
        for unit in text.encode_utf16() {
            bytes.extend(if little {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        variants.push(bytes);
    }
    for (index, bytes) in variants.iter().enumerate() {
        let path = root.join(format!("input-{index}.txt"));
        std::fs::write(&path, bytes).unwrap();
        let mut command = probe(&program);
        command.arg("--file").arg(&path).arg("--json");
        assert_eq!(invoke(command, None, root), text);
        let mut command = probe(&program);
        command.args(["--file", "-", "--json"]);
        assert_eq!(invoke(command, Some(bytes), root), text);
    }
    println!(
        "M1_CLI_ENCODING_OK shell=native input=argument,utf8,utf8bom,utf16le,utf16be,stdin asciiJson=true daemonConnections=0"
    );
    #[cfg(unix)]
    {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "exec \"$@\"", "fixture"])
            .arg(&program)
            .args([
                "--probe", "prompt", "id", "stream", "cwd", "--text", argument, "--json",
            ]);
        assert_eq!(invoke(command, None, root), argument);
        println!(
            "M1_CLI_ENCODING_OK shell=posix input=argument asciiJson=true daemonConnections=0"
        );
    }
    #[cfg(windows)]
    {
        let batch = root.join("encoding.cmd");
        std::fs::write(&batch, "@echo off\r\nchcp 936 >nul\r\n\"%CAFE_CLI_PROBE%\" --probe prompt id stream cwd --text \"%CAFE_CLI_TEXT%\" --json\r\nexit /b %errorlevel%\r\n").unwrap();
        let mut command = Command::new("cmd.exe");
        command
            .args(["/D", "/V:OFF", "/C"])
            .arg(&batch)
            .env("CAFE_CLI_PROBE", &program)
            .env("CAFE_CLI_TEXT", argument);
        assert_eq!(invoke(command, None, root), argument);
        std::fs::write(&batch, "@echo off\r\nchcp 936 >nul\r\n\"%CAFE_CLI_PROBE%\" --probe prompt id stream cwd --file \"%CAFE_CLI_FILE%\" --json\r\nexit /b %errorlevel%\r\n").unwrap();
        for index in 0..variants.len() {
            let mut command = Command::new("cmd.exe");
            command
                .args(["/D", "/V:OFF", "/C"])
                .arg(&batch)
                .env("CAFE_CLI_PROBE", &program)
                .env("CAFE_CLI_FILE", root.join(format!("input-{index}.txt")));
            assert_eq!(invoke(command, None, root), text);
        }
        println!(
            "M1_CLI_ENCODING_OK shell=cmd936 input=argument,utf8,utf8bom,utf16le,utf16be asciiJson=true daemonConnections=0"
        );
        let script = root.join("encoding.ps1");
        std::fs::write(&script, "param([string]$Probe,[string]$InputFile,[string]$Kind)\n$ErrorActionPreference='Stop'\nif ($Kind -eq 'text') { $text=[IO.File]::ReadAllText($InputFile,[Text.UTF8Encoding]::new($false,$true)); & $Probe --probe prompt id stream cwd --text $text --json } else { & $Probe --probe prompt id stream cwd --file $InputFile --json }; exit $LASTEXITCODE\n").unwrap();
        let arg_file = root.join("argument.txt");
        std::fs::write(&arg_file, argument).unwrap();
        for shell in ["powershell.exe", "pwsh.exe"] {
            let available = Command::new(shell)
                .args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "exit 0",
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            if available
                .as_ref()
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
                && std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
            {
                println!(
                    "M1_CLI_ENCODING_SKIPPED shell={shell} reason=notInstalled requiredInCI=true"
                );
                continue;
            }
            assert!(available.unwrap().success(), "required shell unavailable");
            for (kind, file) in std::iter::once(("text", arg_file.clone())).chain(
                (0..variants.len()).map(|index| ("file", root.join(format!("input-{index}.txt")))),
            ) {
                let mut command = Command::new(shell);
                command
                    .args([
                        "-NoLogo",
                        "-NoProfile",
                        "-NonInteractive",
                        "-ExecutionPolicy",
                        "Bypass",
                        "-File",
                    ])
                    .arg(&script)
                    .arg(&program)
                    .arg(file)
                    .arg(kind);
                assert_eq!(
                    invoke(command, None, root),
                    if kind == "text" { argument } else { text }
                );
            }
            println!(
                "M1_CLI_ENCODING_OK shell={shell} input=argument,utf8,utf8bom,utf16le,utf16be asciiJson=true daemonConnections=0"
            );
        }
    }
}
