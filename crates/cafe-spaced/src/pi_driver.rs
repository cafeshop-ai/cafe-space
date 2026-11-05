//! Bounded I/O for a Host-owned Pi process. No wire admission, launch or credentials here.
use crate::managed::Streams;
use cafe_space_adapters::pi::{self, Command, Observation, Session, framing};
use cafe_space_protocol::{ApprovalDecision, SessionState};
use std::{
    collections::BTreeMap,
    io::{self, BufReader, Read},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel},
    },
    thread,
    time::{Duration, Instant},
};

const QUEUE: usize = 8;
const POLL_RECORDS: usize = 32;
const DEADLINE: Duration = Duration::from_secs(30);

struct Write {
    id: String,
    approval: bool,
    bytes: Vec<u8>,
}
enum Input {
    Record(framing::Record),
    Written { id: String, approval: bool },
    Closed(&'static str),
}

/// The caller retains `Process` ownership, enforces caller/run/cwd/stream fences, and must stop
/// the process on construction failure or a Failed observation. Never wait for worker joins
/// under Host's admission lock; process teardown closes the workers' OS pipes.
pub struct Driver {
    session: Session,
    writes: SyncSender<Write>,
    incoming: Receiver<Input>,
    deadlines: BTreeMap<String, (Instant, bool)>,
    cancelled: Arc<AtomicBool>,
    hydrate_history: bool,
    history_request: Option<String>,
}
impl Driver {
    pub fn new(streams: Streams, private_native_id: String, now: Instant) -> io::Result<Self> {
        let (writes, queued) = sync_channel::<Write>(QUEUE);
        let (events, incoming) = sync_channel(QUEUE);
        let cancelled = Arc::new(AtomicBool::new(false));
        let Streams {
            mut stdin,
            stdout,
            mut stderr,
        } = streams;
        let writer_events = events.clone();
        let stop = cancelled.clone();
        thread::Builder::new()
            .name("pi-stdin".into())
            .spawn(move || {
                while let Ok(write) = queued.recv() {
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    let event = match framing::write_record(&mut stdin, &write.bytes) {
                        Ok(()) => Input::Written {
                            id: write.id,
                            approval: write.approval,
                        },
                        Err(error) => Input::Closed(error.0),
                    };
                    let failed = matches!(event, Input::Closed(_));
                    if writer_events.send(event).is_err() || failed {
                        break;
                    }
                }
            })
            .map_err(|_| io::Error::other("PI_WORKER_UNAVAILABLE"))?;
        thread::Builder::new()
            .name("pi-stdout".into())
            .spawn(move || {
                let mut records = framing::Records::new(BufReader::new(stdout));
                loop {
                    let event = match records.next_record() {
                        Ok(Some(record)) => Input::Record(record),
                        Ok(None) => Input::Closed("PI_CONNECTION_CLOSED"),
                        Err(error) => Input::Closed(error.0),
                    };
                    let failed = matches!(event, Input::Closed(_));
                    if events.send(event).is_err() || failed {
                        break;
                    }
                }
            })
            .map_err(|_| io::Error::other("PI_WORKER_UNAVAILABLE"))?;
        thread::Builder::new()
            .name("pi-stderr".into())
            .spawn(move || {
                let mut chunk = [0u8; crate::managed::OUTPUT_CHUNK_BYTES];
                loop {
                    match stderr.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(_) => {}
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
            })
            .map_err(|_| io::Error::other("PI_WORKER_UNAVAILABLE"))?;
        let mut driver = Self {
            session: Session::new(private_native_id),
            writes,
            incoming,
            deadlines: BTreeMap::new(),
            cancelled,
            hydrate_history: false,
            history_request: None,
        };
        driver
            .send(Command::GetState, now)
            .map_err(io::Error::other)?;
        Ok(driver)
    }
    pub fn state(&self) -> &SessionState {
        if self.hydrate_history && self.session.state() == &SessionState::Ready {
            &SessionState::Starting
        } else {
            self.session.state()
        }
    }

    pub(crate) fn require_history(&mut self) {
        self.hydrate_history = true;
    }

    pub fn require_local_ui(&mut self) -> Option<Observation> {
        self.session.require_local_ui()
    }

    /// Returns a correlation ID only when queued. It does not claim delivery or acceptance.
    pub fn send(&mut self, command: Command, now: Instant) -> pi::Result<String> {
        let outbound = self.session.command(command)?;
        let id = outbound.id;
        let result = self.enqueue(
            Write {
                id: id.clone(),
                approval: false,
                bytes: outbound.bytes,
            },
            now,
        );
        if result.is_err() {
            self.session.expire(&id);
        }
        result.map(|()| id)
    }
    pub fn approve(
        &mut self,
        id: &str,
        decision: ApprovalDecision,
        now: Instant,
    ) -> pi::Result<()> {
        if matches!(self.state(), SessionState::Failed(_)) {
            return Err(pi::Error("PI_CONNECTION_CLOSED"));
        }
        if self.deadlines.contains_key(id) {
            return Err(pi::Error("PI_APPROVAL_PENDING"));
        }
        let bytes = self.session.approval_response(id, decision)?;
        self.enqueue(
            Write {
                id: id.into(),
                approval: true,
                bytes,
            },
            now,
        )
    }
    fn enqueue(&mut self, write: Write, now: Instant) -> pi::Result<()> {
        if self.deadlines.len() >= pi::MAX_PENDING {
            return Err(pi::Error("PI_COMMAND_LIMIT"));
        }
        let id = write.id.clone();
        let approval = write.approval;
        self.writes.try_send(write).map_err(|error| {
            pi::Error(match error {
                TrySendError::Full(_) => "PI_INPUT_BACKPRESSURE",
                TrySendError::Disconnected(_) => "PI_CONNECTION_CLOSED",
            })
        })?;
        self.deadlines.insert(id, (now + DEADLINE, approval));
        Ok(())
    }
    fn failed(&mut self, code: &'static str) -> Vec<Observation> {
        self.cancelled.store(true, Ordering::Release);
        let mut observations: Vec<_> = std::mem::take(&mut self.deadlines)
            .into_iter()
            .filter_map(|(id, (_, approval))| {
                approval.then_some(Observation::Outcome {
                    id,
                    outcome: pi::Outcome::Unknown("PI_WRITE_OUTCOME_UNKNOWN"),
                })
            })
            .collect();
        observations.extend(self.session.disconnected(code));
        observations
    }
    /// An observed root-process exit is abnormal unless Host already initiated close/update.
    pub fn process_exited(&mut self) -> Vec<Observation> {
        if matches!(self.state(), SessionState::Failed(_)) {
            return vec![];
        }
        self.failed("PI_CONNECTION_CLOSED")
    }

    /// Nonblocking bounded work. A Failed observation requires immediate Host process teardown.
    pub fn poll(&mut self, now: Instant) -> Vec<Observation> {
        if matches!(self.state(), SessionState::Failed(_)) {
            return vec![];
        }
        let mut observations = Vec::new();
        for _ in 0..POLL_RECORDS {
            let event = match self.incoming.try_recv() {
                Ok(event) => event,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => Input::Closed("PI_CONNECTION_CLOSED"),
            };
            match event {
                Input::Record(record) => match self.session.observe(record) {
                    Ok(next) => {
                        if self.hydrate_history && next.contains(&Observation::HistoryTruncated) {
                            observations.extend(self.failed("PI_HISTORY_UNAVAILABLE"));
                            return observations;
                        }
                        if self.hydrate_history
                            && next.iter().any(|value| {
                                matches!(
                                    value,
                                    Observation::State(
                                        SessionState::Running | SessionState::WaitingForLocalUi
                                    ) | Observation::Message(_)
                                        | Observation::Approval(_)
                                )
                            })
                        {
                            observations.extend(self.failed("PI_STARTUP_BUSY"));
                            return observations;
                        }
                        let mut loaded = false;
                        for value in next {
                            if let Observation::Outcome { id, outcome } = &value {
                                self.deadlines.remove(id);
                                if self.history_request.as_ref() == Some(id)
                                    && *outcome != pi::Outcome::Applied
                                {
                                    observations.extend(self.failed("PI_HISTORY_UNAVAILABLE"));
                                    return observations;
                                }
                            }
                            if self.hydrate_history
                                && value == Observation::State(SessionState::Ready)
                            {
                                continue;
                            }
                            loaded |= matches!(&value, Observation::HistoryLoaded { .. });
                            observations.push(value);
                        }
                        if loaded && self.hydrate_history {
                            self.hydrate_history = false;
                            self.history_request = None;
                            observations.push(Observation::State(SessionState::Ready));
                        } else if self.hydrate_history
                            && self.history_request.is_none()
                            && self.session.state() == &SessionState::Ready
                        {
                            match self.send(Command::GetMessages, now) {
                                Ok(id) => self.history_request = Some(id),
                                Err(error) => {
                                    observations.extend(self.failed(error.0));
                                    return observations;
                                }
                            }
                        }
                    }
                    Err(error) => {
                        observations.extend(self.failed(error.0));
                        return observations;
                    }
                },
                Input::Written { id, approval: true } => {
                    // Settlement may have closed the dialog before the local writer confirmation.
                    if self.deadlines.remove(&id).is_some() {
                        observations.push(
                            self.session
                                .approval_written(&id)
                                .unwrap_or(Observation::ApprovalDispatched { id }),
                        );
                    }
                }
                Input::Written { .. } => {}
                Input::Closed(code) => {
                    observations.extend(self.failed(code));
                    return observations;
                }
            }
        }
        if let Some(id) = self
            .deadlines
            .iter()
            .find_map(|(id, (deadline, _))| (now >= *deadline).then(|| id.clone()))
        {
            if let Some(outcome) = self.session.expire(&id) {
                observations.push(outcome);
            }
            observations.extend(self.failed("PI_RESPONSE_TIMEOUT"));
        }
        observations
    }
}
impl Drop for Driver {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn isolated() -> (Driver, SyncSender<Input>, Receiver<Write>) {
        let (writes, queued) = sync_channel(QUEUE);
        let (events, incoming) = sync_channel(QUEUE);
        let mut session = Session::new("synthetic-native".into());
        let query = session.command(Command::GetState).unwrap();
        session.observe(serde_json::json!({"type":"response","id":query.id,"command":"get_state","success":true,"data":{"sessionId":"synthetic-native","isStreaming":false,"isCompacting":false,"pendingMessageCount":0}})).unwrap();
        (
            Driver {
                session,
                writes,
                incoming,
                deadlines: BTreeMap::new(),
                cancelled: Arc::new(AtomicBool::new(false)),
                hydrate_history: false,
                history_request: None,
            },
            events,
            queued,
        )
    }
    fn confirmation(driver: &mut Driver) -> String {
        driver.session.observe(serde_json::json!({"type":"extension_ui_request","id":"private-confirm","method":"confirm","title":"Synthetic"})).unwrap().into_iter().find_map(|value| if let Observation::Approval(approval)=value {Some(approval.id)} else {None}).unwrap()
    }
    #[test]
    fn settlement_before_writer_confirmation_still_reports_dispatch_not_native_acknowledgement() {
        let (mut driver, events, queued) = isolated();
        let id = confirmation(&mut driver);
        driver
            .approve(&id, ApprovalDecision::AllowOnce, Instant::now())
            .unwrap();
        let written = queued.try_recv().unwrap();
        driver
            .session
            .observe(serde_json::json!({"type":"agent_settled","aborted":false}))
            .unwrap();
        assert!(
            events
                .send(Input::Written {
                    id: written.id,
                    approval: written.approval
                })
                .is_ok()
        );
        assert_eq!(
            driver.poll(Instant::now()),
            vec![Observation::ApprovalDispatched { id }]
        );
        assert!(driver.deadlines.is_empty());
    }
    #[test]
    fn blocked_approval_write_deadline_is_unknown_and_closes_the_dialog_without_replay() {
        let (mut driver, _events, _queued) = isolated();
        let id = confirmation(&mut driver);
        let now = Instant::now();
        driver.approve(&id, ApprovalDecision::Deny, now).unwrap();
        let observed = driver.poll(now + DEADLINE);
        assert!(observed.contains(&Observation::Outcome {
            id: id.clone(),
            outcome: pi::Outcome::Unknown("PI_WRITE_OUTCOME_UNKNOWN")
        }));
        assert!(observed.contains(&Observation::ApprovalClosed { id }));
        assert!(matches!(driver.state(), SessionState::Failed(_)));
        assert!(driver.cancelled.load(Ordering::Acquire));
    }
    #[test]
    fn observed_process_exit_leaves_pending_commands_unknown_exactly_once() {
        let (mut driver, _events, _queued) = isolated();
        let id = driver
            .send(
                Command::Prompt {
                    message: "synthetic".into(),
                },
                Instant::now(),
            )
            .unwrap();
        let observed = driver.process_exited();
        assert!(observed.contains(&Observation::Outcome {
            id,
            outcome: pi::Outcome::Unknown("PI_COMMAND_OUTCOME_UNKNOWN")
        }));
        assert!(matches!(driver.state(), SessionState::Failed(_)));
        assert!(driver.process_exited().is_empty());
        assert!(driver.poll(Instant::now()).is_empty());
        assert!(driver.cancelled.load(Ordering::Acquire));
    }

