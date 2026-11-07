//! Private Pi registration and launch factory, serialized with Host admission.
use super::*;
use cafe_space_adapters::pi::install::Installation;

pub(super) fn start_failure(error: io::Error) -> RequestResult {
    let code = error.to_string();
    match code.as_str() {
        "INVALID_REQUEST"
        | "ID_CONFLICT"
        | "SESSION_LIMIT"
        | "SESSION_NOT_FOUND"
        | "PROCESS_LIMIT"
        | "PROJECT_NOT_APPROVED"
        | "PROJECT_ROOT_CHANGED"
        | "PROJECT_ROOT_UNAVAILABLE"
        | "STALE_CWD_FENCE"
        | "RUNTIME_UNAVAILABLE"
        | "RESUME_UNAVAILABLE"
        | "ATTACH_UNAVAILABLE"
        | "NATIVE_SESSION_IN_USE"
        | "NATIVE_SESSION_ID_CONFLICT"
        | "PI_LAUNCH_INVALID"
        | "PI_RESUME_UNAVAILABLE"
        | "PI_SESSION_DIRECTORY_INVALID"
        | "OWNER_SETTINGS_INVALID"
        | "OWNER_SETTINGS_UNTRUSTED"
        | "OWNER_SETTINGS_READ_FAILED"
        | "OWNER_PATH_REQUIRED"
        | "OWNER_PATH_INVALID" => rejected(&code),
        // All remaining failures can follow durable creation; never tell a caller to create another ID.
        _ => pi_commands::unknown("PI_START_UNKNOWN"),
    }
}

pub(super) fn configure_runtime(root: &Path, paths: Option<(&Path, &Path)>) -> RequestResult {
    match crate::pi_runtime::configure(root, paths) {
        Ok(()) => applied(Response::Runtimes(RuntimeList {
            items: crate::pi_runtime::list(root),
        })),
        Err(error) => {
            let code = error.to_string();
            match code.as_str() {
                "PI_RUNTIME_WRITE_UNKNOWN" => pi_commands::unknown("PI_RUNTIME_WRITE_UNKNOWN"),
                "PI_RUNTIME_UNTRUSTED"
                | "PI_RUNTIME_READ_FAILED"
                | "PI_RUNTIME_INVALID"
                | "PI_INSTALLATION_INVALID"
                | "PI_NODE_UNAVAILABLE"
                | "PI_PACKAGE_UNAVAILABLE"
                | "PI_PACKAGE_INVALID"
                | "PI_VERSION_UNSUPPORTED"
                | "PI_CLI_UNAVAILABLE" => rejected(&code),
                _ => rejected("PI_RUNTIME_INVALID"),
            }
        }
    }
}

impl Host {
    /// Explicit owner registration; passive inspection only, never an install or Agent launch.
    pub fn configure_pi_runtime(
        &self,
        access: Access,
        paths: Option<(&Path, &Path)>,
    ) -> io::Result<Vec<RuntimeSummary>> {
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
        crate::pi_runtime::configure(&inner.root, paths)?;
        Ok(crate::pi_runtime::list(&inner.root))
    }

    /// Native owner entry only; returns names/readiness, never proxy credentials or paths.
    pub fn owner_environment_status(&self, access: Access) -> io::Result<OwnerEnvironmentStatus> {
        if access != Access::Local {
            return Err(invalid("LOCAL_ONLY"));
        }
        let inner = self.lock()?;
        crate::owner_settings::status(&inner.root)
    }

    /// Explicit local patch, applied only to future children. Never import a shell or retry writes.
    pub fn patch_owner_environment(
        &self,
        access: Access,
        changes: BTreeMap<String, Option<String>>,
    ) -> io::Result<OwnerEnvironmentStatus> {
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
        crate::owner_settings::patch(&inner.root, changes)
    }

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
        inner.start_installed_pi(params, installation)
    }
}

impl Inner {
    pub(super) fn start_installed_pi(
        &mut self,
        params: &CreateSessionParams,
        installation: &Installation,
    ) -> io::Result<SessionSnapshot> {
        if sessions::get(&self.state.database, &params.session_id)
            .map_err(session_io)?
            .is_some()
        {
            return sessions::create(&self.state.database, params, None, None)
                .map(|(session, _)| session)
                .map_err(session_io);
        }
        if self.processes.len() >= 32 {
            return Err(invalid("PROCESS_LIMIT"));
        }
        self.check_event_capacity()?;
        let runtime = crate::pi_runtime::summary(true);
        let (project, cwd) = projects::checked(
            &self.state.database,
            &self.root,
            &params.project_id,
            &params.expected_cwd_fence,
        )
        .map_err(session_io)?;
        let native_id = match &params.start {
            SessionStart::New => pi_commands::opaque_id()?,
            SessionStart::Resume(source) => {
                native_sessions::get(&self.state.database, &source.source_session_id)
                    .map_err(session_io)?
                    .ok_or_else(|| invalid("RESUME_UNAVAILABLE"))?
            }
            SessionStart::Attach(_) => return Err(invalid("ATTACH_UNAVAILABLE")),
        };
        // Reuse lifecycle/identity validation before any filesystem change or process launch.
        let transaction = self
            .state
            .database
            .transaction()
            .map_err(io::Error::other)?;
        sessions::create(&transaction, params, Some(&project), Some(&runtime))
            .map_err(session_io)?;
        native_sessions::record(&transaction, params, &native_id).map_err(session_io)?;
        transaction.rollback().map_err(io::Error::other)?;
        let environment = crate::owner_settings::environment(&self.root)?;
        let session_dir = self
            .root
            .join("agent-sessions")
            .join(format!("pi-{native_id}"));
        let _guards = if matches!(params.start, SessionStart::Resume(_)) {
            (
                cafe_space_ipc::private::existing_directory(&self.root.join("agent-sessions"))?,
                cafe_space_ipc::private::existing_directory(&session_dir)?,
            )
        } else {
            native_sessions::directory(&self.root, &native_id)?
        };
        let history = if matches!(params.start, SessionStart::Resume(_)) {
            Some(native_sessions::history(
                &self.root, &native_id, &_guards.1,
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
        self.launch_pi_session(
            params,
            &runtime,
            &installation.node,
            &args,
            native_id,
            Some(environment),
        )
    }

    pub(super) fn launch_pi_session(
        &mut self,
        params: &CreateSessionParams,
        runtime: &RuntimeSummary,
        program: &Path,
        args: &[OsString],
        private_native_id: String,
        environment: Option<crate::managed::Environment>,
    ) -> io::Result<SessionSnapshot> {
        let (session, streams) = self.launch_session(
            params,
            runtime,
            program,
            args,
            Some(&private_native_id),
            environment,
        )?;
        if let Some(streams) = streams {
            match Driver::new(streams, private_native_id, Instant::now()) {
                Ok(mut driver) => {
                    if matches!(params.start, SessionStart::Resume(_)) {
                        driver.require_history();
                    }
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
