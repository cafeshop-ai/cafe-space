//! Job assignment is atomic with creation; only explicit standard-stream handles are inherited.
#![allow(unsafe_code)]
use super::{Exit, checked_cwd, error};
use cafe_space_ipc::project_root::Directory;
use std::{
    ffi::{OsStr, OsString},
    io,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    ptr,
};
use windows_sys::Win32::{
    Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::{JobObjects::*, Threading::*},
};

fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut result: Vec<_> = value.encode_wide().collect();
    if result.contains(&0) {
        return Err(error("PROCESS_ARGUMENT_INVALID"));
    }
    result.push(0);
    Ok(result)
}
fn quoted(value: &OsStr, output: &mut Vec<u16>) -> io::Result<()> {
    output.push(b'"' as u16);
    let mut slashes = 0;
    for unit in value.encode_wide() {
        if unit == 0 {
            return Err(error("PROCESS_ARGUMENT_INVALID"));
        }
        if unit == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        output.extend(std::iter::repeat_n(
            b'\\' as u16,
            if unit == b'"' as u16 {
                slashes * 2 + 1
            } else {
                slashes
            },
        ));
        output.push(unit);
        slashes = 0;
    }
    output.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    output.push(b'"' as u16);
    Ok(())
}
fn inheritable(handle: &impl AsRawHandle) -> io::Result<OwnedHandle> {
    let mut duplicate = ptr::null_mut();
    // SAFETY: source is borrowed/live; DuplicateHandle initializes a new independently owned handle.
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            handle.as_raw_handle(),
            GetCurrentProcess(),
            &mut duplicate,
            0,
            1,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(error("PROCESS_PIPE_FAILED"));
    }
    // SAFETY: this function exclusively owns the successful duplicate.
    Ok(unsafe { OwnedHandle::from_raw_handle(duplicate) })
}
struct Attributes(Vec<usize>);
impl Attributes {
    fn new(
        handles: &[windows_sys::Win32::Foundation::HANDLE],
        jobs: &[windows_sys::Win32::Foundation::HANDLE],
    ) -> io::Result<Self> {
        let mut bytes = 0;
        // SAFETY: the sizing call writes bytes and intentionally receives no allocation.
        unsafe {
            InitializeProcThreadAttributeList(ptr::null_mut(), 2, 0, &mut bytes);
        }
        if bytes == 0 {
            return Err(error("PROCESS_HANDLE_LIST_FAILED"));
        }
        let mut storage = vec![0usize; bytes.div_ceil(std::mem::size_of::<usize>())];
        // SAFETY: storage is aligned and sized by the immediately preceding OS query.
        if unsafe {
            InitializeProcThreadAttributeList(storage.as_mut_ptr().cast(), 2, 0, &mut bytes)
        } == 0
        {
            return Err(error("PROCESS_HANDLE_LIST_FAILED"));
        }
        let mut list = Self(storage);
        for (attribute, values) in [
            (PROC_THREAD_ATTRIBUTE_HANDLE_LIST, handles),
            (PROC_THREAD_ATTRIBUTE_JOB_LIST, jobs),
        ] {
            // SAFETY: list is initialized; callers keep both handle arrays live until list destruction.
            if unsafe {
                UpdateProcThreadAttribute(
                    list.0.as_mut_ptr().cast(),
                    0,
                    attribute as usize,
                    values.as_ptr().cast(),
                    std::mem::size_of_val(values),
                    ptr::null_mut(),
                    ptr::null(),
                )
            } == 0
            {
                return Err(error("PROCESS_ATTRIBUTES_FAILED"));
            }
        }
        Ok(list)
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: only initialized lists reach this destructor; backing storage stays alive here.
        unsafe {
            DeleteProcThreadAttributeList(self.0.as_mut_ptr().cast());
        }
    }
}

