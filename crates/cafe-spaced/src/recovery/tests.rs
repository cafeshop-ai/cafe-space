use super::*;
use crate::{host::Host, update::Access};
use cafe_space_ipc::{BuildFlavor, installation::binary_hash};
use cafe_space_protocol::*;
use std::{collections::BTreeMap, io::Write, path::PathBuf};

fn fixture(marker: bool) -> (tempfile::TempDir, PathBuf, State) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(BuildFlavor::Dev.directory_name());
    drop(private::directory(&root).unwrap());
    drop(private::file(&root.join("state.sqlite3")).unwrap());
    let database = Connection::open(root.join("state.sqlite3")).unwrap();
    database
        .execute_batch(include_str!("../../../../fixtures/migrations/v1.sql"))
        .unwrap();
    database
        .execute_batch("CREATE TABLE migration_history (sentinel TEXT);")
        .unwrap();
    drop(database);
    if marker {
        Handover {
            old_hash: "a".repeat(64),
            target_hash: "b".repeat(64),
            phase: HandoverPhase::Committed,
            migration: None,
        }
        .write(&root)
        .unwrap();
    }
    let state = State::open(&root, BuildFlavor::Dev).unwrap();
    assert_eq!(
        state.recovery.as_ref().unwrap().phase,
        RecoveryPhase::Failed
    );
    (temp, root, state)
}

fn bytes(root: &Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(root)
        .unwrap()
        .filter_map(|entry| {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_str().unwrap();
            (path.is_file() && (name.starts_with("state.sqlite3") || name == LATCH_FILE))
                .then(|| (name.into(), std::fs::read(&path).unwrap()))
        })
        .collect()
}

fn result(host: &Host, access: Access, request: Request) -> RequestResult {
    host.dispatch(access, 1, request).unwrap().result
}

fn health(host: &Host) -> HealthResponse {
    let RequestResult::Applied(value) = result(host, Access::Remote, Request::GetStatus) else {
        panic!("health rejected");
    };
    let Response::Health(health) = value.result else {
        panic!("health missing");
    };
    assert!(!health.ready);
    assert_eq!(health.awaiting_update_confirmation, None);
    health
}

fn restore_request(confirm: bool, code: RecoveryCode) -> Request {
    Request::RestoreBackup(RestoreBackupParams { confirm, code })
}

