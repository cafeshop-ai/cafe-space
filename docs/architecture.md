# Café Space — Architecture (draft v0.2)

> Status: **draft for discussion**, 2026-11-03. The repository contains the M0 workspace, the draft local API v2 contract, a read-only headless daemon with owner-only IPC, SQLite WAL, locking and diagnostic logs. Explicit per-user autostart registration exists with disposable-runner checks; production binary installation/update handover, session execution, adapters, the full UI and the Tauri shell remain unimplemented. See the README for current limits.
> Predecessor: `pi-cafe-space` in `cafeshop-ai/pi-packages` (Pi extension + Go relay + React web), referred to below as **v0**.
> Decisions marked **[open]** need an owner decision; each carries a recommendation. See §14.
> Technical claims are checked in [research/technical-verification.md](research/technical-verification.md): round 1 for the original Go daemon, round 2 for the Rust daemon after D15 (both 2026-11-03).
> **Decided (2026-11-03):**
> - D1: the desktop shell is **Tauri 2**.
> - D9: the daemon is **`cafe-spaced`** and the CLI is **`cafe-space`** (§5.5).
> - D13: the webview reaches the daemon through the **Rust IPC proxy**, confirmed on all three OSes by the M0 spike.
> - D15: everything on the computer (daemon, CLI, desktop shell) is written in **Rust**, and the UI in **React + TypeScript**. The cloud stays the existing **Go** service, unchanged.
> - D12: without a Café login, each computer has one local account, and remote devices must sign in as it. Remote access is for the owner only; the Café-login mode is not defined yet.
> - Product rules P1–P5 (§2.1):
>   - one device room per computer, and any number of collaboration rooms;
>   - remote clients may create sessions; without a Café login, the only remote client is the owner signed in as the computer's single local account (D12);
>   - room Goals may drive autonomous agent work;
>   - no account login in v1;
>   - the cloud address is configurable, default `space.cafecode.work`.
> - Product direction P6–P9 (2026-11-04, §2.1):
>   - personal remote use and collaboration rooms carry equal weight, so the order stays M1 → M2 → M3;
>   - in v1 only the owner can make their agents act;
>   - remote clients are browsers by default, with an optional native app or CLI later;
>   - the cloud is hosted by Café by default and can be self-hosted.

## 1. Why a new version

v0 grew out of a Pi extension. The office-side Go gateway exists to relay Pi extensions, Pi is the only first-class agent, and Codex support had to be bolted on through a side path (ACP managed sessions). Three problems follow:

1. **Agent coupling.** Every new agent needs a different integration route instead of one interface.
2. **No owner of state.** Session lifetime is split between Pi processes, the gateway and the browser; nothing on the machine can run without one of them.
3. **Collaboration cannot reach agents.** People + AI rooms live in the cloud, but nothing on the user's machine can deliver a room message into a running agent session reliably (see the v0 listening-design discussion).

Café Space v1 is a **standalone application**: installed once per computer, it owns the agent sessions it runs, exposes them to the user's own devices, and connects them to people and other agents in rooms. Pi becomes one agent runtime among several.

## 2. Goals and non-goals

### Goals (v1)

- **G1 Local agent host.** Start, open, observe, steer and stop sessions of several agent runtimes (Pi, Codex, Claude Code first) from one place, with honest status.
- **G2 Use your computer from anywhere.** Phone and browser access to those sessions through a private room link + password, without opening an inbound port. Each computer hosts one device room (P1). Your other devices sign in as your local account and can work in existing sessions and create new ones (P2, D12).
- **G3 People + AI rooms.** Discussion, tasks and a shared Goal across machines.
  - A computer can take part in any number of rooms (P1).
  - Local agents can be members, and room messages are delivered into their sessions with truthful receipts.
  - A room Goal can drive bound agents autonomously within local limits (P3).
- **G4 Survive the UI.** Closing, updating or crashing the UI does not stop agents.
- **G5 Keep v0 users whole.** Existing room links, room passwords and collaboration rooms keep working through the transition.

### Non-goals (v1)

- Running models or agents in the cloud. The cloud routes, signals and stores room text; it never executes work.
- Mirroring terminal screens as the primary UI (see §5.3; a terminal mode is a possible later addition).
- Native mobile apps in the first release. Phones use the web UI; an optional native app or CLI may follow (P8).
- Multi-user daemons, or one daemon serving several OS users.
- Sandboxing agents. Agents keep their own native permission models; Space adds approval gates but is not an OS sandbox.
- Account login (Café SSO). It is not available in v1 (P4). Nothing in v1 may depend on an account.

### 2.1 Product rules (decided 2026-11-03)

| ID | Rule | Architectural consequence |
|---|---|---|
| P1 | Each computer hosts **one** device-sharing room. There is **no limit** on collaboration rooms. | One remote gateway per daemon. The collaboration connector manages N room memberships, so per-room work must be cheap; bounded by technical limits, not product ones (§8) |
| P2 | **Remote clients can create sessions.** | Refined by **D12**. Without a Café login, the computer has **one local account**, and a remote device must sign in *as that account*. A remote client is therefore always the owner on another device, never a third party. Owner-only settings stay on the computer (§7, §11) |
| P3 | **Goals can drive autonomous action.** | Bound agents may take turns without a per-step human prompt. This requires local opt-in per binding, budgets and hard stop conditions (§8.1) |
| P4 | **No Café login** in v1. | No identity service dependency. Remote access authenticates against the computer's **local account** (D12). Each signed-in device holds its own device key and appears in the owner's device list. Collaboration members are identified by room membership tokens (§9). The Café-login mode is deliberately left undefined for now |
| P5 | The **cloud address is configurable**; default `https://space.cafecode.work`. | One configured cloud origin per daemon. The desktop UI never calls the cloud directly; the daemon does. Changing the address re-homes the device room (§9) |
| P6 | **Personal remote use and collaboration rooms carry equal weight** in v1 (2026-11-04). | The roadmap order stays M1 → M2 → M3 (§15). Neither milestone is cut to make room for the other |
| P7 | **In v1, only the owner can make their agents act** (2026-11-04). | Confirms D12. Teammates influence an agent only through room messages and Goals. Whether a bound agent acts on a Goal is the owner's local opt-in (§8.1). Letting a teammate start work on my computer ("@ my agent") is a v2 **team mode**: it needs Café login, per-person isolation (a separate OS user, VM or container) and owner approval. See [reference projects](research/reference-projects.md) |
| P8 | **Remote clients are browsers by default, with an optional native app or CLI** (2026-11-04). | The browser path must work without installing anything: WebRTC plus a 443 end-to-end encrypted relay (D3). The channel interface (§7) must also admit a native client using a stronger channel (WireGuard- or Tailscale-style) later, with the same protocol v2, sign-in and device approval |
| P9 | **The cloud is hosted by Café by default and can be self-hosted** (2026-11-04). | Café operates signaling, collaboration rooms, TURN and the 443 relay, and carries their traffic cost, so relays need per-device quotas and rate limits. Self-hosting uses the configurable address (P5) |

## 3. Principles carried over from v0

These were learned the hard way and stay non-negotiable:

