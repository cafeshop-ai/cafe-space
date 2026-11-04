//! Isolated foreground-process checks. These never install or change login startup.

#[cfg(windows)]
use cafe_space_ipc::BuildFlavor;
use cafe_space_ipc::{
    framing::{read_frame, write_frame},
    installation::{Handover, HandoverPhase, binary_hash},
    local,
};
use cafe_space_protocol::{
    ClientMessage, ClientRequest, PROTOCOL_VERSION, PrepareUpdateParams, Request,
    RequestMessageType, RequestResult, Response, ServerMessage, UpdateMode, UpdateState,
    decode_server_message, encode_client_message,
};
use std::{
    io,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};

fn command(temp: &tempfile::TempDir) -> Command {
    command_for(
        temp,
        std::path::Path::new(env!("CARGO_BIN_EXE_cafe-spaced-dev")),
    )
}

fn command_for(temp: &tempfile::TempDir, program: &std::path::Path) -> Command {
    let base = temp.path();
    let mut command = Command::new(program);
    command
        .env("HOME", base)
        .env("USERPROFILE", base)
        .env("LOCALAPPDATA", base)
        .env("XDG_DATA_HOME", base.join("data"))
        .env("XDG_RUNTIME_DIR", base.join("runtime"))
        .env("TMPDIR", base)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn paths(temp: &tempfile::TempDir) -> (PathBuf, PathBuf) {
    #[cfg(windows)]
    {
        (
            temp.path().join("cafe-space-dev"),
            local::endpoint(BuildFlavor::Dev).unwrap(),
        )
    }
    #[cfg(target_os = "linux")]
    {
        (
            temp.path().join("data/cafe-space-dev"),
            temp.path()
                .join("runtime/cafe-space-dev/cafe-spaced-dev.sock"),
        )
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let root = temp
            .path()
            .join("Library/Application Support/cafe-space-dev");
        let normal = root.join("run/cafe-spaced-dev.sock");
        let endpoint = if normal.as_os_str().as_bytes().len() < 104 {
            normal
        } else {
            temp.path().join("cafe-space-dev/cafe-spaced-dev.sock")
        };
        (root, endpoint)
    }
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

#[test]
fn version_and_argument_validation_do_not_create_state() {
    let temp = tempfile::tempdir().unwrap();
    let (root, _) = paths(&temp);
    let output = command(&temp).arg("--version").output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        env!("CARGO_PKG_VERSION")
    );
    assert!(!root.exists());
    assert!(
        !command(&temp)
            .args(["--check-state", "extra"])
            .stdout(Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    assert!(!root.exists());
    assert!(
        !command(&temp)
            .arg("--check-state")
            .stdout(Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    assert!(!root.exists());
}

#[test]
#[ignore = "requires separately built pinned schema-v1 through schema-v4 binaries on a disposable hosted runner"]
fn old_binaries_accept_their_schema_but_refuse_current_without_overwriting_data() {
    assert_eq!(
        std::env::var("RUNNER_ENVIRONMENT").as_deref(),
        Ok("github-hosted")
    );
    for (from, variable) in [
        (1, "CAFE_SPACE_TEST_M0_BINARY"),
        (2, "CAFE_SPACE_TEST_V2_BINARY"),
        (3, "CAFE_SPACE_TEST_V3_BINARY"),
        (4, "CAFE_SPACE_TEST_V4_BINARY"),
    ] {
        check_old_schema_binary(from, variable);
    }
}

fn check_old_schema_binary(from: u32, variable: &str) {
    let old = PathBuf::from(std::env::var_os(variable).expect("pinned old binary required"));
    assert!(old.is_absolute());
    assert_ne!(
        old.canonicalize().unwrap(),
        std::path::Path::new(env!("CARGO_BIN_EXE_cafe-spaced-dev"))
            .canonicalize()
            .unwrap()
    );
    let temp = tempfile::tempdir().unwrap();
    let root = paths(&temp).0;
    std::fs::create_dir_all(root.parent().unwrap()).unwrap();
    let _root = cafe_space_ipc::private::directory(&root).unwrap();
    drop(cafe_space_ipc::private::file(&root.join("cafe-spaced-dev.lock")).unwrap());
    drop(cafe_space_ipc::private::file(&root.join("state.sqlite3")).unwrap());
    let database = rusqlite::Connection::open(root.join("state.sqlite3")).unwrap();
    database
        .execute_batch(include_str!("../../../fixtures/migrations/v1.sql"))
        .unwrap();
    if from >= 2 {
        database
            .execute_batch(include_str!("../../../fixtures/migrations/v2.sql"))
            .unwrap();
    }
    if from >= 3 {
        database
            .execute_batch(include_str!("../../../fixtures/migrations/v3.sql"))
            .unwrap();
    }
    if from == 4 {
        database
            .execute_batch(include_str!("../../../fixtures/migrations/v4.sql"))
            .unwrap();
    }
    drop(database);
    let check = |program: &std::path::Path, flags: &[&str]| {
        command_for(&temp, program)
            .args(flags)
            .stdout(Stdio::null())
            .status()
            .unwrap()
    };
    assert!(
        check(&old, &["--check-state"]).success(),
        "the actual old binary must accept its supported fixture"
    );
    drop(cafe_spaced::state::State::open(&root, cafe_space_ipc::BuildFlavor::Dev).unwrap());
    let db = std::fs::read(root.join("state.sqlite3")).unwrap();
    // Read the SQLite header, not a new connection that could recreate unprotected WAL/SHM on Windows.
    assert_eq!(
        u32::from_be_bytes(db[60..64].try_into().unwrap()),
        cafe_space_ipc::installation::STATE_SCHEMA_VERSION
    );
    let backups: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|value| value == "bak"))
        .collect();
    assert_eq!(backups.len(), 1);
    let backup = std::fs::read(&backups[0]).unwrap();
    assert!(
        !check(&old, &["--check-state"]).success(),
        "old rollback check must reject the new schema"
    );
    assert!(
        !check(&old, &[]).success(),
        "old startup must reject the new schema before binding IPC"
    );
    assert_eq!(std::fs::read(root.join("state.sqlite3")).unwrap(), db);
    assert_eq!(std::fs::read(&backups[0]).unwrap(), backup);
    let output = command(&temp)
        .arg("--check-state")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    if !output.status.success() {
        for entry in std::fs::read_dir(&root).unwrap() {
            let entry = entry.unwrap();
            eprintln!(
                "state file {:?} bytes={}",
                entry.file_name(),
                entry.metadata().unwrap().len()
            );
        }
    }
    assert!(
        output.status.success(),
        "current --check-state: status={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    println!(
        "M1_SCHEMA_OLD_BINARY_OK from={from} to={} oldSchemaAccepted=true checkRejected=true startupRejected=true statePreserved=true backupPreserved=true",
        cafe_space_ipc::installation::STATE_SCHEMA_VERSION
    );
}

#[test]
#[ignore = "requires an explicitly supplied pinned schema-v4 binary; isolated fixtures only"]
fn schema_v4_binary_refuses_current_without_overwriting_data() {
    check_old_schema_binary(4, "CAFE_SPACE_TEST_V4_BINARY");
}

#[test]
fn schema_probe_keeps_sidecars_private_and_current_binary_compatible() {
    for from in [1, 2] {
        let temp = tempfile::tempdir().unwrap();
        let root = paths(&temp).0;
        std::fs::create_dir_all(root.parent().unwrap()).unwrap();
        let _root = cafe_space_ipc::private::directory(&root).unwrap();
        drop(cafe_space_ipc::private::file(&root.join("state.sqlite3")).unwrap());
        let database = rusqlite::Connection::open(root.join("state.sqlite3")).unwrap();
        database
            .execute_batch(include_str!("../../../fixtures/migrations/v1.sql"))
            .unwrap();
        if from == 2 {
            database
                .execute_batch(include_str!("../../../fixtures/migrations/v2.sql"))
                .unwrap();
        }
        drop(database);
        drop(cafe_spaced::state::State::open(&root, cafe_space_ipc::BuildFlavor::Dev).unwrap());
        let before = std::fs::read(root.join("state.sqlite3")).unwrap();
        assert_eq!(
            u32::from_be_bytes(before[60..64].try_into().unwrap()),
            cafe_space_ipc::installation::STATE_SCHEMA_VERSION
        );
        for suffix in ["-wal", "-shm", "-journal"] {
            let path = root.join(format!("state.sqlite3{suffix}"));
            match cafe_space_ipc::private::existing_file(&path) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => panic!("insecure sidecar {suffix}: {error}"),
            }
        }
        let output = command(&temp)
            .arg("--check-state")
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "from={from}: status={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(std::fs::read(root.join("state.sqlite3")).unwrap(), before);
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires private /run/user and /tmp mounts; disposable GitHub-hosted runners only"]
async fn foreground_without_xdg_runtime_serves_owner_ipc() {
    use std::os::unix::fs::MetadataExt;
    assert_eq!(
        std::env::var("CAFE_SPACE_TEST_RUNTIME_FALLBACK").as_deref(),
        Ok("1")
    );
    assert_eq!(
        std::env::var("RUNNER_ENVIRONMENT").as_deref(),
        Ok("github-hosted")
    );
    let temp = tempfile::tempdir().unwrap();
    let root = paths(&temp).0;
    let uid = temp.path().metadata().unwrap().uid();
    let standard = PathBuf::from(format!("/run/user/{uid}"));
    assert!(
        matches!(std::fs::symlink_metadata(&standard), Err(error) if error.kind()==io::ErrorKind::NotFound),
        "the test must exercise the missing /run/user branch"
    );
    let legacy = PathBuf::from(format!("/tmp/cafe-space-dev-{uid}"));
    let squatted = std::env::var("CAFE_SPACE_TEST_SQUATTED_TMP").unwrap() == "1";
    let legacy_before = if squatted {
        let metadata = std::fs::symlink_metadata(&legacy).unwrap();
        assert_ne!(
            metadata.uid(),
            uid,
            "the obsolete path must be held by another user"
        );
        Some(metadata)
    } else {
        assert!(
            matches!(std::fs::symlink_metadata(&legacy), Err(error) if error.kind()==io::ErrorKind::NotFound)
        );
        None
    };
    let directory = root.join("run");
    let endpoint = directory.join("cafe-spaced-dev.sock");
    assert!(
        matches!(std::fs::symlink_metadata(&endpoint), Err(error) if error.kind()==io::ErrorKind::NotFound),
        "refusing to use an existing endpoint"
    );
    let mut child = OwnedChild(
        command(&temp)
            .env_remove("XDG_RUNTIME_DIR")
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut stream = loop {
            assert!(child.0.try_wait().unwrap().is_none(), "owned headless daemon exited");
            match local::connect(&endpoint).await {
                Ok(stream) => break stream,
                Err(error) if matches!(error.kind(),io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) => tokio::time::sleep(Duration::from_millis(10)).await,
                Err(error) => panic!("headless IPC failed: {error}"),
            }
        };
        write_frame(&mut stream,include_bytes!("../../../fixtures/v2/client-hello.json")).await.unwrap();
        assert!(matches!(decode_server_message(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap(),ServerMessage::Welcome(_)));
        let status = ClientMessage::Request(ClientRequest {
            message_type: RequestMessageType::Request,
            protocol_version: PROTOCOL_VERSION,
            request_id: "headless-identity".into(),
            request: Request::GetStatus,
        });
        write_frame(&mut stream, &encode_client_message(&status).unwrap()).await.unwrap();
        let health=decode_server_message(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap();
        let identity=cafe_space_ipc::installation::daemon_identity(&root).unwrap();
        assert_eq!(identity.pid,child.0.id());
        assert!(matches!(health,ServerMessage::Response(ref value) if matches!(&value.result,RequestResult::Applied(data) if matches!(&data.result,Response::Health(value) if value.event_stream_id.as_ref()==Some(&identity.event_stream_id)))));
        write_frame(&mut stream,include_bytes!("../../../fixtures/v2/shutdown-request.json")).await.unwrap();
        assert!(matches!(decode_server_message(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap(),ServerMessage::Response(ref value) if matches!(&value.result,RequestResult::Applied(data) if matches!(data.result,Response::ShutdownAccepted))));
        loop {
            if let Some(status)=child.0.try_wait().unwrap() { assert!(status.success()); break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    assert!(Handover::read(&root).unwrap().is_none());
    assert!(
        std::fs::read_to_string(root.join("logs/daemon.log"))
            .unwrap()
            .contains("DAEMON_STOPPED")
    );
    if let Some(before) = legacy_before {
        let after = std::fs::symlink_metadata(&legacy).unwrap();
        assert_eq!(
            (before.dev(), before.ino(), before.uid(), before.mode()),
            (after.dev(), after.ino(), after.uid(), after.mode())
        );
    } else {
        assert!(
            matches!(std::fs::symlink_metadata(&legacy), Err(error) if error.kind()==io::ErrorKind::NotFound)
        );
    }
    let metadata = std::fs::metadata(&directory).unwrap();
    assert_eq!(metadata.uid(), uid);
    assert_eq!(metadata.mode() & 0o777, 0o700);
    println!("M1_LINUX_DATA_RUNTIME_OK squatted={squatted}");
}

#[tokio::test]
async fn update_reply_durable_marker_and_dedicated_exit_are_real_and_state_check_is_read_only() {
    for control_only in [false, true] {
        check_foreground_update(
            std::path::Path::new(env!("CARGO_BIN_EXE_cafe-spaced-dev")),
            control_only,
        )
        .await;
    }
}

#[tokio::test]
#[ignore = "requires an explicitly supplied pinned protocol-2 binary; isolated foreground processes only"]
async fn frozen_control_updates_the_actual_pinned_protocol2_binary() {
    let old = PathBuf::from(
        std::env::var_os("CAFE_SPACE_TEST_V3_BINARY")
            .expect("pinned schema-v3/protocol-2 binary required"),
    );
    assert!(old.is_absolute());
    assert_ne!(
        old.canonicalize().unwrap(),
        std::path::Path::new(env!("CARGO_BIN_EXE_cafe-spaced-dev"))
            .canonicalize()
            .unwrap()
    );
    check_old_schema_binary(3, "CAFE_SPACE_TEST_V3_BINARY");
    check_foreground_update(&old, true).await;
    println!(
        "D18_CONTROL_V2_PINNED_BINARY_OK fullClientVersion={PROTOCOL_VERSION} controlVersion=2 bootMatched=true hashMatched=true drainMarker=true exit42=true statePreserved=true"
    );
}

async fn check_foreground_update(program: &std::path::Path, control_only: bool) {
    let wire_version = if control_only {
        cafe_space_protocol::control::VERSION
    } else {
        PROTOCOL_VERSION
    };
    let encode = if control_only {
        cafe_space_protocol::control::encode_client_message
    } else {
        encode_client_message
    };
    let decode = if control_only {
        cafe_space_protocol::control::decode_server_message
    } else {
        decode_server_message
    };
    let temp = tempfile::tempdir().unwrap();
    let (root, endpoint) = paths(&temp);
    #[cfg(windows)]
    if local::connect(&endpoint).await.is_ok() {
        panic!("another dev daemon already owns the pipe; refusing any mutating test request");
    }
    drop(cafe_space_ipc::private::directory(&temp.path().join("runtime")).unwrap());
    let mut child = OwnedChild(
        command_for(&temp, program)
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let checking = async {
        let mut stream = loop {
            if child.0.try_wait().unwrap().is_some() {
                panic!("owned daemon exited before binding isolated IPC");
            }
            match local::connect(&endpoint).await {
                Ok(stream) => break stream,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                    ) =>
                {
                    tokio::time::sleep(Duration::from_millis(10)).await
                }
                Err(error) => panic!("isolated IPC connection failed: {error}"),
            }
        };
        #[cfg(windows)]
        assert_eq!(
            local::server_process_id(&stream).unwrap(),
            child.0.id(),
            "never mutate another process using the same pipe name"
        );
        let hello = cafe_space_protocol::ClientMessage::Hello(cafe_space_protocol::ClientHello {
            message_type: cafe_space_protocol::HelloMessageType::Hello,
            supported_protocol_versions: vec![if control_only {
                cafe_space_protocol::control::VERSION
            } else {
                PROTOCOL_VERSION
            }],
            client_version: env!("CARGO_PKG_VERSION").into(),
        });
        write_frame(&mut stream, &encode(&hello).unwrap())
            .await
            .unwrap();
        assert!(matches!(
            decode(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap(),
            ServerMessage::Welcome(_)
        ));
        let health = ClientMessage::Request(ClientRequest {
            message_type: RequestMessageType::Request,
            protocol_version: wire_version,
            request_id: "isolated-identity".into(),
            request: Request::GetStatus,
        });
        write_frame(&mut stream, &encode(&health).unwrap())
            .await
            .unwrap();
        let health = decode(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap();
        let identity = cafe_space_ipc::installation::daemon_identity(&root).unwrap();
        let same_hash = binary_hash(program).unwrap();
        assert_eq!(identity.pid, child.0.id());
        assert!(
            matches!(health, ServerMessage::Response(ref value) if matches!(&value.result, RequestResult::Applied(data) if matches!(&data.result, Response::Health(health) if health.event_stream_id.as_ref() == Some(&identity.event_stream_id) && health.binary_hash.as_deref() == Some(same_hash.as_str()))))
        );
        let unchanged = ClientMessage::Request(ClientRequest {
            message_type: RequestMessageType::Request,
            protocol_version: wire_version,
            request_id: "isolated-shell-only".into(),
            request: Request::PrepareUpdate(PrepareUpdateParams {
                mode: UpdateMode::Now,
                target_hash: same_hash,
            }),
        });
        write_frame(&mut stream, &encode(&unchanged).unwrap())
            .await
            .unwrap();
        let unchanged = decode(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap();
        assert!(
            matches!(unchanged, ServerMessage::Response(ref value) if matches!(&value.result, RequestResult::Applied(data) if matches!(&data.result, Response::Update(update) if update.update == UpdateState::None)))
        );
        assert!(child.0.try_wait().unwrap().is_none());
        assert_eq!(Handover::read(&root).unwrap(), None);
        if !control_only {
            let subscribe = ClientMessage::Request(ClientRequest {
                message_type: RequestMessageType::Request,
                protocol_version: wire_version,
                request_id: "isolated-subscribe".into(),
                request: Request::SubscribeEvents(cafe_space_protocol::SubscribeEventsParams {
                    after: None,
                }),
            });
            write_frame(&mut stream, &encode(&subscribe).unwrap())
                .await
                .unwrap();
            assert!(matches!(
                decode(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap(),
                ServerMessage::Response(_)
            ));
        }
        let request = ClientMessage::Request(ClientRequest {
            message_type: RequestMessageType::Request,
            protocol_version: wire_version,
            request_id: "isolated-update".into(),
            request: Request::PrepareUpdate(PrepareUpdateParams {
                mode: UpdateMode::Now,
                target_hash: "b".repeat(64),
            }),
        });
        write_frame(&mut stream, &encode(&request).unwrap())
            .await
            .unwrap();
        let response = decode(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap();
        assert!(
            matches!(response, ServerMessage::Response(ref value) if matches!(&value.result, RequestResult::Applied(data) if matches!(&data.result, Response::Update(update) if update.update == UpdateState::Draining)))
        );
        if !control_only {
            let event = decode(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap();
            assert!(
                matches!(event, ServerMessage::Event(ref value) if value.sequence == 1 && matches!(&value.event, cafe_space_protocol::Event::UpdateChanged(update) if update.update == UpdateState::Draining))
            );
        }
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert_eq!(status.code(), Some(42));
    };
    tokio::time::timeout(Duration::from_secs(10), checking)
        .await
        .unwrap();
    let marker = Handover::read(&root).unwrap().unwrap();
    assert_eq!(marker.phase, HandoverPhase::Draining);
    assert_eq!(marker.target_hash, "b".repeat(64));
    assert_eq!(marker.old_hash, binary_hash(program).unwrap());
    let database_before = std::fs::read(root.join("state.sqlite3")).unwrap();
    let logs_before = std::fs::read(root.join("logs/daemon.log")).unwrap();
    assert!(
        command_for(&temp, program)
            .arg("--check-state")
            .stdout(Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        std::fs::read(root.join("state.sqlite3")).unwrap(),
        database_before
    );
    assert_eq!(
        std::fs::read(root.join("logs/daemon.log")).unwrap(),
        logs_before
    );
    assert_eq!(Handover::read(&root).unwrap(), Some(marker));
}
