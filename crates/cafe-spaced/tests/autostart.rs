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
    assert_eq!(
        std::env::var("RUNNER_ENVIRONMENT").as_deref(),
        Ok("github-hosted")
    );
    let m0 = std::path::PathBuf::from(
        std::env::var_os("CAFE_SPACE_TEST_M0_BINARY").expect("pinned M0 binary required"),
    );
    let mut source = std::fs::File::open(&m0).unwrap();
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
    check_migration_recovery(&root, &program, &m0).await;
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

async fn check_migration_recovery(
    root: &std::path::Path,
    program: &std::path::Path,
    m0: &std::path::Path,
) {
    use cafe_space_ipc::{
        client::Client,
        installation::{self as files, Handover, HandoverPhase},
        updater,
    };
    use cafe_space_protocol::{Request, UpdateRecoveryAction};
    let target = std::path::Path::new(env!("CARGO_BIN_EXE_cafe-spaced-dev"));
    let old_hash = files::binary_hash(m0).unwrap();
    let target_hash = files::binary_hash(target).unwrap();
    assert_ne!(old_hash, target_hash);
    for conflict in [true, false] {
        let lease = updater::acquire(FLAVOR).await.unwrap();
        updater::system(&lease, Action::Stop).await.unwrap();
        drop(updater::stopped_guard(FLAVOR, root).await.unwrap());
        let database = rusqlite::Connection::open(root.join("state.sqlite3")).unwrap();
        database
            .execute_batch(if conflict {
                "CREATE TABLE migration_history (sentinel TEXT);"
            } else {
                "DROP TABLE migration_history;"
            })
            .unwrap();
        drop(database);
        let before = std::fs::read(root.join("state.sqlite3")).unwrap();
        files::stage_binary(target, &program.with_extension("next"), &target_hash).unwrap();
        let mut marker = Handover {
            old_hash: old_hash.clone(),
            target_hash: target_hash.clone(),
            phase: HandoverPhase::Swapping,
            migration: None,
        };
        marker.write(root).unwrap();
        files::activate_staged(program, &old_hash, &target_hash).unwrap();
        marker.phase = HandoverPhase::Starting;
        marker.write(root).unwrap();
        updater::system(&lease, Action::Start).await.unwrap();
        let health = updater::wait_ready(FLAVOR, root, &target_hash)
            .await
            .unwrap();
        assert!(!health.ready && health.awaiting_update_confirmation == Some(true));
        marker.phase = HandoverPhase::Committed;
        marker.write(root).unwrap();
        let (mut client, health) = Client::connect(FLAVOR, root).await.unwrap();
        marker = Handover::read(root).unwrap().unwrap();
        let migration = marker.migration.clone().unwrap();
        assert_eq!(migration.completed, !conflict);
        if conflict {
            assert!(!health.ready && health.awaiting_update_confirmation.is_none());
            assert_eq!(marker.phase, HandoverPhase::RecoveryRequired);
            // The installer client normalizes rejections; the API test checks the exact wire code.
            assert_eq!(
                client.request(Request::ListSessions).await.unwrap_err(),
                "INSTALL_REQUEST_REJECTED"
            );
        } else {
            assert!(health.ready && health.awaiting_update_confirmation.is_none());
            assert_eq!(marker.phase, HandoverPhase::Committed);
            // A post-transaction failure must never make binary rollback overwrite the v2 database.
            marker.phase = HandoverPhase::RecoveryRequired;
            marker.write(root).unwrap();
        }
        drop(client);
        updater::system(&lease, Action::Stop).await.unwrap();
        drop(updater::stopped_guard(FLAVOR, root).await.unwrap());
        let database = std::fs::read(root.join("state.sqlite3")).unwrap();
        let backup_path = root.join(&migration.backup_file);
        let backup = std::fs::read(&backup_path).unwrap();
        if conflict {
            assert_eq!(database, before);
        }
        drop(lease);
        assert_eq!(
            updater::recover(FLAVOR, UpdateRecoveryAction::Finish).await,
            Err("INSTALL_EXPLICIT_RESTORE_REQUIRED")
        );
        let rollback = updater::recover(FLAVOR, UpdateRecoveryAction::Rollback).await;
        if conflict {
            rollback.unwrap();
            assert_eq!(files::private_binary_hash(program).unwrap(), old_hash);
            let rolled_back = Handover::read(root).unwrap().unwrap();
            assert_eq!(rolled_back.phase, HandoverPhase::RolledBack);
            assert!(rolled_back.migration.is_none());
            check_empty_sessions(&local::endpoint(FLAVOR).unwrap()).await;
        } else {
            assert_eq!(rollback, Err("INSTALL_ROLLBACK_STATE_REFUSED"));
            assert_eq!(files::private_binary_hash(program).unwrap(), target_hash);
            assert_eq!(Handover::read(root).unwrap(), Some(marker));
        }
        assert_eq!(std::fs::read(root.join("state.sqlite3")).unwrap(), database);
        assert_eq!(std::fs::read(backup_path).unwrap(), backup);
        assert!(!std::fs::read_dir(root).unwrap().any(|entry| {
            entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|extension| extension == "writing")
        }));
        println!(
            "M1_SCHEMA_RECOVERY_OS_OK schema={} rollback={} statePreserved=true backupPreserved=true",
            if conflict { 1 } else { 2 },
            if conflict { "restored_m0" } else { "refused" }
        );
    }
}
