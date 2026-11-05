//! Read-only configured model query; local full-wire access does not enable production runtimes.
use super::*;
use cafe_space_adapters::pi::{Command, Outcome};
use tokio::sync::oneshot;

/// Native evidence only; private rejection details must not be serialized into wire errors.
#[derive(Debug)]
pub enum ModelQueryResult {
    Applied(Vec<ModelInfo>),
    Rejected(io::Error),
    Unknown(&'static str),
}

pub(super) struct Pending {
    params: OpenSessionParams,
    reply: oneshot::Sender<ModelQueryResult>,
}

impl Host {
    /// A closed receiver means no authoritative result. Never retry a query automatically.
    pub fn list_pi_models(
        &self,
        access: Access,
        params: OpenSessionParams,
    ) -> io::Result<oneshot::Receiver<ModelQueryResult>> {
        if access != Access::Local {
            return Err(invalid("LOCAL_ONLY"));
        }
        encode_client_message(&ClientMessage::Request(ClientRequest {
            message_type: RequestMessageType::Request,
            protocol_version: PROTOCOL_VERSION,
            request_id: "validation".into(),
            request: Request::ListModels(params.clone()),
        }))
        .map_err(|_| invalid("INVALID_REQUEST"))?;
        let mut inner = self.lock()?;
        inner.refresh_commit()?;
        if inner.normal_shutdown {
            return Err(invalid("SHUTDOWN_IN_PROGRESS"));
        }
        if inner.migration_failed {
            return Err(invalid("SCHEMA_MIGRATION_FAILED"));
        }
        if inner.provisional || !inner.gate.admits_work() {
            return Err(invalid("UPDATE_IN_PROGRESS"));
        }
        let session = sessions::open(
            &inner.state.database,
            &params.session_id,
            &params.expected_stream_id,
            &params.expected_cwd_fence,
        )
        .map_err(session_io)?;
        let _directory = projects::checked(
            &inner.state.database,
            &inner.root,
            &session.project_id,
            &params.expected_cwd_fence,
        )
        .map_err(session_io)?;
        if !session.capabilities.model_control {
            return Err(invalid("CAPABILITY_UNSUPPORTED"));
        }
        if session.state != SessionState::Ready || inner.pi_owned_runs.contains(&session.id) {
            return Err(invalid("PI_NOT_READY"));
        }
        if inner
            .pi_pending
            .keys()
            .chain(inner.pi_model_queries.keys())
            .any(|(id, _)| id == &session.id)
        {
            return Err(invalid("PI_COMMAND_PENDING"));
        }
        let native = inner
            .drivers
            .get_mut(&session.id)
            .ok_or_else(|| invalid("PI_CONNECTION_CLOSED"))?
            .send(Command::GetAvailableModels, Instant::now())
            .map_err(|error| invalid(error.0))?;
        let (reply, received) = oneshot::channel();
        inner
            .pi_model_queries
            .insert((session.id, native), Pending { params, reply });
        inner.refresh_activity();
        Ok(received)
    }
}

impl Inner {
    pub(super) fn pi_models_response(
        &mut self,
        session: &str,
        native: &str,
        models: Vec<ModelInfo>,
    ) {
        if let Some(pending) = self
            .pi_model_queries
            .remove(&(session.into(), native.into()))
        {
            let result = sessions::open(
                &self.state.database,
                session,
                &pending.params.expected_stream_id,
                &pending.params.expected_cwd_fence,
            )
            .map_err(session_io)
            .and_then(|current| {
                if current.state != SessionState::Ready || self.pi_owned_runs.contains(session) {
                    return Err(invalid("PI_NOT_READY"));
                }
                projects::checked(
                    &self.state.database,
                    &self.root,
                    &current.project_id,
                    &pending.params.expected_cwd_fence,
                )
                .map_err(session_io)?;
                Ok(models)
            });
            let _ = pending.reply.send(match result {
                Ok(models) => ModelQueryResult::Applied(models),
                Err(error) => ModelQueryResult::Rejected(error),
            });
        }
    }

