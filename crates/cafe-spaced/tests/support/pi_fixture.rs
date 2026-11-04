//! Included by the standalone managed harness, so POSIX supervisor entry runs before libtest.
use super::*;
use cafe_space_adapters::pi::{
    self, Command as RpcCommand, Observation, Outcome, Session,
    framing::{Records, write_record},
};
use cafe_space_protocol::{ApprovalDecision, DeliveryMode, SessionState};
use std::sync::mpsc::{Receiver, sync_channel};

struct Fake {
    session: Session,
    owner: Process,
    stdin: std::io::PipeWriter,
    records: Receiver<pi::Result<Option<pi::framing::Record>>>,
    stderr: Option<std::thread::JoinHandle<usize>>,
}
pub(super) fn spawn_fake(root: &Path, mode: &str) -> Process {
    let node = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|path| path.join(if cfg!(windows) { "node.exe" } else { "node" }))
        .find(|path| path.is_file())
        .expect("native Node required for offline fixture")
        .canonicalize()
        .unwrap();
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/fake-pi.mjs")
        .canonicalize()
        .unwrap();
    let args: Vec<OsString> = [
        pi::install::node_path(&script).into_os_string(),
        "--mode".into(),
        "rpc".into(),
        "--offline".into(),
        "--no-extensions".into(),
        "--no-approve".into(),
        "--no-themes".into(),
        "--session-dir".into(),
        root.as_os_str().to_owned(),
        "--session-id".into(),
        "native-fixture-1".into(),
        "--fake-mode".into(),
        mode.into(),
    ]
    .into();
    Process::spawn(&node, &args, &project_root::inspect(root).unwrap()).unwrap()
}
impl Fake {
    fn new(root: &Path, mode: &str) -> Self {
        let mut owner = spawn_fake(root, mode);
        let streams = owner.take_streams().unwrap();
        let (sent, records) = sync_channel(8);
        std::thread::spawn(move || {
            let mut reader = Records::new(std::io::BufReader::new(streams.stdout));
            loop {
                let record = reader.next_record();
                let done = !matches!(&record, Ok(Some(_)));
                if sent.send(record).is_err() || done {
                    break;
                }
            }
        });
        let mut stderr = streams.stderr;
        let stderr = std::thread::spawn(move || {
            let mut bytes = [0; 8192];
            let mut count = 0;
            loop {
                match stderr.read(&mut bytes) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => count += n,
                }
            }
            count
        });
        let mut fake = Self {
            session: Session::new("native-fixture-1".into()),
            owner,
            stdin: streams.stdin,
            records,
            stderr: Some(stderr),
        };
        fake.send(RpcCommand::GetState);
        fake.next().unwrap().expect("synthetic startup EOF");
        assert_eq!(fake.session.state(), &SessionState::Ready);
        fake
    }
    fn send(&mut self, command: RpcCommand) -> String {
        let command = self.session.command(command).unwrap();
        write_record(&mut self.stdin, &command.bytes).unwrap();
        command.id
    }
    fn next(&mut self) -> pi::Result<Option<Vec<Observation>>> {
        self.records
            .recv_timeout(DEADLINE)
            .expect("bounded fake RPC read deadline")?
            .map(|record| self.session.observe(record))
            .transpose()
    }
    fn settled(&mut self) -> Vec<Observation> {
        let mut observations = Vec::new();
        for _ in 0..32 {
            observations.extend(self.next().unwrap().expect("unexpected synthetic EOF"));
            if observations
                .iter()
                .any(|value| matches!(value, Observation::State(SessionState::Ready)))
            {
                return observations;
            }
        }
        panic!("synthetic fixture never settled")
    }
}
impl Drop for Fake {
    fn drop(&mut self) {
        let _ = self.owner.stop();
        if let Some(stderr) = self.stderr.take() {
            let _ = stderr.join();
        }
    }
}