- **Structured, not scraped.** Transcripts, tool calls, status and results come from native agent events, never from parsing screen output. Missing evidence is shown as unknown.
- **Unknown outcomes are never retried.** Creates are idempotent by client-chosen ID; a lost response is reconciled, not resent. "Dispatched" never means "done".
- **Independent gates.** Who you are (account and device), whether you may enter (password) and whether you may control (approval) are separate checks; none implies another.
- **No secrets in URLs, argv, logs or room messages.** Tokens live in the OS secret store or owner-only files.
- **Room content is untrusted data.** A message is never a local instruction; only the local user authorizes local effects.
- **Bounded everything.** Frame sizes, queues, history, registries and retries have explicit limits.

## 4. System overview

```text
                         ┌──────────────────────── Café cloud (space.cafecode.work) ───────────────────────┐
                         │  web assets · room registry & signaling · TURN · collaboration rooms · identity*│
                         └───────▲──────────────────────▲─────────────────────────────▲────────────────────┘
                                 │ signaling (HTTPS/WS) │ collab HTTPS JSON           │ OIDC*
   Phone / laptop browser ───────┘                      │                             │
        │  WebRTC (direct or via TURN), end-to-end encrypted                          │
        ▼                                                                             │
 ┌──────────────────────────── User's computer ────────────────────────────────────────┴───────┐
 │                                                                                              │
 │   Tauri 2 shell (tray, windows, notifications, updater) ── hosts ──► web UI (webview)        │
 │   `cafe-space` CLI ────────┐                                         │                       │
 │                            ▼                                         ▼                       │
 │                 ┌─────────────────────── cafe-spaced (daemon) ───────────────────────┐       │
 │                 │ Local API · Session manager · Agent adapters · Remote gateway      │       │
 │                 │ Room connector (collab) · Inbox & delivery · Store (SQLite + logs) │       │
 │                 └──────┬───────────────────┬──────────────────────┬───────────────────┘       │
 │                        │ RPC (stdio/JSON)  │ ACP / app-server     │ v0 host protocol        │
 │                        ▼                   ▼                      ▼                          │
 │                   Pi (RPC mode)      Codex / Claude Code      Existing Pi + v0 extension     │
 └──────────────────────────────────────────────────────────────────────────────────────────────┘
```

\* Identity / OIDC are not used in v1 (P4, no login). The cloud origin shown is the configurable default (P5).

Three tiers:

| Tier | Runs | Owns | Never does |
|---|---|---|---|
| **Agents** | Native CLIs started or attached by the daemon | Model calls, tools, their own conversation storage | Talk to the cloud on Space's behalf |
| **Daemon (`cafe-spaced`)** | One per OS user, starts at login | Session lifecycle, adapter state, remote gateway, room connections, local store, secrets | Render UI; execute room content as commands |
| **Clients** | Desktop shell, local web UI, `cafe-space` CLI, remote browsers | Presentation and user intent | Hold agent credentials or spawn agents directly |

The **cloud** is a separate deployment. v1 uses two of its services: cloud (assets, registry, signaling, collaboration) and TURN. The v0 identity service is not used until login ships (P4). The cloud address is configurable (P5, §9).

**Languages (D15).**
- **Rust** for everything on the computer: `cafe-spaced`, the `cafe-space` CLI and the Tauri shell, in one Cargo workspace (§13).
- **TypeScript + React** for the UI. Its wire types are generated from the Rust types, so the protocol is defined once.
- **Go** for the cloud only. It is v0's service, kept unchanged (§9).
- v0's Go code on the computer side (gateway, protocol, managed sessions, room identity) is **ported, not linked**. Fixtures recorded from the Go code pin its behavior before the port.

## 5. The daemon (`cafe-spaced`)

### 5.1 Process model

