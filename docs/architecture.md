# Café Space — Architecture (draft v0.1)

> Status: **draft for discussion**, 2026-11-03. Nothing here is implemented in this repository yet.
> Predecessor: `pi-cafe-space` in `cafeshop-ai/pi-packages` (Pi extension + Go relay + React web), referred to below as **v0**.
> Decisions marked **[open]** need an owner decision; each carries a recommendation. See §14.
> **Decided:** D1 desktop shell is **Tauri 2** (2026-11-03).

## 1. Why a new version

v0 grew out of a Pi extension. The office-side Go gateway exists to relay Pi extensions, Pi is the only first-class agent, and Codex support had to be bolted on through a side path (ACP managed sessions). Three problems follow:

1. **Agent coupling.** Every new agent needs a different integration route instead of one interface.
2. **No owner of state.** Session lifetime is split between Pi processes, the gateway and the browser; nothing on the machine can run without one of them.
3. **Collaboration cannot reach agents.** People + AI rooms live in the cloud, but nothing on the user's machine can deliver a room message into a running agent session reliably (see the v0 listening-design discussion).

Café Space v1 is a **standalone application**: installed once per computer, it owns the agent sessions it runs, exposes them to the user's own devices, and connects them to people and other agents in rooms. Pi becomes one agent runtime among several.

## 2. Goals and non-goals

### Goals (v1)

- **G1 Local agent host.** Start, open, observe, steer and stop sessions of several agent runtimes (Pi, Codex, Claude Code first) from one place, with honest status.
- **G2 Use your computer from anywhere.** Phone and browser access to those sessions through a private room link + password, without opening an inbound port.
- **G3 People + AI rooms.** Discussion, tasks and a shared Goal across machines; local agents can be members, and room messages are delivered into their sessions with truthful receipts.
- **G4 Survive the UI.** Closing, updating or crashing the UI does not stop agents.
- **G5 Keep v0 users whole.** Existing room links, room passwords and collaboration rooms keep working through the transition.

### Non-goals (v1)

- Running models or agents in the cloud. The cloud routes, signals and stores room text; it never executes work.
- Mirroring terminal screens as the primary UI (see §5.3; a terminal mode is a possible later addition).
- Native mobile apps. Phones use the web UI.
- Multi-user daemons, or one daemon serving several OS users.
- Sandboxing agents. Agents keep their own native permission models; Space adds approval gates but is not an OS sandbox.

## 3. Principles carried over from v0

These were learned the hard way and stay non-negotiable:

- **Structured, not scraped.** Transcripts, tool calls, status and results come from native agent events, never from parsing screen output. Missing evidence is shown as unknown.
- **Unknown outcomes are never retried.** Creates are idempotent by client-chosen ID; a lost response is reconciled, not resent. "Dispatched" never means "done".
- **Independent gates.** Identity (who you are), room password (may you enter) and approval (may you control) are separate checks; none implies another.
- **No secrets in URLs, argv, logs or room messages.** Tokens live in the OS secret store or owner-only files.
- **Room content is untrusted data.** A message is never a local instruction; only the local user authorizes local effects.
- **Bounded everything.** Frame sizes, queues, history, registries and retries have explicit limits.

## 4. System overview

```text
                         ┌──────────────────────── Café cloud (space.cafecode.work) ───────────────────────┐
                         │  web assets · room registry & signaling · TURN · collaboration rooms · identity │
                         └───────▲──────────────────────▲─────────────────────────────▲────────────────────┘
                                 │ signaling (HTTPS/WS) │ collab HTTPS JSON           │ OIDC
   Phone / laptop browser ───────┘                      │                             │
        │  WebRTC (direct or via TURN), end-to-end encrypted                          │
        ▼                                                                             │
 ┌──────────────────────────── User's computer ────────────────────────────────────────┴───────┐
 │                                                                                              │
 │   Tauri 2 shell (tray, windows, notifications, updater) ── hosts ──► web UI (webview)        │
 │   `space` CLI ─────────────┐                                         │                       │
 │                            ▼                                         ▼                       │
 │                 ┌────────────────────────── spaced (daemon) ─────────────────────────┐       │
 │                 │ Local API · Session manager · Agent adapters · Remote gateway      │       │
 │                 │ Room connector (collab) · Inbox & delivery · Store (SQLite + logs) │       │
 │                 └──────┬───────────────────┬──────────────────────┬───────────────────┘       │
 │                        │ RPC (stdio/JSON)  │ ACP / app-server     │ v0 host protocol        │
 │                        ▼                   ▼                      ▼                          │
 │                   Pi (RPC mode)      Codex / Claude Code      Existing Pi + v0 extension     │
 └──────────────────────────────────────────────────────────────────────────────────────────────┘
```

