//! Owner-only native Pi launch factory. No wire runtime admission.
use super::*;
use cafe_space_adapters::pi::install::{Installation, VERSION};

impl Host {
    /// Trusted local installation only; native identity and exact Resume file stay inside Host.
    pub fn start_installed_pi(
        &self,
        access: Access,
        params: &CreateSessionParams,
        installation: &Installation,
    ) -> io::Result<SessionSnapshot> {
        if access != Access::Local {
            return Err(invalid("LOCAL_ONLY"));
        }
        let mut inner = self.lock()?;
        inner.refresh_commit()?;
        if inner.migration_failed {
            return Err(invalid("SCHEMA_MIGRATION_FAILED"));
        }
        if inner.normal_shutdown || inner.provisional || !inner.gate.admits_work() {
            return Err(invalid("UPDATE_IN_PROGRESS"));
        }
        if sessions::get(&inner.state.database, &params.session_id)
            .map_err(session_io)?
            .is_some()
        {
            return sessions::create(&inner.state.database, params, None, None)
                .map(|(session, _)| session)
                .map_err(session_io);
        }
        if inner.processes.len() >= 32 {
            return Err(invalid("PROCESS_LIMIT"));
        }
        inner.check_event_capacity()?;
        let runtime = RuntimeSummary {
            id: "pi".into(),
            name: "Pi".into(),
            version: Some(VERSION.into()),
            available: true,
            tool_approval: ToolApproval::NeverAsks,
        };
        let (project, cwd) = projects::checked(
            &inner.state.database,
            &inner.root,
            &params.project_id,
            &params.expected_cwd_fence,
        )
        .map_err(session_io)?;
        let native_id = match &params.start {
            SessionStart::New => pi_commands::opaque_id()?,
            SessionStart::Resume(source) => {
                native_sessions::get(&inner.state.database, &source.source_session_id)
                    .map_err(session_io)?
                    .ok_or_else(|| invalid("RESUME_UNAVAILABLE"))?
            }
            SessionStart::Attach(_) => return Err(invalid("ATTACH_UNAVAILABLE")),
        };
        // Reuse lifecycle/identity validation before any filesystem change or process launch.
        let transaction = inner
            .state
            .database
            .transaction()
            .map_err(io::Error::other)?;
        sessions::create(&transaction, params, Some(&project), Some(&runtime))
            .map_err(session_io)?;
        native_sessions::record(&transaction, params, &native_id).map_err(session_io)?;
        transaction.rollback().map_err(io::Error::other)?;
        let session_dir = inner
            .root
            .join("agent-sessions")
            .join(format!("pi-{native_id}"));
        let _guards = if matches!(params.start, SessionStart::Resume(_)) {
            (
                cafe_space_ipc::private::existing_directory(&inner.root.join("agent-sessions"))?,
                cafe_space_ipc::private::existing_directory(&session_dir)?,
            )
        } else {
            native_sessions::directory(&inner.root, &native_id)?
        };
        let history = if matches!(params.start, SessionStart::Resume(_)) {
            Some(native_sessions::history(
                &inner.root,
                &native_id,
                &_guards.1,
            )?)
        } else {
            None
        };
        let args = installation
            .arguments(
                &native_id,
                &session_dir,
                None,
                history
                    .as_ref()
                    .map(|(path, _file)| (path.as_path(), cwd.path.as_path())),
            )
            .map_err(|error| invalid(error.0))?;
        inner.launch_pi_session(params, &runtime, &installation.node, &args, native_id)
    }
}

impl Inner {
    pub(super) fn launch_pi_session(
        &mut self,
        params: &CreateSessionParams,
        runtime: &RuntimeSummary,
        program: &Path,
        args: &[OsString],
        private_native_id: String,
    ) -> io::Result<SessionSnapshot> {
        let (session, streams) =
            self.launch_session(params, runtime, program, args, Some(&private_native_id))?;
        if let Some(streams) = streams {
            match Driver::new(streams, private_native_id, Instant::now()) {
                Ok(driver) => {
                    self.drivers.insert(session.id.clone(), driver);
                }
                Err(_) => {
                    self.record_pi_state(
                        &session.id,
                        SessionState::Failed(FailureState {
                            code: "PI_WORKER_UNAVAILABLE".into(),
                        }),
                    )?;
                    self.stop_process(&session.id, SessionStopReason::AgentExited)?;
                    return Err(invalid("PI_WORKER_UNAVAILABLE"));
                }
            }
        }
        Ok(session)
    }
}