#[test]
fn restore_keeps_original_bytes_and_backup_and_restart_never_migrates() {
    for marker in [false, true] {
        let (_temp, root, mut state) = fixture(marker);
        let latch = state.recovery.clone().unwrap();
        let backup = root.join(latch.backup_file.as_ref().unwrap());
        let before = bytes(&root);
        restore(&mut state, &root).unwrap();
        let after = state.recovery.as_ref().unwrap();
        assert_eq!(after.phase, RecoveryPhase::Restored);
        for suffix in SUFFIXES {
            if let Some(expected) = before.get(&format!("state.sqlite3{suffix}")) {
                assert_eq!(
                    &std::fs::read(
                        root.join(format!("{}{suffix}", after.failed_state.as_ref().unwrap()))
                    )
                    .unwrap(),
                    expected
                );
            }
        }
        assert_eq!(
            std::fs::read(&backup).unwrap(),
            std::fs::read(root.join("state.sqlite3")).unwrap()
        );
        assert_eq!(
            state
                .database
                .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            state
                .database
                .query_row("SELECT value FROM legacy_probe WHERE id=2", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "Résumé 茶"
        );
        assert!(
            state
                .database
                .execute_batch("CREATE TABLE forbidden(id)")
                .is_err()
        );
        assert!(matches!(
            restore(&mut state, &root),
            Err(RestoreError::Rejected("RESTORE_NOT_AVAILABLE"))
        ));
        drop(state);
        let restored = bytes(&root);
        for _ in 0..3 {
            let state = State::open(&root, BuildFlavor::Dev).unwrap();
            assert!(state.database_file.is_none());
            let host = Host::new(state, &root, "boot-recovery".into(), "b".repeat(64)).unwrap();
            let health = health(&host);
            assert_eq!(
                health.recovery.unwrap(),
                RecoveryState {
                    code: RecoveryCode::RollbackRequired,
                    from_schema: 1,
                    to_schema: 6,
                    backup: true
                }
            );
            assert!(matches!(
                result(&host, Access::Local, Request::ListSessions),
                RequestResult::Rejected(_)
            ));
            assert_eq!(bytes(&root), restored);
        }
    }
}

#[test]
fn local_confirmation_is_required_and_remote_health_contains_no_paths() {
    let (_temp, root, state) = fixture(true);
    let host = Host::new(state, &root, "boot-recovery".into(), "b".repeat(64)).unwrap();
    let before = bytes(&root);
    let status = health(&host);
    assert_eq!(
        status.recovery.as_ref().unwrap().code,
        RecoveryCode::SchemaMigrationFailed
    );
    let encoded = serde_json::to_string(&status).unwrap();
    assert!(
        !encoded.contains("state.sqlite3")
            && !encoded.contains("dataDirectory")
            && !encoded.contains("backupFile")
    );
    for request in [
        Request::GetRecoveryDetails,
        restore_request(true, RecoveryCode::SchemaMigrationFailed),
    ] {
        assert!(
            matches!(result(&host, Access::Remote, request), RequestResult::Rejected(r) if r.error.code=="LOCAL_ONLY")
        );
    }
    assert!(
        matches!(result(&host, Access::Local, restore_request(false, RecoveryCode::SchemaMigrationFailed)), RequestResult::Rejected(r) if r.error.code=="RESTORE_CONFIRMATION_REQUIRED")
    );
    assert!(
        matches!(result(&host, Access::Local, restore_request(true, RecoveryCode::RollbackRequired)), RequestResult::Rejected(r) if r.error.code=="RESTORE_NOT_AVAILABLE")
    );
    assert_eq!(bytes(&root), before);
    assert!(
        matches!(result(&host, Access::Local, Request::GetRecoveryDetails), RequestResult::Applied(r) if matches!(&r.result, Response::RecoveryDetails(d) if d.backup_file.is_some() && d.state_files.iter().any(|path| path.ends_with("recovery.json"))))
    );
    assert!(
        matches!(result(&host, Access::Local, restore_request(true, RecoveryCode::SchemaMigrationFailed)), RequestResult::Applied(r) if r.result==Response::Empty)
    );
    assert_eq!(
        health(&host).recovery.unwrap().code,
        RecoveryCode::RollbackRequired
    );
}

#[test]
fn missing_corrupt_and_linked_backups_are_not_restored() {
    for mode in ["missing", "corrupt", "hardlink"] {
        let (_temp, root, mut state) = fixture(false);
        let backup = root.join(
            state
                .recovery
                .as_ref()
                .unwrap()
                .backup_file
                .as_ref()
                .unwrap(),
        );
        match mode {
            "missing" => std::fs::remove_file(&backup).unwrap(),
            "corrupt" => {
                let mut file = private::file(&backup).unwrap();
                file.set_len(0).unwrap();
                file.write_all(b"not a database").unwrap();
            }
            _ => std::fs::hard_link(&backup, root.join("linked-backup")).unwrap(),
        }
        let before = bytes(&root);
        assert!(matches!(
            restore(&mut state, &root),
            Err(RestoreError::Rejected(_))
        ));
        assert_eq!(bytes(&root), before);
        assert_eq!(
            state.recovery.as_ref().unwrap().phase,
            RecoveryPhase::Failed
        );
        assert_eq!(
            status(&root, state.recovery.as_ref().unwrap()).backup,
            mode == "corrupt"
        );
    }
}

#[test]
fn interrupted_restore_finalizes_only_verified_copies_with_retained_evidence() {
    for completed in [false, true] {
        let (_temp, root, mut state) = fixture(false);
        if completed {
            restore(&mut state, &root).unwrap();
        }
        let mut latch = state.recovery.clone().unwrap();
        latch.phase = RecoveryPhase::Restoring;
        latch.restored_hash =
            Some(binary_hash(&root.join(latch.backup_file.as_ref().unwrap())).unwrap());
        latch.write(&root).unwrap();
        drop(state);
        let previous_database = std::fs::read(root.join("state.sqlite3")).unwrap();
        let count = std::fs::read_dir(&root).unwrap().count();
        let state = State::open(&root, BuildFlavor::Dev).unwrap();
        assert_eq!(
            state.recovery.as_ref().unwrap().phase,
            if completed {
                RecoveryPhase::Restored
            } else {
                RecoveryPhase::Failed
            }
        );
        assert_eq!(
            std::fs::read(root.join("state.sqlite3")).unwrap(),
            previous_database
        );
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), count);
    }
}

