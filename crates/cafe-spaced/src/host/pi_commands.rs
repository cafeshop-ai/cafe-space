//! Native-only Host command routing. No production runtime selection or wire admission.
use super::*;
use cafe_space_adapters::pi::{self, Command, Outcome};
use tokio::sync::oneshot;

pub(super) struct Pending {
    reply: oneshot::Sender<RequestResult>,
    effect: Effect,
}
pub(super) struct Confirmation {
    native: String,
    pub(super) request: ApprovalRequest,
}
enum Effect {
    Approval {
        id: String,
        decision: ApprovalDecision,
    },
    Prompt,
    Input,
    Clear {
        run: String,
    },
    Abort,
    SetConfiguration,
    ReadConfiguration {
        observed: bool,
    },
}
impl Effect {
    fn cancelling(&self) -> bool {
        matches!(self, Self::Clear { .. } | Self::Abort)
    }
}

impl Host {
    /// Native Rust entry only. The receiver resolves after observed acceptance/rejection, not
    /// enqueue. A dropped receiver never cancels/replays input; a closed sender means unknown.
    pub fn command_pi(
        &self,
        access: Access,
        request: Request,
    ) -> io::Result<oneshot::Receiver<RequestResult>> {
        let mut inner = self.lock()?;
        inner.refresh_commit()?;
        inner.command_pi(access, request)
    }
}

