//! Invoked only by the explicitly opted-in disposable-runner autostart test.
#![allow(unsafe_code)]
use cafe_space_ipc::{BuildFlavor, client::Client, installation, local, startup, updater};
use std::{
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr,
    sync::Arc,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{HWND, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        QueryFullProcessImageNameW, WaitForSingleObject,
    },
    UI::WindowsAndMessaging::*,
};

pub async fn check(flavor: BuildFlavor, lease: &Arc<startup::Lease>) {
    let root = lease.root();
    let program = lease.program();
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    assert_eq!(
        std::env::var("RUNNER_ENVIRONMENT").as_deref(),
        Ok("github-hosted")
    );
    assert_eq!(
        std::env::var("CAFE_SPACE_TEST_AUTOSTART").as_deref(),
        Ok("1")
    );
    let expected_hash = installation::private_binary_hash(program).unwrap();
    let endpoint = local::endpoint(flavor).unwrap();
    for flags in [ENDSESSION_LOGOFF, 0] {
        updater::system(lease, startup::Action::Start)
            .await
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        let (client, health) = loop {
            match Client::connect(flavor, root).await {
                Ok(value) => break value,
                Err(code) => {
                    assert!(Instant::now() < deadline, "task start failed: {code}");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        };
        assert_eq!(health.binary_hash.as_deref(), Some(expected_hash.as_str()));
        let pipe = local::connect(&endpoint).await.unwrap();
        let pid = local::server_process_id(&pipe).unwrap();
        assert_eq!(pid, installation::daemon_identity(root).unwrap().pid);
        // SAFETY: PID comes from the owner-checked pipe; hold the process to avoid PID reuse.
        let raw = unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            )
        };
        assert!(!raw.is_null());
        // SAFETY: OpenProcess returned a newly owned handle; no termination rights were requested.
        let process = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut image = vec![0u16; 32768];
        let mut length = image.len() as u32;
        // SAFETY: the held process and writable image buffer remain live for the call.
        assert_ne!(
            unsafe {
                QueryFullProcessImageNameW(
                    process.as_raw_handle(),
                    PROCESS_NAME_WIN32,
                    image.as_mut_ptr(),
                    &mut length,
                )
            },
            0
        );
        let image = String::from_utf16(&image[..length as usize]).unwrap();
        assert_eq!(
            std::fs::canonicalize(image).unwrap(),
            std::fs::canonicalize(program).unwrap()
        );
        let window = find_window(pid);
        // SAFETY: query only the window belonging to the held, verified task process.
        unsafe {
            assert!(GetParent(window).is_null());
            assert_eq!(GetWindowLongW(window, GWL_STYLE) as u32 & WS_VISIBLE, 0);
        }
        let stopped_before = std::fs::read_to_string(root.join("logs/daemon.log"))
            .unwrap()
            .matches("DAEMON_STOPPED")
            .count();
        assert_eq!(send(window, pid, WM_QUERYENDSESSION, 0, flags), 1);
        assert_eq!(send(window, pid, WM_ENDSESSION, 0, flags), 0);
        // SAFETY: query only the held process; cancelled end must not terminate it.
        assert_eq!(
            unsafe { WaitForSingleObject(process.as_raw_handle(), 0) },
            WAIT_TIMEOUT
        );
        let (mut probe, same) = Client::connect(flavor, root).await.unwrap();
        assert_eq!(same.event_stream_id, health.event_stream_id);
        assert_eq!(same.binary_hash, health.binary_hash);
        assert_eq!(
            probe
                .request(cafe_space_protocol::Request::GetStatus)
                .await
                .unwrap(),
            cafe_space_protocol::Response::Health(same)
        );
        drop(probe);
        drop(client);
        drop(pipe);
        assert_eq!(send(window, pid, WM_ENDSESSION, 1, flags), 0);
        // SAFETY: wait on this exact held process, never look up another PID to stop.
        assert_eq!(
            unsafe { WaitForSingleObject(process.as_raw_handle(), 10000) },
            WAIT_OBJECT_0
        );
        let log = std::fs::read_to_string(root.join("logs/daemon.log")).unwrap();
        assert_eq!(log.matches("DAEMON_STOPPED").count(), stopped_before + 1);
        println!(
            "M1_WINDOWS_TASK_SESSION_END_OK logoff={} hidden=true topLevel=true queryNoStop=true cancelledNoStop=true taskProcessExited=true realLogoff=false",
            flags != 0
        );
    }
    // Restore the running target before the caller injects the separate recovery-required cut.
    updater::system(lease, startup::Action::Start)
        .await
        .unwrap();
    let health = updater::wait_ready(flavor, root, &expected_hash)
        .await
        .unwrap();
    assert!(health.ready && health.awaiting_update_confirmation.is_none());
}

fn find_window(pid: u32) -> HWND {
    let mut window = ptr::null_mut();
    loop {
        // SAFETY: enumerates top-level windows; no message is sent to an unverified window.
        window = unsafe { FindWindowExW(ptr::null_mut(), window, ptr::null(), ptr::null()) };
        assert!(
            !window.is_null(),
            "task process has no session-end window on this desktop"
        );
        let mut owner = 0;
        let mut class = [0u16; 128];
        // SAFETY: output storage is valid and window is checked for ownership below.
        let length = unsafe {
            GetWindowThreadProcessId(window, &mut owner);
            GetClassNameW(window, class.as_mut_ptr(), class.len() as i32)
        };
        if owner == pid
            && length > 0
            && String::from_utf16_lossy(&class[..length as usize])
                .starts_with("CafeSpaceSessionEnd-")
        {
            return window;
        }
    }
}

fn send(window: HWND, pid: u32, message: u32, value: usize, flags: u32) -> usize {
    let mut owner = 0;
    let mut result = 0;
    // SAFETY: recheck ownership before targeting only this task's window, never HWND_BROADCAST.
    unsafe {
        GetWindowThreadProcessId(window, &mut owner);
    }
    assert_eq!(owner, pid);
    // SAFETY: scalar session-end parameters contain no pointers; bounded wait is not retried.
    assert_ne!(
        unsafe {
            SendMessageTimeoutW(
                window,
                message,
                value,
                flags as isize,
                SMTO_ABORTIFHUNG | SMTO_BLOCK,
                10000,
                &mut result,
            )
        },
        0,
        "session-end result unknown; do not retry"
    );
    result
}
