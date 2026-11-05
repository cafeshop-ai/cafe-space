//! GUI-loaded console processes need a top-level window for session-end notifications.
#![allow(unsafe_code)]
use crate::host::Host;
use std::{
    cell::RefCell,
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WAIT_FAILED, WAIT_OBJECT_0, WPARAM},
    System::{
        LibraryLoader::GetModuleHandleW,
        Threading::{CreateEventW, GetCurrentThreadId, INFINITE, SetEvent},
    },
    UI::WindowsAndMessaging::*,
};

thread_local! {
    // Each window belongs to its own pump thread; no global raw Host pointer crosses callbacks.
    static CONTEXT: RefCell<Option<Arc<Context>>> = const { RefCell::new(None) };
}

struct Context {
    host: Arc<Host>,
    // The event wakes the owning pump without posting to a potentially recycled HWND.
    stop: OwnedHandle,
    handle: AtomicUsize,
}

pub(super) struct Window {
    context: Arc<Context>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Window {
    pub(super) fn new(host: Arc<Host>) -> io::Result<Self> {
        let (sent, received) = std::sync::mpsc::sync_channel(1);
        // SAFETY: creates an unnamed, non-inheritable event used only by this window guard.
        let stop = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if stop.is_null() {
            return Err(failed());
        }
        let context = Arc::new(Context {
            host,
            // SAFETY: this newly created event has exactly one OwnedHandle owner.
            stop: unsafe { OwnedHandle::from_raw_handle(stop) },
            handle: AtomicUsize::new(0),
        });
        let owned = context.clone();
        let thread = thread::Builder::new()
            .name("session-end".into())
            .spawn(move || {
                CONTEXT.with(|value| *value.borrow_mut() = Some(owned.clone()));
                // SAFETY: queries the current module/thread; no borrowed memory is passed to Windows.
                let instance = unsafe { GetModuleHandleW(ptr::null()) };
                let name: Vec<u16> =
                    format!("CafeSpaceSessionEnd-{}\0", unsafe { GetCurrentThreadId() })
                        .encode_utf16()
                        .collect();
                let class = WNDCLASSW {
                    lpfnWndProc: Some(window_proc),
                    hInstance: instance,
                    lpszClassName: name.as_ptr(),
                    ..Default::default()
                };
                // SAFETY: class/name stay live until the window is destroyed and the class unregistered.
                if unsafe { RegisterClassW(&class) } == 0 {
                    let _ = sent.send(Err(failed()));
                    return;
                }
                // SAFETY: creates a hidden TOP-LEVEL window, not a message-only window (which misses broadcasts).
                let handle = unsafe {
                    CreateWindowExW(
                        0,
                        name.as_ptr(),
                        name.as_ptr(),
                        0,
                        0,
                        0,
                        0,
                        0,
                        ptr::null_mut(),
                        ptr::null_mut(),
                        instance,
                        ptr::null(),
                    )
                };
                if handle.is_null() {
                    let _ = sent.send(Err(failed()));
                } else {
                    owned.handle.store(handle as usize, Ordering::Release);
                    if sent.send(Ok(())).is_ok() {
                        let mut message = MSG::default();
                        loop {
                            let event = owned.stop.as_raw_handle();
                            // SAFETY: event remains owned for the entire wait; only this thread pumps its queue.
                            let result = unsafe {
                                MsgWaitForMultipleObjectsEx(
                                    1,
                                    &event,
                                    INFINITE,
                                    QS_ALLINPUT,
                                    MWMO_INPUTAVAILABLE,
                                )
                            };
                            if result == WAIT_OBJECT_0 {
                                break;
                            }
                            if result == WAIT_FAILED {
                                request_stop();
                                break;
                            }
                            // Bound each batch so a message flood cannot starve the stop event.
                            for _ in 0..32 {
                                // SAFETY: writable message storage and this thread's queue are valid.
                                if unsafe {
                                    PeekMessageW(&mut message, ptr::null_mut(), 0, 0, PM_REMOVE)
                                } == 0
                                {
                                    break;
                                }
                                if message.message == WM_QUIT {
                                    break;
                                }
                                // SAFETY: message was obtained from the owning thread's queue.
                                unsafe {
                                    TranslateMessage(&message);
                                    DispatchMessageW(&message);
                                }
                            }
                            if message.message == WM_QUIT {
                                break;
                            }
                        }
                        // SAFETY: only this thread creates/destroys its window; it may already be destroyed by WM_CLOSE.
                        let live = owned.handle.load(Ordering::Acquire) != 0;
                        if live {
                            unsafe {
                                DestroyWindow(handle);
                            }
                        }
                    } else {
                        // SAFETY: startup receiver disappeared, so no guard can tear down our window.
                        unsafe {
                            DestroyWindow(handle);
                        }
                    }
                }
                // SAFETY: no live window of this class remains; name and module are still valid.
                unsafe {
                    UnregisterClassW(name.as_ptr(), instance);
                }
                CONTEXT.with(|value| *value.borrow_mut() = None);
            })?;
        match received.recv() {
            Ok(Ok(())) => Ok(Self {
                context,
                thread: Some(thread),
            }),
            _ => {
                let _ = thread.join();
                Err(failed())
            }
        }
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: the unnamed event is still owned; no HWND/PID lookup or message-queue capacity is involved.
        unsafe {
            SetEvent(self.context.stop.as_raw_handle());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
fn failed() -> io::Error {
    io::Error::other("WINDOWS_SESSION_WINDOW_FAILED")
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_QUERYENDSESSION => 1,
        WM_ENDSESSION => {
            if wparam != 0 {
                // Windows may terminate us after this callback returns; finish durable cleanup here.
                request_stop();
            }
            0
        }
        WM_CLOSE => {
            // SAFETY: Windows called this on the owning pump thread for its live window.
            unsafe {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_DESTROY => {
            CONTEXT.with(|value| {
                if let Some(context) = value.borrow().as_ref() {
                    context.handle.store(0, Ordering::Release);
                }
            });
            // Losing the notification window must not leave a daemon running without its stop path.
            request_stop();
            // SAFETY: exits only this window's dedicated pump, never the daemon runtime.
            unsafe {
                PostQuitMessage(0);
            }
            0
        }
        // SAFETY: forwards untouched callback arguments to the OS default procedure.
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn request_stop() {
    CONTEXT.with(|value| {
        if let Some(context) = value.borrow().as_ref() {
            if context.host.stop_owned_processes().is_err() {
                let _ = context.host.log("WINDOWS_SESSION_END_FAILED");
            }
            context.host.shutdown.notify_one();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{state::State, update::Access};
    use cafe_space_ipc::BuildFlavor;
    use cafe_space_protocol::*;
    use std::time::{Duration, Instant};

    #[test]
    #[ignore = "owned child target only; invoked by hidden-window test"]
    fn synthetic_owned_target() {
        if std::fs::read("synthetic-target-permission").ok().as_deref() != Some(b"isolated") {
            return;
        }
        std::fs::write(
            format!("synthetic-target-{}.pid", std::process::id()),
            std::process::id().to_string(),
        )
        .unwrap();
        thread::sleep(Duration::from_secs(60));
    }

    #[tokio::test]
    async fn closing_the_owned_window_requests_stop_and_drop_never_uses_a_stale_hwnd() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(BuildFlavor::Dev.directory_name());
        let host = Arc::new(
            Host::new(
                State::open(&root, BuildFlavor::Dev).unwrap(),
                &root,
                "window-close".into(),
                "a".repeat(64),
            )
            .unwrap(),
        );
        let window = Window::new(host.clone()).unwrap();
        let handle = window.context.handle.load(Ordering::Acquire);
        // SAFETY: queries only this owned test window, never another app's window.
        unsafe {
            assert!(GetParent(handle as HWND).is_null());
            assert_eq!(
                GetWindowLongW(handle as HWND, GWL_STYLE) as u32 & WS_VISIBLE,
                0
            );
            SendMessageW(handle as HWND, WM_CLOSE, 0, 0);
        }
        tokio::time::timeout(Duration::from_secs(2), host.shutdown.notified())
            .await
            .unwrap();
        assert!(host.normal_shutdown_requested().unwrap());
        assert_eq!(window.context.handle.load(Ordering::Acquire), 0);
        // Drop signals an owned event, not the destroyed HWND (which Windows may already have reused).
        drop(window);
        println!(
            "M1_WINDOWS_SESSION_WINDOW_OK hidden=true topLevel=true unexpectedCloseStops=true staleHwndUnused=true registration=false"
        );
    }

    #[tokio::test]
    async fn hidden_window_defers_cleanup_until_confirmed_end_and_persists_owned_sessions() {
        let contract: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../fixtures/process-ownership/windows-stop.json"
        ))
        .unwrap();
        assert_eq!(
            contract["cases"],
            serde_json::json!([
                "query_only",
                "cancelled_end",
                "logoff",
                "shutdown",
                "unexpected_close",
                "storage_failure"
            ])
        );
        for (flags, storage_failure) in [(ENDSESSION_LOGOFF, false), (0, false), (0, true)] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join(BuildFlavor::Dev.directory_name());
            let cwd = temp.path().join("project");
            std::fs::create_dir(&cwd).unwrap();
            std::fs::write(cwd.join("synthetic-target-permission"), b"isolated").unwrap();
            let host = Arc::new(
                Host::new(
                    State::open(&root, BuildFlavor::Dev).unwrap(),
                    &root,
                    "stop-boot".into(),
                    "a".repeat(64),
                )
                .unwrap(),
            );
            let RequestResult::Applied(project) = host
                .dispatch(
                    Access::Local,
                    1,
                    Request::AddProject(AddProjectParams {
                        root: cwd.to_str().unwrap().into(),
                        name: None,
                    }),
                )
                .unwrap()
                .result
            else {
                panic!("project")
            };
            let Response::Project(project) = project.result else {
                panic!("project")
            };
            let params = CreateSessionParams {
                session_id: "owned-stop".into(),
                project_id: project.id,
                runtime_id: "synthetic".into(),
                expected_cwd_fence: project.cwd_fence,
                start: SessionStart::New,
            };
            let runtime = RuntimeSummary {
                id: "synthetic".into(),
                name: "Synthetic".into(),
                version: None,
                available: true,
                tool_approval: ToolApproval::NeverAsks,
            };
            let (_, _streams) = host
                .launch_session(
                    &params,
                    &runtime,
                    &std::env::current_exe().unwrap(),
                    &[
                        "--ignored".into(),
                        "--exact".into(),
                        "runtime::windows_shutdown::tests::synthetic_owned_target".into(),
                    ],
                )
                .unwrap();
            let mut second = params.clone();
            second.session_id = "owned-stop-2".into();
            let (_, _second_streams) = host
                .launch_session(
                    &second,
                    &runtime,
                    &std::env::current_exe().unwrap(),
                    &[
                        "--ignored".into(),
                        "--exact".into(),
                        "runtime::windows_shutdown::tests::synthetic_owned_target".into(),
                    ],
                )
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            let pids = loop {
                let pids: Vec<u32> = std::fs::read_dir(&cwd)
                    .unwrap()
                    .filter_map(|entry| {
                        let entry = entry.unwrap();
                        if entry.path().extension()?.to_str()? != "pid" {
                            return None;
                        }
                        std::fs::read_to_string(entry.path()).ok()?.parse().ok()
                    })
                    .collect();
                if pids.len() == 2 {
                    break pids;
                }
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(10));
            };
            use std::os::windows::io::{AsRawHandle, FromRawHandle};
            use windows_sys::Win32::{
                Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
                System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
            };
            let processes: Vec<_> = pids
                .into_iter()
                .map(|pid| {
                    // SAFETY: opens only PIDs published by owned children, without termination rights.
                    let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
                    assert!(!raw.is_null());
                    // SAFETY: each successful handle is newly owned and closed once by OwnedHandle.
                    unsafe { OwnedHandle::from_raw_handle(raw) }
                })
                .collect();
            let window = Window::new(host.clone()).unwrap();
            let handle = window.context.handle.load(Ordering::Acquire);
            let deadline = Instant::now();
            // SAFETY: test sends query/cancel messages only to its own hidden window, never broadcasts.
            unsafe {
                assert_eq!(
                    SendMessageW(handle as HWND, WM_QUERYENDSESSION, 0, flags as isize),
                    1
                );
                assert_eq!(
                    SendMessageW(handle as HWND, WM_ENDSESSION, 0, flags as isize),
                    0
                );
            }
            assert!(deadline.elapsed() < Duration::from_secs(2));
            assert!(!host.normal_shutdown_requested().unwrap());
            // SAFETY: query only the held child process handle, without PID reuse.
            for process in &processes {
                assert_eq!(
                    unsafe { WaitForSingleObject(process.as_raw_handle(), 0) },
                    WAIT_TIMEOUT
                );
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(20), host.shutdown.notified())
                    .await
                    .is_err()
            );
            let RequestResult::Applied(before) = host
                .dispatch(
                    Access::Local,
                    1,
                    Request::GetSession(GetSessionParams {
                        session_id: params.session_id.clone(),
                    }),
                )
                .unwrap()
                .result
            else {
                panic!("session")
            };
            let Response::Session(before) = before.result else {
                panic!("session")
            };
            assert_eq!(before.session.session.state, SessionState::Starting);
            let (mut events, _) = host
                .dispatch(
                    Access::Local,
                    1,
                    Request::SubscribeEvents(SubscribeEventsParams { after: None }),
                )
                .unwrap()
                .subscription
                .unwrap();
            let database = rusqlite::Connection::open(root.join("state.sqlite3")).unwrap();
            if storage_failure {
                database.execute_batch("CREATE TRIGGER fail_first_stop BEFORE UPDATE ON sessions WHEN OLD.id = 'owned-stop' BEGIN SELECT RAISE(FAIL, 'synthetic stop persistence failure'); END;").unwrap();
            }
            // SAFETY: confirmed end applies ONLY to this synthetic window and owned temporary Host.
            assert_eq!(
                unsafe { SendMessageW(handle as HWND, WM_ENDSESSION, 1, flags as isize) },
                0
            );
            tokio::time::timeout(Duration::from_secs(2), host.shutdown.notified())
                .await
                .unwrap();
            assert!(host.normal_shutdown_requested().unwrap());
            // SAFETY: even a failed terminal write must not leave another owned child alive at return.
            for process in &processes {
                assert_eq!(
                    unsafe { WaitForSingleObject(process.as_raw_handle(), 0) },
                    WAIT_OBJECT_0,
                    "owned child survived confirmed end; storage_failure={storage_failure}"
                );
            }
            if storage_failure {
                let snapshot: String = database
                    .query_row(
                        "SELECT snapshot FROM sessions WHERE id=?1",
                        [&second.session_id],
                        |row| row.get(0),
                    )
                    .unwrap();
                let snapshot: SessionSnapshot = serde_json::from_str(&snapshot).unwrap();
                assert_eq!(snapshot.state, SessionState::Stopped);
                assert!(
                    std::fs::read_to_string(root.join("logs/daemon.log"))
                        .unwrap()
                        .contains("WINDOWS_SESSION_END_FAILED")
                );
                database
                    .execute_batch("DROP TRIGGER fail_first_stop;")
                    .unwrap();
            }
            // A repeated notification may finish a failed write, but must not duplicate terminal events.
            assert_eq!(
                unsafe { SendMessageW(handle as HWND, WM_ENDSESSION, 1, flags as isize) },
                0
            );
            drop(database);
            let mut stopped = Vec::new();
            while let Ok(event) = events.try_recv() {
                if let Event::SessionChanged(value) = event.event
                    && value.session.state == SessionState::Stopped
                {
                    stopped.push(value.session.id);
                }
            }
            if storage_failure {
                stopped.sort();
            }
            assert_eq!(
                stopped,
                [params.session_id.clone(), second.session_id.clone()]
            );
            drop(window);
            drop(host);
            let host = Host::new(
                State::open(&root, BuildFlavor::Dev).unwrap(),
                &root,
                "next-boot".into(),
                "a".repeat(64),
            )
            .unwrap();
            for session_id in [&params.session_id, &second.session_id] {
                let RequestResult::Applied(after) = host
                    .dispatch(
                        Access::Local,
                        1,
                        Request::GetSession(GetSessionParams {
                            session_id: session_id.clone(),
                        }),
                    )
                    .unwrap()
                    .result
                else {
                    panic!("persisted")
                };
                let Response::Session(after) = after.result else {
                    panic!("persisted")
                };
                assert_eq!(after.session.session.state, SessionState::Stopped);
                assert_eq!(
                    after.session.session.stop_reason,
                    Some(SessionStopReason::AgentExited)
                );
                assert!(after.session.session.active_run_id.is_none());
            }
            println!(
                "M1_WINDOWS_SESSION_END_OK logoff={} storageFailure={} queryNoStop=true cancelledNoStop=true ownedCleanupBeforeReturn=true terminalPersisted=true repeatedEndIdempotent=true registration=false",
                flags != 0,
                storage_failure
            );
        }
    }
}
