//! Exercises production I/O workers with an offline process, under native Process ownership.
use super::*;
use cafe_space_adapters::pi::{Command as RpcCommand, Observation, Outcome};
use cafe_space_protocol::{ApprovalDecision, SessionState};
use cafe_spaced::pi_driver::Driver;

fn collect(
    driver: &mut Driver,
    done: impl Fn(&Driver, &[Observation]) -> bool,
) -> Vec<Observation> {
    let mut observations = Vec::new();
    until(|| {
        observations.extend(driver.poll(Instant::now()));
        done(driver, &observations)
    });
    observations
}
fn started(root: &Path, mode: &str) -> (Process, Driver) {
    let mut process = pi_fixture::spawn_fake(root, mode);
    let mut driver = Driver::new(
        process.take_streams().unwrap(),
        "native-fixture-1".into(),
        Instant::now(),
    )
    .unwrap();
    assert_eq!(driver.state(), &SessionState::Starting);
    assert!(
        driver
            .send(
                RpcCommand::Prompt {
                    message: "not-before-identity".into()
                },
                Instant::now()
            )
            .is_err()
    );
    collect(&mut driver, |driver, _| {
        driver.state() == &SessionState::Ready
    });
    (process, driver)
}
pub fn run() {
    let root = tempfile::tempdir().unwrap();
    for mode in ["normal", "stderr-flood"] {
        let (mut process, mut driver) = started(root.path(), mode);
        driver
            .send(
                RpcCommand::Prompt {
                    message: "display-limits".into(),
                },
                Instant::now(),
            )
            .unwrap();
        let observed = collect(&mut driver, |_, observed| {
            observed
                .iter()
                .any(|value| matches!(value, Observation::State(SessionState::Ready)))
        });
        let display = format!("{observed:?}");
        assert!(
            display.contains("RPC record omitted") && display.contains("display limit survived")
        );
        assert!(!display.contains("synthetic-private-diagnostics"));
        driver
            .send(
                RpcCommand::Prompt {
                    message: "approval".into(),
                },
                Instant::now(),
            )
            .unwrap();
        let observed = collect(&mut driver, |_, observed| {
            observed
                .iter()
                .any(|value| matches!(value, Observation::Approval(_)))
        });
        let approval = observed
            .iter()
            .find_map(|value| {
                if let Observation::Approval(approval) = value {
                    Some(approval.id.clone())
                } else {
                    None
                }
            })
            .unwrap();
        assert!(
            driver
                .approve(&approval, ApprovalDecision::AllowForSession, Instant::now())
                .is_err()
        );
        driver
            .approve(&approval, ApprovalDecision::AllowOnce, Instant::now())
            .unwrap();
        assert!(
            driver
                .approve(&approval, ApprovalDecision::Deny, Instant::now())
                .is_err()
        );
        let observed = collect(&mut driver, |_, observed| {
            observed
                .iter()
                .any(|value| matches!(value, Observation::State(SessionState::Ready)))
        });
        assert!(
            observed
                .iter()
                .any(|value| matches!(value,Observation::ApprovalDispatched {id} if id==&approval))
        );
        driver
            .send(
                RpcCommand::Prompt {
                    message: "busy".into(),
                },
                Instant::now(),
            )
            .unwrap();
        collect(&mut driver, |driver, _| {
            driver.state() == &SessionState::Running
        });
        assert!(driver.send(RpcCommand::Abort, Instant::now()).is_err());
        let clear = driver.send(RpcCommand::ClearQueue, Instant::now()).unwrap();
        collect(&mut driver, |_, observed| {
            observed.iter().any(|value|matches!(value,Observation::Outcome {id,outcome:Outcome::Applied} if id==&clear))
        });
        driver.send(RpcCommand::Abort, Instant::now()).unwrap();
        collect(&mut driver, |driver, _| {
            driver.state() == &SessionState::Ready
        });
        process.stop().unwrap();
        drop(driver);
        println!(
            "M1_PI_DRIVER_OK case={mode} modelCalls=0 startupIdentity=true boundedOutput=true approvalWriteConfirmed=true cancellation=true stderrPrivate=true"
        );
    }
    for mode in ["crash", "malformed", "no-response"] {
        let (mut process, mut driver) = started(root.path(), mode);
        let now = Instant::now();
        let id = driver
            .send(
                RpcCommand::Prompt {
                    message: "synthetic".into(),
                },
                now,
            )
            .unwrap();
        let observed = if mode == "no-response" {
            driver.poll(now + Duration::from_secs(31))
        } else {
            collect(&mut driver, |driver, _| {
                matches!(driver.state(), SessionState::Failed(_))
            })
        };
        assert!(observed.iter().any(|value|matches!(value,Observation::Outcome {id: found,outcome:Outcome::Unknown(_)} if found==&id)));
        assert!(matches!(driver.state(), SessionState::Failed(_)));
        assert!(driver.send(RpcCommand::GetState, Instant::now()).is_err());
        assert!(driver.poll(Instant::now()).is_empty());
        process.stop().unwrap();
        drop(driver);
        println!(
            "M1_PI_DRIVER_FAILURE_OK case={mode} modelCalls=0 outcomeUnknown=true noReplay=true"
        );
    }
    {
        let (mut process, mut driver) = started(root.path(), "no-read");
        let now = Instant::now();
        let mut queued = 0;
        for _ in 0..32 {
            match driver.send(
                RpcCommand::Prompt {
                    message: "x".repeat(900000),
                },
                now,
            ) {
                Ok(_) => queued += 1,
                Err(error) => {
                    assert_eq!(error.0, "PI_INPUT_BACKPRESSURE");
                    break;
                }
            }
        }
        assert!((1..=9).contains(&queued));
        let observed = driver.poll(now + Duration::from_secs(31));
        assert!(matches!(driver.state(), SessionState::Failed(_)));
        assert_eq!(
            observed
                .iter()
                .filter(|value| matches!(
                    value,
                    Observation::Outcome {
                        outcome: Outcome::Unknown(_),
                        ..
                    }
                ))
                .count(),
            queued
        );
        process.stop().unwrap();
        drop(driver);
        println!(
            "M1_PI_DRIVER_BACKPRESSURE_OK modelCalls=0 queueBounded=true noReadBlockedWriter=true deadlineUnknown=true processReaped=true"
        );
    }
}
