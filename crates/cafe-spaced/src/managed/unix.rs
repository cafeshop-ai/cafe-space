//! A live supervisor anchors the process group; a pipe EOF, not a remembered PID, triggers cleanup.
#![allow(unsafe_code)]
use super::{Exit, checked_cwd, error};
use cafe_space_ipc::project_root::{self, Directory};
use std::{
    ffi::OsString,
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::process::{CommandExt, ExitStatusExt},
    },
    path::Path,
    process::{Child, Command, ExitCode, Stdio},
    time::{Duration, Instant},
};

pub(super) struct Owned {
    child: Child,
    lease: Option<std::io::PipeWriter>,
    report: std::io::PipeReader,
    exit: Option<Exit>,
}
impl Owned {
    pub fn id(&self) -> u32 {
        self.child.id()
    }
    pub fn try_wait(&mut self) -> io::Result<Option<Exit>> {
        if self.exit.is_some() {
            return Ok(self.exit);
        }
        if self.child.try_wait()?.is_none() {
            return Ok(None);
        }
        self.lease.take();
        let mut data = Vec::new();
        (&mut self.report).take(9).read_to_end(&mut data)?;
        self.exit = Some(if data.len() == 8 {
            let code = i32::from_le_bytes(data[..4].try_into().unwrap());
            let signal = i32::from_le_bytes(data[4..].try_into().unwrap());
            Exit {
                code: (code != -1).then_some(code),
                signal: (signal != -1).then_some(signal),
            }
        } else if data.is_empty() {
            Exit {
                code: None,
                signal: None,
            }
        } else {
            return Err(error("PROCESS_SUPERVISOR_STATUS_INVALID"));
        });
        Ok(self.exit)
    }
    pub fn stop(&mut self) -> io::Result<Exit> {
        self.lease.take();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(exit) = self.try_wait()? {
                return Ok(exit);
            }
            if Instant::now() >= deadline {
                return Err(error("PROCESS_STOP_TIMEOUT"));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn duplicate_above_standard(file: &impl AsRawFd) -> io::Result<OwnedFd> {
    // SAFETY: borrowed fd is live; F_DUPFD_CLOEXEC creates an independent descriptor >= 5.
    let fd = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 5) };
    if fd < 0 {
        return Err(error("PROCESS_PIPE_FAILED"));
    }
    // SAFETY: only this function owns the newly returned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

pub(super) fn spawn(
    program: &Path,
    args: &[OsString],
    directory: &Directory,
    env: Vec<(OsString, OsString)>,
    input: std::io::PipeReader,
    output: std::io::PipeWriter,
    errors: std::io::PipeWriter,
) -> io::Result<Owned> {
    let (lease_read, lease) = std::io::pipe()?;
    let (report, report_write) = std::io::pipe()?;
    let lease_fd = duplicate_above_standard(&lease_read)?;
    let report_fd = duplicate_above_standard(&report_write)?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("--managed-supervisor")
        .arg(&directory.identity)
        .arg(program)
        .args(args)
        .current_dir(&directory.path)
        .env_clear()
        .envs(env)
        .stdin(Stdio::from(input))
        .stdout(Stdio::from(output))
        .stderr(Stdio::from(errors))
        .process_group(0);
    let read = lease_fd.as_raw_fd();
    let write = report_fd.as_raw_fd();
    // SAFETY: the post-fork closure only uses async-signal-safe dup2 and constructs a fixed error.
    // Both sources are >= 5, so mapping the first cannot overwrite the second.
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(read, 3) < 0 || libc::dup2(write, 4) < 0 {
                return Err(io::Error::from_raw_os_error(libc::EIO));
            }
            Ok(())
        });
    }
    checked_cwd(directory)?;
    let child = command.spawn().map_err(|_| error("PROCESS_SPAWN_FAILED"))?;
    Ok(Owned {
        child,
        lease: Some(lease),
        report,
        exit: None,
    })
}

