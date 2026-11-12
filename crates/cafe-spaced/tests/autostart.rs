//! Opt-in hosted-runner checks. Ordinary cargo test never registers login startup.
use cafe_space_ipc::{
    BuildFlavor,
    framing::{read_frame, write_frame},
    local, private,
    startup::{self, Action},
};
use cafe_space_protocol::ServerMessage;
use std::{
    io,
    time::{Duration, Instant},
};

#[cfg(windows)]
#[path = "support/windows_session_end.rs"]
mod windows_session_end;

const FLAVOR: BuildFlavor = BuildFlavor::Dev;
const M0_API: &str = include_str!("../../../fixtures/migrations/m0-api.json");

#[test]
fn pinned_m0_probe_is_not_the_current_full_api() {
    let api: serde_json::Value = serde_json::from_str(M0_API).unwrap();
    assert_eq!(
        api["sourceCommit"],
        "3953e33250bee0a41eb2bf220a91043779b3c01e"
    );
    assert_eq!(
        api["hello"]["supportedProtocolVersions"],
        serde_json::json!([2])
    );
    let bytes = serde_json::to_vec(&api["listSessions"]).unwrap();
    assert!(cafe_space_protocol::decode_client_message(&bytes).is_err());
    assert!(
        cafe_space_protocol::control::decode_request(&bytes)
            .unwrap()
            .1
            .is_none()
    );
}

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
    // This probe deliberately targets pinned full-v2 M0, not this build's full API.
    let api: serde_json::Value = serde_json::from_str(M0_API).unwrap();
    write_frame(&mut stream, &serde_json::to_vec(&api["hello"]).unwrap())
        .await
        .unwrap();
    let received = tokio::time::timeout(Duration::from_secs(5), read_frame(&mut stream))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(
        cafe_space_protocol::control::decode_server_message(&received).unwrap(),
        ServerMessage::Welcome(_)
    ));
    write_frame(
        &mut stream,
        &serde_json::to_vec(&api["listSessions"]).unwrap(),
    )
    .await
    .unwrap();
    let received = tokio::time::timeout(Duration::from_secs(5), read_frame(&mut stream))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    // M0's empty list has no session capabilities to translate; never decode it as full v3.
    let response: serde_json::Value = serde_json::from_slice(&received).unwrap();
    assert_eq!(response["type"], "response");
    assert_eq!(response["protocolVersion"], 2);
    assert_eq!(response["requestId"], api["listSessions"]["requestId"]);
    assert_eq!(response["result"]["status"], "applied");
    let result = &response["result"]["data"]["result"];
    assert_eq!(result["kind"], "sessions");
    assert_eq!(result["data"]["items"], serde_json::json!([]));
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
    let restarts = if cfg!(target_os = "macos") { 3 } else { 1 };
    for _ in 0..restarts {
        startup::execute(FLAVOR, Action::Stop).unwrap();
        #[cfg(windows)]
        {
            let log = std::fs::read_to_string(root.join("logs/daemon.log")).unwrap();
            assert!(
                log.contains("DAEMON_STOP_REQUESTED") && log.contains("DAEMON_STOPPED"),
                "task stop must exercise the normal owner-only shutdown before hard-stop fallback"
            );
        }
        // No test-side sleep or retry may hide an incomplete Stop.
        startup::execute(FLAVOR, Action::Start).unwrap();
        check_empty_sessions(&endpoint).await;
    }
    #[cfg(target_os = "macos")]
    println!("M1_LAUNCHD_IMMEDIATE_RESTART_OK cycles={restarts} testRetries=0");
    check_migration_recovery(&root, &program, &m0).await;
    #[cfg(windows)]
    {
        startup::run_key(FLAVOR, true).unwrap();
        assert!(startup::run_key(FLAVOR, true).is_err());
        startup::run_key(FLAVOR, false).unwrap();
        assert!(startup::run_key(FLAVOR, false).is_err());
        startup::run_key(FLAVOR, true).unwrap(); // The package uninstall must remove both registrations.
    }
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
        assert!(
            !program.exists(),
            "uninstall must remove the private daemon executable"
        );
        assert!(
            startup::run_key(FLAVOR, false).is_err(),
            "Run key was already removed"
        );
        startup::execute(FLAVOR, Action::Uninstall).unwrap(); // Already uninstalled is success.
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
    use cafe_space_protocol::{
        RecoveryCode, Request, Response, RestoreBackupParams, UpdateRecoveryAction,
    };
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
                "CREATE TABLE migration_history (sentinel TEXT); INSERT INTO migration_history VALUES ('explicit-restore-kept');"
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
            phase: HandoverPhase::Draining,
            migration: None,
        };
        marker.write(root).unwrap();
        cafe_space_ipc::recovery::RecoveryLatch::authorize_retry(root, &marker).unwrap();
        marker.phase = HandoverPhase::Swapping;
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
            assert!(
                updater::checked_recovery_rollback(FLAVOR, root, program)
                    .unwrap()
                    .is_some()
            );
        } else {
            assert!(health.ready && health.awaiting_update_confirmation.is_none());
            assert_eq!(marker.phase, HandoverPhase::Committed);
            assert!(
                cafe_space_ipc::recovery::RecoveryLatch::read(root)
                    .unwrap()
                    .is_none()
            );
            #[cfg(windows)]
            windows_session_end::check(FLAVOR, &lease).await;
            // A post-transaction failure must never make binary rollback overwrite the migrated database.
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
        if !conflict {
            assert_eq!(
                updater::recover(FLAVOR, UpdateRecoveryAction::Rollback).await,
                Err("INSTALL_EXPLICIT_RESTORE_REQUIRED")
            );
            assert_eq!(files::private_binary_hash(program).unwrap(), target_hash);
            assert_eq!(std::fs::read(root.join("state.sqlite3")).unwrap(), database);
            assert_eq!(std::fs::read(&backup_path).unwrap(), backup);
            let lease = updater::acquire(FLAVOR).await.unwrap();
            updater::system(&lease, Action::Start).await.unwrap();
            let deadline = Instant::now() + Duration::from_secs(20);
            let (mut client, health) = loop {
                if let Ok(connected) = Client::connect(FLAVOR, root).await {
                    break connected;
                }
                assert!(Instant::now() < deadline, "recovery daemon did not listen");
                tokio::time::sleep(Duration::from_millis(100)).await;
            };
            assert!(!health.ready);
            assert_eq!(
                health.recovery.unwrap().code,
                RecoveryCode::SchemaMigrationFailed
            );
            assert!(
                updater::checked_recovery_rollback(FLAVOR, root, program)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                client
                    .request(Request::RestoreBackup(RestoreBackupParams {
                        confirm: true,
                        code: RecoveryCode::SchemaMigrationFailed,
                    }))
                    .await
                    .unwrap(),
                Response::Empty
            );
            assert!(
                matches!(client.request(Request::GetStatus).await.unwrap(), Response::Health(h)
                if !h.ready && h.recovery.as_ref().is_some_and(|r| r.code == RecoveryCode::RollbackRequired))
            );
            let latch = cafe_space_ipc::recovery::RecoveryLatch::read(root)
                .unwrap()
                .unwrap();
            assert_eq!(
                std::fs::read(root.join(latch.failed_state.unwrap())).unwrap(),
                database
            );
            assert_eq!(std::fs::read(root.join("state.sqlite3")).unwrap(), backup);
            assert!(
                updater::checked_recovery_rollback(FLAVOR, root, program)
                    .unwrap()
                    .is_some()
            );
            println!(
                "M1_SCHEMA_EXPLICIT_RESTORE_OS_OK directRollbackRejected=true restored=true ready=false evidencePreserved=true"
            );
        }
        updater::recover(FLAVOR, UpdateRecoveryAction::Rollback)
            .await
            .unwrap();
        {
            assert_eq!(files::private_binary_hash(program).unwrap(), old_hash);
            let rolled_back = Handover::read(root).unwrap().unwrap();
            assert_eq!(rolled_back.phase, HandoverPhase::RolledBack);
            assert!(rolled_back.migration.is_none());
            check_empty_sessions(&local::endpoint(FLAVOR).unwrap()).await;
            // The old daemon re-enables WAL; verify data instead of expecting identical SQLite header bytes.
            let restored = rusqlite::Connection::open_with_flags(
                root.join("state.sqlite3"),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            assert_eq!(
                restored
                    .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                    .unwrap(),
                1
            );
            if conflict {
                assert_eq!(
                    restored
                        .query_row("SELECT sentinel FROM migration_history", [], |r| r
                            .get::<_, String>(0))
                        .unwrap(),
                    "explicit-restore-kept"
                );
            }
        }
        let latch = cafe_space_ipc::recovery::RecoveryLatch::read(root)
            .unwrap()
            .unwrap();
        assert_eq!(
            latch.phase,
            if conflict {
                cafe_space_ipc::recovery::RecoveryPhase::Failed
            } else {
                cafe_space_ipc::recovery::RecoveryPhase::Restored
            }
        );
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
            if conflict {
                1
            } else {
                cafe_space_ipc::installation::STATE_SCHEMA_VERSION
            },
            if conflict { "direct_m0" } else { "restored_m0" }
        );
    }
}
