//! Explicit registration and owned fake New/Resume through real private IPC; no Pi/model calls.
use super::*;
use cafe_space_adapters::pi::install::{self, CLI, PACKAGE, VERSION};
use cafe_space_ipc::{BuildFlavor, framing::write_frame, local, private};
use cafe_space_protocol::*;
use cafe_spaced::{api, host::Host, state::State, update::Access};
use pi_commands_fixture::{prompt, response};
use tokio::io::AsyncWriteExt;

fn wire(host: &Host, request: Request) -> RequestResult {
    #[cfg(windows)]
    let temp = tempfile::tempdir().unwrap();
    #[cfg(windows)]
    let endpoint = std::path::PathBuf::from(format!(
        r"\\.\pipe\cafe-pi-registration-{}-{}",
        std::process::id(),
        temp.path().file_name().unwrap().to_str().unwrap()
    ));
    #[cfg(unix)]
    let socket_dir = tempfile::tempdir_in("/tmp").unwrap();
    #[cfg(unix)]
    let endpoint = socket_dir.path().join("private/registration.sock");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let mut listener = local::Listener::bind(&endpoint).await.unwrap();
            let serving = async {
                api::serve_connection(listener.accept().await.unwrap(), host)
                    .await
                    .unwrap();
            };
            let checking = async {
                let mut client = local::connect(&endpoint).await.unwrap();
                write_frame(
                    &mut client,
                    &encode_client_message(&ClientMessage::Hello(ClientHello {
                        message_type: HelloMessageType::Hello,
                        supported_protocol_versions: vec![PROTOCOL_VERSION],
                        client_version: "registration-fixture".into(),
                    }))
                    .unwrap(),
                )
                .await
                .unwrap();
                assert!(matches!(
                    pi_models_wire_fixture::receive(&mut client, host, false).await,
                    ServerMessage::Welcome(_)
                ));
                pi_models_wire_fixture::send(&mut client, "registration", request).await;
                let result = pi_models_wire_fixture::result(
                    pi_models_wire_fixture::receive(&mut client, host, false).await,
                    "registration",
                );
                assert!(
                    !serde_json::to_string(&result)
                        .unwrap()
                        .contains("synthetic-private-package")
                );
                client.shutdown().await.unwrap();
                result
            };
            let ((), result) =
                tokio::time::timeout(DEADLINE, async { tokio::join!(serving, checking) })
                    .await
                    .unwrap();
            result
        })
}
fn wire_response(host: &Host, request: Request) -> Response {
    let RequestResult::Applied(value) = wire(host, request) else {
        panic!("applied wire response")
    };
    value.result
}

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
        assert!(!matches!(
            state,
            SessionState::Failed(_) | SessionState::Stopped
        ));
        state == SessionState::Ready
    });
    view(host, id).session
}
fn create(host: &Host, params: &CreateSessionParams) -> SessionSnapshot {
    let Response::Session(value) = wire_response(host, Request::CreateSession(params.clone()))
    else {
        panic!("created session")
    };
    value.session.session
}
fn turn(host: &Host, session: &SessionSnapshot, content: &str) {
    assert!(matches!(
        wire(host, prompt(session, content, DeliveryMode::Steer)),
        RequestResult::Dispatched(_)
    ));
    ready(host, &session.id);
}
fn close(host: &Host, session: &SessionSnapshot) {
    assert!(matches!(
        wire(
            host,
            Request::CloseSession(CloseSessionParams {
                session_id: session.id.clone(),
                expected_stream_id: session.stream_id.clone().unwrap(),
                expected_cwd_fence: session.cwd_fence.clone(),
            })
        ),
        RequestResult::Dispatched(_)
    ));
    until(|| {
        host.poll_processes(Instant::now()).unwrap();
        view(host, &session.id).session.state == SessionState::Stopped
    });
}
fn unavailable(host: &Host, params: &CreateSessionParams) {
    assert!(
        matches!(wire(host, Request::CreateSession(params.clone())), RequestResult::Rejected(value) if value.error.code == "RUNTIME_UNAVAILABLE")
    );
}