    #[test]
    fn resume_startup_hydrates_before_ready_without_sending_any_prompt() {
        let (mut driver, events, queued) = isolated();
        driver.session = Session::new("synthetic-native".into());
        driver.require_history();
        let query = driver.send(Command::GetState, Instant::now()).unwrap();
        let _state_write = queued.try_recv().unwrap();
        events.send(Input::Record(framing::Record::Json(serde_json::json!({"type":"response", "id":query, "command":"get_state", "success":true, "data":{"sessionId":"synthetic-native", "isStreaming":false, "isCompacting":false, "pendingMessageCount":0}})))).unwrap();
        let observed = driver.poll(Instant::now());
        assert!(!observed.contains(&Observation::State(SessionState::Ready)));
        assert_eq!(driver.state(), &SessionState::Starting);
        let history = queued.try_recv().unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&history.bytes).unwrap()["type"],
            "get_messages"
        );
        events.send(Input::Record(framing::Record::Json(serde_json::json!({"type":"response", "id":history.id, "command":"get_messages", "success":true, "data":{"messages":[{"role":"user","content":"Prior context"}]}})))).unwrap();
        let observed = driver.poll(Instant::now());
        let loaded = observed
            .iter()
            .position(|value| matches!(value, Observation::HistoryLoaded { .. }))
            .unwrap();
        let ready = observed
            .iter()
            .position(|value| *value == Observation::State(SessionState::Ready))
            .unwrap();
        assert!(loaded < ready);
        assert_eq!(driver.state(), &SessionState::Ready);
        assert!(driver.deadlines.is_empty());
        assert!(queued.try_recv().is_err());
    }

