//! Owner-only local transport. No TCP listener and no command replay.

use crate::BuildFlavor;
use std::{
    io,
    path::{Path, PathBuf},
};

pub fn endpoint(flavor: BuildFlavor) -> io::Result<PathBuf> {
    #[cfg(windows)]
    {
        Ok(PathBuf::from(format!(
            r"\\.\pipe\{}-{}",
            flavor.daemon_name(),
            crate::windows_security::current_sid()?
        )))
    }
    #[cfg(target_os = "linux")]
    {
        let configured = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
        let standard = PathBuf::from(format!("/run/user/{}", crate::private::current_uid()));
        linux_endpoint(flavor, configured.as_deref(), &standard)
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let root = flavor.data_dir().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "user data directory is unavailable",
            )
        })?;
        let name = format!("{}.sock", flavor.daemon_name());
        let normal = root.join("run").join(&name);
        if normal.as_os_str().as_bytes().len() < 104 {
            return Ok(normal);
        }
        let temp = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "absolute per-user TMPDIR is required for long socket paths",
                )
            })?;
        let fallback = temp.join(flavor.directory_name()).join(name);
        if fallback.as_os_str().as_bytes().len() >= 104 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Unix socket path exceeds macOS limit",
            ));
        }
        Ok(fallback)
    }
}

#[cfg(target_os = "linux")]
fn linux_endpoint(
    flavor: BuildFlavor,
    configured: Option<&Path>,
    standard: &Path,
) -> io::Result<PathBuf> {
    let directory = if let Some(runtime) = configured {
        if !runtime.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "XDG_RUNTIME_DIR must be absolute",
            ));
        }
        runtime.join(flavor.directory_name())
    } else {
        match std::fs::symlink_metadata(standard) {
            Ok(_) => {
                drop(crate::private::directory(standard)?);
                standard.join(flavor.directory_name())
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => PathBuf::from(format!(
                "/tmp/{}-{}",
                flavor.directory_name(),
                crate::private::current_uid()
            )),
            Err(error) => return Err(error),
        }
    };
    Ok(directory.join(format!("{}.sock", flavor.daemon_name())))
}

/// Recoverable resource exhaustion and clients disappearing during accept must not kill the host.
pub fn transient_accept_error(error: &io::Error) -> bool {
    if matches!(
        error.kind(),
        io::ErrorKind::Interrupted
            | io::ErrorKind::WouldBlock
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::NotConnected
            | io::ErrorKind::OutOfMemory
    ) {
        return true;
    }
    #[cfg(unix)]
    {
        matches!(
            error.raw_os_error(),
            Some(libc::EMFILE | libc::ENFILE | libc::ENOBUFS | libc::ENOMEM)
        )
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{
            ERROR_NO_SYSTEM_RESOURCES, ERROR_NOT_ENOUGH_MEMORY, ERROR_OPERATION_ABORTED,
            ERROR_OUTOFMEMORY, ERROR_PIPE_NOT_CONNECTED, ERROR_TOO_MANY_OPEN_FILES,
        };
        error.raw_os_error().is_some_and(|code| {
            matches!(
                code as u32,
                ERROR_TOO_MANY_OPEN_FILES
                    | ERROR_NOT_ENOUGH_MEMORY
                    | ERROR_OUTOFMEMORY
                    | ERROR_NO_SYSTEM_RESOURCES
                    | ERROR_OPERATION_ABORTED
                    | ERROR_PIPE_NOT_CONNECTED
            )
        })
    }
}

#[cfg(unix)]
pub use unix::{ClientStream, Listener, ServerStream, connect};
#[cfg(windows)]
pub use windows::{ClientStream, Listener, ServerStream, connect};