pub fn run() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(BuildFlavor::Dev.directory_name());
    let cwd = temp.path().join("project");
    let package = temp.path().join("synthetic-private-package");
    std::fs::create_dir(&cwd).unwrap();
    std::fs::create_dir_all(package.join("dist/bundle")).unwrap();
    let metadata = serde_json::to_vec(&serde_json::json!({
        "name":PACKAGE, "version":VERSION, "type":"module", "bin":{"pi":CLI}
    }))
    .unwrap();
    std::fs::write(package.join("package.json"), &metadata).unwrap();
    let script = install::node_path(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test/fake-pi.mjs")
            .canonicalize()
            .unwrap(),
    );
    std::fs::write(
        package.join(CLI),
        format!(
            "import {{pathToFileURL}} from 'node:url'; await import(pathToFileURL({}));\n",
            serde_json::to_string(&script).unwrap(),
        ),
    )
    .unwrap();
    let (node, _) = pi_fixture::command(&cwd, "normal");
    let mut host = Host::new(
        State::open(&root, BuildFlavor::Dev).unwrap(),
        &root,
        "registration-first".into(),
        "a".repeat(64),
    )
    .unwrap();
    assert_eq!(
        host.configure_pi_runtime(Access::Remote, Some((&node, &package)))
            .unwrap_err()
            .to_string(),
        "LOCAL_ONLY"
    );
    assert!(!root.join("pi-runtime.json").exists());
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
        session_id: "registered-source".into(),
        project_id: project.id,
        runtime_id: "pi".into(),
        expected_cwd_fence: project.cwd_fence,
        start: SessionStart::New,
    };
    unavailable(&host, &params);
    let before = response(&host, Request::ListSessions);
    let register = Request::RegisterPiRuntime(RegisterPiRuntimeParams {
        node: node.to_str().unwrap().into(),
        package_root: package.to_str().unwrap().into(),
    });
    for request in [register.clone(), Request::UnregisterPiRuntime] {
        assert!(
            matches!(host.dispatch(Access::Remote, 1, request).unwrap().result, RequestResult::Rejected(value) if value.error.code == "LOCAL_ONLY")
        );
    }
    let Response::Runtimes(list) = wire_response(&host, register.clone()) else {
        panic!("runtimes")
    };
    let runtimes = list.items;
    assert_eq!(runtimes.len(), 1);
    assert!(runtimes[0].available);
    assert_eq!(response(&host, Request::ListSessions), before);
    assert!(
        matches!(response(&host, Request::ListRuntimes), Response::Runtimes(list) if list.items == runtimes)
    );
    let Response::Subscription(snapshot) = response(
        &host,
        Request::SubscribeEvents(SubscribeEventsParams { after: None }),
    ) else {
        panic!("snapshot")
    };
    assert_eq!(snapshot.snapshot.unwrap().runtimes, runtimes);
    assert!(
        matches!(host.dispatch(Access::Remote, 1, Request::CreateSession(params.clone())).unwrap().result,
        RequestResult::Rejected(value) if value.error.code == "LOCAL_ONLY")
    );
    let created = create(&host, &params);
    let session = ready(&host, &params.session_id);
    assert_eq!(create(&host, &params), session);
    assert_eq!(created.stream_id, session.stream_id);
    let folder = std::fs::read_dir(root.join("agent-sessions"))
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
    // Ensure the fake's history is user-owned on elevated Windows, matching the native fixture policy.
    drop(private::file(&folder.join(format!("2000-01-01_{native}.jsonl"))).unwrap());
    assert!(
        matches!(wire_response(&host, Request::UnregisterPiRuntime), Response::Runtimes(list) if list.items.is_empty())
    );
    let mut rejected = params.clone();
    rejected.session_id = "disabled-new".into();
    unavailable(&host, &rejected);
    turn(&host, &session, "remember");
    close(&host, &session);
    let stopped = view(&host, &params.session_id).session;
    assert_eq!(create(&host, &params), stopped);
    wire_response(&host, register);
    drop(host);
    host = Host::new(
        State::open(&root, BuildFlavor::Dev).unwrap(),
        &root,
        "registration-reopened".into(),
        "a".repeat(64),
    )
    .unwrap();
    assert!(
        matches!(response(&host, Request::ListRuntimes), Response::Runtimes(list) if list.items[0].available)
    );
    let mut resume = params.clone();
    resume.session_id = "registered-resumed".into();
    resume.start = SessionStart::Resume(ResumeStartParams {
        source_session_id: params.session_id.clone(),
    });
    create(&host, &resume);
    let resumed = ready(&host, &resume.session_id);
    let hydrated = view(&host, &resume.session_id);
    assert!(!hydrated.messages.is_empty());
    assert!(
        serde_json::to_string(&hydrated)
            .unwrap()
            .contains("Final synthetic")
    );
    let count = hydrated.messages.len();
    turn(&host, &resumed, "recall");
    assert!(view(&host, &resume.session_id).messages.len() > count);
    std::fs::write(package.join("package.json"), b"{}").unwrap();
    assert!(
        matches!(response(&host, Request::ListRuntimes), Response::Runtimes(list) if !list.items[0].available)
    );
    unavailable(&host, &rejected);
    assert_eq!(
        create(&host, &resume),
        view(&host, &resume.session_id).session
    );
    close(&host, &resumed);
    let public = serde_json::to_string(&response(&host, Request::ListSessions)).unwrap();
    assert!(!public.contains("synthetic-private-package"));
    assert!(!public.contains("package_root"));
    println!(
        "M1_PI_REGISTRATION_OK nativeIpc=true registrationWire=true new=true resume=true explicitReconcile=true removedBlocksNew=true changedPackageRejected=true pathsPrivate=true modelCalls=0 realAgent=false"
    );
}
