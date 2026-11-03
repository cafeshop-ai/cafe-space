# Café Space

Café Space is a local-first desktop host for agent sessions, remote access, and collaboration rooms. Its desktop daemon and CLI are Rust; the UI is TypeScript/React; the existing cloud remains Go.

## Build and test

Requirements: Rust 1.89.0 (pinned by `rust-toolchain.toml`), Node.js 22.12 or newer, and pnpm 10.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo build --workspace --all-targets
pnpm --dir web install --frozen-lockfile
pnpm --dir web build
```

`cargo build --workspace --all-targets` builds both release and development executables. The executable entrypoint selects the flavor, so development builds always use the separate `-dev` names and `cafe-space-dev` data directory without Cargo features.

See [`docs/architecture.md`](docs/architecture.md) for the design and milestones. The M0 scaffold now includes the draft local API v2 wire types, strict Rust codecs, generated TypeScript types and fixtures. The pinned v0 Go corpus lives in `fixtures/v0-go/`; new daemon API examples live in `fixtures/v2/`. The daemon now runs headless over owner-only OS IPC, with SQLite WAL, a single-instance lock and bounded diagnostic logging. Its read-only API supports the version handshake, health and empty views/subscriptions. Autostart installation, session execution, adapters, the full UI and the Tauri shell are still unimplemented. See [`docs/protocol/v2-local-api.md`](docs/protocol/v2-local-api.md) for the draft contract.

## Run the daemon manually (M0)

```sh
cargo run -p cafe-spaced --bin cafe-spaced-dev
```

This foreground command uses only the separate development names and paths in architecture §5.5; Ctrl-C stops it. Linux requires an absolute `XDG_RUNTIME_DIR`. It creates its private data directory, `state.sqlite3`, `cafe-spaced-dev.lock` and `logs/daemon.log`. It **does not register autostart, start an agent, or open TCP/HTTP ports**. The CLI is still a scaffold; automated tests exercise real local IPC directly.

- Unix sockets use mode 0600 in a 0700 directory, with peer-UID checks. Windows pipes have a current-user-only DACL, reject remote clients and require the first pipe instance. Existing insecure state permissions, final symlinks/junctions and hard-linked state files are rejected, not silently repaired; macOS state paths with extended ACLs are rejected too.
- At most 32 connections are served. Hello must arrive within 5 seconds; after a frame starts, the rest must arrive within 30 seconds. Writes have a 30-second deadline. Timeout/cancellation closes the connection; commands are never replayed. Idle established connections can remain open.
- SQLite uses WAL, `synchronous=FULL`, a five-second busy timeout and a version/application marker. Unknown or newer databases are rejected without migration. M0 has no session tables.
- Diagnostics contain only timestamps and bounded event codes, not payloads, credentials or paths. The single 1 MiB log is reset when full; historical retention is not implemented yet.
- Tests use temporary directories and unique endpoints, including real framing, exclusive binds, lock contention and a killed-process WAL recovery test. They do not modify login/startup configuration.