fn inherited_pipe(fd: i32, access: i32) -> io::Result<()> {
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fd is checked before ownership is assumed; metadata is correctly sized writable storage.
    if unsafe { libc::fstat(fd, metadata.as_mut_ptr()) } != 0 {
        return Err(error("PROCESS_SUPERVISOR_INVALID"));
    }
    // SAFETY: fstat succeeded and initialized metadata.
    if unsafe { metadata.assume_init() }.st_mode & libc::S_IFMT != libc::S_IFIFO {
        return Err(error("PROCESS_SUPERVISOR_INVALID"));
    }
    // SAFETY: fcntl reads fd flags and then marks this valid descriptor close-on-exec.
    if unsafe { libc::fcntl(fd, libc::F_GETFL) } & libc::O_ACCMODE != access
        || unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } != 0
    {
        return Err(error("PROCESS_SUPERVISOR_INVALID"));
    }
    Ok(())
}
fn close_other_descriptors() -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: no threads exist in this internal entry point; only descriptors >= 5 are closed.
        if unsafe { libc::syscall(libc::SYS_close_range, 5u32, u32::MAX, 0u32) } == 0 {
            return Ok(());
        }
    }
    #[cfg(target_os = "linux")]
    let descriptors: Vec<i32> = {
        // Collect before closing so enumeration never closes its own directory mid-iteration.
        let mut descriptors = Vec::new();
        for entry in std::fs::read_dir("/proc/self/fd")?.take(65537) {
            if descriptors.len() == 65536 {
                return Err(error("PROCESS_SUPERVISOR_INVALID"));
            }
            let entry = entry?;
            let fd = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse().ok())
                .ok_or_else(|| error("PROCESS_SUPERVISOR_INVALID"))?;
            descriptors.push(fd);
        }
        descriptors
    };
    #[cfg(target_os = "macos")]
    let descriptors: Vec<i32> = {
        // SAFETY: query our own single-threaded process's fd table without allocating a descriptor.
        let bytes = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDLISTFDS,
                0,
                std::ptr::null_mut(),
                0,
            )
        };
        if bytes <= 0 || bytes > 1_048_576 {
            return Err(error("PROCESS_SUPERVISOR_INVALID"));
        }
        let mut entries = vec![
            libc::proc_fdinfo {
                proc_fd: 0,
                proc_fdtype: 0
            };
            (bytes as usize).div_ceil(std::mem::size_of::<libc::proc_fdinfo>())
                + 16
        ];
        let size = std::mem::size_of_val(entries.as_slice());
        // SAFETY: entries is sized/aligned fd-info storage and pid is still this process.
        let count = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDLISTFDS,
                0,
                entries.as_mut_ptr().cast(),
                size as i32,
            )
        };
        if count <= 0
            || count as usize >= size
            || !(count as usize).is_multiple_of(std::mem::size_of::<libc::proc_fdinfo>())
        {
            return Err(error("PROCESS_SUPERVISOR_INVALID"));
        }
        entries[..count as usize / std::mem::size_of::<libc::proc_fdinfo>()]
            .iter()
            .map(|entry| entry.proc_fd)
            .collect()
    };
    for fd in descriptors.into_iter().filter(|fd| *fd >= 5) {
        // SAFETY: no other threads can reuse descriptors during this internal entry point.
        unsafe {
            libc::close(fd);
        }
    }
    Ok(())
}
fn kill_group() -> ! {
    // SAFETY: supervisor entry validated that this process is the leader of its own fresh group.
    unsafe {
        libc::kill(0, libc::SIGKILL);
        libc::_exit(127);
    }
}

pub(super) fn supervisor(args: &[OsString]) -> io::Result<ExitCode> {
    // SAFETY: getpid/getpgrp only read kernel process identities; never signal an unvalidated group.
    if args.len() < 2
        || unsafe { libc::getpid() != libc::getpgrp() }
        || !Path::new(&args[1]).is_absolute()
    {
        return Err(error("PROCESS_SUPERVISOR_INVALID"));
    }
    inherited_pipe(3, libc::O_RDONLY)?;
    inherited_pipe(4, libc::O_WRONLY)?;
    close_other_descriptors()?;
    // SAFETY: ownership is transferred only after both inherited descriptor kinds were validated.
    let _lease = unsafe { File::from_raw_fd(3) };
    // SAFETY: fd 4 is the validated, exclusively owned status writer.
    let mut report = unsafe { File::from_raw_fd(4) };
    let cwd = std::env::current_dir()?;
    let current = project_root::inspect(&cwd)?;
    if args[0].as_os_str() != std::ffi::OsStr::new(&current.identity) {
        return Err(error("PROJECT_ROOT_CHANGED"));
    }
    let mut watch = libc::pollfd {
        fd: 3,
        events: libc::POLLIN | libc::POLLHUP | libc::POLLERR,
        revents: 0,
    };
    // SAFETY: watch is one live, writable pollfd; zero timeout checks an already-ended lease.
    if unsafe { libc::poll(&mut watch, 1, 0) } != 0 {
        kill_group();
    }
    let child = Command::new(&args[1])
        .args(&args[2..])
        .current_dir(&cwd)
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(_) => {
            let _ = report.write_all(&[127, 0, 0, 0, 255, 255, 255, 255]);
            kill_group();
        }
    };
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut bytes = [0u8; 8];
                bytes[..4].copy_from_slice(&status.code().unwrap_or(-1).to_le_bytes());
                bytes[4..].copy_from_slice(&status.signal().unwrap_or(-1).to_le_bytes());
                let _ = report.write_all(&bytes);
                kill_group();
            }
            Ok(None) => {}
            Err(_) => kill_group(),
        }
        // SAFETY: the validated lease fd remains live; a bounded poll also observes natural target exit.
        let result = unsafe { libc::poll(&mut watch, 1, 20) };
        if result > 0
            || (result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted)
        {
            kill_group();
        }
    }
}
