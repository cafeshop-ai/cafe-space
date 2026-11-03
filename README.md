# Café Space

Café Space is a local-first desktop host for agent sessions, remote access, and collaboration rooms. Its desktop daemon and CLI are Rust; the UI is TypeScript/React; the existing cloud remains Go.

## Build and test

Requirements: stable Rust 1.85 or newer, Node.js 22.12 or newer, and pnpm 10.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo build --workspace --all-targets
pnpm --dir web install --frozen-lockfile
pnpm --dir web build
```

Build the isolated development binaries with development-only names and data-directory identifiers:

```sh
cargo build -p cafe-spaced --no-default-features --features dev-build
cargo build -p cafe-space-cli --no-default-features --features dev-build
```

See [`docs/architecture.md`](docs/architecture.md) for the design and milestones. The crates in this initial M0 skeleton define package boundaries only; the daemon, IPC, adapters, protocol, UI, and Tauri shell are not implemented yet. In particular, `desktop/` waits for its Tauri milestone, and `fixtures/` stays empty until Go-produced fixtures can be recorded rather than invented.

`cargo build --workspace --all-targets` builds the standard binaries and their separately named development binaries. The `*-dev` flavor uses isolated development data directory names.
