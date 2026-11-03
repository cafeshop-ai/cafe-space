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
    let base = temp.path();
    let mut command = Command::new(env!("CARGO_BIN_EXE_cafe-spaced-dev"));
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

#[tokio::test]
async fn update_reply_durable_marker_and_dedicated_exit_are_real_and_state_check_is_read_only() {
    let temp = tempfile::tempdir().unwrap();
    let (root, endpoint) = paths(&temp);
    #[cfg(windows)]
    if local::connect(&endpoint).await.is_ok() {
        panic!("another dev daemon already owns the pipe; refusing any mutating test request");
    }
    drop(cafe_space_ipc::private::directory(&temp.path().join("runtime")).unwrap());
    let mut child = OwnedChild(command(&temp).stdout(Stdio::null()).spawn().unwrap());
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
        write_frame(
            &mut stream,
            include_bytes!("../../../fixtures/v2/client-hello.json"),
        )
        .await
        .unwrap();
        assert!(matches!(
            decode_server_message(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap(),
            ServerMessage::Welcome(_)
        ));
        let same_hash =
            binary_hash(std::path::Path::new(env!("CARGO_BIN_EXE_cafe-spaced-dev"))).unwrap();
        let unchanged = ClientMessage::Request(ClientRequest {
            message_type: RequestMessageType::Request,
            protocol_version: PROTOCOL_VERSION,
            request_id: "isolated-shell-only".into(),
            request: Request::PrepareUpdate(PrepareUpdateParams {
                mode: UpdateMode::Now,
                target_hash: same_hash,
            }),
        });
        write_frame(&mut stream, &encode_client_message(&unchanged).unwrap())
            .await
            .unwrap();
        let unchanged =
            decode_server_message(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap();
        assert!(
            matches!(unchanged, ServerMessage::Response(ref value) if matches!(&value.result, RequestResult::Applied(data) if matches!(&data.result, Response::Update(update) if update.update == UpdateState::None)))
        );
        assert!(child.0.try_wait().unwrap().is_none());
        assert_eq!(Handover::read(&root).unwrap(), None);
        let subscribe = ClientMessage::Request(ClientRequest {
            message_type: RequestMessageType::Request,
            protocol_version: PROTOCOL_VERSION,
            request_id: "isolated-subscribe".into(),
            request: Request::SubscribeEvents(cafe_space_protocol::SubscribeEventsParams {
                after: None,
            }),
        });
        write_frame(&mut stream, &encode_client_message(&subscribe).unwrap())
            .await
            .unwrap();
        assert!(matches!(
            decode_server_message(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap(),
            ServerMessage::Response(_)
        ));
        let request = ClientMessage::Request(ClientRequest {
            message_type: RequestMessageType::Request,
            protocol_version: PROTOCOL_VERSION,
            request_id: "isolated-update".into(),
            request: Request::PrepareUpdate(PrepareUpdateParams {
                mode: UpdateMode::Now,
                target_hash: "b".repeat(64),
            }),
        });
        write_frame(&mut stream, &encode_client_message(&request).unwrap())
            .await
            .unwrap();
        let response =
            decode_server_message(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap();
        assert!(
            matches!(response, ServerMessage::Response(ref value) if matches!(&value.result, RequestResult::Applied(data) if matches!(&data.result, Response::Update(update) if update.update == UpdateState::Draining)))
        );
        let event =
            decode_server_message(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap();
        assert!(
            matches!(event, ServerMessage::Event(ref value) if value.sequence == 1 && matches!(&value.event, cafe_space_protocol::Event::UpdateChanged(update) if update.update == UpdateState::Draining))
        );
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
    assert_eq!(
        marker.old_hash,
        binary_hash(std::path::Path::new(env!("CARGO_BIN_EXE_cafe-spaced-dev"))).unwrap()
    );
    let database_before = std::fs::read(root.join("state.sqlite3")).unwrap();
    let logs_before = std::fs::read(root.join("logs/daemon.log")).unwrap();
    assert!(
        command(&temp)
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
