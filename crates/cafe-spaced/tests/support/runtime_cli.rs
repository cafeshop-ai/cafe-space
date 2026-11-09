//! Real CLI/daemon registration on disposable CI; never launches an Agent or changes login startup.
use super::*;
use cafe_space_adapters::pi::install::{CLI, PACKAGE, VERSION};
use std::time::Instant;

fn cli(temp: &tempfile::TempDir, args: &[&str]) -> RequestResult {
    let executable = std::path::Path::new(env!("CARGO_BIN_EXE_cafe-spaced-dev"))
        .parent()
        .unwrap()
        .join(if cfg!(windows) {
            "cafe-space-dev.exe"
        } else {
            "cafe-space-dev"
        });
    assert!(
        executable.is_file(),
        "build both workspace binaries before the opt-in test"
    );
    let output = temp.path().join("runtime-cli.stdout.json");
    let mut child = OwnedChild(
        command_for(temp, &executable)
            .args(args)
            .stdout(std::fs::File::create(&output).unwrap())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "runtime CLI deadline");
        std::thread::sleep(Duration::from_millis(10));
    };
    let bytes = std::fs::read(output).unwrap();
    assert!(bytes.is_ascii());
    assert!(bytes.len() < 65536);
    let result: RequestResult = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        status.success(),
        matches!(
            result,
            RequestResult::Applied(_) | RequestResult::Dispatched(_)
        )
    );
    assert!(
        !String::from_utf8(bytes)
            .unwrap()
            .contains("synthetic-private-package")
    );
    result
}

fn runtime_count(result: RequestResult, count: usize) {
    assert!(
        matches!(result, RequestResult::Applied(value) if matches!(value.result,
        Response::Runtimes(ref list) if list.items.len() == count && list.items.iter().all(|item| item.available)))
    );
}

#[tokio::test]
#[ignore = "requires both workspace binaries and exclusive default endpoints on a disposable hosted runner"]
async fn registration_cli_uses_owner_ipc_without_installing_or_starting_an_agent() {
    assert_eq!(
        std::env::var("RUNNER_ENVIRONMENT").as_deref(),
        Ok("github-hosted")
    );
    assert_eq!(
        std::env::var("CAFE_SPACE_TEST_RUNTIME_CLI").as_deref(),
        Ok("1")
    );
    let temp = tempfile::tempdir().unwrap();
    let (root, endpoint) = paths(&temp);
    #[cfg(windows)]
    assert!(
        local::connect(&endpoint).await.is_err(),
        "another dev daemon owns the pipe"
    );
    drop(cafe_space_ipc::private::directory(&temp.path().join("runtime")).unwrap());
    let package = temp.path().join("synthetic-private-package");
    std::fs::create_dir_all(package.join("dist/bundle")).unwrap();
    std::fs::write(
        package.join("package.json"),
        serde_json::to_vec(&serde_json::json!({
            "name":PACKAGE, "version":VERSION, "bin":{"pi":CLI}
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        package.join(CLI),
        "throw Error('registration must not run me');",
    )
    .unwrap();
    let node = std::env::current_exe().unwrap();
    let mut daemon = OwnedChild(command(&temp).stdout(Stdio::null()).spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            daemon.0.try_wait().unwrap().is_none(),
            "owned daemon exited before readiness"
        );
        if let Ok(identity) = cafe_space_ipc::installation::daemon_identity(&root) {
            assert_eq!(identity.pid, daemon.0.id());
            break;
        }
        assert!(Instant::now() < deadline, "owned daemon readiness deadline");
        std::thread::sleep(Duration::from_millis(10));
    }
    runtime_count(cli(&temp, &["runtime", "list"]), 0);
    runtime_count(
        cli(
            &temp,
            &[
                "runtime",
                "register",
                "pi",
                node.to_str().unwrap(),
                package.to_str().unwrap(),
            ],
        ),
        1,
    );
    runtime_count(cli(&temp, &["runtime", "list"]), 1);
    assert!(!root.join("agent-sessions").exists());
    let before = std::fs::read(root.join("pi-runtime.json")).unwrap();
    std::fs::write(package.join("package.json"), b"{}").unwrap();
    assert!(
        matches!(cli(&temp, &["runtime", "register", "pi", node.to_str().unwrap(), package.to_str().unwrap()]),
        RequestResult::Rejected(value) if value.error.code == "PI_VERSION_UNSUPPORTED")
    );
    assert_eq!(std::fs::read(root.join("pi-runtime.json")).unwrap(), before);
    runtime_count(cli(&temp, &["runtime", "remove", "pi"]), 0);
    runtime_count(cli(&temp, &["runtime", "list"]), 0);
    assert!(
        matches!(cli(&temp, &["session", "list", "--json"]), RequestResult::Applied(value)
        if matches!(value.result, Response::Sessions(ref list) if list.items.is_empty()))
    );
    assert!(!root.join("agent-sessions").exists());
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut stream = local::connect(&endpoint).await.unwrap();
        write_frame(&mut stream, include_bytes!("../../../../fixtures/v2/client-hello.json")).await.unwrap();
        assert!(matches!(decode_server_message(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap(), ServerMessage::Welcome(_)));
        write_frame(&mut stream, include_bytes!("../../../../fixtures/v2/shutdown-request.json")).await.unwrap();
        assert!(matches!(decode_server_message(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap(),
            ServerMessage::Response(value) if matches!(&value.result, RequestResult::Applied(data) if matches!(data.result, Response::ShutdownAccepted))));
        loop {
            if let Some(status) = daemon.0.try_wait().unwrap() { assert!(status.success()); break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    println!(
        "M1_RUNTIME_CLI_OK realCli=true realDaemon=true privateRegistration=true invalidPinPreservesState=true agentProcesses=0 modelCalls=0"
    );
}
