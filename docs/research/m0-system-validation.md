# M0 system-manager and shutdown validation

## Verified baseline

Source: `077cb5ae647d023068a131c7ea4cf52ce1f26eb9` (2026-11-03), Rust **1.95.0** across the main and desktop workspaces.

- [Base CI 41736606528](https://github.com/cafeshop-ai/cafe-space/actions/runs/41736606528): Windows, macOS, Linux x64 and Linux ARM64 passed, including explicit disposable-runner startup checks and Linux foreground IPC without `XDG_RUNTIME_DIR`.
- [Native CI 41736606614](https://github.com/cafeshop-ai/cafe-space/actions/runs/41736606614): Windows, macOS and Linux passed the actual OS-manager installation/update/recovery test and native WebView probe. All three jobs' assertion-backed markers were read directly.
- The preceding Rust 1.89 source `9b777fb` independently passed [base CI 41733873090](https://github.com/cafeshop-ai/cafe-space/actions/runs/41733873090) and [native CI 41733873230](https://github.com/cafeshop-ai/cafe-space/actions/runs/41733873230). `1d1af4d` also passed [base CI 41731571468](https://github.com/cafeshop-ai/cafe-space/actions/runs/41731571468) and [native CI 41731571385](https://github.com/cafeshop-ai/cafe-space/actions/runs/41731571385).

The compiler upgrade has its own native evidence; it is not inferred from the older compiler's green runs. Local Rust 1.95 checks also passed 153 workspace tests, nine desktop tests and strict Clippy for both workspaces; OS-registration tests remain deliberately ignored on the development machine.

## Task #7: actual installation and handover

Each native job emitted this assertion-backed marker after executing its OS-manager test:

```text
M0_HANDOVER_OS_OK install=true now=true whenIdle=true rollback=true recoveryCuts=true cliRecovery=true shellOnlyNoop=true statePreserved=true
```

| OS | Native job |
|---|---|
| Linux | [126267302850](https://github.com/cafeshop-ai/cafe-space/actions/runs/41736606614/job/126267302850) |
| macOS | [126267303041](https://github.com/cafeshop-ai/cafe-space/actions/runs/41736606614/job/126267303041) |
| Windows | [126267302371](https://github.com/cafeshop-ai/cafe-space/actions/runs/41736606614/job/126267302371) |

`desktop/src-tauri/src/installation.rs::os_manager_install_update_and_rollback` verifies private copy-out and compiled SHA-256; actual system launch and boot identity; immediate and idle handover; failed-target binary rollback; interrupted rename/start/commit recovery cuts; independent CLI finish/rollback; shell-only matching-binary no-op; and preservation of a SQLite sentinel. A provisional target remains non-ready until explicit commit. Recovery never overwrites SQLite or inversely migrates its schema.

The Windows failed-target regression no longer reproduces in the green runs above; the original `INSTALL_SERVICE_FAILED action=Verify kind=Other` has not been isolated to a single causal change. An independently reproduced defect was that a supposedly read-only SQLite compatibility check created WAL/SHM side effects. The shared check now rejects nonempty WAL/hot journals and uses an immutable URI only after that rejection. Tests cover live/newer state, recovery journals, non-ASCII/spaces/`#`/`%` paths, byte preservation and absence of new sidecars.

All three native probes also emitted `nativeWebview=true`, `channel=true`, `sessions=0`, `daemonSurvivedShellExit=true`, `startupRegistration=false`. The probe itself does not register startup; the separate explicitly opted-in OS-manager test does so only on its disposable runner.

## Task #13: reliability follow-up

The table records the M0 baseline above. M1-10 replaces its Linux `/tmp` fallback with `<data dir>/run`; the older green runs do not verify that replacement. See the current path rules in architecture §5.5 and the dedicated M1 CI checks.

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

The Windows stop result covers explicit owner-only IPC stop/uninstall, **not OS logoff/shutdown notifications** (`CTRL_LOGOFF_EVENT`, `CTRL_SHUTDOWN_EVENT`, `WM_QUERYENDSESSION`). M1-9 remains required and must not be skipped on the strength of task #13.

M0 has **no real agent registry or sessions**. Busy/drain admission tests use synthetic work; actual OS handovers exercise the idle M0 daemon. M1 must connect orderly stop to real agent cancellation/process ownership before checkpoint. This evidence does not validate real-session update/resume, signed installers, shell self-updates, WebRTC, or the overall project Goal.

No development-machine startup registration, existing Café identity, production TURN allocation, or production deployment was changed for these checks.
