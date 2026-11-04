//! Standalone synthetic fixture: its supervisor entry runs before any libtest/async runtime.
#![allow(unsafe_code)]
use cafe_space_ipc::project_root;
use cafe_spaced::managed::{self, Process};
use std::{
    ffi::OsString,
    fs::File,
    io::{Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const DEADLINE: Duration = Duration::from_secs(10);
fn until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + DEADLINE;
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "synthetic process deadline exceeded"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
struct Owner(Child);
impl Drop for Owner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn fixture(mode: &str, root: &Path, extra: &[OsString]) -> Process {
    let mut args = vec![OsString::from(mode), root.as_os_str().to_owned()];
    args.extend_from_slice(extra);
    Process::spawn(
        &std::env::current_exe().unwrap(),
        &args,
        &project_root::inspect(root).unwrap(),
    )
    .unwrap()
}
fn read_pid(path: &Path) -> u32 {
    std::fs::read_to_string(path).unwrap().parse().unwrap()
}
fn running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: signal zero only queries an observed fixture PID; it does not send a signal.
        if unsafe { libc::kill(pid as i32, 0) } != 0 {
            return false;
        }
        #[cfg(target_os = "linux")]
        if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            && stat
                .rsplit_once(") ")
                .is_some_and(|(_, fields)| fields.starts_with('Z'))
        {
            return false;
        }
        true
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, WAIT_TIMEOUT},
            System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
        };
        // SAFETY: this observed fixture PID is opened only for a non-mutating zero-time wait.
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            return false;
        }
        // SAFETY: the successful handle remains live through the query and is then closed once.
        let alive = unsafe { WaitForSingleObject(handle, 0) == WAIT_TIMEOUT };
        // SAFETY: only this helper owns the opened handle.
        unsafe {
            CloseHandle(handle);
        }
        alive
    }
}
fn inheritable_sentinel(root: &Path) -> (File, OsString) {
    let file = File::create(root.join("sentinel-do-not-inherit")).unwrap();
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let fd = file.as_raw_fd();
        // SAFETY: only this fixture process deliberately marks its own file inheritable.
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_SETFD, 0) }, 0);
        (file, fd.to_string().into())
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
        let handle = file.as_raw_handle();
        // SAFETY: this fixture deliberately makes its own live file handle inheritable.
        assert_ne!(
            unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) },
            0
        );
        (file, (handle as usize).to_string().into())
    }
}
fn assert_sentinel_absent(raw: &OsString) {
    #[cfg(unix)]
    {
        let fd: i32 = raw.to_str().unwrap().parse().unwrap();
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: only queries the supplied descriptor number, never assumes its ownership.
        if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } == 0 {
            // SAFETY: fstat initialized stat; a reused pipe is not the fixture's regular file.
            assert_ne!(
                unsafe { stat.assume_init() }.st_mode & libc::S_IFMT,
                libc::S_IFREG,
                "nonstandard regular file leaked into target"
            );
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;
        let handle: usize = raw.to_str().unwrap().parse().unwrap();
        let mut path = [0u16; 1024];
        // SAFETY: GetFinalPathName only queries this number; path is a correctly sized output array.
        let count = unsafe {
            GetFinalPathNameByHandleW(handle as _, path.as_mut_ptr(), path.len() as u32, 0)
        };
        if count > 0 && (count as usize) < path.len() {
            assert!(
                !String::from_utf16_lossy(&path[..count as usize])
                    .contains("sentinel-do-not-inherit"),
                "ambient inheritable handle leaked into target"
            );
        }
    }
}
fn helper(args: &[OsString]) -> bool {
    match args[0].to_str().unwrap() {
        "--fixture-owner" => {
            let root = Path::new(&args[1]);
            let mode = args[2].to_str().unwrap();
            let (_sentinel, handle) = inheritable_sentinel(root);
            let mut process = fixture("--fixture-tree", root, &[handle]);
            until(|| root.join("grandchild.pid").exists());
            std::fs::write(root.join("ready"), b"ready").unwrap();
            let mut byte = [0];
            std::io::stdin().read_exact(&mut byte).unwrap();
            match mode {
                "stop" => {
                    process.stop().unwrap();
                }
                "drop" => {
                    drop(process);
                }
                "exit" => {
                    process.stdin.as_mut().unwrap().write_all(b"x").unwrap();
                    let mut exit = None;
                    until(|| {
                        exit = process.try_wait().unwrap();
                        exit.is_some()
                    });
                    assert_eq!(exit.unwrap().code, Some(23));
                }
                _ => panic!("hard-kill fixture must not receive input"),
            }
            std::fs::write(root.join("done"), b"done").unwrap();
            true
        }
        "--fixture-tree" => {
            let root = Path::new(&args[1]);
            assert_eq!(
                std::env::current_dir().unwrap().canonicalize().unwrap(),
                root.canonicalize().unwrap()
            );
            assert!(std::env::var_os("CAFE_SPACE_TEST_SECRET").is_none());
            assert!(std::env::var_os("NODE_OPTIONS").is_none());
            assert_sentinel_absent(&args[2]);
            std::fs::write(root.join("child.pid"), std::process::id().to_string()).unwrap();
            let _grandchild = Command::new(std::env::current_exe().unwrap())
                .arg("--fixture-grandchild")
                .arg(root)
                .spawn()
                .unwrap();
            let mut byte = [0];
            let _ = std::io::stdin().read_exact(&mut byte);
            std::process::exit(23);
        }
        "--fixture-grandchild" => {
            std::fs::write(
                Path::new(&args[1]).join("grandchild.pid"),
                std::process::id().to_string(),
            )
            .unwrap();
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        "--fixture-burst" => {
            let block = [b'x'; 32768];
            for _ in 0..8 {
                std::io::stdout().write_all(&block).unwrap();
            }
            for _ in 0..8 {
                std::io::stderr().write_all(&block).unwrap();
            }
            true
        }
        _ => false,
    }
}
fn host_registry_cases() {
    use cafe_space_ipc::{
        BuildFlavor,
        installation::{Handover, HandoverPhase},
    };
    use cafe_space_protocol::*;
    use cafe_spaced::{host::Host, state::State, update::Access};
    for mode in ["now", "idle", "shutdown", "close_pending"] {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join(BuildFlavor::Dev.directory_name());
        let cwd = temp.path().join("project");
        std::fs::create_dir(&cwd).unwrap();
        let host = Host::new(
            State::open(&data, BuildFlavor::Dev).unwrap(),
            &data,
            "a".repeat(32),
            "a".repeat(64),
        )
        .unwrap();
        let result = host
            .dispatch(
                Access::Local,
                1,
                Request::AddProject(AddProjectParams {
                    root: cwd.to_str().unwrap().into(),
                    name: Some("Synthetic".into()),
                }),
            )
            .unwrap()
            .result;
        let RequestResult::Applied(result) = result else {
            panic!("project required")
        };
        let Response::Project(project) = result.result else {
            panic!("project required")
        };
        let params = CreateSessionParams {
            session_id: "synthetic-owned".into(),
            project_id: project.id.clone(),
            runtime_id: "synthetic".into(),
            expected_cwd_fence: project.cwd_fence.clone(),
            start: SessionStart::New,
        };
        let runtime = RuntimeSummary {
            id: "synthetic".into(),
            name: "Synthetic only".into(),
            version: None,
            available: true,
        };
        let args = [
            OsString::from("--fixture-tree"),
            cwd.as_os_str().to_owned(),
            OsString::from("0"),
        ];
        let (session, streams) = host
            .launch_session(&params, &runtime, &std::env::current_exe().unwrap(), &args)
            .unwrap();
        let mut streams = streams.unwrap();
        assert_eq!(session.state, SessionState::Starting);
        assert!(
            host.launch_session(&params, &runtime, &std::env::current_exe().unwrap(), &args)
                .unwrap()
                .1
                .is_none()
        );
        until(|| cwd.join("grandchild.pid").exists());
        let child = read_pid(&cwd.join("child.pid"));
        let grandchild = read_pid(&cwd.join("grandchild.pid"));
        if mode == "shutdown" {
            let dispatch = host.dispatch(Access::Local, 1, Request::Shutdown).unwrap();
            assert!(dispatch.exit_after_reply && host.normal_shutdown_requested().unwrap());
            assert!(Handover::read(&data).unwrap().is_none());
        } else {
            let update_mode = if mode == "now" {
                UpdateMode::Now
            } else {
                UpdateMode::WhenIdle
            };
            let dispatch = host
                .dispatch(
                    Access::Local,
                    1,
                    Request::PrepareUpdate(PrepareUpdateParams {
                        mode: update_mode,
                        target_hash: "b".repeat(64),
                    }),
                )
                .unwrap();
            if mode != "now" {
                assert!(!dispatch.exit_after_reply);
                assert!(Handover::read(&data).unwrap().is_none());
                let result = host
                    .dispatch(Access::Local, 1, Request::GetStatus)
                    .unwrap()
                    .result;
                assert!(
                    matches!(result, RequestResult::Applied(value) if matches!(value.result, Response::Health(HealthResponse {update: UpdateState::Pending, ..})))
                );
                if mode == "idle" {
                    streams.stdin.write_all(b"x").unwrap();
                    until(|| {
                        host.poll_processes().unwrap();
                        Handover::read(&data).unwrap().is_some()
                    });
                } else {
                    let close = host
                        .dispatch(
                            Access::Local,
                            1,
                            Request::CloseSession(CloseSessionParams {
                                session_id: session.id.clone(),
                                expected_stream_id: session.stream_id.clone().unwrap(),
                                expected_cwd_fence: session.cwd_fence.clone(),
                            }),
                        )
                        .unwrap();
                    assert!(close.exit_after_reply);
                }
            } else {
                assert!(dispatch.exit_after_reply);
            }
            assert_eq!(
                Handover::read(&data).unwrap().unwrap().phase,
                HandoverPhase::Draining
            );
        }
        until(|| !running(child) && !running(grandchild));
        // Shutdown intentionally rejects further work; inspect its durable outcome after releasing Host.
        drop(host);
        let db = rusqlite::Connection::open(data.join("state.sqlite3")).unwrap();
        let json: String = db
            .query_row(
                "SELECT snapshot FROM sessions WHERE id=?1",
                [&session.id],
                |row| row.get(0),
            )
            .unwrap();
        let persisted: SessionSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(persisted.state, SessionState::Stopped);
        assert_eq!(
            persisted.stop_reason,
            Some(if mode == "close_pending" {
                SessionStopReason::SessionClosed
            } else {
                SessionStopReason::AgentExited
            })
        );
        assert!(persisted.active_run_id.is_none());
        println!(
            "M1_PROCESS_REGISTRY_OK case={mode} liveCount=true admissionSerialized=true treeReaped=true terminalPersisted=true"
        );
    }
}

fn run_cases() {
    for mode in ["crash", "stop", "drop", "exit"] {
        let temp = tempfile::tempdir().unwrap();
        let mut owner = Owner(
            Command::new(std::env::current_exe().unwrap())
                .arg("--fixture-owner")
                .arg(temp.path())
                .arg(mode)
                .env("CAFE_SPACE_TEST_SECRET", "synthetic-not-a-credential")
                .env("NODE_OPTIONS", "--synthetic-not-real")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        until(|| temp.path().join("ready").exists());
        let child = read_pid(&temp.path().join("child.pid"));
        let grandchild = read_pid(&temp.path().join("grandchild.pid"));
        assert!(running(child) && running(grandchild));
        if mode == "crash" {
            owner.0.kill().unwrap();
        } else {
            owner.0.stdin.as_mut().unwrap().write_all(b"x").unwrap();
        }
        until(|| owner.0.try_wait().unwrap().is_some());
        until(|| !running(child) && !running(grandchild));
        if mode != "crash" {
            assert!(temp.path().join("done").exists());
        }
        println!(
            "M1_PROCESS_TREE_OK case={mode} childGone=true grandchildGone=true environmentScrubbed=true handlesScrubbed=true cwdUsed=true"
        );
    }
    let temp = tempfile::tempdir().unwrap();
    let mut burst = fixture("--fixture-burst", temp.path(), &[]);
    let consume = |mut output: managed::Output| {
        let mut buffer = [0u8; 65536];
        let mut count = 0;
        loop {
            let n = output.read(&mut buffer).unwrap();
            if n == 0 {
                break;
            }
            assert!(n <= managed::OUTPUT_CHUNK_BYTES);
            count += n;
        }
        count
    };
    let stdout = burst.stdout.take().unwrap();
    let stderr = burst.stderr.take().unwrap();
    let (sent, got) = std::sync::mpsc::sync_channel(2);
    for output in [stdout, stderr] {
        let sent = sent.clone();
        std::thread::spawn(move || {
            sent.send(consume(output)).unwrap();
        });
    }
    for _ in 0..2 {
        assert_eq!(got.recv_timeout(DEADLINE).unwrap(), 262144);
    }
    until(|| burst.try_wait().unwrap().is_some());
    assert_eq!(burst.try_wait().unwrap().unwrap().code, Some(0));
    let root = temp.path().join("approved");
    std::fs::create_dir(&root).unwrap();
    let directory = project_root::inspect(&root).unwrap();
    std::fs::rename(&root, temp.path().join("old")).unwrap();
    std::fs::create_dir(&root).unwrap();
    let result = Process::spawn(&std::env::current_exe().unwrap(), &[], &directory);
    assert_eq!(result.err().unwrap().to_string(), "PROJECT_ROOT_CHANGED");
    assert!(Process::spawn(&temp.path().join("missing"), &[], &directory).is_err());
    println!(
        "M1_PROCESS_OWNERSHIP_OS_OK chunkBounded=true originalExitPreserved=true replacedCwdRejected=true missingExecutableRejected=true"
    );
}
fn main() -> std::io::Result<std::process::ExitCode> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if let Some(result) = managed::supervisor(&args) {
        return result;
    }
    if !args.is_empty() && helper(&args) {
        return Ok(std::process::ExitCode::SUCCESS);
    }
    run_cases();
    host_registry_cases();
    Ok(std::process::ExitCode::SUCCESS)
}
