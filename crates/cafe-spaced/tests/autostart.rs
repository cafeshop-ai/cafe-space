//! Opt-in hosted-runner checks. Ordinary cargo test never registers login startup.
use cafe_space_ipc::{
    BuildFlavor,
    framing::{read_frame, write_frame},
    local, private,
    startup::{self, Action},
};
use cafe_space_protocol::{ServerMessage, decode_server_message};
use std::{
    io,
    time::{Duration, Instant},
};

const FLAVOR: BuildFlavor = BuildFlavor::Dev;

async fn check_empty_sessions(endpoint: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut stream = loop {
        match local::connect(endpoint).await {
            Ok(stream) => break stream,
            Err(error) => {
                assert!(
                    Instant::now() < deadline,
                    "OS-started daemon did not listen: {error}"
                );
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    };
    write_frame(
        &mut stream,
        include_bytes!("../../../fixtures/v2/client-hello.json"),
    )
    .await
    .unwrap();
    let received = tokio::time::timeout(Duration::from_secs(5), read_frame(&mut stream))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(
        decode_server_message(&received).unwrap(),
        ServerMessage::Welcome(_)
    ));
    write_frame(
        &mut stream,
        include_bytes!("../../../fixtures/v2/client-request.json"),
    )
    .await
    .unwrap();
    let received = tokio::time::timeout(Duration::from_secs(5), read_frame(&mut stream))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(
        decode_server_message(&received).unwrap(),
        ServerMessage::Response(cafe_space_protocol::ServerResponse { result: cafe_space_protocol::RequestResult::Applied(data), .. })
            if matches!(data.result, cafe_space_protocol::Response::Sessions(ref list) if list.items.is_empty())
    ));
    drop(stream);
}

#[tokio::test]
#[ignore = "registers login startup; only enabled explicitly on disposable CI runners"]
async fn os_login_registration_runs_private_daemon_and_uninstalls() {
    assert_eq!(
        std::env::var("GITHUB_ACTIONS").as_deref(),
        Ok("true"),
        "requires a disposable GitHub Actions runner"
    );
    assert_eq!(
        std::env::var("CAFE_SPACE_TEST_AUTOSTART").as_deref(),
        Ok("1"),
        "explicit test opt-in required"
    );
    let root = FLAVOR.data_dir().unwrap();
    assert!(
        !root.exists(),
        "refusing to touch pre-existing development state"
    );
    let _root = private::directory(&root).unwrap();
    let bin = root.join("bin");
    let _bin = private::directory(&bin).unwrap();
    let program = FLAVOR.installed_daemon().unwrap();
    let mut target = private::file(&program).unwrap();
    let mut source = std::fs::File::open(env!("CARGO_BIN_EXE_cafe-spaced-dev")).unwrap();
    io::copy(&mut source, &mut target).unwrap();
    target.sync_all().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        target
            .set_permissions(std::fs::Permissions::from_mode(0o700))
            .unwrap();
    }
    drop(target);
    // Keep startup state available on failure for runner logs; always try to unregister our exact definition.
    struct Cleanup;
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = startup::execute(FLAVOR, Action::Uninstall);
        }
    }
    startup::execute(FLAVOR, Action::Install).unwrap();
    let cleanup = Cleanup;
    assert!(
        startup::execute(FLAVOR, Action::Install).is_err(),
        "must not overwrite existing registration"
    );
    startup::execute(FLAVOR, Action::Start).unwrap();
    let endpoint = local::endpoint(FLAVOR).unwrap();
    check_empty_sessions(&endpoint).await;
    startup::execute(FLAVOR, Action::Stop).unwrap();
    #[cfg(windows)]
    {
        let log = std::fs::read_to_string(root.join("logs/daemon.log")).unwrap();
        assert!(
            log.contains("DAEMON_STOP_REQUESTED") && log.contains("DAEMON_STOPPED"),
            "task stop must exercise the normal owner-only shutdown before hard-stop fallback"
        );
    }
    startup::execute(FLAVOR, Action::Start).unwrap();
    check_empty_sessions(&endpoint).await;
    startup::execute(FLAVOR, Action::Uninstall).unwrap();
    std::mem::forget(cleanup);
    assert!(
        root.join("state.sqlite3").exists(),
        "uninstall must preserve data"
    );
    assert!(
        startup::execute(FLAVOR, Action::Start).is_err(),
        "uninstalled service must not be started"
    );
    #[cfg(windows)]
    {
        startup::run_key(FLAVOR, true).unwrap();
        assert!(startup::run_key(FLAVOR, true).is_err());
        startup::run_key(FLAVOR, false).unwrap();
        assert!(startup::run_key(FLAVOR, false).is_err());
    }
}
