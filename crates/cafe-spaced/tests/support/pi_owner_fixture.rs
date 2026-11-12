//! Native owner settings -> actual owned fake child. No wire API, model, shell or credentials.
use super::*;
use cafe_space_adapters::pi::install::{self, Installation};
use cafe_space_ipc::{BuildFlavor, private};
use cafe_space_protocol::*;
use cafe_spaced::{host::Host, state::State, update::Access};
use pi_commands_fixture::{prompt, receive, response};
use std::collections::BTreeMap;

fn view(host: &Host, id: &str) -> SessionView {
    let Response::Session(value) = response(
        host,
        Request::GetSession(GetSessionParams {
            session_id: id.into(),
        }),
    ) else {
        panic!("session")
    };
    value.session
}
fn ready(host: &Host, id: &str) -> SessionSnapshot {
    until(|| {
        host.poll_processes(Instant::now()).unwrap();
        let state = view(host, id).session.state;
        assert!(
            !matches!(state, SessionState::Failed(_) | SessionState::Stopped),
            "owner fixture startup/turn failed: {state:?}"
        );
        state == SessionState::Ready
    });
    view(host, id).session
}
fn turn(host: &Host, session: &SessionSnapshot, content: &str, expected: &str) {
    let mut reply = host
        .command_pi(Access::Local, prompt(session, content, DeliveryMode::Steer))
        .unwrap();
    assert!(matches!(
        receive(host, &mut reply, false),
        RequestResult::Dispatched(_)
    ));
    ready(host, &session.id);
    let json = serde_json::to_string(&view(host, &session.id)).unwrap();
    if json.contains("Owner fixture mismatch:") {
        for key in ["proxy", "ca", "signing", "path"] {
            if json.contains(&format!("mismatch: {key}")) {
                panic!("synthetic owner environment mismatch: {key}");
            }
        }
    }
    assert!(json.contains(expected));
    for private_value in [
        "synthetic-password",
        "synthetic-ca-file",
        "synthetic-signing-socket",
    ] {
        assert!(!json.contains(private_value));
    }
}
fn close(host: &Host, session: &SessionSnapshot) {
    assert!(matches!(
        host.dispatch(
            Access::Local,
            1,
            Request::CloseSession(CloseSessionParams {
                session_id: session.id.clone(),
                expected_stream_id: session.stream_id.clone().unwrap(),
                expected_cwd_fence: session.cwd_fence.clone(),
            })
        )
        .unwrap()
        .result,
        RequestResult::Dispatched(_)
    ));
}

