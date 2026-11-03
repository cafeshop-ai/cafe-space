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
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "absolute XDG_RUNTIME_DIR is required",
                )
            })?;
        Ok(runtime
            .join(flavor.directory_name())
            .join(format!("{}.sock", flavor.daemon_name())))
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

#[cfg(unix)]
pub use unix::{ClientStream, Listener, ServerStream, connect};
#[cfg(windows)]
pub use windows::{ClientStream, Listener, ServerStream, connect};

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
            if let Ok(metadata) = std::fs::symlink_metadata(&self.path) {
                if metadata.ino() == self.identity.ino() && metadata.dev() == self.identity.dev() {
                    let _ = std::fs::remove_file(&self.path);
                }
            }
        }
    }

    pub async fn connect(path: &Path) -> io::Result<ClientStream> {
        let directory =
            crate::private::directory(path.parent().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "missing IPC parent")
            })?)?;
        let metadata = std::fs::symlink_metadata(path)?;
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
            self.pending.connect().await?;
            // Keep an instance alive while replacing it, so nobody can seize the pipe name.
            let next = create(&self.path, false)?;
            Ok(std::mem::replace(&mut self.pending, next))
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

    #[tokio::test]
    async fn real_endpoint_round_trip_excludes_second_listener_and_rebinds() {
        let temp = tempfile::tempdir().unwrap();
        let path = test_endpoint(&temp);
        let mut listener = Listener::bind(&path).await.unwrap();
        assert!(Listener::bind(&path).await.is_err());
        let mut client = connect(&path).await.unwrap();
        let mut server = listener.accept().await.unwrap();
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
