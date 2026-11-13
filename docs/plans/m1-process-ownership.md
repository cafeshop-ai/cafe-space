# M1-3: owned process trees

Status: M1-3 accepted at 400a448 (room review 208, task result 210). Same-SHA base CI 41813687155 passed 4/4 and Native desktop 41813687480 passed 3/3, with native raw evidence. Follow-ups below are being implemented for M1-4; no runnable production adapter or real-agent smoke.

## Scope and contract

The offline [contract](../../fixtures/process-ownership/contract.json) precedes the native implementation. Its cases use only a Rust fixture executable. There is no new public protocol method or test-only agent launcher in a release daemon.

- `cafe-spaced::managed` owns OS resources, streams and process exit observations. The Host owns the live registry under its existing admission mutex. Adapter detection/commands and busy/idle interpretation remain M1-4/5/8.
- Spawn uses an absolute executable and rechecks the approved `Directory` identity immediately before launching. It actually sets that directory as cwd. POSIX rechecks again in the supervisor before launching the target. This is not filesystem isolation or an OS sandbox.
- D20: children inherit the daemon's full environment, including API keys, proxy/CA variables, SSH agent, NODE_OPTIONS and platform variables. Only inherited CAFE_SPACE_* names are removed; no session-specific names are currently used or injected. Owner overrides win; on Unix a proxy override replaces both uppercase and lowercase forms, and removal restores each inherited value. Windows names remain case-insensitive. Override names are bounded and exclude reserved names, equals signs and control characters; at most 256 overrides and 64 KiB encoded private settings. The inherited environment is not truncated to that override budget. Values remain private; status is masked, and no values are logged or published as events. No login shell is executed. Process/handle ownership is unchanged.
- stdin/stdout/stderr use bounded native pipes, not growing transcript buffers. Read chunks are at most 8 KiB. Slow readers apply backpressure; adapters must drain both outputs and bound their own record decoders. No background unbounded output queue is added.

## Native ownership

**Windows:** create a non-inheritable job with `KILL_ON_JOB_CLOSE`. `CreateProcessW` uses `CREATE_SUSPENDED` and `STARTUPINFOEX` with the explicit inherited-handle list containing only the child's three standard streams. `PROC_THREAD_ATTRIBUTE_JOB_LIST` assigns the job atomically with creation, eliminating the crash window of a later `AssignProcessToJobObject` call. Only then resume the primary thread. Job breakaway is not enabled. Closing the job also handles a daemon crash in the kernel, including before resume. This [job-list attribute](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute) requires Windows 10 / Server 2016 or newer; unavailable support fails closed, without a post-creation assignment fallback. A native regression test confirms membership while still suspended and termination on last-job-handle close before any resume. Managed spawn rejects `.bat` / `.cmd`, including case variants, resolved targets and trailing-dot/space Win32 aliases, with `PROCESS_PROGRAM_UNSUPPORTED`; it never falls back to a shell.

**POSIX:** start this executable's private supervisor entry point in its own process group, before the daemon opens SQLite or starts Tokio. FD 3 is the read end of a liveness pipe; only the owner holds its write end. FD 4 carries at most one fixed-size original-child exit report. Both are CLOEXEC before launching the agent; other nonstandard descriptors are closed. The supervisor remains the process-group anchor. Lease EOF, explicit stop or target exit kills the entire group, including the supervisor and ordinary grandchildren. It does not signal a remembered/reused PID. The supervisor verifies the identity of an opened `.` directory and the target inherits that cwd; it does not resolve/chdir through the pathname a second time. A deliberately detached process (`setsid` or a different process group) is outside this model, as in v0. If the supervisor itself receives `SIGKILL` (including OOM kill), other group members are not automatically killed; this is a known limitation, not sandbox containment. Exit waits currently poll every 20 ms per supervisor and 100 ms in the daemon; pidfd/kqueue-based event-driven waiting is a later follow-up.

The supervisor is internal process machinery, not a network or local IPC command. It validates its group leadership and inherited pipe kinds before any group signal. Invalid direct invocation fails without touching daemon state.

## Host and update integration

The process registry, not persisted `starting`/`ready` rows, determines whether any owned process remains alive. Spawn admission and process removal share the Host mutex with the update gate. A 100 ms runtime poll refreshes the live count even when no process has just exited. `now` and local Shutdown stop owned trees before checkpointing; runtime signal/error exits also clean up. `when_idle` conservatively waits for every owned process to exit in this stage; it does not mistake a ready adapter for an exited process. Close completion records `SESSION_CLOSED`; other observed process exits record `AGENT_EXITED`. M1-8 will replace this conservative rule with observed busy/idle and the full update-specific stop lifecycle.

Production `CreateSession` still returns `RUNTIME_UNAVAILABLE` until an actual adapter is wired. Synthetic tests exercise the same process/registry code without inventing a runnable production runtime.

## Verification and remaining limits

Run native tests on Windows, macOS, Linux x64 and Linux arm64. Include owner hard-kill, explicit stop, natural target exit with a surviving grandchild, inherited secrets/handles, cwd identity replacement, output chunk limits and spawn failure cleanup. A PID in a fixture is an observation only, not persistent ownership authority. Tests must always clean up their own processes on failure.

Task #28 is done at the reviewed 400a448. Later hardening changes require their own same-SHA native validation and do not reuse that acceptance evidence. Real model calls, production deployment and sandbox claims are excluded.
