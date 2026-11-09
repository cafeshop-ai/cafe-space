# Thin desktop shell (M0 / D13)

The bundled React UI talks to the daemon through Tauri commands and `Channel`,
not HTTP. The Rust proxy uses the same owner-only socket / named pipe and strict
v2 codec as the CLI. JavaScript never receives a local token or a shell command.
Project root paths travel only in explicit local project requests/results over the
owner-only IPC; shared lists/events remain path-free. There is no shell, filesystem
or localhost Tauri plugin.
Only the bundled `main` window has the explicit command permissions; remote
origins have none. CSP permits IPC, not cloud or loopback network connections.
The opener plugin is pinned to 2.5.5, compatible with Tauri 2.11.6 (2.6+
requires Tauri 2.12). It grants only `opener:allow-open-url` for explicit HTTP,
HTTPS and mailto URLs in the local main window, using the default system handler.
It has no default permission set, path/reveal access or caller-selected program;
automatic link-click injection is disabled so the UI confirmation cannot be
bypassed by the plugin. Opening a URL does not grant the WebView network access.

## Build

Use the repository's pinned Rust toolchain and Node / pnpm versions. Install the
[platform prerequisites](https://v2.tauri.app/start/prerequisites/) for Tauri 2.
On Debian/Ubuntu the native WebKitGTK 4.1 and GTK 3 development packages are needed.
The desktop has its own Cargo workspace / lockfile so a headless daemon build does
not acquire a WebView dependency. `tauri` 2.11.6 / `tauri-build` 2.6.3 and the JS API
2.11.1 remain pinned; both workspaces use the repository baseline Rust 1.95.0.

```text
pnpm --dir web install --frozen-lockfile
pnpm --dir web test
pnpm --dir web build
cargo build --locked -p cafe-spaced --bin cafe-spaced-dev
cargo test --locked --manifest-path desktop/src-tauri/Cargo.toml
cargo build --locked --manifest-path desktop/src-tauri/Cargo.toml
```

Debug builds connect to `cafe-spaced-dev`; release builds to `cafe-spaced`.
Release app identity is `ai.cafeshop.cafe-space`; debug uses the explicit
`tauri.dev.conf.json` identity `ai.cafeshop.cafe-space-dev`. A test enforces that
the two native configs differ only in identity. Any future debug packaging must
select that development config too; signed packaging is not part of M0.
The matching daemon artifact must be built **before** the desktop. The build script
embeds a snapshot of that executable plus its SHA-256 and Cargo version; it never
executes the daemon. For release, first build `cargo build --locked --release -p
cafe-spaced --bin cafe-spaced`, then build the desktop with `--release`.

Installation is explicit: the UI offers **安装并启用登录自启**, copies only verified
bytes into the private data directory, and delegates startup to the OS manager.
Opening the window does not register or start anything. Closing it leaves an
already-started daemon alive. No `tauri-plugin-shell` or daemon-core dependency
is added to the shell; the embedded binary is an opaque payload.

Updates offer idle-time handover or a confirmation dialog for immediate handover.
The daemon, not a UI idle check, owns the pending lease and admission barrier.
The native controller holds the startup-operation lock, verifies the OS definition
and running hash/boot identity, waits for draining to exit, then moves `.next` /
`.prev` under the single-instance lock. It commits only after a matching hello and
health response. Semantic downgrades are refused (build metadata alone does not
change precedence). Failed target startup falls back only after the old executable
passes its read-only state check; recovery never copies or replaces SQLite.

The UI exposes pending cancellation, progress, and separate **完成更新** /
**回到上一版** choices for interrupted handovers. `recovery_required` offers only
explicit restore, identifies the state/marker/previous-binary files, and never
overwrites SQLite. The shared IPC updater also powers the headless CLI:

```text
cafe-space-dev status
cafe-space-dev update finish
cafe-space-dev update rollback
```

These commands require no desktop installation/process or embedded daemon payload.
Use the controller from the target daemon's source revision (or a newer compatible
controller), replacing the CLI/desktop before updating the daemon. Build the shell
and its embedded target from the same checkout. Old M0/schema-2 controllers cannot
read schema-3 migration markers and report `INSTALL_MARKER_INVALID`; use the current
controller for recovery, never edit or delete the marker to bypass validation.
Packaging and shell self-updates are not implemented, so this ordering is an explicit
installation requirement rather than an automatic mixed-version guard.
Finish verifies the staged/installed target and commits its matching health; it
never silently chooses rollback. Rollback requires the old read-only state check.
A provisional daemon reports `ready: false` and `awaitingUpdateConfirmation: true`
until committed; neither client claims a ready session view from that response.
An older/manual binary without the hash, private boot record and read-only check
contract cannot be adopted for automated updates. Native unit tests cover payload
integrity and wrong-boot refusal; these do **not** prove full OS-manager handover.
Disposable-runner end-to-end handover verification is still pending.