#[test]
fn restore_io_failure_is_unknown_and_preserves_database_and_backup() {
    let (_temp, root, state) = fixture(false);
    drop(state);
    let mut state = State::open(&root, BuildFlavor::Dev).unwrap();
    let wal = root.join("state.sqlite3-wal");
    if wal.exists() {
        std::fs::remove_file(&wal).unwrap();
    }
    drop(private::directory(&wal).unwrap());
    let database = std::fs::read(root.join("state.sqlite3")).unwrap();
    let backup = root.join(
        state
            .recovery
            .as_ref()
            .unwrap()
            .backup_file
            .as_ref()
            .unwrap(),
    );
    let original_backup = std::fs::read(&backup).unwrap();
    assert!(matches!(
        restore(&mut state, &root),
        Err(RestoreError::Unknown)
    ));
    drop(state);
    let state = State::open(&root, BuildFlavor::Dev).unwrap();
    assert_eq!(
        state.recovery.as_ref().unwrap().phase,
        RecoveryPhase::Failed
    );
    assert_eq!(std::fs::read(root.join("state.sqlite3")).unwrap(), database);
    assert_eq!(std::fs::read(&backup).unwrap(), original_backup);
}

#[test]
fn unknown_restore_is_reconciled_by_status_and_can_be_explicitly_retried_in_the_same_host() {
    let (_temp, root, state) = fixture(false);
    drop(state);
    let state = State::open(&root, BuildFlavor::Dev).unwrap();
    let host = Host::new(state, &root, "same-host".into(), "b".repeat(64)).unwrap();
    let before = std::fs::read(root.join("state.sqlite3")).unwrap();
    let wal = root.join("state.sqlite3-wal");
    if wal.exists() {
        std::fs::remove_file(&wal).unwrap();
    }
    drop(private::directory(&wal).unwrap());
    assert!(
        matches!(result(&host, Access::Local, restore_request(true, RecoveryCode::SchemaMigrationFailed)),
        RequestResult::Unknown(error) if error.error.code == "RECOVERY_OUTCOME_UNKNOWN")
    );
    let interrupted = RecoveryLatch::read(&root).unwrap().unwrap();
    assert_eq!(interrupted.phase, RecoveryPhase::Restoring);
    let backup = root.join(interrupted.backup_file.as_ref().unwrap());
    let backup_bytes = std::fs::read(&backup).unwrap();

    assert_eq!(
        health(&host).recovery.unwrap().code,
        RecoveryCode::SchemaMigrationFailed
    );
    let reconciled = RecoveryLatch::read(&root).unwrap().unwrap();
    assert_eq!(reconciled.phase, RecoveryPhase::Failed);
    assert_eq!(reconciled.failed_state, interrupted.failed_state);
    assert_eq!(std::fs::read(root.join("state.sqlite3")).unwrap(), before);
    assert_eq!(std::fs::read(&backup).unwrap(), backup_bytes);

    std::fs::remove_dir(&wal).unwrap();
    assert!(
        matches!(result(&host, Access::Local, restore_request(true, RecoveryCode::SchemaMigrationFailed)),
        RequestResult::Applied(value) if value.result == Response::Empty)
    );
    assert_eq!(
        health(&host).recovery.unwrap().code,
        RecoveryCode::RollbackRequired
    );
    let restored = RecoveryLatch::read(&root).unwrap().unwrap();
    assert_eq!(restored.phase, RecoveryPhase::Restored);
    assert_ne!(restored.failed_state, interrupted.failed_state);
    assert_eq!(
        std::fs::read(root.join(interrupted.failed_state.unwrap())).unwrap(),
        before
    );
    assert_eq!(std::fs::read(&backup).unwrap(), backup_bytes);
    assert_eq!(
        std::fs::read(root.join("state.sqlite3")).unwrap(),
        backup_bytes
    );
}

#[test]
fn restoring_revokes_an_interrupted_upgrade_retry() {
    let (_temp, root, mut state) = fixture(true);
    let latch = state.recovery.as_mut().unwrap();
    latch.retry = Some(cafe_space_ipc::recovery::RecoveryRetry {
        old_hash: "a".repeat(64),
        target_hash: latch.failed_hash.clone(),
    });
    latch.write(&root).unwrap();
    restore(&mut state, &root).unwrap();
    let restored = RecoveryLatch::read(&root).unwrap().unwrap();
    assert_eq!(restored.phase, RecoveryPhase::Restored);
    assert!(restored.retry.is_none());
}