impl Inner {
    fn command_pi(
        &mut self,
        access: Access,
        request: Request,
    ) -> io::Result<oneshot::Receiver<RequestResult>> {
        if access != Access::Local {
            return Err(invalid("LOCAL_ONLY"));
        }
        if self.normal_shutdown {
            return Err(invalid("SHUTDOWN_IN_PROGRESS"));
        }
        if self.migration_failed {
            return Err(invalid("SCHEMA_MIGRATION_FAILED"));
        }
        if self.provisional || !self.gate.admits_work() {
            return Err(invalid("UPDATE_IN_PROGRESS"));
        }
        encode_client_message(&ClientMessage::Request(ClientRequest {
            message_type: RequestMessageType::Request,
            protocol_version: PROTOCOL_VERSION,
            request_id: "validation".into(),
            request: request.clone(),
        }))
        .map_err(|_| invalid("INVALID_REQUEST"))?;
        let (id, stream, cwd) = match &request {
            Request::Prompt(p) => (&p.session_id, &p.expected_stream_id, &p.expected_cwd_fence),
            Request::Cancel(p) => (&p.session_id, &p.expected_stream_id, &p.expected_cwd_fence),
            Request::RespondToApproval(p) => {
                (&p.session_id, &p.expected_stream_id, &p.expected_cwd_fence)
            }
            Request::SetModel(p) => (&p.session_id, &p.expected_stream_id, &p.expected_cwd_fence),
            Request::SetThinking(p) => {
                (&p.session_id, &p.expected_stream_id, &p.expected_cwd_fence)
            }
            _ => return Err(invalid("NOT_IMPLEMENTED")),
        };
        let session = sessions::open(&self.state.database, id, stream, cwd).map_err(session_io)?;
        // Check the held directory identity as well as the opaque generation before external I/O.
        let _directory =
            projects::checked(&self.state.database, &self.root, &session.project_id, cwd)
                .map_err(session_io)?;
        if !self.drivers.contains_key(id) {
            return Err(invalid("PI_CONNECTION_CLOSED"));
        }
        if self
            .pi_model_queries
            .keys()
            .any(|(session, _)| session == id)
        {
            return Err(invalid("PI_COMMAND_PENDING"));
        }
        let pending: Vec<_> = self
            .pi_pending
            .iter()
            .filter(|((s, _), _)| s == id)
            .map(|(_, p)| &p.effect)
            .collect();
        if pending.iter().any(|effect| effect.cancelling()) {
            return Err(invalid("PI_CANCEL_PENDING"));
        }
        if let Request::RespondToApproval(p) = &request {
            if p.decision == ApprovalDecision::AllowForSession {
                return Err(invalid("PI_APPROVAL_SCOPE_UNSUPPORTED"));
            }
            let confirmation = self
                .pi_approvals
                .get(&(session.id.clone(), p.approval_id.clone()))
                .ok_or_else(|| invalid("PI_APPROVAL_NOT_FOUND"))?;
            let native = confirmation.native.clone();
            if self
                .pi_pending
                .contains_key(&(session.id.clone(), native.clone()))
            {
                return Err(invalid("PI_APPROVAL_PENDING"));
            }
            self.check_event_capacity()?;
            self.drivers
                .get_mut(&session.id)
                .ok_or_else(|| invalid("PI_CONNECTION_CLOSED"))?
                .approve(&native, p.decision.clone(), Instant::now())
                .map_err(|error| invalid(error.0))?;
            let (reply, received) = oneshot::channel();
            self.pi_pending.insert(
                (session.id, native),
                Pending {
                    reply,
                    effect: Effect::Approval {
                        id: p.approval_id.clone(),
                        decision: p.decision.clone(),
                    },
                },
            );
            self.refresh_activity();
            return Ok(received);
        }
        let (command, effect) = match request {
            Request::Prompt(p) => {
                if !session.capabilities.delivery_control {
                    return Err(invalid("CAPABILITY_UNSUPPORTED"));
                }
                if session.state == SessionState::Ready {
                    if !pending.is_empty() || self.pi_owned_runs.contains(&session.id) {
                        return Err(invalid("PI_COMMAND_PENDING"));
                    }
                    (Command::Prompt { message: p.content }, Effect::Prompt)
                } else if session.state == SessionState::Running
                    && self.pi_owned_runs.contains(&session.id)
                {
                    // Leave capacity for clear_queue + abort even under a flood of queued inputs.
                    if pending.len() >= pi::MAX_PENDING - 2 {
                        return Err(invalid("PI_COMMAND_LIMIT"));
                    }
                    (
                        match p.delivery {
                            DeliveryMode::Steer => Command::Steer { message: p.content },
                            DeliveryMode::FollowUp => Command::FollowUp { message: p.content },
                        },
                        Effect::Input,
                    )
                } else {
                    return Err(invalid("PI_NOT_READY"));
                }
            }
            Request::Cancel(p) => {
                if !self.pi_owned_runs.contains(&session.id)
                    || session.active_run_id.as_deref() != Some(&p.run_id)
                    || !matches!(
                        session.state,
                        SessionState::Running | SessionState::WaitingForLocalUi
                    )
                {
                    return Err(invalid("STALE_RUN"));
                }
                (Command::ClearQueue, Effect::Clear { run: p.run_id })
            }
            Request::SetModel(p) => {
                if !session.capabilities.model_control {
                    return Err(invalid("CAPABILITY_UNSUPPORTED"));
                }
                if session.state != SessionState::Ready
                    || !pending.is_empty()
                    || self.pi_owned_runs.contains(&session.id)
                {
                    return Err(invalid("PI_NOT_READY"));
                }
                (
                    Command::SetModel {
                        provider: p.provider.clone(),
                        model_id: p.model_id.clone(),
                    },
                    Effect::SetConfiguration,
                )
            }
            Request::SetThinking(p) => {
                if !session.capabilities.thinking_control {
                    return Err(invalid("CAPABILITY_UNSUPPORTED"));
                }
                if session.state != SessionState::Ready
                    || !pending.is_empty()
                    || self.pi_owned_runs.contains(&session.id)
                {
                    return Err(invalid("PI_NOT_READY"));
                }
                (
                    Command::SetThinkingLevel {
                        level: p.level.clone(),
                    },
                    Effect::SetConfiguration,
                )
            }
            _ => unreachable!(),
        };
        self.check_event_capacity()?;
        let (reply, received) = oneshot::channel();
        let prompt = matches!(effect, Effect::Prompt);
        self.pi_enqueue(&session.id, command, Pending { reply, effect })?;
        if prompt {
            self.pi_owned_runs.insert(session.id);
        }
        Ok(received)
    }

    fn pi_enqueue(&mut self, id: &str, command: Command, pending: Pending) -> io::Result<()> {
        let sent = self
            .drivers
            .get_mut(id)
            .ok_or(pi::Error("PI_CONNECTION_CLOSED"))
            .and_then(|driver| driver.send(command, Instant::now()));
        let native = match sent {
            Ok(native) => native,
            Err(error) => {
                let code = if matches!(
                    pending.effect,
                    Effect::SetConfiguration | Effect::ReadConfiguration { .. }
                ) {
                    "PI_CONFIGURATION_OUTCOME_UNKNOWN"
                } else if matches!(pending.effect, Effect::Abort) {
                    "PI_CANCEL_OUTCOME_UNKNOWN"
                } else {
                    error.0
                };
                let _ = pending.reply.send(unknown(code));
                return Err(invalid(error.0));
            }
        };
        self.pi_pending.insert((id.into(), native), pending);
        self.refresh_activity();
        Ok(())
    }