/// Bind an update controller to the expected process, not another daemon using the same pipe name.
#[cfg(windows)]
pub fn server_process_id(stream: &ClientStream) -> io::Result<u32> {
    crate::windows_security::pipe_server_pid(stream)
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::{
        fs::{File, Metadata},
        os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
        time::Duration,
    };
    use tokio::net::{UnixListener, UnixStream};
    pub type ServerStream = UnixStream;
    pub type ClientStream = UnixStream;

    pub struct Listener {
        listener: UnixListener,
        path: PathBuf,
        identity: Metadata,
        _directory: File,
    }

    impl Listener {
        /// Caller must hold the data-directory lock until this listener is dropped.
        pub async fn bind(path: &Path) -> io::Result<Self> {
            let directory = crate::private::directory(path.parent().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "missing IPC parent")
            })?)?;
            match std::fs::symlink_metadata(path) {
                Ok(metadata) => {
                    if !metadata.file_type().is_socket()
                        || metadata.uid() != directory.metadata()?.uid()
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "refusing to replace non-owner socket path",
                        ));
                    }
                    match tokio::time::timeout(Duration::from_secs(1), UnixStream::connect(path))
                        .await
                    {
                        Ok(Err(e)) if e.kind() == io::ErrorKind::ConnectionRefused => {
                            std::fs::remove_file(path)?
                        }
                        _ => {
                            return Err(io::Error::new(
                                io::ErrorKind::AddrInUse,
                                "IPC endpoint is already in use",
                            ));
                        }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            let listener = UnixListener::bind(path)?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            let identity = std::fs::symlink_metadata(path)?;
            Ok(Self {
                listener,
                path: path.to_path_buf(),
                identity,
                _directory: directory,
            })
        }

        pub async fn accept(&mut self) -> io::Result<ServerStream> {
            let (stream, _) = self.listener.accept().await?;
            if stream.peer_cred()?.uid() != self.identity.uid() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "IPC peer is not the owner",
                ));
            }
            Ok(stream)
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            if let Ok(metadata) = std::fs::symlink_metadata(&self.path)
                && metadata.ino() == self.identity.ino()
                && metadata.dev() == self.identity.dev()
            {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }

    pub async fn connect(path: &Path) -> io::Result<ClientStream> {
        let metadata = std::fs::symlink_metadata(path)?;
        let directory =
            crate::private::directory(path.parent().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "missing IPC parent")
            })?)?;
        if !metadata.file_type().is_socket()
            || metadata.mode() & 0o777 != 0o600
            || metadata.uid() != directory.metadata()?.uid()
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "IPC socket is not owner-only",
            ));
        }
        let stream = UnixStream::connect(path).await?;
        if stream.peer_cred()?.uid() != metadata.uid() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "IPC server is not the owner",
            ));
        }
        Ok(stream)
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use crate::windows_security::{OwnerSecurity, check_owner_only};
    use tokio::net::windows::named_pipe::{
        ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
    };
    pub type ServerStream = NamedPipeServer;
    pub type ClientStream = NamedPipeClient;

    pub struct Listener {
        pending: NamedPipeServer,
        path: PathBuf,
    }

    fn validate_name(path: &Path) -> io::Result<()> {
        let name = path
            .to_str()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid pipe name"))?;
        if !name.starts_with(r"\\.\pipe\")
            || name.len() == 9
            || name[9..].contains(['\\', '/', '\0'])
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "local pipe name required",
            ));
        }
        Ok(())
    }

    fn create(path: &Path, first: bool) -> io::Result<NamedPipeServer> {
        validate_name(path)?;
        let security = OwnerSecurity::new(false)?;
        let mut attributes = security.attributes();
        #[allow(unsafe_code)]
        // SAFETY: attributes and its descriptor remain alive until synchronous pipe creation returns.
        unsafe {
            ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(
                    path,
                    std::ptr::addr_of_mut!(attributes).cast(),
                )
        }
    }

    impl Listener {
        /// first_pipe_instance prevents attaching to a pre-existing or squatted endpoint.
        pub async fn bind(path: &Path) -> io::Result<Self> {
            Ok(Self {
                pending: create(path, true)?,
                path: path.to_path_buf(),
            })
        }

        pub async fn accept(&mut self) -> io::Result<ServerStream> {
            let connected = self.pending.connect().await;
            // Replace failed instances too; keep the old one alive until its replacement exists.
            let next = create(&self.path, false)?;
            let stream = std::mem::replace(&mut self.pending, next);
            connected?;
            Ok(stream)
        }
    }

    pub async fn connect(path: &Path) -> io::Result<ClientStream> {
        validate_name(path)?;
        // Tokio's default SECURITY_IDENTIFICATION prevents server impersonation of this client.
        let stream = ClientOptions::new().open(path)?;
        check_owner_only(&stream)?;
        Ok(stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn absent_runtime_uses_private_standard_or_uid_scoped_tmp_without_repairing_bad_config() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let standard = temp.path().join("standard");
        let _guard = crate::private::directory(&standard).unwrap();
        assert_eq!(
            linux_endpoint(BuildFlavor::Dev, None, &standard).unwrap(),
            standard.join("cafe-space-dev/cafe-spaced-dev.sock")
        );
        let missing = temp.path().join("missing");
        let fallback = linux_endpoint(BuildFlavor::Dev, None, &missing).unwrap();
        assert_eq!(
            fallback,
            PathBuf::from(format!(
                "/tmp/cafe-space-dev-{}/cafe-spaced-dev.sock",
                crate::private::current_uid()
            ))
        );
        assert!(!missing.exists());
        assert_ne!(
            fallback,
            linux_endpoint(BuildFlavor::Release, None, &missing).unwrap()
        );
        assert!(linux_endpoint(BuildFlavor::Dev, Some(Path::new("relative")), &missing).is_err());
        std::fs::set_permissions(&standard, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(linux_endpoint(BuildFlavor::Dev, None, &standard).is_err());
        assert_eq!(
            std::fs::metadata(&standard).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn connecting_to_an_absent_socket_never_creates_its_directory() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("absent/socket");
        assert!(connect(&path).await.is_err());
        assert!(!path.parent().unwrap().exists());
    }
    use crate::framing::{read_frame, write_frame};

    fn test_endpoint(temp: &tempfile::TempDir) -> PathBuf {
        #[cfg(windows)]
        {
            PathBuf::from(format!(
                r"\\.\pipe\cafe-space-test-{}-{}",
                std::process::id(),
                temp.path().file_name().unwrap().to_str().unwrap()
            ))
        }
        #[cfg(unix)]
        {
            temp.path().join("ipc/s")
        }
    }

    #[test]
    fn accept_errors_retry_resources_and_disappearing_clients_but_not_broken_endpoints() {
        for kind in [
            io::ErrorKind::ConnectionAborted,
            io::ErrorKind::ConnectionReset,
            io::ErrorKind::BrokenPipe,
            io::ErrorKind::WouldBlock,
            io::ErrorKind::OutOfMemory,
        ] {
            assert!(transient_accept_error(&io::Error::from(kind)));
        }
        for kind in [
            io::ErrorKind::InvalidInput,
            io::ErrorKind::NotFound,
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::AddrInUse,
        ] {
            assert!(!transient_accept_error(&io::Error::from(kind)));
        }
        #[cfg(unix)]
        for code in [libc::EMFILE, libc::ENFILE, libc::ENOBUFS, libc::ENOMEM] {
            assert!(transient_accept_error(&io::Error::from_raw_os_error(code)));
        }
        #[cfg(windows)]
        for code in [4, 8, 14, 109, 232, 233, 995, 1450] {
            assert!(
                transient_accept_error(&io::Error::from_raw_os_error(code)),
                "OS error {code}"
            );
        }
    }

    #[tokio::test]
    async fn endpoint_accepts_new_client_after_an_early_disconnect() {
        let temp = tempfile::tempdir().unwrap();
        let path = test_endpoint(&temp);
        let mut listener = Listener::bind(&path).await.unwrap();
        drop(connect(&path).await.unwrap());
        match listener.accept().await {
            Ok(stream) => drop(stream),
            Err(error) => assert!(
                transient_accept_error(&error),
                "unexpected accept error: {error}"
            ),
        }
        let mut client = connect(&path).await.unwrap();
        let mut server = listener.accept().await.unwrap();
        let sending = write_frame(&mut client, b"new connection");
        let receiving = read_frame(&mut server);
        let (sent, received) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            tokio::join!(sending, receiving)
        })
        .await
        .unwrap();
        sent.unwrap();
        assert_eq!(received.unwrap().unwrap(), b"new connection");
    }

    #[tokio::test]
    async fn real_endpoint_round_trip_excludes_second_listener_and_rebinds() {
        let temp = tempfile::tempdir().unwrap();
        let path = test_endpoint(&temp);
        let mut listener = Listener::bind(&path).await.unwrap();
        let mut client = connect(&path).await.unwrap();
        let mut server = listener.accept().await.unwrap();
        #[cfg(windows)]
        assert_eq!(server_process_id(&client).unwrap(), std::process::id());
        // The Unix stale-socket probe briefly connects; don't mistake it for our test client.
        assert!(Listener::bind(&path).await.is_err());
        let sending = async {
            write_frame(&mut client, "中文 🧪".as_bytes())
                .await
                .unwrap();
        };
        let receiving = async {
            assert_eq!(
                read_frame(&mut server).await.unwrap().unwrap(),
                "中文 🧪".as_bytes()
            );
        };
        tokio::join!(sending, receiving);
        drop(client);
        drop(server);
        drop(listener);
        // Windows IOCP may release the last in-flight handle on the next reactor tick.
        let _rebound = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                match Listener::bind(&path).await {
                    Ok(listener) => break listener,
                    Err(error)
                        if cfg!(windows) && error.kind() == io::ErrorKind::PermissionDenied =>
                    {
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await
                    }
                    Err(error) => panic!("rebind failed: {error}"),
                }
            }
        })
        .await
        .unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stale_socket_is_recovered_but_a_regular_file_is_never_deleted() {
        let temp = tempfile::tempdir().unwrap();
        let path = test_endpoint(&temp);
        let _directory = crate::private::directory(path.parent().unwrap()).unwrap();
        drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
        let listener = Listener::bind(&path).await.unwrap();
        drop(listener);
        std::fs::write(&path, "do not delete").unwrap();
        assert!(Listener::bind(&path).await.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "do not delete");
    }
}