#[test]
fn only_a_new_explicit_committed_handover_can_retry_after_either_rollback() {
    for (restored, authorized, committed) in [
        (false, false, true),
        (false, true, false),
        (false, true, true),
        (true, true, false),
        (true, true, true),
    ] {
        let (_temp, root, mut state) = fixture(true);
        let original = state.recovery.clone().unwrap();
        let source = root.join(original.backup_file.as_ref().unwrap());
        let original_backup = std::fs::read(&source).unwrap();
        if restored {
            restore(&mut state, &root).unwrap();
        }
        drop(state);
        let hash = binary_hash(&std::env::current_exe().unwrap()).unwrap();
        let mut marker = Handover {
            old_hash: "a".repeat(64),
            target_hash: hash.clone(),
            phase: HandoverPhase::Draining,
            migration: None,
        };
        marker.write(&root).unwrap();
        if authorized {
            RecoveryLatch::authorize_retry(&root, &marker).unwrap();
        }
        let retired = RecoveryLatch::read(&root).unwrap().unwrap();
        marker.phase = if committed {
            HandoverPhase::Committed
        } else {
            HandoverPhase::Starting
        };
        marker.write(&root).unwrap();
        let state = State::open(&root, BuildFlavor::Dev).unwrap();
        assert!(state.database_file.is_none());
        let host = Host::new(state, &root, "retry-boot".into(), hash).unwrap();
        if !committed {
            assert!(
                matches!(result(&host, Access::Local, Request::GetStatus), RequestResult::Applied(r) if matches!(&r.result, Response::Health(h) if !h.ready && h.recovery.is_none() && h.awaiting_update_confirmation==Some(true)))
            );
            assert_eq!(RecoveryLatch::read(&root).unwrap().unwrap(), retired);
            marker.phase = HandoverPhase::Committed;
            marker.write(&root).unwrap();
        }
        assert_eq!(
            health(&host).recovery.unwrap().code,
            RecoveryCode::SchemaMigrationFailed
        );
        let current = RecoveryLatch::read(&root).unwrap().unwrap();
        if authorized {
            assert_ne!(current.backup_file, original.backup_file);
            let archive = std::fs::read_dir(&root)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| {
                    let name = path.file_name().unwrap().to_string_lossy();
                    name.ends_with(".recovery.json")
                        || (name.starts_with("recovery.") && name != "recovery.json")
                })
                .unwrap();
            assert_eq!(
                serde_json::from_slice::<RecoveryLatch>(&std::fs::read(archive).unwrap()).unwrap(),
                retired
            );
        } else {
            assert_eq!(current, original);
        }
        assert_eq!(std::fs::read(source).unwrap(), original_backup);
    }
}

#[test]
fn stale_handover_cannot_clear_a_restored_latch() {
    for phase in [HandoverPhase::Starting, HandoverPhase::Committed] {
        let (_temp, root, mut state) = fixture(true);
        restore(&mut state, &root).unwrap();
        let before = bytes(&root);
        let hash = binary_hash(&std::env::current_exe().unwrap()).unwrap();
        let marker = Handover {
            old_hash: "a".repeat(64),
            target_hash: hash.clone(),
            phase,
            migration: None,
        };
        marker.write(&root).unwrap();
        assert!(RecoveryLatch::authorize_retry(&root, &marker).is_err());
        assert!(state.reopen_for_upgrade(&root).is_err());
        assert!(RecoveryLatch::retire(&root).is_err());
        drop(state);
        let state = State::open(&root, BuildFlavor::Dev).unwrap();
        let host = Host::new(state, &root, "stale-boot".into(), hash).unwrap();
        assert_eq!(
            health(&host).recovery.unwrap().code,
            RecoveryCode::RollbackRequired
        );
        assert_eq!(bytes(&root), before);
    }
}