pub(super) struct Owned {
    process: OwnedHandle,
    job: OwnedHandle,
    pid: u32,
    exit: Option<Exit>,
}
impl Owned {
    pub fn id(&self) -> u32 {
        self.pid
    }
    pub fn try_wait(&mut self) -> io::Result<Option<Exit>> {
        if self.exit.is_some() {
            return Ok(self.exit);
        }
        // SAFETY: the owned process handle is live, and zero timeout never blocks.
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), 0) } {
            WAIT_TIMEOUT => return Ok(None),
            WAIT_OBJECT_0 => {}
            _ => return Err(error("PROCESS_WAIT_FAILED")),
        }
        let mut code = 0;
        // SAFETY: process has exited; code is valid writable storage.
        if unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) } == 0 {
            return Err(error("PROCESS_WAIT_FAILED"));
        }
        // Natural target exit must not leave its still-running grandchildren behind.
        // SAFETY: this non-breakaway job belongs exclusively to this process owner.
        if unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) } == 0 {
            return Err(error("PROCESS_STOP_FAILED"));
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
            // SAFETY: a live job and correctly sized writable accounting structure are provided.
            if unsafe {
                QueryInformationJobObject(
                    self.job.as_raw_handle(),
                    JobObjectBasicAccountingInformation,
                    (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                    std::mem::size_of_val(&accounting) as u32,
                    ptr::null_mut(),
                )
            } == 0
            {
                return Err(error("PROCESS_WAIT_FAILED"));
            }
            if accounting.ActiveProcesses == 0 {
                break;
            }
            if std::time::Instant::now() >= deadline {
                return Err(error("PROCESS_STOP_TIMEOUT"));
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        self.exit = Some(Exit {
            code: Some(code as i32),
            signal: None,
        });
        Ok(self.exit)
    }
    pub fn stop(&mut self) -> io::Result<Exit> {
        if let Some(exit) = self.try_wait()? {
            return Ok(exit);
        }
        // SAFETY: job contains only our assigned tree; no PID lookup or reuse is involved.
        if unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) } == 0 {
            return Err(error("PROCESS_STOP_FAILED"));
        }
        // SAFETY: the process handle remains owned while waiting for kernel-confirmed termination.
        if unsafe { WaitForSingleObject(self.process.as_raw_handle(), 5000) } != WAIT_OBJECT_0 {
            return Err(error("PROCESS_STOP_TIMEOUT"));
        }
        self.try_wait()?
            .ok_or_else(|| error("PROCESS_STOP_TIMEOUT"))
    }
}

