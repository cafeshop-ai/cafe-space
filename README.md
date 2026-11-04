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

See [`docs/architecture.md`](docs/architecture.md) for the design and milestones. The M0 scaffold now includes the draft local API v2 wire types, strict Rust codecs, generated TypeScript types and fixtures. The pinned v0 Go corpus lives in `fixtures/v0-go/`; new daemon API examples live in `fixtures/v2/`. The daemon now runs headless over owner-only OS IPC, with SQLite WAL, a single-instance lock and bounded diagnostic logging. Its read-only API supports the version handshake, health and empty views/subscriptions. Explicit per-user autostart registration is implemented, with opt-in disposable-runner integration checks. The narrow M0 UI port and thin Tauri IPC proxy are implemented with a separate native WebView validation workflow. Explicit verified daemon copy-out, OS startup, update handover and interrupted-update recovery are implemented and exercised by the native workflow. Session execution, adapters and the later UI/distribution milestones remain unimplemented. See [`docs/protocol/v2-local-api.md`](docs/protocol/v2-local-api.md) for the draft contract.

## Connect the desktop UI (M0)

Build `web/` first, then `cargo build --locked --manifest-path desktop/src-tauri/Cargo.toml`.
Run the development daemon below separately, then open the built `cafe-space-desktop` executable.
It uses owner-only IPC and a bounded Tauri `Channel`, without passing tokens to JavaScript.
The UI reads health, sessions and a snapshot/resumed subscription before reporting connected;
M0 truthfully lists zero sessions. Room/remote pages name their later milestones instead of
calling the cloud. Browser-only Vite preview stays disconnected.

See [`desktop/README.md`](desktop/README.md) for native prerequisites, security bounds and the
opt-in real WebView probe. Desktop packaging, tray and shell self-updates remain outside this thin spike.

## Run the daemon manually

```sh
cargo run -p cafe-spaced --bin cafe-spaced-dev
```

This foreground command uses only the separate development names and paths in architecture §5.5; Ctrl-C stops it. Linux uses an absolute `XDG_RUNTIME_DIR` when configured; if unset it uses the owner-only `/run/user/<uid>`, or `<data dir>/run` when that standard directory is absent. Linux socket paths must fit within 107 bytes plus a NUL terminator. An insecure standard directory or relative configured path is rejected, not repaired. The old `/tmp/cafe-space-dev-<uid>` fallback is neither used nor removed; stop any daemon using that old fallback with its matching CLI before upgrading both binaries. It creates its private data directory, `state.sqlite3`, `cafe-spaced-dev.lock` and `logs/daemon.log`. It **does not register autostart, start an agent, or open TCP/HTTP ports**. The CLI exposes explicit startup management, read-only `status` and independent `update finish|rollback` recovery; session commands are not implemented yet. Automated tests exercise real local IPC directly.

- Unix sockets use mode 0600 in a 0700 directory, with peer-UID checks. Windows pipes have a current-user-only DACL, reject remote clients and require the first pipe instance. Existing insecure state permissions, final symlinks/junctions and hard-linked state files are rejected, not silently repaired; macOS state paths with extended ACLs are rejected too.
- At most 32 connections are served. Hello must arrive within 5 seconds; after a frame starts, the rest must arrive within 30 seconds. Writes have a 30-second deadline. Timeout/cancellation closes the connection; commands are never replayed. Idle established connections can remain open. Transient accept/resource errors retry with 100 ms–1 s backoff; a disconnected Windows client does not leave a failed pipe instance in the listener.
- SQLite uses WAL, `synchronous=FULL`, a five-second busy timeout and a version/application marker. M1 upgrades schema 1 or 2 to 3 after one verified private SQLite backup of the starting version, with every migration step and its history row in one transaction. Schema 2 adds sessions; schema 3 adds project records. An uncommitted handover cannot migrate. Unknown/newer databases are rejected. **Older M0/schema-2 daemons cannot reopen schema 3**; rollback never restores a backup automatically. The registry is present, but new sessions remain rejected until approved projects and adapters are connected.
- Diagnostics contain only timestamps and bounded event codes, not payloads, credentials or paths. Diagnostic write failures are counted in memory without stopping the daemon; insecure paths and invalid diagnostic codes are still rejected. The active log and its single `daemon.log.1` archive are each bounded to 1 MiB. Rotation durably replaces the archive before truncating the active log; a rotation failure preserves the active bytes.
- Ordinary tests use temporary directories and unique endpoints, including real framing, exclusive binds, lock contention, killed-process WAL recovery and read-only native definition parsers. They do not modify login/startup configuration. Ignored autostart and default-endpoint fallback integration tests additionally require explicit opt-in **and** a disposable GitHub Actions runner. Windows task stop/uninstall first requests owner-only IPC shutdown and waits for the daemon lock to release; an unconfirmed normal stop is reported before the existing hard-stop fallback. New Linux units use `KillMode=mixed`; exact legacy units are recognized without silently rewriting them (explicit uninstall/install applies the new definition).

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
