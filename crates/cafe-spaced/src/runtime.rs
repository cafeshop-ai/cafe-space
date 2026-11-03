use crate::{api, state::State};
use cafe_space_ipc::{BuildFlavor, local};
use std::{future::Future, io, path::Path};
use tokio::task::JoinSet;

const MAX_CONNECTIONS: usize = 32;

pub async fn serve(
    flavor: BuildFlavor,
    root: &Path,
    endpoint: &Path,
    shutdown: impl Future<Output = io::Result<()>>,
) -> io::Result<()> {
    let mut state = State::open(root, flavor)?;
    let mut listener = local::Listener::bind(endpoint).await?;
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(io::Error::other)?;
    let stream_id: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let mut connections = JoinSet::new();
    state.log("DAEMON_LISTENING")?;
    tokio::pin!(shutdown);
    let result = loop {
        tokio::select! {
            biased;
            result = &mut shutdown => break result,
            finished = connections.join_next(), if !connections.is_empty() => {
                match finished {
                    Some(Ok(Ok(()))) => {}
                    Some(Ok(Err(_))) => state.log("IPC_CONNECTION_CLOSED")?,
                    Some(Err(_)) => state.log("IPC_HANDLER_FAILED")?,
                    None => {}
                }
            }
            stream = listener.accept(), if connections.len() < MAX_CONNECTIONS => {
                match stream {
                    Ok(stream) => {
                        let stream_id = stream_id.clone();
                        connections.spawn(async move { api::serve_connection(stream, &stream_id).await });
                    }
                    Err(error) if error.kind() == io::ErrorKind::PermissionDenied => state.log("IPC_PEER_REJECTED")?,
                    Err(error) => break Err(error),
                }
            }
        }
    };
    // Cancellation closes the entire connection; no partially read/written frame is resumed.
    connections.abort_all();
    while connections.join_next().await.is_some() {}
    state.log("DAEMON_STOPPED")?;
    result
}

pub async fn shutdown_signal() -> io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(windows)]
    {
        tokio::signal::ctrl_c().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cafe_space_ipc::framing::{read_frame, write_frame};
    use cafe_space_protocol::{ServerMessage, decode_server_message};
    use std::time::Duration;

    #[tokio::test]
    async fn daemon_serves_real_ipc_and_stops_without_replaying_requests() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(BuildFlavor::Dev.directory_name());
        #[cfg(unix)]
        let endpoint = temp.path().join("ipc/s");
        #[cfg(windows)]
        let endpoint = std::path::PathBuf::from(format!(
            r"\\.\pipe\cafe-spaced-runtime-test-{}-{}",
            std::process::id(),
            temp.path().file_name().unwrap().to_str().unwrap()
        ));
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let server = serve(BuildFlavor::Dev, &root, &endpoint, async {
            stopped.await.map_err(io::Error::other)
        });
        let client = async {
            let mut stream = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    match local::connect(&endpoint).await {
                        Ok(stream) => break stream,
                        Err(e)
                            if matches!(
                                e.kind(),
                                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                            ) =>
                        {
                            tokio::time::sleep(Duration::from_millis(10)).await
                        }
                        Err(e) => panic!("connect failed: {e}"),
                    }
                }
            })
            .await
            .unwrap();
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
            write_frame(
                &mut stream,
                include_bytes!("../../../fixtures/v2/client-request.json"),
            )
            .await
            .unwrap();
            assert!(matches!(
                decode_server_message(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap(),
                ServerMessage::Response(_)
            ));
            stop.send(()).unwrap();
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(server, client)
        })
        .await
        .unwrap();
        result.unwrap();
        let _reopened = State::open(&root, BuildFlavor::Dev).unwrap();
        let log = std::fs::read_to_string(root.join("logs/daemon.log")).unwrap();
        assert!(log.contains("DAEMON_LISTENING") && log.contains("DAEMON_STOPPED"));
    }
}
