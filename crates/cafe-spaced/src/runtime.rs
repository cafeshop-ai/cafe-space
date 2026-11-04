use crate::{api, host::Host, state::State};
use cafe_space_ipc::{BuildFlavor, installation::binary_hash, local};
use std::{future::Future, io, path::Path, sync::Arc, time::Duration};
use tokio::task::JoinSet;

const MAX_CONNECTIONS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    Signal,
    Update,
}

pub async fn serve(
    flavor: BuildFlavor,
    root: &Path,
    endpoint: &Path,
    shutdown: impl Future<Output = io::Result<()>>,
) -> io::Result<StopReason> {
    let mut state = State::open(root, flavor)?;
    let mut listener = local::Listener::bind(endpoint).await?;
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(io::Error::other)?;
    let stream_id: String = random.iter().map(|b| format!("{b:02x}")).collect();
    state.mark_boot(&stream_id)?;
    let host = Arc::new(Host::new(
        state,
        root,
        stream_id,
        binary_hash(&std::env::current_exe()?)?,
    )?);
    let mut connections = JoinSet::new();
    let mut accept_backoff = Duration::from_millis(100);
    host.log("DAEMON_LISTENING")?;
    let mut process_poll = tokio::time::interval(Duration::from_millis(100));
    process_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tokio::pin!(shutdown);
    let result = loop {
        tokio::select! {
            biased;
            result = &mut shutdown => break result.map(|()| StopReason::Signal),
            _ = host.shutdown.notified() => break requested_reason(&host),
            _ = process_poll.tick() => { if let Err(error) = host.poll_processes() { break Err(error); } }
            finished = connections.join_next(), if !connections.is_empty() => {
                match finished {
                    Some(Ok(Ok(()))) => {}
                    Some(Ok(Err(_))) => host.log("IPC_CONNECTION_CLOSED")?,
                    Some(Err(_)) => host.log("IPC_HANDLER_FAILED")?,
                    None => {}
                }
            }
            stream = listener.accept(), if connections.len() < MAX_CONNECTIONS => {
                match stream {
                    Ok(stream) => {
                        accept_backoff = Duration::from_millis(100);
                        let host = host.clone();
                        connections.spawn(async move { api::serve_connection(stream, &host).await });
                    }
                    Err(error) if error.kind() == io::ErrorKind::PermissionDenied => host.log("IPC_PEER_REJECTED")?,
                    Err(error) if local::transient_accept_error(&error) => {
                        host.log("IPC_ACCEPT_RETRY")?;
                        tokio::select! { result = &mut shutdown => break result.map(|()| StopReason::Signal), _ = host.shutdown.notified() => break requested_reason(&host), _ = tokio::time::sleep(accept_backoff) => {} }
                        accept_backoff = (accept_backoff * 2).min(Duration::from_secs(1));
                    }
                    Err(error) => break Err(error),
                }
            }
        }
    };
    if matches!(&result, Ok(StopReason::Update)) {
        // Subscribed handlers flush draining before closing; idle/non-reading peers cannot hold up an update.
        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            while connections.join_next().await.is_some() {}
        })
        .await;
    }
    // Cancellation closes the entire connection; no partially read/written frame is resumed.
    connections.abort_all();
    while connections.join_next().await.is_some() {}
    let cleanup = host.stop_owned_processes();
    host.log("DAEMON_STOPPED")?;
    cleanup?;
    result
}

fn requested_reason(host: &Host) -> io::Result<StopReason> {
    host.normal_shutdown_requested().map(|normal| {
        if normal {
            StopReason::Signal
        } else {
            StopReason::Update
        }
    })
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
        let mut close = tokio::signal::windows::ctrl_close()?;
        let mut logoff = tokio::signal::windows::ctrl_logoff()?;
        let mut shutdown = tokio::signal::windows::ctrl_shutdown()?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = close.recv() => Ok(()),
            _ = logoff.recv() => Ok(()),
            _ = shutdown.recv() => Ok(()),
        }
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
        for by_request in [false, true] {
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
                if by_request {
                    std::future::pending().await
                } else {
                    stopped.await.map_err(io::Error::other)
                }
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
                    decode_server_message(&read_frame(&mut stream).await.unwrap().unwrap())
                        .unwrap(),
                    ServerMessage::Welcome(_)
                ));
                write_frame(
                    &mut stream,
                    include_bytes!("../../../fixtures/v2/client-request.json"),
                )
                .await
                .unwrap();
                assert!(matches!(
                    decode_server_message(&read_frame(&mut stream).await.unwrap().unwrap())
                        .unwrap(),
                    ServerMessage::Response(_)
                ));
                if by_request {
                    write_frame(
                        &mut stream,
                        include_bytes!("../../../fixtures/v2/shutdown-request.json"),
                    )
                    .await
                    .unwrap();
                    let reply =
                        decode_server_message(&read_frame(&mut stream).await.unwrap().unwrap())
                            .unwrap();
                    assert!(
                        matches!(reply, ServerMessage::Response(value) if matches!(&value.result, cafe_space_protocol::RequestResult::Applied(data) if matches!(data.result,cafe_space_protocol::Response::ShutdownAccepted)))
                    );
                } else {
                    stop.send(()).unwrap();
                }
            };
            let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
                tokio::join!(server, client)
            })
            .await
            .unwrap();
            assert_eq!(result.unwrap(), StopReason::Signal);
            let _reopened = State::open(&root, BuildFlavor::Dev).unwrap();
            let log = std::fs::read_to_string(root.join("logs/daemon.log")).unwrap();
            assert!(log.contains("DAEMON_LISTENING") && log.contains("DAEMON_STOPPED"));
            assert_eq!(log.contains("DAEMON_STOP_REQUESTED"), by_request);
        }
    }
}