    pub(super) fn pi_models_outcome(
        &mut self,
        session: &str,
        native: &str,
        outcome: &Outcome,
    ) -> bool {
        if let Some(pending) = self
            .pi_model_queries
            .remove(&(session.into(), native.into()))
        {
            let result = match outcome {
                Outcome::Rejected(code) => ModelQueryResult::Rejected(invalid(code)),
                Outcome::Unknown(code) => ModelQueryResult::Unknown(code),
                _ => ModelQueryResult::Unknown("PI_MODEL_LIST_UNAVAILABLE"),
            };
            let _ = pending.reply.send(result);
            true
        } else {
            false
        }
    }
}

// Keep status evidence separate from the display-safe error projection.
pub(crate) fn wire_result(result: ModelQueryResult) -> RequestResult {
    let (code, unknown) = match result {
        ModelQueryResult::Applied(items) => return applied(Response::Models(ModelList { items })),
        ModelQueryResult::Rejected(error) => (error.to_string(), false),
        ModelQueryResult::Unknown(code) => (code.to_owned(), true),
    };
    let code = match code.as_str() {
        "LOCAL_ONLY"
        | "INVALID_REQUEST"
        | "SHUTDOWN_IN_PROGRESS"
        | "SCHEMA_MIGRATION_FAILED"
        | "UPDATE_IN_PROGRESS"
        | "SESSION_NOT_FOUND"
        | "STALE_STREAM"
        | "STALE_CWD_FENCE"
        | "PROJECT_NOT_APPROVED"
        | "PROJECT_ROOT_CHANGED"
        | "PROJECT_ROOT_UNAVAILABLE"
        | "CAPABILITY_UNSUPPORTED"
        | "PI_NOT_READY"
        | "PI_COMMAND_PENDING"
        | "PI_CONNECTION_CLOSED"
        | "PI_COMMAND_REJECTED"
        | "PI_RESPONSE_TIMEOUT"
        | "PI_COMMAND_OUTCOME_UNKNOWN"
        | "PI_MODEL_LIST_INVALID"
        | "PI_MODEL_LIST_LIMIT"
        | "PI_WRITE_TIMEOUT"
        | "PI_WRITE_FAILED"
        | "PI_QUEUE_FULL" => code.as_str(),
        _ => "PI_MODEL_LIST_UNAVAILABLE",
    };
    let error = ErrorResult {
        error: ApiError {
            code: code.into(),
            message: if unknown {
                "The model query outcome is unknown; do not retry automatically."
            } else {
                "The model query was rejected; refresh session state before acting."
            }
            .into(),
        },
    };
    if unknown {
        RequestResult::Unknown(error)
    } else {
        RequestResult::Rejected(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_admission_and_response_recheck_capability_state_and_fences() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp
            .path()
            .join(cafe_space_ipc::BuildFlavor::Dev.directory_name());
        let cwd = temp.path().join("project");
        std::fs::create_dir(&cwd).unwrap();
        let host = Host::new(
            State::open(&root, cafe_space_ipc::BuildFlavor::Dev).unwrap(),
            &root,
            "models-gates".into(),
            "a".repeat(64),
        )
        .unwrap();
        let mut session = {
            let inner = host.lock().unwrap();
            let (project, _) = projects::add(
                &inner.state.database,
                &root,
                AddProjectParams {
                    name: None,
                    root: cwd.to_str().unwrap().into(),
                },
            )
            .unwrap();
            sessions::create(
                &inner.state.database,
                &CreateSessionParams {
                    session_id: "session".into(),
                    project_id: project.id.clone(),
                    runtime_id: "pi".into(),
                    expected_cwd_fence: project.cwd_fence.clone(),
                    start: SessionStart::New,
                },
                Some(&project),
                Some(&RuntimeSummary {
                    id: "pi".into(),
                    name: "Synthetic".into(),
                    version: None,
                    available: true,
                    tool_approval: ToolApproval::NeverAsks,
                }),
            )
            .unwrap()
            .0
        };
        let params = OpenSessionParams {
            session_id: session.id.clone(),
            expected_stream_id: session.stream_id.clone().unwrap(),
            expected_cwd_fence: session.cwd_fence.clone(),
        };
        let check = |expected| {
            assert_eq!(
                host.list_pi_models(Access::Local, params.clone())
                    .unwrap_err()
                    .to_string(),
                expected
            )
        };
        check("CAPABILITY_UNSUPPORTED");
        session.capabilities.model_control = true;
        let save = |session: &SessionSnapshot| {
            host.lock()
                .unwrap()
                .state
                .database
                .execute(
                    "UPDATE sessions SET snapshot=?1 WHERE id=?2",
                    rusqlite::params![serde_json::to_string(session).unwrap(), session.id],
                )
                .unwrap();
        };
        save(&session);
        check("PI_NOT_READY");
        session.state = SessionState::Ready;
        save(&session);
        host.lock()
            .unwrap()
            .pi_owned_runs
            .insert(session.id.clone());
        check("PI_NOT_READY");
        host.lock().unwrap().pi_owned_runs.clear();
        check("PI_CONNECTION_CLOSED");
        for (case, expected) in [
            ("stream", "STALE_STREAM"),
            ("cwd", "STALE_CWD_FENCE"),
            ("busy", "PI_NOT_READY"),
            ("owned", "PI_NOT_READY"),
            ("ok", ""),
        ] {
            let mut requested = params.clone();
            match case {
                "stream" => requested.expected_stream_id = "old-stream".into(),
                "cwd" => requested.expected_cwd_fence = "old-cwd".into(),
                "busy" => session.state = SessionState::Starting,
                "owned" => {
                    host.lock()
                        .unwrap()
                        .pi_owned_runs
                        .insert(session.id.clone());
                }
                _ => {}
            }
            save(&session);
            let (reply, mut received) = oneshot::channel();
            let mut inner = host.lock().unwrap();
            inner.pi_model_queries.insert(
                (session.id.clone(), "query".into()),
                Pending {
                    params: requested,
                    reply,
                },
            );
            inner.pi_models_response(
                &session.id,
                "query",
                vec![ModelInfo {
                    provider: "synthetic".into(),
                    id: "tea".into(),
                }],
            );
            match received.try_recv().unwrap() {
                ModelQueryResult::Applied(items) if case == "ok" => assert_eq!(items.len(), 1),
                ModelQueryResult::Rejected(error) => assert_eq!(error.to_string(), expected),
                other => panic!("unexpected late result for {case}: {other:?}"),
            }
            assert!(inner.pi_model_queries.is_empty());
            inner.pi_owned_runs.clear();
            session.state = SessionState::Ready;
        }
    }

    #[test]
    fn wire_projection_preserves_status_without_forwarding_private_error_details() {
        for (native, unknown) in [
            (
                ModelQueryResult::Rejected(invalid("PI_RESPONSE_TIMEOUT")),
                false,
            ),
            (ModelQueryResult::Unknown("PI_RESPONSE_TIMEOUT"), true),
            (
                ModelQueryResult::Rejected(invalid("synthetic private path or credential")),
                false,
            ),
            (
                ModelQueryResult::Unknown("synthetic private path or credential"),
                true,
            ),
        ] {
            let result = wire_result(native);
            assert_eq!(matches!(result, RequestResult::Unknown(_)), unknown);
            let encoded = serde_json::to_string(&result).unwrap();
            assert!(!encoded.contains("synthetic private"));
            assert!(!matches!(
                result,
                RequestResult::Applied(_) | RequestResult::Dispatched(_)
            ));
        }
    }

    #[test]
    fn correlated_model_outcomes_keep_their_class_even_with_the_same_error_code() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp
            .path()
            .join(cafe_space_ipc::BuildFlavor::Dev.directory_name());
        let host = Host::new(
            State::open(&root, cafe_space_ipc::BuildFlavor::Dev).unwrap(),
            &root,
            "models-outcomes".into(),
            "a".repeat(64),
        )
        .unwrap();
        for outcome in [
            Outcome::Rejected("SAME_CODE"),
            Outcome::Unknown("SAME_CODE"),
            Outcome::Applied,
        ] {
            let (reply, mut received) = oneshot::channel();
            let mut inner = host.lock().unwrap();
            inner.pi_model_queries.insert(
                ("session".into(), "native".into()),
                Pending {
                    params: OpenSessionParams {
                        session_id: "session".into(),
                        expected_stream_id: "stream".into(),
                        expected_cwd_fence: "cwd".into(),
                    },
                    reply,
                },
            );
            assert!(inner.pi_models_outcome("session", "native", &outcome));
            match (outcome, received.try_recv().unwrap()) {
                (Outcome::Rejected(_), ModelQueryResult::Rejected(error)) => {
                    assert_eq!(error.to_string(), "SAME_CODE")
                }
                (Outcome::Unknown(_), ModelQueryResult::Unknown(code)) => {
                    assert_eq!(code, "SAME_CODE")
                }
                (Outcome::Applied, ModelQueryResult::Unknown(code)) => {
                    assert_eq!(code, "PI_MODEL_LIST_UNAVAILABLE")
                }
                other => panic!("outcome class changed: {other:?}"),
            }
            assert!(inner.pi_model_queries.is_empty());
            assert!(!inner.pi_models_outcome("session", "native", &Outcome::Unknown("LATE")));
            assert!(matches!(
                received.try_recv(),
                Err(oneshot::error::TryRecvError::Closed)
            ));
        }
    }

    #[test]
    fn model_queries_reject_remote_invalid_and_closed_admission_before_session_access() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp
            .path()
            .join(cafe_space_ipc::BuildFlavor::Dev.directory_name());
        let host = Host::new(
            State::open(&root, cafe_space_ipc::BuildFlavor::Dev).unwrap(),
            &root,
            "models-test".into(),
            "a".repeat(64),
        )
        .unwrap();
        let params = OpenSessionParams {
            session_id: "not-created".into(),
            expected_stream_id: "stream".into(),
            expected_cwd_fence: "cwd".into(),
        };
        let code = |access, params| host.list_pi_models(access, params).unwrap_err().to_string();
        let mut invalid = params.clone();
        invalid.session_id.clear();
        assert_eq!(code(Access::Remote, invalid.clone()), "LOCAL_ONLY");
        assert_eq!(code(Access::Local, invalid), "INVALID_REQUEST");
        host.lock().unwrap().normal_shutdown = true;
        assert_eq!(code(Access::Local, params.clone()), "SHUTDOWN_IN_PROGRESS");
        {
            let mut inner = host.lock().unwrap();
            inner.normal_shutdown = false;
            inner.migration_failed = true;
        }
        assert_eq!(
            code(Access::Local, params.clone()),
            "SCHEMA_MIGRATION_FAILED"
        );
        {
            let mut inner = host.lock().unwrap();
            inner.migration_failed = false;
            inner.provisional = true;
        }
        assert_eq!(code(Access::Local, params), "UPDATE_IN_PROGRESS");
    }
}
