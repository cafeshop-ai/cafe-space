//! Native Host factory + exact native file. Offline; no model or production admission.
use super::*;
use cafe_space_adapters::pi::install::Installation;
use cafe_space_ipc::BuildFlavor;
use cafe_space_protocol::*;
use cafe_spaced::{host::Host, state::State, update::Access};
use pi_commands_fixture::{prompt, receive, response};

fn view(host: &Host, id: &str) -> SessionView {
    let Response::Session(value) = response(
        host,
        Request::GetSession(GetSessionParams {
            session_id: id.into(),
        }),
    ) else {
        panic!("session expected")
    };
    value.session
}
fn ready(host: &Host, id: &str) -> SessionSnapshot {
    until(|| {
        host.poll_processes(Instant::now()).unwrap();
        view(host, id).session.state == SessionState::Ready
    });
    view(host, id).session
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
    until(|| {
        host.poll_processes(Instant::now()).unwrap();
        view(host, &session.id).session.state == SessionState::Stopped
    });
}
fn turn(host: &Host, session: &SessionSnapshot, text: &str) {
    let mut reply = host
        .command_pi(Access::Local, prompt(session, text, DeliveryMode::Steer))
        .unwrap();
    assert!(matches!(
        receive(host, &mut reply, false),
        RequestResult::Dispatched(_)
    ));
    ready(host, &session.id);
}

pub fn run() {
    let contract: serde_json::Value =
        serde_json::from_str(include_str!("../../../../fixtures/pi-rpc/host-resume.json")).unwrap();
    for case in contract["cases"].as_array().unwrap() {
        let case = case.as_str().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join(BuildFlavor::Dev.directory_name());
        let cwd = temp.path().join("project");
        std::fs::create_dir(&cwd).unwrap();
        let mut host = Host::new(
            State::open(&data, BuildFlavor::Dev).unwrap(),
            &data,
            "resume-boot".into(),
            "a".repeat(64),
        )
        .unwrap();
        let Response::Project(project) = response(
            &host,
            Request::AddProject(AddProjectParams {
                name: None,
                root: cwd.to_str().unwrap().into(),
            }),
        ) else {
            panic!("project expected")
        };
        let params = CreateSessionParams {
            session_id: "native-source".into(),
            project_id: project.id,
            runtime_id: "pi".into(),
            expected_cwd_fence: project.cwd_fence,
            start: SessionStart::New,
        };
        let (node, _) = pi_fixture::command(&cwd, "normal");
        let installation = Installation {
            node,
            cli: Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../test/fake-pi.mjs")
                .canonicalize()
                .unwrap(),
        };
        host.start_installed_pi(Access::Local, &params, &installation)
            .unwrap();
        let source = ready(&host, &params.session_id);
        turn(&host, &source, "remember");
        if case != "busy_source" {
            close(&host, &source);
        }
        let folder = std::fs::read_dir(data.join("agent-sessions"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let file = std::fs::read_dir(&folder)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let bytes = std::fs::read(&file).unwrap();
        let mut header: serde_json::Value =
            serde_json::from_slice(bytes.split(|b| *b == b'\n').next().unwrap()).unwrap();
        let native = header["id"].as_str().unwrap().to_owned();
        let mut resume = params.clone();
        resume.session_id = "native-resumed".into();
        resume.start = SessionStart::Resume(ResumeStartParams {
            source_session_id: source.id.clone(),
        });
        match case {
            "missing_history" => {
                std::fs::remove_file(&file).unwrap();
            }
            "ambiguous_history" => {
                std::fs::copy(&file, folder.join(format!("duplicate_{native}.jsonl"))).unwrap();
            }
            "wrong_id" => {
                header["id"] = "other-native".into();
            }
            "wrong_cwd" => {
                header["cwd"] = temp.path().to_str().unwrap().into();
            }
            "bad_version" => {
                header["version"] = 2.into();
            }
            "missing_identity" => {
                rusqlite::Connection::open(data.join("state.sqlite3"))
                    .unwrap()
                    .execute(
                        "DELETE FROM native_sessions WHERE session_id=?1",
                        [&source.id],
                    )
                    .unwrap();
            }
            "stale_cwd" => {
                resume.expected_cwd_fence = "stale-fence".into();
            }
            "cwd_replaced" => {
                std::fs::rename(&cwd, temp.path().join("old-project")).unwrap();
                std::fs::create_dir(&cwd).unwrap();
            }
            "restart_resume" => {
                drop(host);
                host = Host::new(
                    State::open(&data, BuildFlavor::Dev).unwrap(),
                    &data,
                    "next-boot".into(),
                    "a".repeat(64),
                )
                .unwrap();
            }
            "exact_resume" | "busy_source" | "remote" | "idempotent" => {}
            _ => panic!("unknown Resume case"),
        }
        if matches!(case, "wrong_id" | "wrong_cwd" | "bad_version") {
            std::fs::write(
                &file,
                format!("{}\n", serde_json::to_string(&header).unwrap()),
            )
            .unwrap();
        }
        let before = std::fs::read(&file).ok();
        let access = if case == "remote" {
            Access::Remote
        } else {
            Access::Local
        };
        let result = host.start_installed_pi(access, &resume, &installation);
        if matches!(case, "exact_resume" | "restart_resume" | "idempotent") {
            let starting = result.unwrap();
            assert_ne!(starting.stream_id, source.stream_id);
            let resumed = ready(&host, &resume.session_id);
            // Native history, not the display cache, supplies the prior conversation.
            let initial = view(&host, &resume.session_id);
            assert!(initial.messages.is_empty());
            assert!(initial.history_truncated);
            let mut stale = resumed.clone();
            stale.stream_id = source.stream_id.clone();
            assert_eq!(
                host.command_pi(
                    Access::Local,
                    prompt(&stale, "must-not-send", DeliveryMode::Steer)
                )
                .unwrap_err()
                .to_string(),
                "STALE_STREAM"
            );
            turn(&host, &resumed, "recall");
            let displayed = serde_json::to_string(&view(&host, &resume.session_id)).unwrap();
            assert!(displayed.contains("Final synthetic 茶"));
            assert!(!displayed.contains(&native));
            assert!(!displayed.contains("No native history"));
            close(&host, &resumed);
            if case == "idempotent" {
                std::fs::remove_file(&file).unwrap();
                let retry = host
                    .start_installed_pi(Access::Local, &resume, &installation)
                    .unwrap();
                assert_eq!(retry.state, SessionState::Stopped);
                assert_eq!(retry.stream_id, resumed.stream_id);
                assert!(!file.exists(), "retry must not launch or recreate history");
            }
        } else {
            assert!(result.is_err(), "Resume must fail closed: {case}");
            assert_eq!(std::fs::read(&file).ok(), before);
            assert!(matches!(host.dispatch(Access::Local, 1,
                Request::GetSession(GetSessionParams { session_id: resume.session_id.clone() })).unwrap().result,
                RequestResult::Rejected(ref value) if value.error.code == "SESSION_NOT_FOUND"));
        }
        let Response::Runtimes(runtimes) = response(&host, Request::ListRuntimes) else {
            panic!("runtimes")
        };
        assert!(runtimes.items.is_empty());
        host.stop_owned_processes().unwrap();
        println!(
            "M1_PI_HOST_RESUME_OK case={case} modelCalls=0 exactFile=true noFallback=true noReplay=true productionDisabled=true"
        );
    }
}