pub fn run() {
    let root = tempfile::tempdir().unwrap();
    {
        let mut fake = Fake::new(root.path(), "normal");
        fake.send(RpcCommand::Prompt {
            message: "synthetic".into(),
        });
        let observations = fake.settled();
        assert!(observations.iter().any(|value| matches!(value,Observation::Message(message) if format!("{message:?}").contains("Final synthetic 茶"))));
        assert!(observations.iter().any(|value|matches!(value,Observation::Outcome {outcome:Outcome::Accepted(disposition),..} if disposition=="started")));
        fake.send(RpcCommand::Prompt {
            message: "display-limits".into(),
        });
        let observations = fake.settled();
        let displayed = format!("{observations:?}");
        for marker in [
            "RPC record omitted",
            "Transcript incomplete",
            "Image not displayed",
            "Unsupported content",
            "Truncated",
            "display limit survived",
        ] {
            assert!(
                displayed.contains(marker),
                "missing display-limit evidence: {marker}"
            );
        }
        assert!(!displayed.contains("synthetic-not-displayed"));
        for value in &observations {
            if let Observation::Message(message) = value {
                assert!(serde_json::to_vec(message).unwrap().len() <= 65536);
            }
        }
        println!(
            "M1_PI_DISPLAY_LIMITS_OK modelCalls=0 oversizedRecordDrained=true placeholders=true boundedMessages=true settled=true"
        );
        fake.send(RpcCommand::Prompt {
            message: "/handled".into(),
        });
        assert!(fake.next().unwrap().unwrap().iter().any(|value|matches!(value,Observation::Outcome {outcome:Outcome::Accepted(disposition),..} if disposition=="handled")));
        fake.send(RpcCommand::Prompt {
            message: "approval".into(),
        });
        let mut approval = None;
        for _ in 0..3 {
            for value in fake.next().unwrap().unwrap() {
                if let Observation::Approval(value) = value {
                    approval = Some(value);
                }
            }
        }
        let approval = approval.unwrap();
        let bytes = fake
            .session
            .approval_response(&approval.id, ApprovalDecision::AllowOnce)
            .unwrap();
        write_record(&mut fake.stdin, &bytes).unwrap();
        assert_eq!(
            fake.session.approval_written(&approval.id).unwrap(),
            Observation::ApprovalDispatched { id: approval.id }
        );
        fake.settled();
        fake.send(RpcCommand::Prompt {
            message: "busy".into(),
        });
        fake.next().unwrap();
        fake.next().unwrap();
        let steer = fake
            .session
            .prompt("synthetic steer".into(), DeliveryMode::Steer)
            .unwrap();
        write_record(&mut fake.stdin, &steer.bytes).unwrap();
        let follow = fake.send(RpcCommand::FollowUp {
            message: "synthetic follow-up".into(),
        });
        for expected in [follow, steer.id] {
            assert!(
                fake.next()
                    .unwrap()
                    .unwrap()
                    .iter()
                    .any(|value| matches!(value,Observation::Outcome{id,..} if id==&expected))
            );
        }
        fake.send(RpcCommand::ClearQueue);
        fake.next().unwrap();
        let abort = fake.send(RpcCommand::Abort);
        fake.settled();
        assert!(
            fake.next()
                .unwrap()
                .unwrap()
                .contains(&Observation::Outcome {
                    id: abort,
                    outcome: Outcome::Applied
                })
        );
        fake.owner.stop().unwrap();
        assert!(fake.stderr.take().unwrap().join().unwrap() > 0);
        println!(
            "M1_PI_RPC_FIXTURE_OK modelCalls=0 owned=true lfFramed=true reorderedReplies=true settled=true approvalDispatched=true cancellation=true stderrDrained=true"
        );
    }
    for mode in ["crash", "malformed"] {
        let mut fake = Fake::new(root.path(), mode);
        let id = fake.send(RpcCommand::Prompt {
            message: "synthetic".into(),
        });
        let code = match fake.next() {
            Ok(None) => "PI_CONNECTION_CLOSED",
            Err(error) => error.0,
            Ok(Some(_)) => panic!("unexpected success"),
        };
        let observations = fake.session.disconnected(code);
        assert!(observations.contains(&Observation::Outcome {
            id,
            outcome: Outcome::Unknown("PI_COMMAND_OUTCOME_UNKNOWN")
        }));
        println!("M1_PI_RPC_FAILURE_OK case={mode} modelCalls=0 outcome=unknown noReplay=true");
    }
}