    #[test]
    fn rejected_or_busy_startup_history_never_publishes_ready_or_replays() {
        for busy in [false, true] {
            let (mut driver, events, queued) = isolated();
            driver.require_history();
            let id = driver.send(Command::GetMessages, Instant::now()).unwrap();
            driver.history_request = Some(id.clone());
            let _write = queued.try_recv().unwrap();
            let record = if busy {
                serde_json::json!({"type":"agent_start"})
            } else {
                serde_json::json!({"type":"response", "id":id, "command":"get_messages", "success":false, "error":"synthetic-never-forward"})
            };
            events
                .send(Input::Record(framing::Record::Json(record)))
                .unwrap();
            let observed = driver.poll(Instant::now());
            assert!(!observed.contains(&Observation::State(SessionState::Ready)));
            assert!(matches!(driver.state(), SessionState::Failed(_)));
            assert!(!format!("{observed:?}").contains("synthetic-never-forward"));
            assert!(queued.try_recv().is_err());
        }
    }

    #[test]
    fn driver_limits_match_the_offline_contract() {
        let contract: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/pi-rpc/driver.json")).unwrap();
        assert_eq!(contract["queueRecords"], QUEUE);
        assert_eq!(contract["recordsPerPoll"], POLL_RECORDS);
        assert_eq!(contract["responseDeadlineSeconds"], DEADLINE.as_secs());
    }
}