## Bounds and failure semantics

One native connection per window; 16 queued outgoing frames; at most one
unacknowledged incoming Channel delivery. A delivery must be acknowledged within
5 seconds or the whole connection is closed. Hello/connect also have 5-second
deadlines. IPC frames retain the shared 256 KiB limit and frame deadlines.
Native connection IDs fence stale callbacks and closes. Failed/cancelled writes
have an unknown outcome: the proxy never reconnects or replays commands.
Errors use fixed codes, not OS errors containing private paths.

The UI performs `get_status`, `list_sessions` and `subscribe_events` before
claiming to be connected. A resumed subscription keeps the last applied view and
cursor, then applies after+1 through the announced high-water mark before claiming
ready. Snapshot subscriptions use only the snapshot. Session updates preserve
list order. Cursors stay in memory; a gap requires a fresh snapshot, and offline
retained data carries its last-update time.
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

## Opt-in fake Host chat WebView probe

`desktop/scripts/host-probe.mjs` uses the real native Channel, `TauriTransport` and
`DaemonClient` against the owned fake-Pi Host from `fixtures/pi-rpc/desktop-host-v5.json`.
The current `web/src/probe/ui.tsx` peer renders the normal App on the fixture session
route and drives its message box, Send, Stop, Allow once/Deny and two-step Close
controls. App owns the single connection; the driver observes outcomes without
sending the controls directly. This replaces the earlier non-rendering peer only
inside the existing opt-in probe; normal startup is unchanged. It requires an
explicitly enabled debug shell, a fixture-shaped private endpoint held only in Rust,
and a final authoritative stopped transcript before `m1_finish` may report success. Native completion now waits up to five seconds for the exact Channel delivery ACK to be consumed, not merely queued. A receipt survives either waiter/ACK scheduling order but is invalidated when its connection ends or is replaced. Success requires both `M1_HOST_WEBVIEW_OK` and the new `M1_HOST_WEBVIEW_FINAL_ACK_OK`; the runner rejects older generic-only output and reports `finalChannelAck=true`. These new gates still require same-SHA native execution evidence. It checks a completed
normal turn, then holds a second `busy` turn, reads back the same public run ID,
cancels it and requires observed settlement plus authoritative Ready/no active-run
readback. Two further `approval` turns exercise Allow once then Deny, each with
the same public dialog read back before selection, resolved (not expired) state,
settlement and an empty pending-dialog readback. Close/stopped retains the transcript.
The complete native log must match `expectedNativeCommands`: four prompts, one
ordered `clear_queue, abort`, and exactly two `extension_ui_response` writes with
`confirmed=true` then `confirmed=false`. Unknown is a failure, never a retry.

The native workflow prepares the standalone managed test with `cargo test --locked
-p cafe-spaced --test managed --no-run --message-format=json` and saves its UTF-8
stdout as `target/desktop-host-artifacts.json`. After the matching UI/debug shell
artifacts exist, set `CAFE_SPACE_TEST_DESKTOP=1` and run
`node desktop/scripts/host-probe.mjs` (under `xvfb-run --auto-servernum` on headless
Linux). The script itself does not build; do not use artifacts from another revision.
Only the shell child receives `CAFE_SPACE_M1_PROBE=1` and the fixture endpoint.
Data/config and owned processes are temporary; startup is never registered.

The normal-UI peer has local DOM tests for both response/event orders, unknown
outcomes without replay, missing cancel settlement and expiration without dialog
resolution. It has not yet passed a new same-SHA native WebView run; the earlier
programmatic-peer logs do not validate these UI interactions. A successful new
runner reports `normalUi=true` alongside the native command counts. This remains
scripted interaction, not a visual/keyboard/touch review, New session, tool-policy
approval or Resume acceptance. Confirmation is an extension dialog, not a per-tool
permission mechanism. A passing managed/JS test is not a native WebView pass, and
the separately authorized real-Agent gate remains required for acceptance.

## Opt-in active-disconnect fixture check

After preparing the existing managed test artifact, set `CAFE_SPACE_TEST_DESKTOP=1`
and run `node desktop/scripts/host-disconnect.mjs`. It never builds or opens a
WebView. Two disposable fake-Host processes receive one busy or confirmation
prompt each over native IPC. The check reads back Running (plus the pending dialog
for confirmation), disconnects, requires successful fixture exit and exactly
`get_state, prompt` in the native log. The fixture emits its stopped marker only
after the owned fake PID disappears. The native CI workflow runs this after
preparing the managed artifact on each platform.

This checks isolated fixture teardown, not production daemon disconnect policy:
a normal client disconnect must not be inferred to terminate a production session.
It does not exercise an in-flight cancel/selection, native WebView failure, replay,
or final Channel ACK. Windows passed locally using the existing artifact; new
same-SHA cross-platform CI remains required.

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
