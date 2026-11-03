# M0 system-manager and shutdown validation

## Verified baseline

Source: `9b777fbdd27b44bbb534477d080ac6e6ce3dc9e1` (2026-11-03).

- [Base CI 41733873090](https://github.com/cafeshop-ai/cafe-space/actions/runs/41733873090): Windows, macOS, Linux x64 and Linux ARM64 passed, including explicit disposable-runner startup checks and Linux foreground IPC without `XDG_RUNTIME_DIR`.
- [Native CI 41733873230](https://github.com/cafeshop-ai/cafe-space/actions/runs/41733873230): Windows, macOS and Linux passed the actual OS-manager installation/update/recovery test and native WebView probe.
- The preceding `1d1af4d` independently passed [base CI 41731571468](https://github.com/cafeshop-ai/cafe-space/actions/runs/41731571468) and [native CI 41731571385](https://github.com/cafeshop-ai/cafe-space/actions/runs/41731571385).

These runs used Rust 1.89. The subsequent architecture-approved Rust 1.95 baseline is a separate change: local workspace and desktop checks pass, but its new cross-platform CI must be checked independently. Historical green runs are not evidence of a newer compiler's behavior.

## Task #7: actual installation and handover

Each native job emitted this assertion-backed marker after executing its OS-manager test:

```text
M0_HANDOVER_OS_OK install=true now=true whenIdle=true rollback=true recoveryCuts=true cliRecovery=true shellOnlyNoop=true statePreserved=true
```

| OS | Native job |
|---|---|
| Linux | [126257897429](https://github.com/cafeshop-ai/cafe-space/actions/runs/41733873230/job/126257897429) |
| macOS | [126257897779](https://github.com/cafeshop-ai/cafe-space/actions/runs/41733873230/job/126257897779) |
| Windows | [126257897973](https://github.com/cafeshop-ai/cafe-space/actions/runs/41733873230/job/126257897973) |

`desktop/src-tauri/src/installation.rs::os_manager_install_update_and_rollback` verifies private copy-out and compiled SHA-256; actual system launch and boot identity; immediate and idle handover; failed-target binary rollback; interrupted rename/start/commit recovery cuts; independent CLI finish/rollback; shell-only matching-binary no-op; and preservation of a SQLite sentinel. A provisional target remains non-ready until explicit commit. Recovery never overwrites SQLite or inversely migrates its schema.

The Windows failed-target regression no longer reproduces in the green runs above; the original `INSTALL_SERVICE_FAILED action=Verify kind=Other` has not been isolated to a single causal change. An independently reproduced defect was that a supposedly read-only SQLite compatibility check created WAL/SHM side effects. The shared check now rejects nonempty WAL/hot journals and uses an immutable URI only after that rejection. Tests cover live/newer state, recovery journals, non-ASCII/spaces/`#`/`%` paths, byte preservation and absence of new sidecars.

All three native probes also emitted `nativeWebview=true`, `channel=true`, `sessions=0`, `daemonSurvivedShellExit=true`, `startupRegistration=false`. The probe itself does not register startup; the separate explicitly opted-in OS-manager test does so only on its disposable runner.

## Task #13: reliability follow-up

| Requirement | Implementation and verification |
|---|---|
| Transient accept errors do not terminate the daemon | Resource/disappearing-client errors retry with bounded 100 ms–1 s backoff; Windows replaces the failed pipe instance. Real disconnect/reconnect and error-classification tests pass. |
| Diagnostic I/O failures are nonfatal | Failure counts remain in memory; invalid codes and unsafe paths still fail closed. Dedicated tests cover failed append and failed rotation. |
| Linux stop targets the daemon first | New units use `KillMode=mixed`, `Restart=no`. Native definition tests and actual systemd user-manager checks pass. Exact legacy units can be verified/removed but are not silently rewritten. |
| Windows normal stop | Stop/uninstall first use owner-only Shutdown, wait for lock release, and only then use a reported hard-stop fallback if needed. Disposable Task Scheduler integration asserts `DAEMON_STOP_REQUESTED` and `DAEMON_STOPPED`. |
| Linux without `XDG_RUNTIME_DIR` | Owner-only `/run/user/<uid>` or, only if absent, `/tmp/cafe-space[-dev]-<uid>`. Insecure/relative configured paths are rejected, not repaired. Actual foreground IPC/identity/Shutdown checks pass on x64 and ARM64; pure tests exercise insecure and absent directory branches. |
| One previous bounded log | Active `daemon.log` and one `daemon.log.1`, each at most 1 MiB; archive replacement is durable before truncation. Tests prove exact retained bytes, bounded size and preservation of the current log when rotation fails. |

## Failed-run history and boundaries

`725a7e1` base CI failed on both Linux jobs because the new headless test sent the `list_sessions` fixture and then asserted a Health result. `1d1af4d` fixed the test to send an explicit `Request::GetStatus`; the actual IPC fallback had already connected. This was distinct from the earlier unused-import lint failure.

M0 has **no real agent registry or sessions**. Busy/drain admission tests use synthetic work; actual OS handovers exercise the idle M0 daemon. M1 must connect orderly stop to real agent cancellation/process ownership before checkpoint. This evidence does not validate real-session update/resume, signed installers, shell self-updates, WebRTC, or the overall project Goal.

No development-machine startup registration, existing Café identity, production TURN allocation, or production deployment was changed for these checks.
