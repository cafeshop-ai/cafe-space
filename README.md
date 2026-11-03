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

See [`docs/architecture.md`](docs/architecture.md) for the design and milestones. The crates in this initial M0 skeleton define package boundaries only; the daemon, IPC, adapters, protocol, UI, and Tauri shell are not implemented yet. In particular, `desktop/` waits for its Tauri milestone, and `fixtures/` stays empty until Go-produced fixtures can be recorded rather than invented.
