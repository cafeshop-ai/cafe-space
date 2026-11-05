//! Native-only, read-only configured model query. No wire method or runtime admission.
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
            request: Request::OpenSession(params.clone()),
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

#[cfg(test)]
mod tests {
    use super::*;
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
