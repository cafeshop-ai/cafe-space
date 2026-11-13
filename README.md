# Café Space

Café Space is a local-first desktop host for agent sessions, remote access, and collaboration rooms. Its desktop daemon and CLI are Rust; the UI is TypeScript/React; the existing cloud remains Go.

## Build and test

Requirements: Rust 1.95.0 (pinned by `rust-toolchain.toml`), Node.js 22.12 or newer, and pnpm 10.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo build --workspace --all-targets
pnpm --dir web install --frozen-lockfile
pnpm --dir web test
pnpm --dir web build
```

`cargo build --workspace --all-targets` builds both release and development executables. The executable entrypoint selects the flavor, so development builds always use the separate `-dev` names and `cafe-space-dev` data directory without Cargo features.

See [`docs/architecture.md`](docs/architecture.md) for the design and milestones. Current source implements full wire v9, frozen update control v2 and SQLite schema 6, with strict Rust codecs, generated TypeScript bindings, owner-only IPC, private project/session storage and a thin Tauri UI. Full wire v6 introduced durable session activity/previews; v7 added explicit local registration of an existing pinned Pi installation, v8 added recovery, and v9 adds passive local discovery to prefill registration. Registered Pi New/Resume and session controls have offline native-process/IPC coverage. Final-revision cross-platform CI, authorized real-Agent acceptance, the Codex adapter and later remote/collaboration/distribution milestones remain open. The v0 Go corpus is frozen in `fixtures/v0-go/`; current local API examples live in `fixtures/v2/`. See [`docs/protocol/v2-local-api.md`](docs/protocol/v2-local-api.md) for the current contract.

## Connect the desktop UI (M0)

Build `web/` first, then `cargo build --locked --manifest-path desktop/src-tauri/Cargo.toml`.
Run the development daemon below separately, then open the built `cafe-space-desktop` executable.
It uses owner-only IPC and a bounded Tauri `Channel`, without passing tokens to JavaScript.
The UI reads health, sessions and a snapshot/resumed subscription before reporting connected;
An unconfigured installation lists no sessions; registered Pi sessions use the same UI. Room/remote pages name their later milestones instead of
calling the cloud. Browser-only Vite preview stays disconnected.

See [`desktop/README.md`](desktop/README.md) for native prerequisites, security bounds and the
opt-in real WebView probe. Desktop packaging, tray and shell self-updates remain outside this thin spike.

## Run the daemon manually

```sh
cargo run -p cafe-spaced --bin cafe-spaced-dev
```

This foreground command uses only the separate development names and paths in architecture §5.5; Ctrl-C stops it. Linux uses an absolute `XDG_RUNTIME_DIR` when configured; if unset it uses the owner-only `/run/user/<uid>`, or `<data dir>/run` when that standard directory is absent. Linux socket paths must fit within 107 bytes plus a NUL terminator. An insecure standard directory or relative configured path is rejected, not repaired. The old `/tmp/cafe-space-dev-<uid>` fallback is neither used nor removed; stop any daemon using that old fallback with its matching CLI before upgrading both binaries. It creates its private data directory, `state.sqlite3`, `cafe-spaced-dev.lock` and `logs/daemon.log`. It **does not register autostart, start an agent, or open TCP/HTTP ports**. The CLI exposes explicit startup management, `status [--json]`, fenced session commands and independent `update finish|rollback` recovery. Pi becomes selectable only after explicit runtime registration; starting the daemon alone never starts an Agent. Automated tests exercise real local IPC directly.

- Unix sockets use mode 0600 in a 0700 directory, with peer-UID checks. Windows pipes have a current-user-only DACL, reject remote clients and require the first pipe instance. Existing insecure state permissions, final symlinks/junctions and hard-linked state files are rejected, not silently repaired; macOS state paths with extended ACLs are rejected too.
- At most 32 connections are served. Hello must arrive within 5 seconds; after a frame starts, the rest must arrive within 30 seconds. Writes have a 30-second deadline. Timeout/cancellation closes the connection; commands are never replayed. Idle established connections can remain open. Transient accept/resource errors retry with 100 ms–1 s backoff; a disconnected Windows client does not leave a failed pipe instance in the listener.
- SQLite uses WAL, `synchronous=FULL`, a five-second busy timeout and a version/application marker. M1 upgrades schema 1 or 2 to 3 after one verified private SQLite backup of the starting version, with every migration step and its history row in one transaction. Schema 2 adds sessions; schema 3 adds project records. An uncommitted handover cannot migrate. Unknown/newer databases are rejected. **Older M0/schema-2 daemons cannot reopen schema 3**; rollback never restores a backup automatically. New sessions require an approved project and a valid explicitly registered Pi installation.
- Diagnostics contain only timestamps and bounded event codes, not payloads, credentials or paths. Diagnostic write failures are counted in memory without stopping the daemon; insecure paths and invalid diagnostic codes are still rejected. The active log and its single `daemon.log.1` archive are each bounded to 1 MiB. Rotation durably replaces the archive before truncating the active log; a rotation failure preserves the active bytes.
- Ordinary tests use temporary directories and unique endpoints, including real framing, exclusive binds, lock contention, killed-process WAL recovery and read-only native definition parsers. They do not modify login/startup configuration. Ignored autostart and default-endpoint fallback integration tests additionally require explicit opt-in **and** a disposable GitHub Actions runner. Windows task stop/uninstall first requests owner-only IPC shutdown and waits for the daemon lock to release; an unconfirmed normal stop is reported before the existing hard-stop fallback. New Linux units use `KillMode=mixed`; exact legacy units are recognized without silently rewriting them (explicit uninstall/install applies the new definition).

## Local project allowlist (M1-2)

```sh
cafe-space-dev project add /absolute/project --name Tea
cafe-space-dev project list
cafe-space-dev project root <id>
cafe-space-dev project update <id> <cwdFence> --name Tea
cafe-space-dev project update <id> <cwdFence> --root /absolute/reapproved-project
cafe-space-dev project remove <id> <cwdFence>
```

Use `project --help` for the exact syntax. Commands use owner-only local IPC and print ASCII JSON (Unicode is escaped); server rejection codes are preserved and requests are never automatically retried. `list` contains no paths; only local `root` returns one. Labels are shared metadata chosen by the owner. Relative CLI directories are resolved before sending; non-UTF-8 and control-character paths are rejected without lossy conversion.

Adding an already approved directory identity returns the existing project. Changing its identity rotates the random `cwdFence`; label-only edits keep it, even if the directory is offline. A missing or replaced root blocks session admission until an explicit local root update. Nonterminal sessions block root edits/removal, but not label-only edits. Adding a replaced directory at an already recorded path returns `PROJECT_ROOT_CHANGED`; explicitly reapprove it with `project update <id> <cwdFence> --root <directory>`. Removal retains terminal session history; re-adding creates a new ID/fence, so old history cannot silently resume against it. UNC/network filesystems, volume/mount roots (including Linux bind mounts) and daemon-data descendants are rejected. Linux mount-root detection requires kernel `statx` mount IDs (Linux 5.8+); unavailable identity information fails closed. Linux FUSE mount subtypes are checked through `/proc/self/mountinfo`: only `gocryptfs`, `ntfs-3g`/`fuseblk`, `bindfs` and `encfs` are supported; network and unknown FUSE backends are rejected. This only approves a working directory: **it is not an agent sandbox**. Registered Pi launches use the owned native process factory and recheck the directory identity immediately before spawn.

## Register an existing Pi installation

Use matching CLI/daemon builds and an existing Node >=22.19.0 executable plus `@earendil-works/pi-coding-agent` 1.5.0 package directory (the directory containing `package.json`, not a `pi.cmd` shim).

```text
cafe-space-dev runtime register pi <node-executable> <package-directory>
cafe-space-dev runtime list
cafe-space-dev runtime remove pi
```

Registration is explicit and local-only. It checks the package pin and privately saves canonical paths; it does not download packages, execute Node/Pi, import a shell or copy credentials. Availability is structural, not a Node-version or provider-authentication probe. Only owner-only local requests can change registration; lists/events do not expose installation paths. Removing registration blocks new sessions but leaves existing sessions running. A damaged configuration is not repaired automatically. Commands print ASCII JSON and never retry an unknown write; inspect `runtime list` before making another explicit decision.

**Pi tools run as your OS user without asking, not inside a sandbox.** Approve a project separately, then reopen the new-session sheet or use `session new` with runtime ID `pi`. These paths have synthetic IPC coverage; they are not yet authorized real-Agent acceptance.

## Session CLI (M1-7)

Use `cafe-space-dev session --help` for syntax; omit `-dev` for the separate release installation. These commands use the existing owner-only full API, not the installer request wrapper:

```text
cafe-space-dev status --json
cafe-space-dev session list --json
cafe-space-dev session get <sessionId> --json
cafe-space-dev session new <chosenSessionId> <projectId> <runtimeId> <cwdFence> --json
cafe-space-dev session new <chosenSessionId> <projectId> <runtimeId> <cwdFence> --resume <sourceSessionId> --json
cafe-space-dev session prompt <sessionId> <streamId> <cwdFence> --file prompt.txt --delivery follow_up --json
cafe-space-dev session cancel <sessionId> <streamId> <cwdFence> <runId> --json
cafe-space-dev session close <sessionId> <streamId> <cwdFence> --json
```

Copy fences/run IDs from authoritative project/session reads; the CLI never refreshes stale fences and resubmits for you. Choose the new session ID before sending so `session get` can reconcile a lost creation response. `--text` accepts one literal argument; `--file -` reads redirected stdin. Files/stdin use a UTF-8/UTF-16LE/UTF-16BE BOM when present, otherwise strict UTF-8, preserving newlines and rejecting malformed Unicode or over 64 KiB decoded UTF-8. Do not pipe strings through Windows PowerShell 5.1's default ASCII pipeline: use `--text` or a file. Its `Out-File` UTF-16LE BOM is supported. No ANSI/code-page guessing or lossy replacement is performed.

`--json` emits one ASCII JSON result, escaping non-ASCII UTF-16 code units (including surrogate pairs). Without it, session commands print the same outcome as readable UTF-8 JSON. Applied/dispatched exit 0; rejected/unknown exit nonzero and preserve the server code. **Dispatched means delivery acceptance, not turn completion.** A lost response after sending prints `unknown/CLI_OUTCOME_UNKNOWN`; no command is replayed. Connection failures before sending remain errors. Existing non-JSON `status` and project commands are unchanged.

`cargo test -p cafe-space-cli` runs the exact parser/output codec through subprocesses and BOM/stdin fixtures. Windows additionally checks `cmd` code page 936 and PowerShell 5.1/7; an absent shell is explicitly skipped locally but fails GitHub CI. These are synthetic encoding tests, not real-Agent acceptance; the IPC client separately verifies all four outcome variants and no resend after EOF over isolated native endpoints.

## Update controller compatibility

Replace the CLI/desktop controller **before** updating the daemon, using artifacts from the same source revision (or a newer compatible controller). Recovery must use a controller that supports the target schema, not the previous daemon's CLI. The desktop embeds its target daemon; build both from the same checkout. Headless/manual installations must replace the CLI first; do not update only the daemon or keep using an old shell process.

This is an installation requirement, not an enforced package updater: packaging and shell self-updates are not implemented yet. M0/schema-2 controllers strictly reject schema-3 migration markers with `INSTALL_MARKER_INVALID`; use the current CLI/desktop to finish recovery. Do not edit/delete `handover.json`, copy a backup over SQLite or downgrade the controller to bypass the check. A new controller's rollback still asks the old daemon's read-only `--check-state` whether the database is compatible; it never restores database bytes automatically.

## Explicit login startup management

These commands change the **current OS user's** startup configuration; daemon startup never calls them. They require a nonempty owner-only executable already staged at `<data dir>/bin/cafe-spaced-dev[.exe]`. Binary copy/hash verification belongs to the separate installation/update spike and is not provided by these commands yet.

```sh
cafe-space-dev autostart install
cafe-space-dev autostart start
cafe-space-dev autostart stop
cafe-space-dev autostart uninstall
```

`install` registers for the next login without starting now; `start` explicitly asks the OS to launch it. `uninstall` stops it and removes only its matching registration, **not the database or logs**. Existing definitions are never overwritten or silently adopted. A 30-second command timeout reports an unknown outcome; inspect the OS service manager before retrying.

- **Linux:** a user unit in `${XDG_CONFIG_HOME:-~/.config}/systemd/user/`, enabled with `systemctl --user`, explicitly `Restart=no`. A functioning systemd user manager is required; no system-wide service or linger setting is installed by the product.
- **macOS:** `~/Library/LaunchAgents/ai.cafeshop.cafe-spaced-dev.plist`, `RunAtLoad=true`, `KeepAlive=false`; explicit start/stop use `launchctl` in the current `gui/<uid>` domain. A logged-in GUI user session is required.
- **Windows:** a Task Scheduler logon task `cafe-spaced-dev-<user SID>` with an interactive current-user principal at least privilege, no password, no execution time limit and no automatic restart. SID suffixes prevent users colliding in the machine-wide task namespace.
- **Windows fallback:** `autostart install-run-key` and `autostart uninstall-run-key` explicitly manage only the matching `HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run` value. There is **no automatic fallback after a task error**, because registration may already have succeeded. Run entries cannot start/stop an already running daemon; they affect future logins only.

Native templates are emitted directly to keep quoting and restart policy explicit: XML escaping for plist/task definitions and systemd-specific escaping (not shell quoting) for `ExecStart`. No additional service-manager dependency is needed. CI explicitly opts in to install/start/handshake/stop/restart/uninstall on disposable runners; Windows also checks the Run fallback. Linux CI prepares its disposable user manager, but the product never changes system policy.
