//! Owned local process trees; not an agent sandbox. No production runtime is selected here.
use cafe_space_ipc::project_root::{self, Directory};
use std::{
    ffi::{OsStr, OsString},
    io::{self, Read},
    path::Path,
};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as native;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as native;

pub const OUTPUT_CHUNK_BYTES: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exit {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

pub struct Output(std::io::PipeReader);
impl Read for Output {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = buffer.len().min(OUTPUT_CHUNK_BYTES);
        self.0.read(&mut buffer[..count])
    }
}

pub struct Streams {
    pub stdin: std::io::PipeWriter,
    pub stdout: Output,
    pub stderr: Output,
}

pub struct Process {
    native: native::Owned,
    pub stdin: Option<std::io::PipeWriter>,
    pub stdout: Option<Output>,
    pub stderr: Option<Output>,
}

fn error(code: &'static str) -> io::Error {
    io::Error::other(code)
}

pub(super) fn checked_cwd(directory: &Directory) -> io::Result<()> {
    let current = project_root::inspect(&directory.path)?;
    if current.identity != directory.identity {
        return Err(error("PROJECT_ROOT_CHANGED"));
    }
    Ok(())
}

fn environment() -> Vec<(OsString, OsString)> {
    [
        "PATH",
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "SystemRoot",
        "COMSPEC",
        "TEMP",
        "TMP",
        "TMPDIR",
        "USER",
        "LOGNAME",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
    ]
    .into_iter()
    .filter_map(|name| std::env::var_os(name).map(|value| (name.into(), value)))
    .collect()
}

impl Process {
    /// The approved directory comes from projects::checked under the Host's admission lock.
    pub fn spawn(program: &Path, args: &[OsString], directory: &Directory) -> io::Result<Self> {
        if !program.is_absolute() || !program.is_file() {
            return Err(error("PROCESS_EXECUTABLE_UNAVAILABLE"));
        }
        let (input, stdin) = std::io::pipe()?;
        let (stdout, output) = std::io::pipe()?;
        let (stderr, errors) = std::io::pipe()?;
        let native = native::spawn(
            program,
            args,
            directory,
            environment(),
            input,
            output,
            errors,
        )?;
        Ok(Self {
            native,
            stdin: Some(stdin),
            stdout: Some(Output(stdout)),
            stderr: Some(Output(stderr)),
        })
    }
    pub fn take_streams(&mut self) -> io::Result<Streams> {
        Ok(Streams {
            stdin: self
                .stdin
                .take()
                .ok_or_else(|| error("PROCESS_STREAMS_TAKEN"))?,
            stdout: self
                .stdout
                .take()
                .ok_or_else(|| error("PROCESS_STREAMS_TAKEN"))?,
            stderr: self
                .stderr
                .take()
                .ok_or_else(|| error("PROCESS_STREAMS_TAKEN"))?,
        })
    }
    pub fn id(&self) -> u32 {
        self.native.id()
    }
    pub fn try_wait(&mut self) -> io::Result<Option<Exit>> {
        self.native.try_wait()
    }
    /// Reap before releasing the ownership object; never signal a persisted PID.
    pub fn stop(&mut self) -> io::Result<Exit> {
        self.stdin.take();
        self.native.stop()
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Handled before any state directory, SQLite connection or async runtime is opened.
pub fn supervisor(args: &[OsString]) -> Option<io::Result<std::process::ExitCode>> {
    if args.first().map(OsString::as_os_str) != Some(OsStr::new("--managed-supervisor")) {
        return None;
    }
    #[cfg(unix)]
    {
        Some(unix::supervisor(&args[1..]))
    }
    #[cfg(windows)]
    {
        Some(Err(error("PROCESS_SUPERVISOR_UNSUPPORTED")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn process_contract_pins_bounds_and_native_ownership_without_a_wire_launcher() {
        let contract: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../fixtures/process-ownership/contract.json"
        ))
        .unwrap();
        assert_eq!(contract["outputChunkBytes"], OUTPUT_CHUNK_BYTES);
        assert_eq!(contract["cases"].as_array().unwrap().len(), 8);
        assert_eq!(contract["windows"]["breakaway"], false);
        assert_eq!(
            contract["posix"]["agentInheritedDescriptors"],
            serde_json::json!([0, 1, 2])
        );
        for (name, _) in environment() {
            let name = name.to_string_lossy().to_ascii_uppercase();
            assert!(
                !name.contains("TOKEN")
                    && !name.contains("SECRET")
                    && !name.starts_with("CAFE_SPACE_")
                    && name != "NODE_OPTIONS"
                    && name != "LD_PRELOAD"
            );
        }
    }
}