#[tokio::test]
async fn recovery_runtime_binds_real_private_ipc_and_restores_without_restarting_sessions() {
    use cafe_space_ipc::{
        framing::{read_frame, write_frame},
        local,
    };
    use std::time::Duration;
    let (_temp, root, state) = fixture(true);
    drop(state);
    #[cfg(unix)]
    let endpoint = _temp.path().join("ipc/s");
    #[cfg(windows)]
    let endpoint = PathBuf::from(format!(
        r"\\.\pipe\cafe-recovery-test-{}",
        std::process::id()
    ));
    let server = crate::runtime::serve(BuildFlavor::Dev, &root, &endpoint, std::future::pending());
    let client = async {
        let mut stream = loop {
            match local::connect(&endpoint).await {
                Ok(stream) => break stream,
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        };
        write_frame(
            &mut stream,
            include_bytes!("../../../../fixtures/v2/client-hello.json"),
        )
        .await
        .unwrap();
        assert!(matches!(
            decode_server_message(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap(),
            ServerMessage::Welcome(_)
        ));
        for (index, request) in [
            Request::GetStatus,
            Request::GetRecoveryDetails,
            restore_request(true, RecoveryCode::SchemaMigrationFailed),
            Request::GetStatus,
            Request::ListSessions,
            Request::Shutdown,
        ]
        .into_iter()
        .enumerate()
        {
            let frame = ClientMessage::Request(ClientRequest {
                message_type: RequestMessageType::Request,
                protocol_version: PROTOCOL_VERSION,
                request_id: index.to_string(),
                request,
            });
            write_frame(&mut stream, &encode_client_message(&frame).unwrap())
                .await
                .unwrap();
            let ServerMessage::Response(response) =
                decode_server_message(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap()
            else {
                panic!("response expected");
            };
            assert_eq!(response.request_id, index.to_string());
            match index {
                0 | 3 => {
                    let RequestResult::Applied(r) = response.result else {
                        panic!("health rejected");
                    };
                    let Response::Health(h) = r.result else {
                        panic!("health missing");
                    };
                    assert!(!h.ready);
                    assert_eq!(
                        h.recovery.unwrap().code,
                        if index == 0 {
                            RecoveryCode::SchemaMigrationFailed
                        } else {
                            RecoveryCode::RollbackRequired
                        }
                    );
                }
                1 => assert!(
                    matches!(response.result, RequestResult::Applied(r) if matches!(&r.result, Response::RecoveryDetails(d) if d.backup_file.is_some()))
                ),
                2 => assert!(
                    matches!(response.result, RequestResult::Applied(r) if r.result == Response::Empty)
                ),
                4 => assert!(matches!(response.result, RequestResult::Rejected(_))),
                5 => assert!(
                    matches!(response.result, RequestResult::Applied(r) if r.result == Response::ShutdownAccepted)
                ),
                _ => unreachable!(),
            }
        }
    };
    let (served, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(server, client)
    })
    .await
    .unwrap();
    assert_eq!(served.unwrap(), crate::runtime::StopReason::Signal);
    assert_eq!(
        RecoveryLatch::read(&root).unwrap().unwrap().phase,
        RecoveryPhase::Restored
    );
}

#[tokio::test]
async fn frozen_control_health_omits_recovery_fields() {
    use cafe_space_ipc::framing::{read_frame, write_frame};
    let (_temp, root, state) = fixture(true);
    let host = Host::new(state, &root, "boot-recovery".into(), "b".repeat(64)).unwrap();
    let (mut client, server) = tokio::io::duplex(4096);
    let peer = async {
        let hello = ClientMessage::Hello(ClientHello {
            message_type: HelloMessageType::Hello,
            supported_protocol_versions: vec![control::VERSION],
            client_version: "fixture".into(),
        });
        write_frame(
            &mut client,
            &control::encode_client_message(&hello).unwrap(),
        )
        .await
        .unwrap();
        assert!(matches!(
            control::decode_server_message(&read_frame(&mut client).await.unwrap().unwrap())
                .unwrap(),
            ServerMessage::Welcome(_)
        ));
        let request = ClientMessage::Request(ClientRequest {
            message_type: RequestMessageType::Request,
            protocol_version: control::VERSION,
            request_id: "health".into(),
            request: Request::GetStatus,
        });
        write_frame(
            &mut client,
            &control::encode_client_message(&request).unwrap(),
        )
        .await
        .unwrap();
        let reply = read_frame(&mut client).await.unwrap().unwrap();
        assert!(!String::from_utf8_lossy(&reply).contains("\"recovery\":"));
        assert!(
            matches!(control::decode_server_message(&reply).unwrap(), ServerMessage::Response(r) if matches!(&r.result, RequestResult::Applied(r) if matches!(&r.result, Response::Health(h) if !h.ready && h.recovery.is_none())))
        );
        drop(client);
    };
    let (served, ()) = tokio::join!(crate::api::serve_connection(server, &host), peer);
    served.unwrap();
}
