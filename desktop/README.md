# Thin desktop shell (M0 / D13)

The bundled React UI talks to the daemon through Tauri commands and `Channel`,
not HTTP. The Rust proxy uses the same owner-only socket / named pipe and strict
v2 codec as the CLI. No local token, filesystem path or shell command is accepted
from JavaScript. There is no shell, filesystem or localhost Tauri plugin.
Only the bundled `main` window has the five explicit command permissions; remote
origins have none. CSP permits IPC, not cloud or loopback network connections.

## Build

Use the repository's pinned Rust toolchain and Node / pnpm versions. Install the
[platform prerequisites](https://v2.tauri.app/start/prerequisites/) for Tauri 2.
On Debian/Ubuntu the native WebKitGTK 4.1 and GTK 3 development packages are needed.
The desktop has its own Cargo workspace / lockfile so a headless daemon build does
not acquire a WebView dependency. `tauri` 2.11.6 / `tauri-build` 2.6.3 and the JS API
2.11.1 are pinned to the line compatible with Rust 1.89.

```text
pnpm --dir web install --frozen-lockfile
pnpm --dir web test
pnpm --dir web build
cargo test --locked --manifest-path desktop/src-tauri/Cargo.toml
cargo build --locked --manifest-path desktop/src-tauri/Cargo.toml
```

Debug builds connect to `cafe-spaced-dev`; release builds to `cafe-spaced`.
The daemon must already be running. In M0-4/M0-5 the shell does not spawn, install,
restart or stop it. Copy-out / update handover is M0-6, not implemented here.
Closing the shell closes its connection but leaves the independent daemon alive.

## Bounds and failure semantics

One native connection per window; 16 queued outgoing frames; at most one
unacknowledged incoming Channel delivery. A delivery must be acknowledged within
5 seconds or the whole connection is closed. Hello/connect also have 5-second
deadlines. IPC frames retain the shared 256 KiB limit and frame deadlines.
Native connection IDs fence stale callbacks and closes. Failed/cancelled writes
have an unknown outcome: the proxy never reconnects or replays commands.
Errors use fixed codes, not OS errors containing private paths.

The UI performs `get_status`, `list_sessions` and `subscribe_events` before
claiming to be connected. It retains the event cursor only in memory, requests a
snapshot after a gap, and labels offline retained data with its last-update time.
Browser-only Vite preview is explicitly disconnected, not a fake daemon.

## Opt-in real WebView probe

Build the UI, shell and `cargo build --locked -p cafe-spaced --bin cafe-spaced-dev`.
Set `CAFE_SPACE_TEST_DESKTOP=1`, then run `node desktop/scripts/probe.mjs`.
On headless Linux use `xvfb-run --auto-servernum node desktop/scripts/probe.mjs`.

The probe uses newly created temporary data/config directories, refuses an
already occupied development IPC endpoint, starts only its own child processes,
and never registers startup. The actual native WebView must receive the welcome
and empty-session result through `Channel`, initialize all three UI queries,
and report readiness. The debug-only, explicitly enabled `m0_finish` command
confirms Rust received an empty session list, prints a fixed success marker and
exits the shell. The probe then makes a fresh IPC request to prove the daemon
survived shell exit. All temporary processes and data are cleaned up.
The command is inert outside that opt-in debug probe.

`.github/workflows/desktop.yml` runs this on Windows WebView2, macOS WKWebView and
Linux WebKitGTK. A unit test or a queued CI job is not a native WebView pass.

## UI provenance

M0's narrow port follows the UI boundary confirmed in room message #43:
`web/src/layouts/WorkspaceLayout.tsx`, the toolbar/grid/sidebar shell rules, and
`CoffeeActivityMark` are adapted from v0 at commit
`67237383fef260a8111a14910bc97a56195de349` in `pi-packages`.
Space's colors/type replace v0's tokens per `docs/design/ux-ui.md` §7.
The v0 remote/collaboration runtime is deliberately not copied: those pages
truthfully name M2/M3, rather than connecting the desktop directly to the cloud.
The historical v0 Pi icon was rasterized for the M0 native app resources; this is
not a new Space brand/logo design.