    pub(super) fn pi_outcome(
        &mut self,
        id: &str,
        native: &str,
        outcome: Outcome,
    ) -> io::Result<()> {
        if self.pi_models_outcome(id, native, &outcome) {
            return Ok(());
        }
        let Some(pending) = self.pi_pending.remove(&(id.into(), native.into())) else {
            return Ok(());
        };
        let result = match outcome {
            Outcome::Accepted(disposition) => {
                if disposition == "handled"
                    && matches!(pending.effect, Effect::Prompt)
                    && sessions::get(&self.state.database, id)
                        .map_err(session_io)?
                        .is_some_and(|s| s.state == SessionState::Ready)
                {
                    self.pi_owned_runs.remove(id);
                }
                RequestResult::Dispatched(DispatchedResult {
                    operation_id: opaque_id()?,
                })
            }
            Outcome::Rejected(code) => {
                if matches!(pending.effect, Effect::ReadConfiguration { .. }) {
                    let _ = pending
                        .reply
                        .send(unknown("PI_CONFIGURATION_OUTCOME_UNKNOWN"));
                    return Ok(());
                }
                if matches!(pending.effect, Effect::Prompt)
                    && sessions::get(&self.state.database, id)
                        .map_err(session_io)?
                        .is_some_and(|s| s.state == SessionState::Ready)
                {
                    self.pi_owned_runs.remove(id);
                }
                rejected(code)
            }
            Outcome::Unknown(code) => unknown(code),
            Outcome::Applied => match &pending.effect {
                Effect::Approval {
                    id: approval,
                    decision,
                } => {
                    if self
                        .pi_approvals
                        .contains_key(&(id.into(), approval.clone()))
                    {
                        self.check_event_capacity()?;
                        self.pi_approvals.remove(&(id.into(), approval.clone()));
                        self.publish_event(Event::ApprovalResolved(ApprovalResolvedData {
                            session_id: id.into(),
                            approval_id: approval.clone(),
                            decision: decision.clone(),
                        }))?;
                    }
                    RequestResult::Dispatched(DispatchedResult {
                        operation_id: opaque_id()?,
                    })
                }
                Effect::Clear { run } => {
                    let session = sessions::get(&self.state.database, id)
                        .map_err(session_io)?
                        .ok_or_else(|| invalid("SESSION_NOT_FOUND"))?;
                    let driver_state = self.drivers.get(id).map(Driver::state);
                    if session.active_run_id.as_ref() == Some(run)
                        && self.pi_owned_runs.contains(id)
                        && driver_state.is_some_and(|state| {
                            matches!(
                                state,
                                SessionState::Running | SessionState::WaitingForLocalUi
                            )
                        })
                        && matches!(
                            session.state,
                            SessionState::Running | SessionState::WaitingForLocalUi
                        )
                    {
                        // Keep the same admission lock and reply while advancing cancellation.
                        if self
                            .pi_enqueue(
                                id,
                                Command::Abort,
                                Pending {
                                    reply: pending.reply,
                                    effect: Effect::Abort,
                                },
                            )
                            .is_err()
                        {
                            // clear_queue was applied: an abort enqueue failure is not rejection.
                            self.record_pi_state(
                                id,
                                SessionState::Failed(FailureState {
                                    code: "PI_CANCEL_OUTCOME_UNKNOWN".into(),
                                }),
                            )?;
                            self.stop_process(id, SessionStopReason::AgentExited)?;
                            return Ok(());
                        }
                        return Ok(());
                    }
                    if session.state == SessionState::Ready
                        || driver_state == Some(&SessionState::Ready)
                    {
                        applied(Response::Empty)
                    } else {
                        unknown("PI_CANCEL_OUTCOME_UNKNOWN")
                    }
                }
                Effect::SetConfiguration => {
                    if self
                        .pi_enqueue(
                            id,
                            Command::GetState,
                            Pending {
                                reply: pending.reply,
                                effect: Effect::ReadConfiguration { observed: false },
                            },
                        )
                        .is_err()
                    {
                        self.record_pi_state(
                            id,
                            SessionState::Failed(FailureState {
                                code: "PI_CONFIGURATION_OUTCOME_UNKNOWN".into(),
                            }),
                        )?;
                        self.stop_process(id, SessionStopReason::AgentExited)?;
                    }
                    return Ok(());
                }
                Effect::ReadConfiguration { observed: true } => applied(Response::Empty),
                Effect::ReadConfiguration { observed: false } => {
                    unknown("PI_CONFIGURATION_OUTCOME_UNKNOWN")
                }
                Effect::Abort => RequestResult::Dispatched(DispatchedResult {
                    operation_id: opaque_id()?,
                }),
                _ => applied(Response::Empty),
            },
        };
        let _ = pending.reply.send(result);
        Ok(())
    }