- One daemon per OS user and data directory (§5.5). A lock file in the data directory enforces single ownership (as v0's managed registry does).
- Started by the OS at login, or on demand by the shell or CLI ([research S1](research/technical-verification.md#s1-per-user-start-at-login-corrected)):
  - **macOS:** a LaunchAgent.
  - **Linux:** a `systemd --user` unit. M0 emits native user-scoped definitions directly, using the hand-written-file route allowed by [round 2, S1](research/technical-verification.md#s1-rust-per-user-start-at-login-confirmed), with explicit escaping and no extra service-manager dependency. The unit uses `Restart=no`, as Runner's does, so systemd never restarts old daemon bytes during an update handover.
  - **Windows:** a per-user Task Scheduler "at logon" task, with an `HKCU` Run entry as a fallback. A Windows *service* is machine-wide and needs administrator rights, so it is not used.
- Clients connect over **owner-only local IPC**: a Unix socket with 0600 permissions inside a 0700 directory, or a Windows named pipe with an owner-only DACL that rejects remote clients (tokio `ServerOptions`, as in Runner's daemon; [round 2, S2](research/technical-verification.md#s2-rust-owner-only-ipc-confirmed)). The local web UI uses a loopback HTTP/WebSocket listener protected by a per-install token and an Origin check, exactly as v0's local gateway does.
- Agent process trees belong to the daemon. Windows uses Job Objects with `KILL_ON_JOB_CLOSE`. POSIX uses a supervisor with process groups and a liveness pipe. This is v0 `relay/internal/managed`, ported to Rust. Runner's Rust daemon uses the same Job Object model on Windows.
- When the daemon restarts, sessions are recorded as stopped and reopened explicitly; prompts are never auto-replayed (v0 behavior). An attached external session is reconnected, not respawned.

### 5.2 Internal modules

| Module | Responsibility | v0 source to port (behavior pinned by fixtures) |
|---|---|---|
| `api` | Typed request/response + event stream for all clients; one request table shared by UI, CLI and remote clients | `relay/internal/httpserver`, `hub` |
| `session` | Session registry, lifecycle state machine, idempotent create/open/close, fencing (stream / session / cwd) | `relay/internal/managed`, `hub` |
| `adapters/*` | One adapter per agent runtime behind a common interface (§5.3) | `managed/acp.go`, `scripts/acp/*`, Pi RPC launcher |
| `protocol` | Canonical session snapshot / event / command types and codecs (Rust, serde), with shared fixtures; the TS types are generated from them | `relay/internal/protocol`, `src/protocol` |
| `remote` | Room identity, password verification, approvals, WebRTC peer, TURN config | `relay/internal/remote`, `auth` |
| `collab` | Membership in collaboration rooms, durable inbox, delivery to bound sessions (§8) | `scripts/space-agent.mjs` semantics |
| `store` | SQLite (config, registry, bindings) + append-only per-room inbox logs + secret store access | `managed` registry, `space-agent` inbox |
| `files` | Project-scoped file reads/references with sensitive-path rules | v0 file commands |

**Protocol v2 is the daemon's API.** It is one request table and one event stream for every client: the desktop UI (through the Tauri IPC proxy), the CLI (local socket or pipe) and remote browsers (the WebRTC channel). Every transport exchanges the same bounded UTF-8 JSON frame; only the transport's message-boundary delimiter differs.
- **Carried over from v0 protocol v1:**
  - creates are idempotent by client-chosen ID;
  - every operation ends applied, rejected, dispatched or unknown (§12);
  - control requests are strict JSON that rejects unknown fields;
  - frames are bounded;
  - commands are fenced by stream, session and cwd;
  - browsers see opaque target keys, never native IDs (§5.4);
  - capabilities are presentation, never authorization.
- **Events** are sequenced. A client resumes from its last sequence number, or receives a fresh snapshot when the gap is too large.
- **Version handshake.** Each protocol-v2 connection negotiates the highest common protocol version before requests. During migration, the remote UI preserves the legacy host-protocol-v1 path for v0 gateways and never sends them a v2 hello; `version_mismatch` applies only once both peers implement this handshake (§9).
- **Names.** "v0" is the old product and "protocol v1" is its wire format. The `fixtures/v0-go/` baseline starts with v0's shared Go/TypeScript protocol fixtures and specs, copied with source commit, paths and SHA-256 hashes. Only what those fixtures do not cover is recorded from Go, including room identity and signatures (research R5). New daemon protocol-v2 examples live separately in `fixtures/v2/`.

### 5.3 Agent adapters

Every runtime-specific rule lives in one adapter. Shared code asks the adapter; it never switches on the runtime name. This mirrors Runner's `RuntimeAdapter` refactor and replaces v0's hard-coded `pi` / `codex-acp` branches.

```rust
#[async_trait]
pub trait Adapter: Send + Sync {
    fn identity(&self) -> RuntimeInfo;                                  // id, display name, version probe
    async fn detect(&self) -> Result<Installation>;                     // find binary, version, auth state (no prompts sent)
    fn capabilities(&self, inst: &Installation) -> Capabilities;        // new / resume / attach, model control, delivery modes, approvals, files
    async fn launch(&self, spec: LaunchSpec) -> Result<Box<dyn Conn>>;  // new or resume; returns a structured connection
    async fn attach(&self, target: Target) -> Result<Box<dyn Conn>>;    // borrow an existing native session (optional)
}

#[async_trait]
pub trait Conn: Send {
    fn events(&mut self) -> &mut EventStream;                           // normalized transcript / tool / status / approval events
    async fn prompt(&mut self, prompt: Prompt) -> Result<Outcome>;      // Outcome includes Unknown
    async fn cancel(&mut self) -> Result<()>;                           // only for turns this connection owns
    async fn respond(&mut self, id: ApprovalId, decision: Decision) -> Result<()>;
    async fn close(self: Box<Self>) -> Result<()>;                      // detach-only for borrowed sessions
}
```

**Structured-first.** Each adapter speaks the agent's structured interface. These are candidates, to be verified per version before an adapter ships:

| Runtime | Interface | Notes |
|---|---|---|
| Pi (`@earendil-works/pi-coding-agent` 1.5) | RPC mode (`pi --mode rpc`, strict JSONL), plus an optional v0 extension for manual sessions | Confirmed. Wait for `agent_settled`, not `agent_end`. Approvals arrive as `extension_ui_request` dialogs. Split records only on LF ([research A1](research/technical-verification.md#a1-pi-confirmed)) |
| Codex | **Owned sessions:** `@agentclientprotocol/codex-acp` 2.5 (bundles Codex). **Attach only:** the native app-server (JSON-RPC, `experimentalApi`, socket proxy) | Confirmed. The app-server surface moves fast, so pin versions and capability-probe ([A2](research/technical-verification.md#a2-codex-confirmed-two-routes)) |
| Claude Code | `@agentclientprotocol/claude-agent-acp` (ACP over the Claude Agent SDK). The `@zed-industries/*` packages are deprecated | Package confirmed. Auth and resume behavior need a probe before M2 ([A3](research/technical-verification.md#a3-claude-code-corrected)) |
| Generic ACP agent | ACP **wire v1** over stdio. v2 is still alpha | v1 stable covers new, load, resume, list, close, prompt, cancel, mode and config options, plus permission requests ([A4](research/technical-verification.md#a4-acp-versions-confirmed)) |

**One ACP client for three adapters.** Codex (codex-acp), Claude Code (claude-agent-acp) and generic agents all speak ACP.
- The daemon uses the official Rust SDK, `agent-client-protocol`, in the client role on stable v1. v2 stays behind the SDK's `unstable_protocol_v2` feature until it is stable.
- The SDK's major version moves fast (2.2 in September 2026, 5.0.0 on 2026-11-02). It is pinned exactly and upgraded deliberately, with the adapter fixtures as the gate ([round 2, A5](research/technical-verification.md#a5-acp-rust-sdk-confirmed)).

**Capabilities are presentation, never authorization** (v0 `snapshot.agent`). The daemon enforces what an adapter cannot do, regardless of what a client sends.

**Terminal mode [open, D2].** Runner shows that a PTY-first design (each agent's own TUI in a real terminal) supports more agents with less integration work. It is poor on phones, though, and it gives no structured status. Recommendation: v1 ships structured-only; a read-only terminal view is evaluated for agents with no structured interface.

**Agent integration checklist.** Before an adapter ships, it must pass a written checklist modelled on Runner's `runtime-integration.md`:
- identity and detection;
- lifecycle and process ownership;
- first turn and readiness;
- room delivery;
- permissions and trust;
- model and effort control;
- conversation identity and resume;
- honest status and capability boundaries;
- per-platform evidence.

### 5.4 Session model

```text
Project (approved cwd) ─┬─ Session (runtime, mode new|resume|attach, native id kept private)
                        │     └─ state: starting → ready ⇄ running → stopping → stopped | failed(code)
                        └─ Binding (session ↔ collaboration room membership, optional)
```

- **Projects are the allowlist.** Clients choose a project ID; they never send paths, commands or environment variables (v0 rule).
- **Remote devices can create sessions (P2, D12).** A signed-in remote device is the owner, so it can create sessions in any approved project with any ready runtime profile, exactly as at the computer.
  - Every session records which device started it ("this computer", "iPhone").
  - Revoking a device leaves its sessions running but ends its access.
- **Native session IDs stay local.** Browsers get opaque keys (v0 `targetKey`).
- **Borrowed sessions.** A borrowed session (attach) never changes the native model, permissions or history, and closing it only detaches.
- **Every failure code is user-visible** and explained; nothing is retried automatically.

### 5.5 Names and paths (D9)

The daemon is `cafe-spaced` and the CLI is `cafe-space` (decided 2026-11-03). Every other name follows from them.

| Item | Name or path |
|---|---|
| Daemon | `cafe-spaced` (`cafe-spaced.exe` on Windows) |
| CLI | `cafe-space`, on PATH for the user and for every agent the daemon starts. It may be the same executable as the daemon, dispatched by name; that is an M0 build detail |
| Per-user data dir | macOS `~/Library/Application Support/cafe-space/` · Windows `%LOCALAPPDATA%\cafe-space\` · Linux `${XDG_DATA_HOME:-~/.local/share}/cafe-space/` |
| Inside the data dir | `bin/cafe-spaced` (the copy the OS starts, T1) · `cafe-spaced.lock` · the SQLite database · `rooms/` (inbox logs) · `logs/` |
| Local IPC | Linux `$XDG_RUNTIME_DIR/cafe-space/cafe-spaced.sock` · macOS `<data dir>/run/cafe-spaced.sock` · Windows `\\.\pipe\cafe-spaced-<user SID>` |
| Autostart | LaunchAgent label `ai.cafeshop.cafe-spaced` · systemd user unit `cafe-spaced.service` · logon task `cafe-spaced-<user SID>` |
| Desktop app identifier | `ai.cafeshop.cafe-space` |
| Deep links | `cafe-space://` |
| Environment for agents | `CAFE_SPACE_*`, for example the session ID and room binding |
| Development builds | A `-dev` suffix on every name above (`cafe-spaced-dev`, data dir `cafe-space-dev`, and so on), so a development daemon never touches release data |

- **One root for everyone.** The shell, the CLI and the OS service all use the data dir above. Tauri's own app data dir is named after the bundle identifier, so the shell does not keep daemon state there.
- **Pipe names include the user.** Windows named pipes share one machine-wide namespace. With one daemon per OS user, the name carries the user's SID; the owner-only DACL still enforces access.
- **Socket path length.** macOS limits a Unix socket path to 104 bytes. The daemon checks the length and, if it is too long, uses a 0700 directory under the per-user `$TMPDIR` instead.

## 6. Clients

- **Local web UI.** The v0 React app (`web/`) is evolved, not rewritten. Its two entry points (Remote device / Collaboration, from `feat/space-entry-points`) become the app's top-level navigation, plus a new **Agents** area for local sessions. The same build serves the desktop shell, the loopback browser and remote visitors; the capabilities exposed differ per connection role. Its wire types are generated from the daemon's Rust types (for example with `ts-rs`), never written by hand.
- **Desktop shell: Tauri 2 (decided, D1).** A thin native wrapper. It holds no state and owns no agent; if it crashes or updates, agents keep running.
  - **Daemon binary, not a plugin-shell sidecar.** `cafe-spaced` ships inside the bundle. On first run and after each update, the shell copies it to `<data dir>/bin/` (§5.5) and checks its hash. It is then started by the OS service manager (§5.1), or by a detached spawn as a fallback.
    - It is never started through `tauri-plugin-shell`, which kills all its children when the app exits ([research T1](research/technical-verification.md#t1-sidecar-lifetime-corrected)).
    - Running from the data dir also avoids Windows file locks during updates. This is Runner's pattern.
  - **Updates** ([research T2](research/technical-verification.md#t2-updates-and-running-agents-corrected)).
    - A shell-only update never touches the daemon or agents.
    - When the bundled daemon's hash changes, the daemon must restart, which **stops sessions**: agents belong to it. They reopen and resume explicitly, with no prompt replay.
    - The shell offers "update now" or "update when idle".
    - **The daemon owns the update gate.** Only it knows whether work is running, so the shell never polls "idle" and then stops the daemon, which would race with new work. Protocol v2 gets `prepare_update` (`now` or `when_idle`, with the target binary hash) and `cancel_update`. Both are local-only: a remote device can never start an update.
    - **Pending.** With `when_idle`, work continues normally and the UI says the update waits for idle. The pending update is a lease on the connection that asked for it. If that connection closes before draining begins (for example because the user quits the shell), the daemon cancels it, and the old daemon and its agents keep running. There is no resident updater; the next shell launch offers the update again.
    - **Draining.** At the first idle moment, or at once with `now`, the daemon checks and freezes admission in one step. New work is rejected with a stable `UPDATE_IN_PROGRESS` code. Sessions are recorded as stopped, state is flushed, and the daemon writes a handover marker (old hash, target hash, phase), reports that it is ready for handover, and exits with a distinct code. `now` stops running sessions, so the UI lists them and asks first (UX §5.9). Draining cannot be cancelled.
    - **Handover.** Before calling `prepare_update`, the shell stages the new binary as `bin/cafe-spaced.next` and verifies its hash. After the daemon exits, the shell keeps the old binary as `.prev`, swaps in the new one, starts it through the OS service manager and waits for a hello with the new version. If that fails within the deadline, it restores `.prev`, starts it and records a rollback.
    - **Never half an update.** If the shell disappears after draining began, the next start completes or rolls back from the marker: a shell launch finishes the swap or restores `.prev`, and an old daemon started by the OS at login clears the marker as "not applied". `get_status` and an event report `update: none | pending | draining`, so every client shows the same state.
    - **Rollback restores binaries, never data.** Restoring `.prev` does not undo anything the new daemon wrote to SQLite (point raised by the server agent).
      - A new daemon changes no on-disk schema until its handover is committed: it has answered the hello, and the marker says `committed`.
      - A migration first takes a consistent backup (SQLite backup API), then runs in one transaction. The marker records both.
      - Before any rollback, the shell checks that the old binary can read the state on disk. If it cannot, the shell stops and reports. It never downgrades automatically, migrates backwards or overwrites the database; restoring the pre-migration backup is an explicit owner action.
      - An old daemon refuses a newer schema and fails closed. M0 has no migrations and already refuses unknown databases. Its tests cover "refuse rollback on incompatible state" with isolated state files.
    - **Testing.** M0 has no agents, so a busy session is simulated through a test-only hook that is never compiled into release builds. Real busy-session handover is verified again in M1.
  - **UI.** The webview loads the React build bundled in the app, so the signed app, its UI and the daemon version move together.
  - **Transport to the daemon** ([research T3](research/technical-verification.md#t3-webview-to-daemon-transport-confirmed-m0-spike)). Confirmed by the M0 spike on all three OSes (D13).
    - Preferred: a Rust IPC proxy. Tauri commands forward to the owner-only daemon socket, and events stream back through a Tauri `Channel`. The local token never reaches JavaScript, and no CSP or mixed-content question arises.
    - Fallback: direct loopback HTTP/WebSocket, with the token passed through IPC.
    - Never pass the token in a URL.
  - **Remote browsers** load the same React build from the cloud origin they opened (§9), not from the daemon.
  - **Plugins.** Tray, notifications, single-instance, deep links (`cafe-space://`), autostart (for the app; the daemon has its own, §5.1) and the updater use official Tauri plugins, with signed update manifests.
  - **Minimum Tauri 2.10.3.** This is the fix for CVE-2026-42184, which misclassified remote URLs as local on Windows and Android. Current is 2.12.x.
  - **WebView2 on Windows.** Use `embedBootstrapper` for the website download (+1.8 MB, needs internet at install). Use `offlineInstaller` (+127 MB) for offline or Store builds.
  - **Keep the shell thin.** The shell and the daemon share one Cargo workspace (D15), but the shell links only the `protocol` and `ipc` crates. It handles windows and OS integration; all product logic stays in the daemon, so the CLI and remote clients behave identically.
  - **Webview differences.** WKWebView on macOS, WebView2 on Windows (bootstrap the runtime if missing) and WebKitGTK on Linux. These are covered by the UI test matrix, since behavior differs, for example IME input, clipboard and WebRTC in the local window.
- **`cafe-space` CLI.** It ships with the daemon (§5.5). Users and agents run `cafe-space room post`, `cafe-space room read`, `cafe-space session list` and `cafe-space status`. When the daemon starts an agent, it puts `cafe-space` on that agent's PATH, sets scoped `CAFE_SPACE_*` environment variables (session ID, room binding) and installs a short skill telling the agent how to use it. This is Runner's pattern, and it replaces pasting long invitation prompts.
- **Text encoding (all clients).** Room messages and transcripts are often Chinese, so every client keeps text exact. In the v0 development room, a message posted from Windows arrived with each Chinese character replaced by `?`.
  - **Wire.** Protocol v2 and every HTTP body are UTF-8 JSON (`Content-Type: application/json; charset=utf-8`). Decoders reject invalid UTF-8 rather than substituting U+FFFD, and nothing is ever replaced silently.
  - **Where text gets lost on Windows.** Agents call `cafe-space` from shells whose defaults are lossy, and the damage happens before the CLI sees the text:
    - Windows PowerShell 5.1 pipes strings to native programs using `$OutputEncoding`, which defaults to US-ASCII, so each non-ASCII character becomes `?`;
    - its `Invoke-RestMethod` sends a string body as ISO-8859-1 unless the content type names a charset;
    - its `>` and `Out-File` write UTF-16 LE, and `Set-Content` writes the ANSI code page.
  - **CLI input rules.**
    - Message text comes from an argument (`--text`) or a file (`--file`). Rust reads Windows arguments as UTF-16, so arguments are lossless, and agents write files with their own file tool.
    - Files and piped stdin are decoded by their BOM (UTF-8, UTF-16 LE or BE). Without a BOM they must be valid UTF-8. Anything else fails with an error that names the encoding problem, never with a lossy guess.
    - The skill installed for agents says to use `--text` or `--file`, and never to pipe text through Windows PowerShell 5.1.
  - **CLI output.** On a console, Rust writes through the wide-character console API, so any code page displays correctly. Redirected output is UTF-8. `--json` escapes every non-ASCII character as `\uXXXX`, so an agent reading through a shell with a legacy code page (for example 936) still gets the exact text.
  - **Echo check.** After `cafe-space room post`, the CLI compares the text the cloud stored with the text it sent, and reports any difference.
  - **Tests.** Fixtures with Chinese, emoji outside the BMP, combining marks and right-to-left text round-trip through `--text`, `--file` (UTF-8, UTF-8 with BOM, UTF-16 LE) and stdin. The Windows CI job runs them from `cmd` under code page 936, from Windows PowerShell 5.1 and from PowerShell 7.
- **Remote browsers.** These are phones and laptops reaching the daemon through the remote gateway (§7). They are identical to the local UI apart from role-based limits.

## 7. Remote access

v1 keeps the v0 model because it works in production and preserves existing links:

- **One device room per computer (P1).** A room is identified by a public key; the link `<cloud origin>/#/room/<roomKey>` carries no secret. The private key stays on the computer.
- Each browser holds a non-exportable signing key per room. In v1 that key identifies a **device** of the owner.
- Transport is WebRTC: direct when possible, otherwise via TURN. Signaling goes through the cloud, which cannot read the channel.
- **Daemon WebRTC stack (D15).** `webrtc` (webrtc-rs) 0.21.x, built on its sans-I/O `rtc` core. It is pre-1.0, so it is pinned exactly ([round 2, R4](research/technical-verification.md#r4-rust-webrtc-spike)).
  - **Known gap.** Its TURN client allocates over UDP only and skips `turns:` and TCP TURN URLs. A **computer** whose network blocks outbound UDP gets no relay candidate.
  - **Browsers have a narrower fallback than v0's docs suggest.** v0 documents TURN over TLS for restricted networks, but the deployed TURN service has no TLS listener (server agent's read-only check, 2026-11-03). Browsers can try TURN over TCP on the standard TURN port. Whether that connects on a UDP-blocking network is unproven until #8 measures it: it needs that TCP port open, valid credentials and relay ports, and a reachable ICE path from the other side. It cannot cover a computer whose own UDP is blocked. A network that allows nothing but 443 has no route on either side today. These are configuration and listener facts, not connection results.
  - **Mitigation.** The M0 WebRTC spike measures this. If it matters in practice, the fallback is the D3 relay route (an end-to-end encrypted WebSocket over 443), not a change of WebRTC stack.
  - **Carried over from v0.** The tuned settings (SCTP message size, receive buffer, ICE timeouts, loopback candidates) and the chunked DataChannel framing are ported, each with an equivalent confirmed in the spike.

**Who may connect (D12, no Café login).**
- **One local account.** The computer has exactly one local account: a display name and a password, created in the desktop app.
- **Sign-in.** A remote device opening the room link signs in *as that account*. The password is verified on the computer with the v0 scheme (PBKDF2-SHA256, 600,000 iterations), never by the cloud. There is no "guest" role and no nickname-based visitor.
- **Devices.** Each successful sign-in registers the device key in the owner's **device list**: name, browser, first and last seen. Any device can be revoked from the computer.
- **New-device approval [open, D14].** It is recommended on: a device's first sign-in waits for a one-time approval on the computer, so a leaked password alone is not enough.
- **Collaborators** reach the owner's agents only through collaboration rooms (§8), never through the device room.
- **Café-login mode is not defined yet.** When it is, it may add identities beyond the local account, but it must not weaken this one.

**Changes in v1:**

- The remote gateway is a module inside the daemon, not a separate gateway in front of Pi extensions.
- **Permissions** (P2, D12). Both columns are the owner; the difference protects against a stolen device or session.

  | Action | At the computer (desktop app) | Signed-in remote device |
  |---|---|---|
  | View sessions, transcripts, project files | Yes | Yes |
  | Prompt, steer, cancel own turns, answer agent approvals | Yes | Yes |
  | Create, open, close sessions | Yes | Yes |
  | Attach to borrowed native sessions, list native threads | Yes | Yes (D11 resolved by D12) |
  | Bind sessions to rooms, start or stop autonomy | Yes | Yes, within the budgets set on the computer |
  | Change password, reset link, change cloud address, approve or revoke devices, edit projects and autonomy budgets | Yes | **Never**; computer only |
- **Room identity import.** On first run, the daemon imports an existing v0 room identity, with confirmation, so links keep working (G5). The import covers the private key, the password verifier and the control settings. **The v0 room password becomes the local account password.** The account's display name is asked for once. In Rust, `p256` reads v0's SEC1 DER private key, 65-byte public points and 64-byte r‖s signatures, and `pbkdf2` checks the verifier. Import tests run on identity files generated by the v0 Go code ([round 2, R5](research/technical-verification.md#r5-v0-identity-import-in-rust-confirmed)).
- **Transport behind an interface [open, D3].**
  - A second route can be added later without touching the layers above: an end-to-end encrypted WebSocket relay, as in Runner's Noise + PIN pairing and cospace's "dumb encrypted relay".
  - **Why it is needed.**
    - **Measured (R4 case C):** with UDP blocked in both directions on the computer side, and TCP still reachable, the current UDP-only WebRTC path never opened.
    - **Inferred, not measured:** a network that allows only 443 would have no route on either side with the current implementation.
    - **Not tested:** TURN over TCP or TLS, and a 443 relay.
  - **The interface must also admit a native client** (P8) with its own stronger channel. Every channel carries the same protocol v2 frames, sign-in and device approval.
  - **Recommendation:** WebRTC as the primary browser channel, plus the 443 end-to-end encrypted relay built in M2, with the channel interface defined now. Final once R4 is written up for the owner.

## 8. Collaboration rooms

The cloud collaboration service and its HTTPS JSON protocol are unchanged and stay runtime-agnostic. Anyone can still join from a browser, and any agent can join with plain HTTP. What v1 adds is that **the daemon is a first-class room member connector**.

- **Joining.** The user joins a room from the app; the membership token goes to the secret store. A local session can be **bound** to that membership, so the agent speaks in the room as itself. Binding is an explicit local action, per room and per session.
- **Many rooms (P1).** The daemon may hold any number of room memberships, but v0's cloud cannot serve one long-poll per room at scale. It allows 2 waiting reads per member and **256 across the whole server**, plus 1,024 rooms per server and 240 requests per member per minute ([research C1](research/technical-verification.md#c1-many-rooms-per-computer-p1-corrected)). So P1 requires a cloud change in M3, made in the Go cloud (§9). The server agent's read-only investigation (2026-11-03, task #9 in the development room) set its constraints; the limits and the single-process note were re-checked in pi-packages `6723738`, `relay/internal/collaboration/rooms.go:161–163` and `:271–273`.
  - **Multi-room watch.** One waiting read per client wakes on any of its memberships and returns only which rooms changed. Details still come from the existing paged `read`, and the single-room API stays.
  - **Tokens per room.** Each membership token is checked separately and travels only in the request body, never in a URL or log. One room's token can never subscribe another room.
  - **Every change wakes it.** Messages, tasks, Goal revisions, AI pause, membership revocation and room closure all wake the watch, not only the message cursor. As today, a woken read re-runs authorization, so a revoked member or closed room never rides on an earlier check (`rooms.go:448–452`).
  - **Bounded.** Subscriptions per request, request size and waiting connections have limits, and a disconnect or cancel releases its capacity.
  - **Limits change only after measurement.** The store is one process owning a private directory, so it cannot simply be sharded. Several replicas would need a transactional store and cross-instance notification first, which is outside v1 (§9).
  Until then, the daemon long-polls only rooms with a live binding or an open view, and refreshes the others on demand.
- **Delivery.** These are the semantics agreed in the v0 discussion (#24/#25 there), now implementable because the daemon owns the sessions:
  - **Durable inbox.** Long-poll the room into an append-only local inbox, keyed `<room>:m:<seq>`.
  - **Control events.** Goal revision and AI-pause changes become separate control events (`<room>:c:<goalRevision>:<paused>`) and are never merged with chat.
  - **Truthful states.** Each event moves through **fetched → queued → delivered → handled**, and no two states are merged. `delivered` means the adapter accepted the event into the bound session; `handled` is an explicit acknowledgement from the session (`cafe-space room done <seq>`).
  - **Busy sessions.** At most one outstanding chat wake per binding, with a pending count. Control events always go first. A **readiness gate** (starting, needs-human, running, idle) decides when a wake is attempted, following Runner's delivery gate but driven by adapter status events rather than terminal heuristics.
  - **No retries on unknown outcomes.** A delivery whose outcome is unknown stops and is reconciled. A refusal backs off with a bounded delay.
  - **Baseline at bind.** History from before the binding never wakes the agent.
- **Heartbeats.** The daemon sends acks with no model calls. Presence can report the mode truthfully: manual, listener, or host-wake confirmed.
- **Safety.** Room text reaches the agent as untrusted input. Authority to act comes only from the local binding policy (§8.1), never from a message. AI pause makes the binding read-only. Revocation or room closure stops it.

### 8.1 Goal-driven autonomy (P3)

A room Goal may drive bound agents without a human prompting each step. The decision to allow this is local; the Goal only supplies direction.

- **Opt-in per binding.** The owner sets a binding to `autonomous`. The default is `respond`, where turns happen only for delivered messages. Room content, a Goal or another room member can never switch a binding to autonomous.
- **Turn scheduler.** While the latest Goal is `active`, its revision matches the one the binding last acknowledged, and budget remains, the daemon starts work turns for the bound session in a "work → verify → next step" loop. Turns start when:
  - a Goal is created, resumed or revised;
  - a task is assigned to this member;
  - the previous turn ends with a stated next step.
  Only one autonomous turn runs per session at a time, and it never interrupts a human-started turn.
- **Hard stops.** Any of these ends autonomous work and posts one blocker or status message to the room:
  - the Goal is paused, in review, done or deleted;
  - the room's AI pause is on;
  - membership is revoked or the room is closed;
  - the owner presses stop;
  - the budget is exhausted;
  - N consecutive turns pass with no room-visible progress, or the adapter returns an error or unknown outcome.
  Resume requires a Goal resume or an owner action. There is no automatic restart.
- **Budgets** (owner-set, per binding, with daemon-wide ceilings) **[open, D10]**: maximum turns, maximum wall-clock time, maximum concurrent autonomous sessions per computer, and token or cost limits where the adapter reports usage. The UI shows spend against budget live.
- **Scope never grows.** An autonomous turn has exactly the bound session's project, runtime profile and native permissions. It cannot create sessions, attach to borrowed sessions or change bindings, even though the owner's devices can do all three (P2).
- **Evidence.** Every autonomous turn is logged with `goalId`, revision, trigger and outcome. Results are posted to the room and submitted with the Goal's `submit`; only the human reviewer accepts.

## 9. Cloud

The cloud keeps v0's deployment shape: cloud (Go: web assets, room registry, signaling, collaboration store) and TURN. The identity service (OIDC and Café SSO) stays deployed for v0 users, but v1 does not use it until login ships (P4).

| Concern | v1 decision |
|---|---|
| Language | **Go, unchanged (D15).** The cloud is not rewritten. v1's cloud changes (the multi-room watch, §8) are made in the Go code. The Rust daemon and the Go cloud agree through the shared fixtures in `fixtures/`, which both test suites run |
| Collaboration API | Unchanged v1 HTTP contract; additive fields only |
| Device-room signaling | Cloud signaling stays unchanged. The remote compatibility path selects legacy host protocol v1 for v0 gateways; a v0 gateway never receives a v2 hello |
| Web assets | The same React build, embedded in the Go binary. The v0 build chain carries over: the web build is staged into the embedded assets (`relay/internal/webui/assets`), the module is tested and built with the `webembed` tag, and the binary's version and `webDigest` must match the web build (v0 `scripts/refactor/build.mjs:98–107`). After D5, cafe-space's `web/` feeds this step |
| Identity | **Not used in v1 (P4).** Remote access uses the computer's local account and registered device keys (D12), checked on the computer. Collaboration members use membership tokens. The Café-login mode is not defined yet |
| Cloud address (P5) | Configured per daemon, default `https://space.cafecode.work`. HTTPS is required (HTTP only on loopback, for development). The address is shown in settings and embedded in room links |
| Changing the address | Re-registers the device room with the new cloud. Old links stop working, which the UI states before confirming. The room key and password are kept. Collaboration memberships stay tied to the cloud that issued them; each membership stores its own origin |
| Who talks to the cloud | The **daemon only**. The Tauri webview talks to the local daemon, so its content security policy stays fixed whatever the configured address. Remote browsers load the UI from the cloud origin they opened |
| Self-hosting | Any compatible cloud deployment works; the deployment guide moves here with the cloud code (D5) |
| Storage | Collaboration rooms stay plaintext with a 7-day expiry and clear disclosure; no device traffic is stored |
| Repository | **[open, D5]** Cloud code moves into this repo or stays in `pi-packages`. Recommendation: move the **whole** v0 relay module into `cloud/` as it is: entry point, configuration, signaling and registry, collaboration store, web asset embedding, build and tests. Moving only `collaboration/` would not move the cloud. The move changes nothing else: no service split, no storage change, no production change. Protocol and fixtures then live in one place |
| Scaling | One instance. The store is a private directory owned by one process (`rooms.go:271–273`: use a transactional database before multi-replica deployment). Multi-replica deployment is outside v1 |

## 10. Data and storage

| Data | Where | Notes |
|---|---|---|
| Config, projects, session registry, bindings | SQLite (WAL; `rusqlite` with SQLite bundled) in the per-user data dir (§5.5) | Migrations versioned; a crash-safe write precedes every external side effect |
| Room inboxes, delivery records | Append-only JSONL per room | Survives restarts; rebuilds projections; bounded by the room cap |
| Secrets (room private keys, membership tokens, local API token) | OS secret store (Keychain / Credential Manager / Secret Service), owner-only file fallback | Never in logs, argv or URLs. **This protects at rest and from other OS users, not from same-user agents** ([research S4](research/technical-verification.md#s4-secret-store-decision-see-sec1), §11) |
| Agent conversations | The agent's own storage | Space never rewrites native histories |
| Logs | Rotating files | No transcripts, tokens or room text by default |

## 11. Security model

**Trust boundaries:**
1. The local OS user, who is trusted.
2. Local clients, which use owner-only IPC or a token-protected loopback listener.
3. The owner's remote devices, which pass the local account password, then a registered device key, then one-time device approval if D14 is on.
4. The cloud, which routes and stores collaboration text but cannot read device traffic.
5. Room members and agents, whose content is always untrusted.
6. Agents themselves, which act with their native permissions.

**Rules:**
- Projects are an allowlist.
- No client-supplied paths or commands.
- Strict JSON with unknown-field rejection on control APIs.
- Bounded frames.
- Fencing on every command.
- Native IDs stay private.
- Approval requests from agents are routed to a human, or denied by policy; never silently approved.
- **The local account password controls this computer's agents** (P2, D12). Therefore:
  - remote sign-in is rate-limited per device and per room;
  - D14 (new-device approval) is recommended on;
  - the device list is always visible on the computer;
  - changing the password or resetting the link signs out every remote device;
  - owner-only settings cannot be changed remotely.
- Remote session creation and attach are owner actions under D12, which resolves v0 review finding F5: no third party can reach them.
- Autonomous Goal work (P3) needs local opt-in per binding. It stops on any Goal or room control change and never widens a session's scope.
- **Agents can read what the daemon keeps** ([research SEC1](research/technical-verification.md#sec1-guests-agents-and-owner-secrets-decision)). Agents run as the owner's OS user and can execute commands. OS secret stores do not hide secrets from same-user processes, and Windows allows same-user memory reads. Anyone who can start a session can therefore reach the local API token, the room key and membership tokens through the agent.
  - **D12 resolves this for v1.** Without a Café login, only the owner can start sessions, from the computer or as the local account on another device. So this exposure stays within one person's trust.
  - It remains the constraint for the future Café-login mode: any design that lets a *different* person start sessions must run them under a separate OS user or an equivalent sandbox.
- **Loopback endpoints check the Host header as well as the token** ([research SEC2](research/technical-verification.md#sec2-loopback-api-and-dns-rebinding-corrected)). Every loopback endpoint requires Host ∈ {localhost, 127.0.0.1, ::1}, an exact Origin, and the token. An Origin that merely equals the request's own Host is not trusted, because that is what DNS rebinding produces.

## 12. Reliability and observability

- **Health.** The daemon exposes a local health endpoint and a diagnostic report with redacted fields, as in v0 `roomDiagnosticReport`.
- **Outcome states.** Every remote operation has an explicit outcome: applied, rejected, dispatched or unknown. The UI distinguishes them.
- **Restarts.** Sessions are stopped and reopened explicitly. Room connectors resume from their persisted cursors, never from history 0.
- **Metrics.** Local only: delivery latencies, queue depths and adapter errors. There is no telemetry without opt-in.

## 13. Repository layout (proposed)

```text
cafe-space/
  Cargo.toml          Rust workspace: everything that runs on the computer (D15)
  crates/
    cafe-spaced/      daemon: api, session, remote, collab, store, files
    cafe-space-cli/   `cafe-space` CLI, a thin client of the local API (§5.5)
    adapters/         pi, codex, claude, acp (the ACP ones on the official ACP Rust SDK)
    protocol/         wire types and codecs; the TS types are generated from them
    ipc/              owner-only socket and named-pipe client, shared by the CLI and the shell
  desktop/            Tauri 2 app; src-tauri links only `protocol` and `ipc`, bundles web/ and cafe-spaced
  web/                React UI (from v0 web/)
  cloud/              Go module: v0's cloud service, unchanged (only if D5 moves it here)
  fixtures/           versioned protocol fixtures, run by the Rust, TS and Go test suites
  test/               synthetic agents (fake Pi RPC, fake ACP, fake app-server), e2e
  docs/               architecture, decisions (ADRs), runtime checklists, deployment
```

- **Two toolchains for the app**, Cargo and npm. Go is needed only for the cloud.
- **The desktop shell is its own Cargo workspace** (`desktop/src-tauri`, with its own lockfile). It depends on the `protocol` and `ipc` crates by path, so daemon CI does not need WebView system packages. The shared crates must build under both lockfiles, and the desktop workflow checks that.
- **Fixtures before code.** Every new adapter or protocol version comes with fixtures first. Code ported from v0's Go is pinned by fixtures recorded from the Go implementation before the port.
- **Toolchain: Rust 1.95.0** (since `985eaa4`). `rust-toolchain.toml`, the workspace and desktop `rust-version`, and every CI job name the same version. It must be at least the highest compiler requirement of any dependency:
  - **Declared MSRVs:** 1.89 for `rusqlite` 0.41; 1.88 for the ACP SDK 5.0 and `keyring` 4.2. A lower `rust-version` makes Cargo's MSRV-aware resolver (resolver 3) quietly pick older crates than research round 2 verified.
  - **Undeclared requirements:** `webrtc` 0.21.1 needs 1.91 or later through `rtc-mdns`, which declares no `rust-version` (research R4). Resolution cannot catch these, so a dependency upgrade lands only after CI compiles it with the pinned toolchain.
  - 1.95.0 is the version CI verified. Raising it is one commit that changes all four places together.
- **Unsafe code.** Denied workspace-wide with `deny`, not `forbid`, because some OS APIs need it: the Windows pipe security descriptor (S2) and Job Objects (§5.1). It is allowed only in small platform modules, each block with a `SAFETY:` comment.
- **Build flavor.** Release and development names (§5.5) are chosen by each binary's entry point, not by Cargo features, so feature unification (for example `--all-features`) can never give a release-named binary development paths. All names and paths live in one shared crate, which the daemon, the CLI and the shell use.

## 14. Open decisions

| ID | Decision | Options | Recommendation |
|---|---|---|---|
| D1 | Desktop shell | Tauri 2 · Wails (Go) · Electron · none (browser + tray) | **Decided: Tauri 2** (2026-11-03). Chosen for its updater, tray, deep links and signing. The shell stays thin; the daemon runs as its own process (T1). M1 can still run browser + CLI only, so the shell is not on the critical path |
| D2 | Terminal mode | Structured only · plus a read-only terminal · PTY-first like Runner | **Structured only for v1**; revisit per agent |
| D3 | Remote transport | WebRTC + TURN only · plus a 443 end-to-end encrypted relay · plus a native channel | **Recommended (2026-11-04):** WebRTC as the primary browser channel and a 443 end-to-end encrypted relay built in M2. R4 measured that the current UDP-only path does not open when the computer's UDP is blocked. That a 443-only network has no route on either side is inferred from the implementation, not measured. The channel interface also admits an optional native client later (P8). The relay is still an unimplemented candidate; this is final once the R4 write-up is accepted |
| D4 | v0 compatibility depth | Import room identity only · plus the v0 host protocol for manual Pi · full v0 parity | **Identity import plus the v0 host protocol**, so existing Pi extensions can attach during transition |
| D5 | Cloud code location | Move into this repo · keep in `pi-packages` | **Move the whole v0 relay module** into `cloud/` as its own Go module, unchanged: no service split, storage change or production change during the move (server agent's investigation, 2026-11-03). Protocol and fixtures then have one owner |
| D6 | First agents | Pi + Codex · plus Claude Code at launch | **Pi + Codex in M1, Claude Code in M2** after a probe |
| D7 | Platforms | macOS first · all three together | **macOS + Windows first** (v0 users), Linux after the daemon is stable |
| D8 | Approval default for remote control | Off (v0) · on | **Superseded by D12.** There are no third-party visitors to approve. Per-device control is D14 |
| D9 | Name of binaries and data dir | `spaced`/`space` · `cafe-spaced`/`cafe-space` · other | **Decided 2026-11-03: `cafe-spaced` (daemon) and `cafe-space` (CLI).** The data dir, IPC endpoint, autostart labels, app identifier and environment prefix follow from them (§5.5) |
| D10 | Default autonomy budget (P3) | Per binding: turns · wall-clock · cost, and a daemon-wide concurrent limit | Conservative defaults owners can raise: 20 turns, 2 h, 2 concurrent autonomous sessions per computer, cost cap when the adapter reports usage. Stop after 3 turns with no room-visible progress |
| D11 | May remote clients attach to borrowed native sessions or list native threads? | Owner-only · remote too | **Resolved by D12:** every remote client is the owner, so signed-in devices may |
| D12 | Who may use a computer remotely (research SEC1) | Guests with a room password · owner only | **Decided 2026-11-03.**<br>**Without a Café login:** one local account per computer; remote devices must sign in as that account, so remote access is the owner only.<br>**With a Café login:** left undefined for now. Any future design that lets a different person start sessions must isolate them (separate OS user or sandbox) |
| D14 | New-device approval | Off · one-time approval on the computer for each new device | **On.** The account password grants control of the computer's agents, so a password leak alone should not be enough |
| D13 | Webview-to-daemon transport (research T3) | Rust IPC proxy · direct loopback · localhost UI | **Decided 2026-11-03: Rust IPC proxy.** Confirmed by the M0 spike at `c3e3c3b` on macOS WKWebView, Windows WebView2 and Linux WebKitGTK; the daemon outlives the shell (research T3) |
| D15 | Implementation languages | Go daemon + Rust shell · Rust on the computer, Go cloud · Rust everywhere | **Decided 2026-11-03: Rust on the computer** (daemon, CLI, Tauri shell) **and React + TypeScript for the UI. The cloud stays Go, unchanged.**<br>Reasons: one language with the Tauri shell; shared `protocol` and `ipc` crates (D13); the official ACP Rust SDK; Runner as a Rust daemon reference; the daemon's modules are mostly new code anyway.<br>Cloud: it is in production and its contract is unchanged in v1, so a rewrite brings risk without user-visible gain.<br>Risk: Rust WebRTC maturity and UDP-only TURN on the computer side (research R4), settled by the M0 WebRTC spike |

## 15. Roadmap

| Milestone | Scope | Exit criteria |
|---|---|---|
| M0 Foundations | Repo, CI (Cargo and npm; Go only for the cloud), Cargo workspace, protocol v2 draft + fixtures (including fixtures recorded from v0's Go code), daemon skeleton (IPC, SQLite, logging, lock, per-OS autostart), web UI ported to talk to the daemon, **spikes: Tauri transport (D13), daemon copy-out + update handover, and Rust WebRTC (R4)** | Daemon runs headless; UI lists zero sessions; fixtures pass in Rust, TS and the Go cloud; the transport spike passes on macOS, Windows and Linux; the WebRTC spike connects Chrome, Safari and Firefox to the Rust daemon directly and through coturn over UDP, and records what happens when the computer's network blocks UDP |
| M1 Local agent host | Pi (RPC) + Codex adapters, session lifecycle, projects, CLI basics | Create, observe, prompt, cancel and close real sessions; daemon restart keeps truthful states; the CLI passes the text-encoding tests (§6) on all three OSes, including Windows PowerShell 5.1 and code page 936 |
| M2 Remote access | Remote gateway module, ported from v0's Go to Rust; local account and device list (D12); new-device approval (D14); v0 room identity import (password becomes the account password); phone UI; remote session creation; configurable cloud address | An existing v0 room link opens the new daemon on a second device with the old password. A new device needs approval, then creates and closes a session. Revoking a device ends its access. The address change re-homes the room. Race and E2E suites green |
| M3 Collaboration | The cloud's multi-room watch (in Go, §8), room connector with many memberships, binding, inbox and delivery states, CLI and skill for agents, Goal-driven autonomy with budgets and hard stops | A bound local agent receives, handles and acknowledges room messages; an autonomous binding works toward an active Goal and stops on every hard-stop condition; all delivery and autonomy fixtures pass |
| M4 Distribution | Tauri 2 shell (sidecar daemon handover, tray, notifications, deep links), signing and notarization, Tauri updater, autostart | Signed installers on macOS and Windows; a shell-only update leaves agents running; a daemon update stops and then cleanly resumes sessions (update now or when idle); update and rollback tested |
| M5 Transition | v0 deprecation notice, migration guide, Claude Code adapter | v0 users migrated without losing links or rooms |
| M6 Native client (optional, P8) | A native app or CLI client using a stronger channel through the same channel interface, protocol v2, sign-in and device approval | Timing set by the owner after v1. It never replaces the browser path |

## 16. References

- v0 docs in `pi-packages/packages/pi-cafe-space/docs`: `REMOTE_ACCESS.md`, `ROOMS.md`, `COLLABORATION.md`, `MANAGED_SESSIONS.md`, `protocol/v1/*`.
- v0 room discussion records: ACP review (findings F1–F12), listening design (#24/#25), entry-point redesign (#51, branch `feat/space-entry-points`).
- Runner (`github.com/yicheng47/runner`, MIT), used as an architecture reference:
  - `docs/features/archive/645-session-host.md` (the daemon owns state);
  - `docs/tech/runtime-adapters.md` and `docs/arch/runtime-integration.md` (adapters and checklist);
  - `docs/tech/mission-message-delivery.md` (delivery milestones and readiness gate);
  - `docs/tech/remote-control.md` (end-to-end encrypted relay with PIN pairing);
  - its Rust daemon as an implementation reference: `crates/runner-daemon/src/ipc.rs` (owner-only pipe and socket), `docs/arch/windows.md` and `docs/arch/linux.md` (daemon from app data, Job Objects, optional systemd user unit).
- Rust building blocks, with versions in research round 2: `agent-client-protocol` (ACP Rust SDK), `webrtc` and `rtc` (webrtc-rs), `tokio`, `rusqlite`, `keyring-core` and its store crates, `service-manager`, `p256`, `pbkdf2`, `ts-rs`.