pub fn run() {
    let contract: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/process-ownership/owner-environment.json"
    ))
    .unwrap();
    assert_eq!(contract["formatVersion"], 1);
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join(BuildFlavor::Dev.directory_name());
    let cwd = temp.path().join("project");
    std::fs::create_dir(&cwd).unwrap();
    let host = Host::new(
        State::open(&data, BuildFlavor::Dev).unwrap(),
        &data,
        "owner-boot".into(),
        "a".repeat(64),
    )
    .unwrap();
    assert_eq!(
        host.owner_environment_status(Access::Remote)
            .unwrap_err()
            .to_string(),
        "LOCAL_ONLY"
    );
    assert_eq!(
        host.patch_owner_environment(Access::Remote, BTreeMap::new())
            .unwrap_err()
            .to_string(),
        "LOCAL_ONLY"
    );
    assert!(!data.join("owner-environment.json").exists());
    assert!(
        host.owner_environment_status(Access::Local)
            .unwrap()
            .overridden_names
            .is_empty()
    );
    let Response::Project(project) = response(
        &host,
        Request::AddProject(AddProjectParams {
            name: None,
            root: cwd.to_str().unwrap().into(),
        }),
    ) else {
        panic!("project")
    };
    let params = CreateSessionParams {
        session_id: "owner-before".into(),
        project_id: project.id,
        runtime_id: "pi".into(),
        expected_cwd_fence: project.cwd_fence,
        start: SessionStart::New,
    };
    let (node, _) = pi_fixture::command(&cwd, "normal");
    let owner_bin = temp.path().join("owner-bin");
    std::fs::create_dir(&owner_bin).unwrap();
    let owner_path = install::node_path(&owner_bin.canonicalize().unwrap())
        .to_str()
        .unwrap()
        .to_owned();
    let installation = Installation {
        node,
        cli: Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test/fake-pi.mjs")
            .canonicalize()
            .unwrap(),
    };
    host.start_installed_pi(Access::Local, &params, &installation)
        .unwrap();
    let first = ready(&host, &params.session_id);
    let folder = std::fs::read_dir(data.join("agent-sessions"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let native = folder
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .strip_prefix("pi-")
        .unwrap();
    drop(private::file(&folder.join(format!("2000-01-01_{native}.jsonl"))).unwrap());
    turn(
        &host,
        &first,
        "owner-default",
        "Default owner environment verified",
    );
    let changes = [
        ("PATH".into(), Some(owner_path)),
        (
            "HTTPS_PROXY".into(),
            Some("http://synthetic-user:synthetic-password@proxy.invalid:3128".into()),
        ),
        ("SSL_CERT_FILE".into(), Some("synthetic-ca-file".into())),
        (
            "SSH_AUTH_SOCK".into(),
            Some("synthetic-signing-socket".into()),
        ),
    ]
    .into();
    let saved = host
        .patch_owner_environment(Access::Local, changes)
        .unwrap();
    assert!(saved.path_ready);
    assert_eq!(saved.overridden_names.len(), 4);
    turn(
        &host,
        &first,
        "owner-default",
        "Default owner environment verified",
    );
    close(&host, &first);
    drop(host);
    let host = Host::new(
        State::open(&data, BuildFlavor::Dev).unwrap(),
        &data,
        "owner-reopened".into(),
        "a".repeat(64),
    )
    .unwrap();
    assert_eq!(host.owner_environment_status(Access::Local).unwrap(), saved);
    let mut resume = params.clone();
    resume.session_id = "owner-after".into();
    resume.start = SessionStart::Resume(ResumeStartParams {
        source_session_id: params.session_id.clone(),
    });
    host.start_installed_pi(Access::Local, &resume, &installation)
        .unwrap();
    let resumed = ready(&host, &resume.session_id);
    turn(
        &host,
        &resumed,
        "owner-overrides",
        "Explicit owner environment verified",
    );
    close(&host, &resumed);
    let removed = host
        .patch_owner_environment(
            Access::Local,
            ["PATH", "HTTPS_PROXY", "SSL_CERT_FILE", "SSH_AUTH_SOCK"]
                .map(|name| (name.into(), None))
                .into(),
        )
        .unwrap();
    assert!(removed.overridden_names.is_empty());
    let mut restored = resume.clone();
    restored.session_id = "owner-restored".into();
    host.start_installed_pi(Access::Local, &restored, &installation)
        .unwrap();
    let restored = ready(&host, &restored.session_id);
    turn(
        &host,
        &restored,
        "owner-default",
        "Default owner environment verified",
    );
    close(&host, &restored);
    let path = data.join("owner-environment.json");
    let original = std::fs::read(&path).unwrap();
    for (value, code) in [
        ("", "OWNER_PATH_REQUIRED"),
        ("relative/bin", "OWNER_PATH_INVALID"),
    ] {
        let mut settings: serde_json::Value = serde_json::from_slice(&original).unwrap();
        settings["overrides"]["PATH"] = value.into();
        std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
        assert!(
            !host
                .owner_environment_status(Access::Local)
                .unwrap()
                .path_ready
        );
        let mut denied = params.clone();
        denied.session_id = "owner-not-started".into();
        assert_eq!(
            host.start_installed_pi(Access::Local, &denied, &installation)
                .unwrap_err()
                .to_string(),
            code
        );
        assert!(
            matches!(host.dispatch(Access::Local,1,Request::GetSession(GetSessionParams {session_id:denied.session_id})).unwrap().result,
            RequestResult::Rejected(ref value) if value.error.code=="SESSION_NOT_FOUND")
        );
        assert_eq!(
            std::fs::read_dir(data.join("agent-sessions"))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(
            host.start_installed_pi(Access::Local, &resume, &installation)
                .unwrap()
                .state,
            SessionState::Stopped
        );
    }
    std::fs::write(&path, &original).unwrap();
    let Response::Update(_) = response(
        &host,
        Request::PrepareUpdate(PrepareUpdateParams {
            mode: UpdateMode::Now,
            target_hash: "b".repeat(64),
        }),
    ) else {
        panic!("update")
    };
    assert_eq!(
        host.patch_owner_environment(Access::Local, BTreeMap::new())
            .unwrap_err()
            .to_string(),
        "UPDATE_IN_PROGRESS"
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    host.stop_owned_processes().unwrap();
    println!(
        "M1_PI_HOST_OWNER_ENVIRONMENT_OK modelCalls=0 localOnly=true durablePrivatePatch=true noSecretExport=true existingChildUnchanged=true resumeChildUsesOverrides=true removedOverrideRestoresProxy=true invalidPathBeforeTarget=true stoppedRetryNoReplay=true updateGate=true productionDisabled=true"
    );
}