    pub(super) fn pi_configuration_response(
        &mut self,
        id: &str,
        native: &str,
        model: Option<ModelInfo>,
        thinking: Option<ThinkingLevel>,
    ) -> io::Result<()> {
        if sessions::get(&self.state.database, id)
            .map_err(session_io)?
            .is_some_and(|s| s.state == SessionState::Ready)
        {
            self.pi_configuration(id, model, thinking)?;
            if let Some(Pending {
                effect: Effect::ReadConfiguration { observed },
                ..
            }) = self.pi_pending.get_mut(&(id.into(), native.into()))
            {
                *observed = true;
            }
        }
        Ok(())
    }

    fn pi_configuration(
        &mut self,
        id: &str,
        model: Option<ModelInfo>,
        thinking: Option<ThinkingLevel>,
    ) -> io::Result<()> {
        let session = sessions::get(&self.state.database, id)
            .map_err(session_io)?
            .ok_or_else(|| invalid("SESSION_NOT_FOUND"))?;
        if session.state != SessionState::Ready {
            return Err(invalid("PI_CONFIGURATION_OUTCOME_UNKNOWN"));
        }
        self.check_event_capacity()?;
        let mut next = session.clone();
        next.model = model;
        next.thinking_level = thinking;
        let changed =
            sessions::transition(&self.state.database, &session, next).map_err(session_io)?;
        self.session_result(Ok(changed))?;
        Ok(())
    }

    pub(super) fn pi_confirmation(
        &mut self,
        id: &str,
        approval: ApprovalRequest,
    ) -> io::Result<()> {
        let session = sessions::get(&self.state.database, id)
            .map_err(session_io)?
            .ok_or_else(|| invalid("SESSION_NOT_FOUND"))?;
        if matches!(
            session.state,
            SessionState::Stopped | SessionState::Failed(_)
        ) {
            return Ok(());
        }
        if self
            .pi_approvals
            .iter()
            .any(|((s, _), c)| s == id && c.native == approval.id)
        {
            return Err(invalid("PI_DIALOG_INVALID"));
        }
        let request = ApprovalRequest {
            id: opaque_id()?,
            summary: approval.summary,
        };
        let existing: Vec<_> = self
            .pi_approvals
            .iter()
            .filter(|((s, _), _)| s == id)
            .map(|(_, c)| &c.request)
            .collect();
        let bytes = serde_json::to_vec(&request)
            .map_err(io::Error::other)?
            .len();
        let retained = serde_json::to_vec(&existing)
            .map_err(io::Error::other)?
            .len();
        if existing.len() >= pi::MAX_PENDING || bytes + retained > 64 * 1024 {
            if let Some(Observation::State(state)) =
                self.drivers.get_mut(id).and_then(Driver::require_local_ui)
            {
                self.observe_pi_state(id, state)?;
            }
            return Ok(());
        }
        self.check_event_capacity()?;
        let event = Event::ApprovalRequired(ApprovalRequiredData {
            session_id: id.into(),
            approval: request.clone(),
        });
        encode_server_message(&ServerMessage::Event(ServerEvent {
            message_type: EventMessageType::Event,
            protocol_version: PROTOCOL_VERSION,
            event_stream_id: self.stream_id.clone(),
            sequence: self.sequence + 1,
            event: event.clone(),
        }))
        .map_err(|_| invalid("PI_DIALOG_INVALID"))?;
        self.pi_approvals.insert(
            (id.into(), request.id.clone()),
            Confirmation {
                native: approval.id,
                request,
            },
        );
        self.publish_event(event)
    }