pub(super) fn spawn(
    program: &Path,
    args: &[OsString],
    directory: &Directory,
    mut env: Vec<(OsString, OsString)>,
    input: std::io::PipeReader,
    output: std::io::PipeWriter,
    errors: std::io::PipeWriter,
) -> io::Result<Owned> {
    let executable = wide(program.as_os_str())?;
    let cwd = wide(directory.path.as_os_str())?;
    let mut command = Vec::new();
    quoted(program.as_os_str(), &mut command)?;
    for arg in args {
        command.push(b' ' as u16);
        quoted(arg, &mut command)?;
    }
    command.push(0);
    if command.len() > 32767 {
        return Err(error("PROCESS_ARGUMENT_INVALID"));
    }
    env.sort_by_key(|(name, _)| name.to_string_lossy().to_ascii_uppercase());
    let mut environment = Vec::new();
    for (name, value) in env {
        let name = wide(&name)?;
        environment.extend_from_slice(&name[..name.len() - 1]);
        environment.push(b'=' as u16);
        environment.extend(wide(&value)?);
    }
    environment.push(0);
    if environment.len() == 1 {
        environment.push(0);
    }
    let input = inheritable(&input)?;
    let output = inheritable(&output)?;
    let errors = inheritable(&errors)?;
    let handles = [
        input.as_raw_handle(),
        output.as_raw_handle(),
        errors.as_raw_handle(),
    ];
    // SAFETY: null security attributes create a non-inheritable anonymous job.
    let raw_job = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
    if raw_job.is_null() {
        return Err(error("PROCESS_JOB_FAILED"));
    }
    // SAFETY: the successful new job handle has a unique owner.
    let job = unsafe { OwnedHandle::from_raw_handle(raw_job) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: limits has the exact layout/size required by this information class.
    if unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of_val(&limits) as u32,
        )
    } == 0
    {
        return Err(error("PROCESS_JOB_FAILED"));
    }
    // Post-creation assignment leaves an orphan if the daemon dies before AssignProcessToJobObject.
    let jobs = [job.as_raw_handle()];
    let mut attributes = Attributes::new(&handles, &jobs)?;
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = handles[0];
    startup.StartupInfo.hStdOutput = handles[1];
    startup.StartupInfo.hStdError = handles[2];
    startup.lpAttributeList = attributes.0.as_mut_ptr().cast();
    let mut info = PROCESS_INFORMATION::default();
    checked_cwd(directory)?;
    // SAFETY: all terminated strings, environment, stream handles and the initialized attribute list stay live across this call.
    if unsafe {
        CreateProcessW(
            executable.as_ptr(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            1,
            CREATE_SUSPENDED
                | CREATE_UNICODE_ENVIRONMENT
                | CREATE_NO_WINDOW
                | EXTENDED_STARTUPINFO_PRESENT,
            environment.as_ptr().cast(),
            cwd.as_ptr(),
            &startup.StartupInfo,
            &mut info,
        )
    } == 0
    {
        return Err(error("PROCESS_SPAWN_FAILED"));
    }
    // SAFETY: CreateProcessW initialized two newly owned handles; the primary thread is still suspended.
    let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
    // SAFETY: the successful thread handle has a unique owner and is not yet resumed.
    let thread = unsafe { OwnedHandle::from_raw_handle(info.hThread) };
    // SAFETY: CreateProcessW assigned the job atomically and the primary thread handle is live.
    if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
        // SAFETY: this exclusively owned job contains the still-suspended target.
        unsafe {
            TerminateJobObject(job.as_raw_handle(), 1);
            WaitForSingleObject(process.as_raw_handle(), 5000);
        }
        return Err(error("PROCESS_RESUME_FAILED"));
    }
    Ok(Owned {
        process,
        job,
        pid: info.dwProcessId,
        exit: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_owns_suspended_child_before_the_first_instruction() {
        // SAFETY: new anonymous non-inheritable job, uniquely owned below.
        let raw = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        assert!(!raw.is_null());
        // SAFETY: CreateJobObjectW succeeded and ownership is transferred once.
        let job = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: valid job and correctly sized job-limit structure.
        assert_ne!(
            unsafe {
                SetInformationJobObject(
                    job.as_raw_handle(),
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of_val(&limits) as u32,
                )
            },
            0
        );
        let (reader, writer) = std::io::pipe().unwrap();
        let input = inheritable(&reader).unwrap();
        let output = inheritable(&writer).unwrap();
        let errors = inheritable(&writer).unwrap();
        let handles = [
            input.as_raw_handle(),
            output.as_raw_handle(),
            errors.as_raw_handle(),
        ];
        let jobs = [job.as_raw_handle()];
        let mut attributes = Attributes::new(&handles, &jobs).unwrap();
        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = handles[0];
        startup.StartupInfo.hStdOutput = handles[1];
        startup.StartupInfo.hStdError = handles[2];
        startup.lpAttributeList = attributes.0.as_mut_ptr().cast();
        let executable = wide(std::env::current_exe().unwrap().as_os_str()).unwrap();
        let mut info = PROCESS_INFORMATION::default();
        // SAFETY: valid executable, streams and attributes stay live; this test never resumes the child.
        assert_ne!(
            unsafe {
                CreateProcessW(
                    executable.as_ptr(),
                    ptr::null_mut(),
                    ptr::null(),
                    ptr::null(),
                    1,
                    CREATE_SUSPENDED | CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
                    ptr::null(),
                    ptr::null(),
                    &startup.StartupInfo,
                    &mut info,
                )
            },
            0
        );
        // SAFETY: the successful create returned independently owned process/thread handles.
        let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
        // SAFETY: this thread handle is uniquely owned and remains suspended throughout the test.
        let _thread = unsafe { OwnedHandle::from_raw_handle(info.hThread) };
        let mut member = 0;
        // SAFETY: both handles are live and member is writable storage.
        let queried =
            unsafe { IsProcessInJob(process.as_raw_handle(), job.as_raw_handle(), &mut member) };
        drop(attributes);
        drop(job);
        // SAFETY: process handle remains live after the last job handle closes.
        let wait = unsafe { WaitForSingleObject(process.as_raw_handle(), 5000) };
        if wait != WAIT_OBJECT_0 {
            // SAFETY: ensure a failing regression test cannot leak its own suspended process.
            unsafe {
                TerminateProcess(process.as_raw_handle(), 1);
                WaitForSingleObject(process.as_raw_handle(), 5000);
            }
        }
        assert_ne!(queried, 0);
        assert_ne!(
            member, 0,
            "ownership must exist at creation, not after assignment"
        );
        assert_eq!(
            wait, WAIT_OBJECT_0,
            "job close must kill the still-suspended child"
        );
        println!(
            "M1_PROCESS_ATOMIC_JOB_OK assignedAtCreation=true neverResumed=true jobCloseKilled=true"
        );
    }

    #[test]
    fn command_line_quotes_preserve_empty_quotes_and_trailing_slashes() {
        for (input, expected) in [
            ("", "\"\""),
            ("Tea 茶", "\"Tea 茶\""),
            ("a\"b", "\"a\\\"b\""),
            ("C:\\tea\\", "\"C:\\tea\\\\\""),
        ] {
            let mut output = Vec::new();
            quoted(OsStr::new(input), &mut output).unwrap();
            assert_eq!(String::from_utf16(&output).unwrap(), expected);
        }
    }
}