Three tiers:

| Tier | Runs | Owns | Never does |
|---|---|---|---|
| **Agents** | Native CLIs started or attached by the daemon | Model calls, tools, their own conversation storage | Talk to the cloud on Space's behalf |
| **Daemon (`spaced`)** | One per OS user, starts at login | Session lifecycle, adapter state, remote gateway, room connections, local store, secrets | Render UI; execute room content as commands |
| **Clients** | Desktop shell, local web UI, `space` CLI, remote browsers | Presentation and user intent | Hold agent credentials or spawn agents directly |

The **cloud** is a separate deployment with three services (cloud, identity, TURN). It is unchanged in shape from v0 (§9).

## 5. The daemon (`spaced`)

### 5.1 Process model

- One daemon per OS user and data directory. A lock file in the state directory enforces single ownership (as v0's managed registry does).
- Started by the OS at login (LaunchAgent on macOS, a per-user service on Linux, a Startup entry or task on Windows), or on demand by the shell or CLI.
- Clients connect over **owner-only local IPC**: a Unix socket with 0600 permissions, or a Windows named pipe with an owner-only DACL. The local web UI uses a loopback HTTP/WebSocket listener protected by a per-install token and an Origin check, exactly as v0's local gateway does.
- Agent process trees belong to the daemon. Windows uses Job Objects with `KILL_ON_JOB_CLOSE`. POSIX uses a supervisor with process groups and a liveness pipe. This is v0 `relay/internal/managed`, carried over.
- When the daemon restarts, sessions are recorded as stopped and reopened explicitly; prompts are never auto-replayed (v0 behavior). An attached external session is reconnected, not respawned.

### 5.2 Internal modules

| Module | Responsibility | v0 source to reuse |
|---|---|---|
| `api` | Typed request/response + event stream for all clients; one request table shared by UI, CLI and remote clients | `relay/internal/httpserver`, `hub` |
| `session` | Session registry, lifecycle state machine, idempotent create/open/close, fencing (stream / session / cwd) | `relay/internal/managed`, `hub` |
| `adapters/*` | One adapter per agent runtime behind a common interface (§5.3) | `managed/acp.go`, `scripts/acp/*`, Pi RPC launcher |
| `protocol` | Canonical session snapshot / event / command codecs (Go), with shared fixtures and TS types | `relay/internal/protocol`, `src/protocol` |
| `remote` | Room identity, password verification, approvals, WebRTC peer, TURN config | `relay/internal/remote`, `auth` |
| `collab` | Membership in collaboration rooms, durable inbox, delivery to bound sessions (§8) | `scripts/space-agent.mjs` semantics |
| `store` | SQLite (config, registry, bindings) + append-only per-room inbox logs + secret store access | `managed` registry, `space-agent` inbox |
| `files` | Project-scoped file reads/references with sensitive-path rules | v0 file commands |

### 5.3 Agent adapters

Every runtime-specific rule lives in one adapter. Shared code asks the adapter; it never switches on the runtime name. This mirrors Runner's `RuntimeAdapter` refactor and replaces v0's hard-coded `pi` / `codex-acp` branches.

```go
type Adapter interface {
    Identity() RuntimeInfo                 // id, display name, version probe
    Detect(ctx) (Installation, error)      // find binary, version, auth state (no prompts sent)
    Capabilities(inst) Capabilities        // new / resume / attach, model control, delivery modes, approvals, files
    Launch(ctx, spec LaunchSpec) (Conn, error)   // new or resume; returns a structured connection
    Attach(ctx, target Target) (Conn, error)     // borrow an existing native session (optional)
}

type Conn interface {
    Events() <-chan Event                  // normalized transcript / tool / status / approval events
    Prompt(ctx, Prompt) (Outcome, error)   // Outcome includes Unknown
    Cancel(ctx) error                      // only for turns this connection owns
    Respond(ctx, ApprovalID, Decision) error
    Close(ctx) error                       // detach-only for borrowed sessions
}
```

**Structured-first.** Each adapter speaks the agent's structured interface. These are candidates, to be verified per version before an adapter ships:

| Runtime | Interface | Notes |
|---|---|---|
| Pi | RPC mode (`--mode rpc`), plus an optional v0 extension for manual sessions | Production-tested in v0 managed sessions |
| Codex | ACP via `codex-acp`, and/or the native app-server (JSON-RPC; socket proxy for attach) | v0 has ACP new/restore/attach with offline tests; F1–F3 fixes landed on `fix/cafe-acp-release-blockers` |
| Claude Code | An ACP adapter or the Agent SDK streaming interface | Not yet prototyped; needs a probe like v0's `acp:probe` |
| Generic ACP agent | ACP over stdio | A fallback for any ACP-speaking agent |

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
- **Native session IDs stay local.** Browsers get opaque keys (v0 `targetKey`).
- **Borrowed sessions.** A borrowed session (attach) never changes the native model, permissions or history, and closing it only detaches.
- **Every failure code is user-visible** and explained; nothing is retried automatically.

## 6. Clients

- **Local web UI.** The v0 React app (`web/`) is evolved, not rewritten. Its two entry points (Remote device / Collaboration, from `feat/space-entry-points`) become the app's top-level navigation, plus a new **Agents** area for local sessions. The same build serves the desktop shell, the loopback browser and remote visitors; the capabilities exposed differ per connection role.
- **Desktop shell: Tauri 2 (decided, D1).** A thin native wrapper. It holds no state and owns no agent; if it crashes or updates, agents keep running.
  - **Daemon as a sidecar.** `spaced` ships inside the app bundle as a Tauri external binary. On launch the shell connects to a running daemon or starts one, then hands it over to the OS login service. The daemon must never be a child that dies with the window.
  - **UI.** The webview loads the React build bundled in the app, so the signed app, its UI and the daemon version move together. It talks to the daemon over the loopback API.
  - **Token handling.** The Rust side reads the local API token from the OS secret store and passes it to the webview through Tauri IPC, never in a URL. Remote browsers get the same React build, served by the daemon.
  - **Plugins.** Tray, notifications, single-instance, deep links (`cafe-space://`), autostart and the updater use official Tauri plugins. The updater updates the shell and the bundled daemon together, with signed update manifests.
  - **Keep the Rust side small.** Window and OS integration only. All product logic stays in the Go daemon, so the CLI and remote clients behave identically.
  - **Webview differences.** WKWebView on macOS, WebView2 on Windows (bootstrap the runtime if missing) and WebKitGTK on Linux. These are covered by the UI test matrix, since behavior differs, for example IME input, clipboard and WebRTC in the local window.
- **`space` CLI.** It ships with the daemon binary. Users and agents run `space room post`, `space room read`, `space session list` and `space status`. When the daemon starts an agent, it puts `space` on that agent's PATH, sets scoped environment variables (session ID, room binding) and installs a short skill telling the agent how to use it. This is Runner's pattern, and it replaces pasting long invitation prompts.
- **Remote browsers.** These are phones and laptops reaching the daemon through the remote gateway (§7). They are identical to the local UI apart from role-based limits.

## 7. Remote access

v1 keeps the v0 model because it works in production and preserves existing links:

- A room is identified by a public key; the link `…/#/room/<roomKey>` carries no secret. The private key stays on the computer.
- Visitors enter a 6–20 character room password. Each browser holds a non-exportable signing key per room; the visitor ID comes from that key, and the nickname is display-only.
- Approval to control (default off) and in-memory leases are kept. Owner settings can only be changed from the local UI.
- Transport is WebRTC: direct when possible, otherwise via TURN. Signaling goes through the cloud, which cannot read the channel.

**Changes in v1:**

- The remote gateway is a module inside the daemon, not a separate gateway in front of Pi extensions.
- **Room identity import.** On first run, the daemon imports an existing v0 room identity (private key, password verifier, control settings), with confirmation, so links and passwords keep working (G5).
- **Transport behind an interface [open, D3].**
  - A second route can be added later without touching the layers above: an end-to-end encrypted WebSocket relay, as in Runner's Noise + PIN pairing.
  - Reason for leaving room for it: v0's WebRTC end-to-end tests are timing-sensitive and TURN is costly to operate.
  - Recommendation: keep WebRTC for v1, define the channel interface now, and decide on a relay route after measuring connection failures.

## 8. Collaboration rooms

The cloud collaboration service and its HTTPS JSON protocol are unchanged and stay runtime-agnostic. Anyone can still join from a browser, and any agent can join with plain HTTP. What v1 adds is that **the daemon is a first-class room member connector**.

- **Joining.** The user joins a room from the app; the membership token goes to the secret store. A local session can be **bound** to that membership, so the agent speaks in the room as itself. Binding is an explicit local action, per room and per session.
- **Delivery.** These are the semantics agreed in the v0 discussion (#24/#25 there), now implementable because the daemon owns the sessions:
  - **Durable inbox.** Long-poll the room into an append-only local inbox, keyed `<room>:m:<seq>`.
  - **Control events.** Goal revision and AI-pause changes become separate control events (`<room>:c:<goalRevision>:<paused>`) and are never merged with chat.
  - **Truthful states.** Each event moves through **fetched → queued → delivered → handled**, and no two states are merged. `delivered` means the adapter accepted the event into the bound session; `handled` is an explicit acknowledgement from the session (`space room done <seq>`).
  - **Busy sessions.** At most one outstanding chat wake per binding, with a pending count. Control events always go first. A **readiness gate** (starting, needs-human, running, idle) decides when a wake is attempted, following Runner's delivery gate but driven by adapter status events rather than terminal heuristics.
  - **No retries on unknown outcomes.** A delivery whose outcome is unknown stops and is reconciled. A refusal backs off with a bounded delay.
  - **Baseline at bind.** History from before the binding never wakes the agent.
- **Heartbeats.** The daemon sends acks with no model calls. Presence can report the mode truthfully: manual, listener, or host-wake confirmed.
- **Safety.** Room text reaches the agent as untrusted input, and a woken turn is never local authorization. AI pause makes the binding read-only. Revocation or room closure stops it.

## 9. Cloud

The cloud keeps v0's deployment shape: cloud (Go: web assets, room registry, signaling, collaboration store), identity (OIDC and Café SSO, separate service) and TURN.

| Concern | v1 decision |
|---|---|
| Collaboration API | Unchanged v1 HTTP contract; additive fields only |
| Device-room signaling | Unchanged while v0 gateways exist; v2 signaling is negotiated by version |
| Web assets | The same React build, embedded in the Go binary |
| Identity | Stays a separate service; v1 adds an optional app sign-in (account only gates cloud features, never room entry) |
| Storage | Collaboration rooms stay plaintext with a 7-day expiry and clear disclosure; no device traffic is stored |
| Repository | **[open, D5]** Cloud code moves into this repo or stays in `pi-packages`. Recommendation: move it, so protocol and fixtures live in one place |

## 10. Data and storage

| Data | Where | Notes |
|---|---|---|
| Config, projects, session registry, bindings | SQLite (WAL) in the per-user state dir | Migrations versioned; a crash-safe write precedes every external side effect |
| Room inboxes, delivery records | Append-only JSONL per room | Survives restarts; rebuilds projections; bounded by the room cap |
| Secrets (room private keys, membership tokens, local API token) | OS secret store (Keychain / Credential Manager / Secret Service), owner-only file fallback | Never in logs, argv or URLs |
| Agent conversations | The agent's own storage | Space never rewrites native histories |
| Logs | Rotating files | No transcripts, tokens or room text by default |

## 11. Security model

**Trust boundaries:**
1. The local OS user, who is trusted.
2. Local clients, which use owner-only IPC or a token-protected loopback listener.
3. Remote visitors, who pass the room password, then their device key, then approval if enabled.
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
- Any capability that reaches the local machine (create a session, attach to a borrowed session, list native threads) requires local-owner approval when the requester is remote. This resolves v0 review finding F5.

## 12. Reliability and observability

- **Health.** The daemon exposes a local health endpoint and a diagnostic report with redacted fields, as in v0 `roomDiagnosticReport`.
- **Outcome states.** Every remote operation has an explicit outcome: applied, rejected, dispatched or unknown. The UI distinguishes them.
- **Restarts.** Sessions are stopped and reopened explicitly. Room connectors resume from their persisted cursors, never from history 0.
- **Metrics.** Local only: delivery latencies, queue depths and adapter errors. There is no telemetry without opt-in.

## 13. Repository layout (proposed)

```text
cafe-space/
  cmd/spaced/        daemon entry (also `space` CLI via argv[0] or subcommand)
  cmd/space-cloud/   cloud service
  internal/          api, session, adapters/{pi,codex,claude,acp}, protocol, remote, collab, store, files
  protocol/          versioned schemas + shared fixtures (consumed by Go and TS tests)
  web/               React UI (from v0 web/)
  desktop/           Tauri 2 app (Rust shell; bundles web/ build and the spaced sidecar)
  docs/              architecture, decisions (ADRs), runtime checklists, deployment
  test/              synthetic agent fixtures (fake Pi RPC, fake ACP, fake app-server), e2e
```

One Go module with shared internal packages. Every new adapter or protocol version comes with fixtures before code.

## 14. Open decisions

| ID | Decision | Options | Recommendation |
|---|---|---|---|
| D1 | Desktop shell | Tauri 2 · Wails (Go) · Electron · none (browser + tray) | **Decided: Tauri 2** (2026-11-03). Chosen for its updater, tray, deep links and signing. The shell stays thin, with the Go daemon as a sidecar. M1 can still run browser + CLI only, so the shell is not on the critical path |
| D2 | Terminal mode | Structured only · plus a read-only terminal · PTY-first like Runner | **Structured only for v1**; revisit per agent |
| D3 | Remote transport | WebRTC + TURN only · plus a Noise WebSocket relay | **WebRTC for v1**, a channel interface now, decide the relay on measured failure rates |
| D4 | v0 compatibility depth | Import room identity only · plus the v0 host protocol for manual Pi · full v0 parity | **Identity import plus the v0 host protocol**, so existing Pi extensions can attach during transition |
| D5 | Cloud code location | Move into this repo · keep in `pi-packages` | **Move**, so protocol and fixtures have one owner |
| D6 | First agents | Pi + Codex · plus Claude Code at launch | **Pi + Codex in M1, Claude Code in M2** after a probe |
| D7 | Platforms | macOS first · all three together | **macOS + Windows first** (v0 users), Linux after the daemon is stable |
| D8 | Approval default for remote control | Off (v0) · on | **Keep off** for owner-only rooms; **on** by default once a room has more than one human |
| D9 | Name of binaries and data dir | `spaced`/`space` · other | Settle before M1; it affects install paths and migration |

## 15. Roadmap

| Milestone | Scope | Exit criteria |
|---|---|---|
| M0 Foundations | Repo, CI, Go module, protocol v2 draft + fixtures, daemon skeleton (IPC, SQLite, logging, lock), web UI ported to talk to the daemon | Daemon runs headless; UI lists zero sessions; fixtures pass in Go and TS |
| M1 Local agent host | Pi (RPC) + Codex adapters, session lifecycle, projects, CLI basics | Create, observe, prompt, cancel and close real sessions; daemon restart keeps truthful states |
| M2 Remote access | Remote gateway module, v0 room identity import, approvals, phone UI | An existing v0 room link opens the new daemon on a second device; race and E2E suites green |
| M3 Collaboration | Room connector, binding, inbox and delivery states, CLI and skill for agents | A bound local agent receives, handles and acknowledges room messages; all P0 delivery fixtures pass |
| M4 Distribution | Tauri 2 shell (sidecar daemon handover, tray, notifications, deep links), signing and notarization, Tauri updater, autostart | Signed installers on macOS and Windows; shell update without stopping running agents; update and rollback tested |
| M5 Transition | v0 deprecation notice, migration guide, Claude Code adapter | v0 users migrated without losing links or rooms |

## 16. References

- v0 docs in `pi-packages/packages/pi-cafe-space/docs`: `REMOTE_ACCESS.md`, `ROOMS.md`, `COLLABORATION.md`, `MANAGED_SESSIONS.md`, `protocol/v1/*`.
- v0 room discussion records: ACP review (findings F1–F12), listening design (#24/#25), entry-point redesign (#51, branch `feat/space-entry-points`).
- Runner (`github.com/yicheng47/runner`, MIT), used as an architecture reference:
  - `docs/features/archive/645-session-host.md` (the daemon owns state);
  - `docs/tech/runtime-adapters.md` and `docs/arch/runtime-integration.md` (adapters and checklist);
  - `docs/tech/mission-message-delivery.md` (delivery milestones and readiness gate);
  - `docs/tech/remote-control.md` (end-to-end encrypted relay with PIN pairing).