    pub(super) fn pi_confirmation_closed(&mut self, id: &str, native: &str) {
        self.pi_approvals
            .retain(|(session, _), confirmation| session != id || confirmation.native != native);
    }

    pub(super) fn observe_pi_state(&mut self, id: &str, state: SessionState) -> io::Result<()> {
        if state == SessionState::Running && !self.pi_owned_runs.contains(id) {
            self.record_pi_state(
                id,
                SessionState::Failed(FailureState {
                    code: "PI_UNOWNED_RUN".into(),
                }),
            )?;
            self.stop_process(id, SessionStopReason::AgentExited)?;
            return Ok(());
        }
        if matches!(
            state,
            SessionState::Ready | SessionState::Failed(_) | SessionState::Stopped
        ) {
            self.pi_owned_runs.remove(id);
        }
        self.record_pi_state(id, state)
    }

    pub(super) fn pi_forget(&mut self, id: &str) {
        self.pi_owned_runs.remove(id);
        self.pi_model_queries
            .retain(|(session, _), _| session != id);
        self.pi_approvals.retain(|(session, _), _| session != id);
        let keys: Vec<_> = self
            .pi_pending
            .keys()
            .filter(|(s, _)| s == id)
            .cloned()
            .collect();
        for key in keys {
            if let Some(pending) = self.pi_pending.remove(&key) {
                let _ = pending.reply.send(unknown("PI_COMMAND_OUTCOME_UNKNOWN"));
            }
        }
    }
}

