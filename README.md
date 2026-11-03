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

See [`docs/architecture.md`](docs/architecture.md) for the design and milestones. The M0 scaffold now includes the draft local API v2 wire types, strict Rust codecs, generated TypeScript types and fixtures. The pinned v0 Go corpus lives in `fixtures/v0-go/`; new daemon API examples live in `fixtures/v2/`. The daemon also has bounded socket/pipe framing and a read-only connection handler tested in memory (version handshake, health, empty views/subscriptions); executable startup and owner-only OS listeners are not wired yet. Persistence, session execution, adapters, the full UI and the Tauri shell are still unimplemented. See [`docs/protocol/v2-local-api.md`](docs/protocol/v2-local-api.md) for the draft contract.
