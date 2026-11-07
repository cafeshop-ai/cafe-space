//! Real owned pipes and the official SDK, but only our credential-free synthetic ACP peer.
use super::*;
use cafe_space_adapters::acp::{self, Event, schema::StopReason};
use cafe_space_protocol::ApprovalDecision;
use std::io::BufReader;
use tokio::sync::mpsc;

pub(super) fn run() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    for (mode, resume, expected) in [
        ("normal", false, None),
        ("normal", true, None),
        ("permission", false, None),
        ("cancel", false, None),
        ("crash", false, Some("ACP_COMMAND_OUTCOME_UNKNOWN")),
        ("malformed", false, Some("ACP_COMMAND_OUTCOME_UNKNOWN")),
        ("wrong-session", false, Some("ACP_SESSION_MISMATCH")),
        ("load-fails", true, Some("ACP_REQUEST_FAILED")),
    ] {
        let root = tempfile::tempdir().unwrap();
        let node = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|path| path.join(if cfg!(windows) { "node.exe" } else { "node" }))
            .find(|path| path.is_file())
            .expect("native Node required for offline ACP fixture")
            .canonicalize()
            .unwrap();
        let script = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test/fake-acp.mjs")
            .canonicalize()
            .unwrap();
        let args = [
            cafe_space_adapters::pi::install::node_path(&script).into_os_string(),
            mode.into(),
        ];
        let mut owner =
            Process::spawn(&node, &args, &project_root::inspect(root.path()).unwrap()).unwrap();
        let streams = owner.take_streams().unwrap();
        let (send, input) = mpsc::channel(acp::QUEUE);
        let (output, mut writes) = mpsc::channel::<String>(acp::QUEUE);
        let (events, mut observed) = mpsc::channel(acp::QUEUE);
        let reader = std::thread::spawn(move || {
            let mut reader = BufReader::new(streams.stdout);
            loop {
                match acp::framing::read_line(&mut reader) {
                    Ok(Some(line)) => {
                        if send.blocking_send(Ok(line)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = send.blocking_send(Err(std::io::Error::other(error)));
                        break;
                    }
                }
            }
        });
        let writer = std::thread::spawn(move || {
            let mut stdin = streams.stdin;
            while let Some(line) = writes.blocking_recv() {
                if acp::framing::write_line(&mut stdin, &line).is_err() {
                    break;
                }
            }
        });
        let stderr = std::thread::spawn(move || {
            let mut stderr = streams.stderr;
            std::io::copy(&mut stderr, &mut std::io::sink()).unwrap()
        });
        let result = runtime.block_on(async {
            tokio::time::timeout(
                Duration::from_secs(10),
                acp::connect(
                    input,
                    output,
                    events,
                    root.path().into(),
                    resume.then(|| "private-synthetic-session".into()),
                    Box::new(move |session| {
                        Box::pin(async move {
                            match observed.recv().await {
                                Some(Event::Opened { native_id, .. }) => {
                                    assert_eq!(native_id, "private-synthetic-session")
                                }
                                _ => panic!("missing native ACP identity"),
                            }
                            let prompt = session.prompt("synthetic only".into());
                            let controls = async {
                                loop {
                                    match observed.recv().await {
                                        Some(Event::Permission { id, .. }) => {
                                            if mode == "cancel" {
                                                session.cancel().await?;
                                            } else {
                                                session
                                                    .approve(id, ApprovalDecision::AllowOnce)
                                                    .await?;
                                            }
                                            return Ok::<_, acp::Error>(());
                                        }
                                        Some(Event::Update(_))
                                            if mode != "permission" && mode != "cancel" =>
                                        {
                                            return Ok(());
                                        }
                                        Some(_) => {}
                                        None => return Err(acp::Error("ACP_OBSERVER_CLOSED")),
                                    }
                                }
                            };
                            let (response, ()) = tokio::try_join!(prompt, controls)?;
                            assert_eq!(
                                response.stop_reason,
                                if mode == "cancel" {
                                    StopReason::Cancelled
                                } else {
                                    StopReason::EndTurn
                                }
                            );
                            Ok(())
                        })
                    }),
                ),
            )
            .await
        });
        owner.stop().unwrap();
        assert!(owner.try_wait().unwrap().is_some());
        reader.join().unwrap();
        writer.join().unwrap();
        assert_eq!(
            stderr.join().unwrap(),
            0,
            "synthetic ACP peer reported an error"
        );
        assert_eq!(
            result.expect("owned ACP deadline"),
            expected.map_or(Ok(()), |code| Err(acp::Error(code))),
            "mode={mode}"
        );
    }
    println!("M1_ACP_OWNED_FIXTURE_OK cases=8 sdk=5.0.0 wire=1 realAgent=false ownedCleanup=true");
}