pub(super) fn opaque_id() -> io::Result<String> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|_| invalid("PI_ID_UNAVAILABLE"))?;
    Ok(random.iter().map(|byte| format!("{byte:02x}")).collect())
}
fn unknown(code: &str) -> RequestResult {
    RequestResult::Unknown(ErrorResult {
        error: ApiError {
            code: code.into(),
            message: "The command outcome is unknown; do not replay it.".into(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_write_races_and_unknown_never_invent_a_decision_or_expose_native_ids() {
        for ordering in ["written_first", "settled_first", "unknown"] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp
                .path()
                .join(cafe_space_ipc::BuildFlavor::Dev.directory_name());
            let host = Host::new(
                State::open(&root, cafe_space_ipc::BuildFlavor::Dev).unwrap(),
                &root,
                "approval-test".into(),
                "a".repeat(64),
            )
            .unwrap();
            let mut inner = host.lock().unwrap();
            let session = sessions::tests::approved(&inner.state.database, "approval-race");
            inner
                .record_pi_state(&session.id, SessionState::Ready)
                .unwrap();
            inner.pi_owned_runs.insert(session.id.clone());
            inner
                .observe_pi_state(&session.id, SessionState::Running)
                .unwrap();
            inner
                .pi_confirmation(
                    &session.id,
                    ApprovalRequest {
                        id: "private-adapter-dialog".into(),
                        summary: "Synthetic".into(),
                    },
                )
                .unwrap();
            let public = inner
                .pi_approvals
                .values()
                .next()
                .unwrap()
                .request
                .id
                .clone();
            let current = sessions::get(&inner.state.database, &session.id)
                .unwrap()
                .unwrap();
            let RequestResult::Applied(snapshot) =
                inner.session_result(Ok((current, false))).unwrap()
            else {
                panic!("snapshot")
            };
            let Response::Session(snapshot) = snapshot.result else {
                panic!("session")
            };
            assert_eq!(snapshot.session.approvals[0].id, public);
            assert_eq!(snapshot.sequence, inner.history.back().unwrap().sequence);
            assert!(
                matches!(&inner.history.back().unwrap().event, Event::ApprovalRequired(data) if data.approval.id == public)
            );
            let sequence = inner.sequence;
            inner.sequence = MAX_EVENT_SEQUENCE;
            assert!(
                inner
                    .pi_confirmation(
                        &session.id,
                        ApprovalRequest {
                            id: "private-second".into(),
                            summary: "Synthetic".into()
                        }
                    )
                    .is_err()
            );
            assert_eq!(
                inner.pi_approvals.len(),
                1,
                "capacity failure cannot mutate the snapshot"
            );
            inner.sequence = sequence;
            let (reply, mut received) = oneshot::channel();
            inner.pi_pending.insert(
                (session.id.clone(), "private-adapter-dialog".into()),
                Pending {
                    reply,
                    effect: Effect::Approval {
                        id: public.clone(),
                        decision: ApprovalDecision::AllowOnce,
                    },
                },
            );
            if ordering == "written_first" {
                inner
                    .observe_pi(
                        &session.id,
                        vec![Observation::ApprovalDispatched {
                            id: "private-adapter-dialog".into(),
                        }],
                    )
                    .unwrap();
                assert!(inner.history.iter().any(|e| matches!(&e.event, Event::ApprovalResolved(data) if data.approval_id == public && data.decision == ApprovalDecision::AllowOnce)));
            }
            let mut observed = vec![
                Observation::ApprovalClosed {
                    id: "private-adapter-dialog".into(),
                },
                Observation::State(if ordering == "unknown" {
                    SessionState::Failed(FailureState {
                        code: "PI_RESPONSE_TIMEOUT".into(),
                    })
                } else {
                    SessionState::Ready
                }),
            ];
            if ordering == "unknown" {
                observed.push(Observation::Outcome {
                    id: "private-adapter-dialog".into(),
                    outcome: Outcome::Unknown("PI_WRITE_OUTCOME_UNKNOWN"),
                });
            } else if ordering == "settled_first" {
                observed.push(Observation::ApprovalDispatched {
                    id: "private-adapter-dialog".into(),
                });
            }
            inner.observe_pi(&session.id, observed).unwrap();
            let result = received.try_recv().unwrap();
            if ordering == "unknown" {
                assert!(matches!(result, RequestResult::Unknown(_)));
            } else {
                assert!(matches!(result, RequestResult::Dispatched(_)));
            }
            assert!(inner.pi_approvals.is_empty() && inner.pi_pending.is_empty());
            if ordering != "written_first" {
                assert!(
                    !inner
                        .history
                        .iter()
                        .any(|e| matches!(&e.event, Event::ApprovalResolved(_)))
                );
            }
            assert!(
                !serde_json::to_string(&inner.history)
                    .unwrap()
                    .contains("private-adapter-dialog")
            );
        }
    }

    #[test]
    fn settled_state_in_same_poll_is_committed_before_clear_ack_advances_cancellation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp
            .path()
            .join(cafe_space_ipc::BuildFlavor::Dev.directory_name());
        let state = State::open(&root, cafe_space_ipc::BuildFlavor::Dev).unwrap();
        let host = Host::new(state, &root, "cancel-boot".into(), "a".repeat(64)).unwrap();
        let mut inner = host.lock().unwrap();
        let session = sessions::tests::approved(&inner.state.database, "cancel-race");
        inner
            .record_pi_state(&session.id, SessionState::Ready)
            .unwrap();
        inner.pi_owned_runs.insert(session.id.clone());
        inner
            .observe_pi_state(&session.id, SessionState::Running)
            .unwrap();
        let running = sessions::get(&inner.state.database, &session.id)
            .unwrap()
            .unwrap();
        let (reply, mut received) = oneshot::channel();
        inner.pi_pending.insert(
            (session.id.clone(), "clear-reply".into()),
            Pending {
                reply,
                effect: Effect::Clear {
                    run: running.active_run_id.unwrap(),
                },
            },
        );
        inner
            .observe_pi(
                &session.id,
                vec![
                    Observation::Outcome {
                        id: "clear-reply".into(),
                        outcome: Outcome::Applied,
                    },
                    Observation::State(SessionState::Ready),
                ],
            )
            .unwrap();
        assert!(matches!(
            received.try_recv().unwrap(),
            RequestResult::Applied(_)
        ));
        assert!(inner.pi_pending.is_empty());
        assert!(inner.pi_owned_runs.is_empty());
        assert!(
            sessions::get(&inner.state.database, &session.id)
                .unwrap()
                .unwrap()
                .active_run_id
                .is_none()
        );
        assert!(inner.history.iter().any(|e| matches!(&e.event, Event::SessionChanged(data) if data.session.state == SessionState::Running && data.session.active_run_id.is_some())));
        assert!(
            matches!(&inner.history.back().unwrap().event, Event::SessionChanged(data) if data.session.state == SessionState::Ready && data.session.active_run_id.is_none())
        );
    }
}
