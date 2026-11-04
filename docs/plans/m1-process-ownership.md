# M1-3: owned process trees

Status: implementation in progress; no runnable production adapter and no real-agent smoke.

## Scope and contract

The offline [contract](../../fixtures/process-ownership/contract.json) precedes the native implementation. Its cases use only a Rust fixture executable. There is no new public protocol method or test-only agent launcher in a release daemon.

- `cafe-spaced::managed` owns OS resources, streams and process exit observations. The Host owns the live registry under its existing admission mutex. Adapter detection/commands and busy/idle interpretation remain M1-4/5/8.
- Spawn uses an absolute executable and rechecks the approved `Directory` identity immediately before launching. It actually sets that directory as cwd. POSIX rechecks again in the supervisor before launching the target. This is not filesystem isolation or an OS sandbox.
- Child environment is cleared and rebuilt from a small platform-base allowlist. Ambient daemon tokens, arbitrary `CAFE_SPACE_*`, loader injection variables and agent API credentials are never copied automatically. Authorized adapters may later obtain their own credentials from their normal home-directory configuration.
- stdin/stdout/stderr use bounded native pipes, not growing transcript buffers. Read chunks are at most 8 KiB. Slow readers apply backpressure; adapters must drain both outputs and bound their own record decoders. No background unbounded output queue is added.

## Native ownership

**Windows:** create a non-inheritable job with `KILL_ON_JOB_CLOSE`. `CreateProcessW` uses `CREATE_SUSPENDED` and `STARTUPINFOEX` with the explicit inherited-handle list containing only the child's three standard streams. Assign to the job before resuming the primary thread. Every failure terminates the suspended child and releases all handles; a child never executes unowned. Job breakaway is not enabled. Closing the job also handles a daemon crash in the kernel.

**POSIX:** start this executable's private supervisor entry point in its own process group, before the daemon opens SQLite or starts Tokio. FD 3 is the read end of a liveness pipe; only the owner holds its write end. FD 4 carries at most one fixed-size original-child exit report. Both are CLOEXEC before launching the agent; other nonstandard descriptors are closed. The supervisor remains the process-group anchor. Lease EOF, explicit stop or target exit kills the entire group, including the supervisor and ordinary grandchildren. It does not signal a remembered/reused PID. A deliberately detached process is outside this process-group model, as in v0.

The supervisor is internal process machinery, not a network or local IPC command. It validates its group leadership and inherited pipe kinds before any group signal. Invalid direct invocation fails without touching daemon state.

## Host and update integration

The process registry, not persisted `starting`/`ready` rows, determines whether any owned process remains alive. Spawn admission and process removal share the Host mutex with the update gate. `now` and local Shutdown stop/reap owned trees before checkpointing. `when_idle` conservatively waits for every owned process to exit in this stage; it does not mistake a ready adapter for an exited process. M1-8 will replace this conservative rule with observed busy/idle, ordered stopping and durable session outcomes.

Production `CreateSession` still returns `RUNTIME_UNAVAILABLE` until an actual adapter is wired. Synthetic tests exercise the same process/registry code without inventing a runnable production runtime.

## Verification and remaining limits

Run native tests on Windows, macOS, Linux x64 and Linux arm64. Include owner hard-kill, explicit stop, natural target exit with a surviving grandchild, inherited secrets/handles, cwd identity replacement, output chunk limits and spawn failure cleanup. A PID in a fixture is an observation only, not persistent ownership authority. Tests must always clean up their own processes on failure.

Task #28 stays doing until Claude reviews the final code and the same final SHA passes base CI 4/4 plus Native desktop 3/3 with original native test evidence. Real model calls, production deployment and sandbox claims are excluded.
