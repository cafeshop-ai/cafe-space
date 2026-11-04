# Technical verification of the v1 architecture (rounds 1–2)

> Date: 2026-11-03. Scope: the technical claims in [`../architecture.md`](../architecture.md), checked against v0 source (`cafeshop-ai/pi-packages` `main` @ `3294487`), package registries and upstream source or docs.
> Status legend:
> - **Confirmed**: verified, matches the architecture.
> - **Corrected**: verified, and the architecture was wrong or incomplete; fixed in the same change.
> - **Spike**: cannot be settled by reading; needs a prototype.
> - **Decision**: verified, but it forces an owner decision.
>
> Nothing here was built or run against a live service. Package versions are as of the date above.
>
> **Round 2** (same day) re-checks the daemon for Rust after D15: the computer side is Rust, and the cloud stays Go. See [Round 2](#round-2-rust-daemon-d15). Round-1 items S1–S4 describe the Go libraries and are kept as history.

## Summary

| # | Area | Claim checked | Status |
|---|---|---|---|
| T1 | Tauri | Daemon can ship as a sidecar without dying with the app | **Corrected**: plugin-shell kills its children on exit |
| T2 | Tauri | Shell updates do not stop agents | **Corrected**: true for shell-only updates; a daemon update restarts sessions |
| T3 | Tauri | Webview talks to the daemon over loopback HTTP | **Confirmed** by the M0 spike: option A, the Rust IPC proxy, on all three native webviews |
| T4 | Tauri | Required plugins exist for v2 | **Confirmed**, with a minimum version for CVE-2026-42184 |
| T5 | Tauri | WebView2 runtime handling on Windows | **Confirmed**: five install modes |
| S1 | Daemon (Go; see round 2) | Per-user autostart on all three OSes | **Corrected**: Windows needs a logon task, not a service |
| S2 | Daemon (Go; see round 2) | Owner-only IPC on Windows | **Confirmed**: go-winio SDDL |
| S3 | Daemon (Go; see round 2) | CGO-free SQLite | **Confirmed**: modernc.org/sqlite |
| S4 | Daemon (Go; see round 2) | OS secret store protects daemon secrets | **Decision**: it does not protect them from same-user agents (see SEC1) |
| A1 | Agents | Pi RPC interface and v0 flags | **Confirmed** (Pi 1.5.0), with two adapter rules |
| A2 | Agents | Codex via ACP and/or app-server | **Confirmed**: both viable; app-server surface is fast-moving |
| A3 | Agents | Claude Code via an ACP adapter | **Corrected**: package renamed; built on the Claude Agent SDK |
| A4 | Agents | ACP methods we rely on are stable | **Confirmed** for ACP v1; v2 is alpha |
| R1 | Remote | v0 room identity can be imported | **Confirmed**: simple versioned JSON |
| R2 | Remote | Cloud address configurable (P5) | **Confirmed**: v0 code is already parameterized |
| R3 | Remote | Guest session creation (P2) has a v0 basis | **Confirmed**, but see SEC1 |
| C1 | Collab | Daemon can hold many room long-polls (P1) | **Corrected**: cloud limits make this a cloud change |
| SEC1 | Security | Owner/guest separation holds when guests create sessions | **Decision**: it does not hold against a same-user agent. **Resolved by D12**: remote access is owner-only |
| SEC2 | Security | Loopback API is safe from DNS rebinding | **Corrected**: v0's generic Origin check alone is not enough; add a Host allowlist |

## Tauri 2 shell

### T1. Sidecar lifetime: Corrected

`tauri-plugin-shell` (2.6.0) stores every spawned child, sidecars included, and kills them all on `RunEvent::Exit`. The source documents it: "all spawned child processes are killed when the application exits" (`plugins/shell/src/lib.rs`, lines 125 and 163–171).

**Consequence.** If the shell starts `cafe-spaced` through plugin-shell, quitting the app kills every agent. That breaks G4.

**Design.** Follow Runner (#645):
- The bundle carries the daemon binary.
- On first run and after updates, the shell copies it to `<app data>/bin/` and verifies its hash.
- The daemon is started from there through the OS service manager (S1), or with a detached spawn as a fallback. It is never started as a plugin-shell child.
- The shell only connects to it.

### T2. Updates and running agents: Corrected

- **Windows file locks.** On Windows, a running executable cannot be replaced, so a daemon inside the install directory would block or break updates. Runner copies `runnerd` into app data for exactly this reason: "the files `runnerd` runs from live in app data", and an update leaves the daemon running.
- **Shell-only update.** It does not touch the daemon or the agents.
- **Daemon update.** The new build finds a hash mismatch and restarts the daemon. Agent processes belong to the daemon (Job Objects or process groups, v0 `managed`), so **a daemon restart stops sessions**. They are reopened and resumed explicitly; prompts are never replayed.
- **Drain policy.** The shell offers "update now" (sessions stop) or "update when idle" (wait until no session is running).

The architecture's M4 exit criterion "shell update without stopping running agents" is narrowed to shell-only updates.

### T3. Webview to daemon transport: Confirmed (M0 spike)

The architecture assumed the webview (origin `tauri://localhost` on macOS, `http://tauri.localhost` on Windows) calls the daemon at `http://127.0.0.1:<port>`. I found no authoritative statement on whether macOS WKWebView treats that as blocked mixed content. Three options to prototype in M0:

| Option | How | Pros | Cons |
|---|---|---|---|
| A. Rust IPC proxy | The webview calls Tauri commands; the Rust side forwards to the daemon socket and streams events back through a Tauri `Channel` | No CSP or mixed-content issue. The token never reaches JavaScript. Uses the owner-only socket directly | The React app needs a transport abstraction (it has one for v0's gateway). Rust grows slightly |
| B. Direct loopback | CSP `connect-src http://127.0.0.1:* ws://127.0.0.1:*`; the token passed through IPC | Simplest; same code path as the browser | Depends on T3's answer per platform |
| C. Localhost UI | The [localhost plugin](https://v2.tauri.app/plugin/localhost/) or `WebviewUrl::External(daemon URL)` | Same origin as the daemon | Remote-URL IPC capability needed; weaker isolation |

Recommendation: prototype **A** first. It removes the open question and keeps tokens out of the webview.

**Result (2026-11-03).** Option A was built in M0 (tasks #5 and #6) and passed the native desktop workflow at `c3e3c3b` on macOS WKWebView, Windows WebView2 and Linux WebKitGTK. Each run went native webview → Tauri command → Rust proxy → owner-only socket or pipe → daemon → `Channel` → React, ending in an empty session list. A second IPC request after the shell exited confirmed that the daemon outlives it. Options B and C were not needed, so the WKWebView mixed-content question no longer matters.

### T4. Plugins and versions: Confirmed

Current crates (crates.io, 2026-10):

| Crate | Version |
|---|---|
| `tauri` | 2.12.5 |
| `tauri-plugin-updater` | 2.14.1 |
| `tauri-plugin-shell` | 2.6.0 |
| `tauri-plugin-deep-link` | 2.6.3 |
| `tauri-plugin-single-instance` | 2.5.5 |
| `tauri-plugin-autostart` | 2.7.0 |
| `tauri-plugin-notification` | 2.8.0 |
| `tauri-plugin-process` | 2.4.0 |

The JS packages are `@tauri-apps/api` and `@tauri-apps/cli` 2.12.4.

**Minimum Tauri 2.10.3.** CVE-2026-42184 (`is_local_url()` could treat remote URLs as local origins; Windows and Android) is fixed in 2.10.3 ([NVD](https://nvd.nist.gov/vuln/detail/cve-2026-42184)).

`tauri-plugin-autostart` starts the **app** at login, not the daemon. Daemon autostart is S1.

### T5. WebView2: Confirmed

The bundle option `webviewInstallMode` takes one of five modes ([Windows installer guide](https://v2.tauri.app/distribute/windows-installer/), [config reference](https://tauri.app/reference/config/)):

| Mode | Size added | Internet at install |
|---|---|---|
| `downloadBootstrapper` (default) | ~0 MB | Required |
| `embedBootstrapper` | ~1.8 MB | Required |
| `offlineInstaller` | ~127 MB | Not required (also needed for the Microsoft Store) |
| `fixedRuntime` | ~180 MB | Not required; we would have to patch it ourselves |
| `skip` | 0 MB | Breaks the app if the runtime is missing |

Recommendation: `embedBootstrapper` for the website download, and `offlineInstaller` only for offline or Store builds.

## Daemon

### S1. Per-user start at login: Corrected

`github.com/kardianos/service` v1.3.0 supports an `UserService` option:
- **macOS:** `~/Library/LaunchAgents/<name>.plist` (`service_darwin.go`).
- **Linux:** `systemctl --user` (`service_systemd_linux.go`).
- **Windows:** it only creates services through the Service Control Manager (`mgr.Connect`/`CreateService`). Those are machine-wide and need administrator rights.

Design: use kardianos (or hand-written plist and unit files) on macOS and Linux. On Windows, register a **per-user Task Scheduler "at logon" task**, or an `HKCU\…\Run` entry as a fallback. Runner has no login item in phase 1 and starts the daemon from the app or CLI.

### S2. Owner-only IPC: Confirmed

- **Windows:** `github.com/Microsoft/go-winio` v0.6.4 `PipeConfig.SecurityDescriptor` takes an SDDL string (`pipe.go:491–520`). An owner-only DACL is therefore expressible.
- **Unix:** sockets in a 0700 directory, with 0600 permissions.

### S3. SQLite without CGO: Confirmed

`modernc.org/sqlite` v1.61.1 (2026-10-31) is pure Go, which keeps the daemon cross-compilable with `CGO_ENABLED=0` like v0's relay. The CI matrix must still run it on every target (darwin/arm64, windows/amd64, linux/amd64 and linux/arm64).

### S4. Secret store: Decision (see SEC1)

- **macOS:** `github.com/zalando/go-keyring` v0.2.8 writes through `/usr/bin/security -i`, with the command on **stdin**, not argv (`keyring_darwin.go:76–86`). So it does not leak through `ps`. Items created through the `security` tool can be read back through the same tool by any process of the same user.
- **Windows and Linux:** Credential Manager and an unlocked Secret Service collection are likewise readable by same-user processes. Windows also lets a same-user process read another's memory by default.
- `99designs/keyring` (last release 2022) is not a candidate.

An OS secret store protects secrets at rest and from other OS users. It does **not** protect them from agents, which run as the same user and can execute commands.

## Agents

### A1. Pi: Confirmed

- **Package:** `@earendil-works/pi-coding-agent` 1.5.0 (2026-11-02). The old `@mariozechner/pi-coding-agent` stopped at 0.73.1.
- **Flags:** RPC mode is `pi --mode rpc`. Pi 1.5's `docs/cli.md` still lists every flag v0 depends on: `--session-id`, `--session-dir`, `--offline`, `--no-approve`, `--no-extensions`, `-e`, `--no-themes` and `--name`.
- **Protocol** (`docs/rpc.md`): strict JSONL on stdin and stdout, plus extension UI records.

Two adapter rules follow:
1. **Split records only on LF.** Node's `readline` also splits on U+2028 and U+2029; Go's `bufio.Scanner` with the default `ScanLines` is correct. Read stdout continuously, because Pi honors backpressure.
2. **A prompt response carries a `disposition`.** It is not completion. Wait for `agent_settled`, not `agent_end`, because retries, compaction, steering and follow-ups can follow `agent_end`. A `handled` disposition starts no run.

Approvals: Pi has no built-in tool-permission prompts. Interactive needs arrive as `extension_ui_request` dialogs (`select`, `confirm`, `input`, `editor`) and are answered with `extension_ui_response`. That is the adapter's approval channel. Steering exists: `steer`, `follow_up` and `abort` are RPC commands.

### A2. Codex: Confirmed (two routes)

- **ACP route.** `@agentclientprotocol/codex-acp` 2.5.0 (unchanged from v0's pin):
  - it bundles a compatible `@openai/codex` (0.166.0 current);
  - it supports model, reasoning effort, approval, sandbox and permission-profile selection;
  - it supports permission requests, terminal output, plans and native sub-agent sessions;
  - it supports ChatGPT, API-key and custom gateway authentication. Its README advises `NO_BROWSER=1` for headless use.
- **App-server route.** `codex app-server`: JSON-RPC over stdio with `initialize`, `thread/*` and `turn/*`, and approval requests such as `item/commandExecution/requestApproval`. Many features sit behind `initialize.capabilities.experimentalApi`, and the surface changes frequently. v0 evidence already used `thread/read`, `thread/resume`, `turn/start` and `turn/interrupt` live over `codex app-server proxy --sock` (the attach path).

Recommendation: use **codex-acp for owned sessions**, and the **app-server only for attach** (borrowed sessions). Pin the versions, and capability-probe at startup.

### A3. Claude Code: Corrected

The ACP adapter is now `@agentclientprotocol/claude-agent-acp` 0.100.0 (published 2026-11-03). `@zed-industries/claude-code-acp` and `@zed-industries/claude-agent-acp` are deprecated ("renamed to @agentclientprotocol/claude-agent-acp").

It implements ACP on the official Claude Agent SDK (`@anthropic-ai/claude-agent-sdk` 0.3.316 dependency; Claude Code itself is 2.1.317). It supports tool calls with permission requests, slash commands, MCP and sub-agents.

Still unverified, and needing a probe before M2: its authentication model in a headless daemon (Claude Code login vs API key) and its resume behavior.

### A4. ACP versions: Confirmed

From `agentclientprotocol/agent-client-protocol` `schema/*/meta.json`:

- **v1 stable, agent methods:** `initialize`, `authenticate`, `logout`, `session/new`, `session/load`, `session/resume`, `session/list`, `session/close`, `session/delete`, `session/prompt`, `session/cancel`, `session/set_mode` and `session/set_config_option`.
- **v1 stable, client methods:** `session/update`, `session/request_permission`, `fs/read_text_file`, `fs/write_text_file`, `terminal/*` and `elicitation/*`.
- **v1 unstable only:** `session/fork`, `session/status`, `session/set_title`, `session/archive`, and `providers/*`.
- **v2** is `2.0.0-alpha.13` (2026-11-02). It drops `session/load` and `set_mode` and adds `tool_call/cancel` and `auth/login`.
- **TS SDK:** `@agentclientprotocol/sdk` 2.4.0. v0 pinned 2.1.0; the npm major version is not the wire version.

Decision: target **ACP wire v1 (stable)** and treat v2 as future work. Model and effort selection goes through `session/set_config_option`, not adapter-specific flags.

## Remote access

### R1. v0 room identity import: Confirmed

v0 `relay/internal/remote/room_identity.go` stores one strict JSON file:

```json
{"version":1,"privateKey":"<base64url DER EC P-256>","salt":"<16 bytes>","verifier":"<32 bytes>","revision":1}
```

- The verifier is PBKDF2-SHA256 with 600,000 iterations.
- Room-wide verification is limited to 120 attempts a minute, with one verification at a time.
- The room key in links is the base64url uncompressed public point (87 characters).
- Control settings live beside it in `<identity>.control.json`.

Import means: read the file, move the private key into the v1 store, keep the salt, verifier and revision, and copy the control settings. Links and passwords then stay valid. The password is checked by the computer, never by the cloud.

### R2. Configurable cloud (P5): Confirmed

No non-test Go relay or web source hard-codes `space.cafecode.work`. Only script defaults do (`room-setup.mjs --origin`, `install-client.mjs --server`, `doctor.mjs` and others), and they take a flag. v0 relays already accept `AllowedOrigins` (up to 64) and a configured public origin.

### R3. Guest session creation (P2): Confirmed basis

In v0's public room mode, every password-verified visitor is an "operator". `remote/session.go` lets operators `list`, `threads`, `create` and `open` managed sessions when `RoomManagement` is on, and returns `LOCAL_OWNER_REQUIRED` for non-read operations when the approval mode is on. P2 generalized this to all visitors. D12 then narrowed it to the owner's own devices (one local account), which removes the third-party case. See SEC1.

## Collaboration

### C1. Many rooms per computer (P1): Corrected

The v0 cloud (`relay/internal/collaboration/rooms.go`, `contract.go`) enforces:

| Limit | Value |
|---|---|
| Waiting reads per member | 2 |
| Waiting reads per server, all users | **256** (`totalWaits`) |
| Rooms per server (default) | 1024 |
| Requests per member per minute | 240 |
| Anonymous requests per client per minute | 600 (6,000 global) |
| Sends per member per minute | 20 |
| Creates per client per minute | 5 |
| Joins per client per minute | 30 |
| Messages per room | 500 |
| Members per room | 64 |
| Retention | 7 days |

One long-poll per room per computer would exhaust the **server-wide 256 waiting reads** with a few hundred room memberships across all users. "No limit on rooms" (P1) therefore needs a cloud change. Recommendation:
- add a **multi-room watch** operation: one waiting read per client that wakes on any of its memberships and returns the cursors that changed;
- raise or shard `totalWaits`.

Until then, the daemon long-polls only bound or open rooms and refreshes the rest on demand.

## Security

### SEC1. Guests, agents and owner secrets: Decision

> **Resolved 2026-11-03 by owner decision D12.**
> - **Without a Café login:** the computer has one local account, and remote devices must sign in as that account. Every remote client is therefore the owner, and the exposure below stays within one person's trust.
> - **With a Café login:** not yet defined. The analysis below still applies to it: letting a *different* person start sessions requires isolation (option b).
> - D14 (new-device approval, recommended on) covers the remaining risk that the account password leaks.

Agents run as the owner's OS user and can execute commands (S4). With P2, a guest can start a session and ask its agent to read what the daemon keeps for the owner:
- the local API token;
- room private key;
- collaboration membership tokens.

Read access is enough. Even read-only agent sandboxes allow reading most files, and Windows allows same-user memory reads. **So in v1, a guest who can create sessions can, with effort, obtain owner-level control of the room and that computer's Space data.** v0 has the same property (project allowlists were documented as "not an OS sandbox"). P2 makes it the default path rather than an opt-in owner feature.

| Option | Strength | Cost |
|---|---|---|
| a. Accept and state it | Guests with session creation are trusted like the owner on that computer. The setup flow says so before the first guest project is enabled | None; honest |
| b. Separate OS user for guest sessions | Real isolation of secrets and files | High; per-OS account management, file sharing and agent auth setup |
| c. Native per-app keychain ACLs | Protects stored keys on macOS only (signed daemon, no `security` CLI) | Medium; does not cover Windows or Linux, or tokens in memory |

Recommendation: **(a) for v1**, with the consequence written into the guest-project setup screen and the architecture. **(b) is the v2 hardening path.** This needs the owner's decision (D12).

### SEC2. Loopback API and DNS rebinding: Corrected

v0's generic `auth.Origins.Allowed` (`relay/internal/auth/origin.go`) accepts a request when `Origin` equals `scheme://Host` of the request itself. A DNS-rebinding page sends a matching `Origin` and `Host` (`evil.example:37891`) and would pass that check alone.

v0 is still protected because every such endpoint also requires the local token. The owner control endpoint (`remote/room_agent.go:109`) additionally requires a loopback peer, `Host` ∈ {localhost, 127.0.0.1, ::1}, an exact `Origin` and a custom header.

v1 applies the stricter rule to **every** loopback endpoint: Host allowlist + Origin + token. It never relies on Origin==Host alone.

## Round 2: Rust daemon (D15)

After D15, the daemon, the CLI and the shell are Rust, and the cloud stays Go. Round 2 re-checks every item that depended on a Go library, plus the questions that are new with Rust. Versions are from the crates.io API on 2026-11-03. Claims come from the published crate sources and from Runner (`yicheng47/runner` @ `9094032`), whose daemon is Rust. Nothing was compiled.

| # | Area | Claim checked | Status |
|---|---|---|---|
| S1 | Daemon | Per-user autostart with a Rust crate | **Confirmed**: same split as round 1; Windows still needs a logon task |
| S2 | Daemon | Owner-only IPC in Rust | **Confirmed**: tokio pipe options plus an owner-only SDDL, as in Runner |
| S3 | Daemon | SQLite in Rust | **Confirmed**: `rusqlite` with `bundled`; the C compiler it needs is already required by Rust's linkers, `ring` and Tauri |
| S4 | Daemon | Secret store in Rust | **Confirmed**; the decision is unchanged (SEC1, resolved by D12) |
| A1 | Agents | Pi JSONL framing in Rust | **Confirmed**: split on LF only |
| A5 | Agents | An official ACP SDK for the client role | **Confirmed**, with an exact version pin |
| R4 | Remote | A Rust WebRTC stack can replace pion | **Spike**: a recommended line exists, but TURN is UDP-only on the computer side |
| R5 | Remote | v0 room identity can be read in Rust | **Confirmed** |

### S1 (Rust). Per-user start at login: Confirmed

- `service-manager` 0.11.0 accepts `ServiceLevel::User` for launchd and systemd (`src/launchd.rs:214–223`, `src/systemd.rs:198–207`).
- Its Windows backends refuse it: "sc.exe does not support user-level services" (`src/sc.rs:268–277`) and "Windows does not support user-level services" (`src/winsw.rs:517–526`).
- The round-1 design therefore stands. macOS and Linux use a LaunchAgent and a `systemd --user` unit, through the crate or hand-written files. Windows uses a per-user logon task, or an `HKCU` Run entry as a fallback.
- Runner ships an optional systemd user unit, installed disabled, with `Restart=no`. The reason: systemd must never restart old daemon bytes while the app hands over to an updated daemon (`docs/arch/linux.md:92–105`). v1 adopts the same setting.

### S2 (Rust). Owner-only IPC: Confirmed

- **Windows.** tokio 1.54.1 `ServerOptions` provides `reject_remote_clients` (`src/net/windows/named_pipe.rs:2170`) and `unsafe create_with_security_attributes_raw` (`:2310`). Runner reads the user SID from the process token, builds the SDDL `D:P(A;;GA;;;<SID>)`, and creates its pipe with both options (`crates/runner-daemon/src/ipc.rs:159–214`).
- **Unix.** Runner binds the socket and then sets 0600 (`ipc.rs:137–148`), which leaves a short window with default permissions. v1 creates the socket inside a 0700 directory, so that window grants nothing.

### S3 (Rust). SQLite: Confirmed

- `rusqlite` 0.41.1 with the `bundled` feature compiles SQLite into the binary (`Cargo.toml:82–85`), so there is no system SQLite dependency. Runner's daemon uses `rusqlite` with `bundled` on all three OSes.
- **Build cost: small.** Bundling compiles SQLite's C source, so it needs a C compiler. A Rust desktop build has one anyway:
  - Rust links through the platform toolchain: MSVC Build Tools on Windows, Xcode Command Line Tools on macOS, the system `cc` on Linux. Each includes a C compiler, and Tauri lists the same tools as prerequisites.
  - `ring` 0.17.14, the default crypto backend of `webrtc` (`crypto-ring`), already compiles C through `cc` (`Cargo.toml:252`). So the daemon needs a C compiler even without SQLite.
  - CI builds on each OS in any case, because Tauri builds each OS's installer on that OS (Windows cross-builds are experimental), and macOS signing and notarization run on macOS.
  - What round 1 had and Rust loses is Go's `CGO_ENABLED=0` cross-compilation from one host. Tauri rules that out regardless.
  - The real extra cost is compiling SQLite once per clean build, which the CI build cache removes.

### S4 (Rust). Secret store: Confirmed (decision unchanged)

- `keyring` 4.2.0 is now a thin wrapper. The API lives in `keyring-core` 1.0.1, and each platform store is its own crate: `apple-native-keyring-store` 1.0.2, `windows-native-keyring-store` 1.1.0 and `zbus-secret-service-keyring-store` 1.0.1.
- Its README advises applications to depend on `keyring-core` and the stores they need, rather than on `keyring`. v1 does that.
- The SEC1 conclusion does not depend on the language. Same-user processes can reach the OS store, and D12 keeps that exposure within the owner's trust.
- Not yet checked: which keychain access control the Apple store sets on the items it creates.

### A1 (Rust). Pi framing: Confirmed

`BufRead::read_until(b'\n')` splits only on LF, as Pi's RPC mode requires. Rust's standard library has no equivalent of Node's splitting on U+2028 and U+2029. A trailing CR can be trimmed safely, because a JSON string cannot contain a raw CR.

### A5. ACP Rust SDK: Confirmed

- `agent-client-protocol` 5.0.0 (2026-11-02, Apache-2.0, `agentclientprotocol/rust-sdk`) supports the client role.
  - `Client.v1()` starts a stable v1 connection.
  - v2 is available only behind the `unstable_protocol_v2` feature (`src/lib.rs:22–29`).
  - Its README quick start launches an agent process as a client, using the `process` feature.
- It pins `agent-client-protocol-schema` at `=1.15.1`.
- **Fast-moving majors.** A crates.io snapshot from 2026-09-18 listed 2.2.0 as the newest version; 5.0.0 followed on 2026-11-02. v1 pins the exact version and upgrades on purpose, with the adapter fixtures as the gate.

### R4. Rust WebRTC: Spike

- **Version.** `webrtc` 0.21.1 (2026-10-11, MIT or Apache-2.0) is built on the sans-I/O `rtc` 0.21.1.
  - Its README calls 0.21.x the recommended line and the run-up to a stable 1.0 API. While the version is 0.x, a minor bump may break.
  - The 0.20.0 announcement (2026-07-31) moved the older Tokio-coupled 0.17 line to bug fixes only.
- **TURN is UDP-only on our side.** `rtc-ice` 0.21.1 parses `turns:` and `?transport=tcp` URLs (`src/url/mod.rs`). The async crate's TURN relayer then skips them: "Skipping unsupported secure TURN url" and "Skipping unsupported non-UDP TURN url" (`src/peer_connection/transport/turn_relayer.rs:268–278`).
  - Browsers still use TURN over TCP and TLS, so a phone on a restricted network is unaffected.
  - A computer whose network blocks outbound UDP gets no relay candidate. v0 documents TURN over TLS for restricted networks (`docs/REMOTE_ACCESS.md:175`).
- **v0 tuning to carry over.**
  - v0 sets the SCTP maximum message size and receive buffer, STUN gathering and ICE timeouts, and loopback candidates (`relay/internal/remote/rtc.go:42`).
  - It frames DataChannel messages in chunks with a replay check (`rtc.go:21–25`).
  - Each needs an equivalent in the spike.
- **Spike (M0).**
  - Chrome, Safari and Firefox connect to a Rust daemon directly and through coturn over UDP.
  - The v0 framing round-trips at the maximum frame size.
  - A UDP-blocked computer network is recorded as a known failure.
  - If that failure matters in practice, the D3 relay route is the fallback.
- **Compiler requirement found by the spike.** `webrtc` 0.21.1 does not build on Rust 1.89. Its dependency `rtc-mdns` 0.21.1 calls `Ipv4Addr::from_octets` (`src/proto/mod.rs:669`), which is stable only from Rust 1.91.0, and the crate declares no `rust-version`. Cargo's MSRV-aware resolver therefore cannot catch it. Only compiling with the pinned toolchain in CI does, so the main toolchain moves to a CI-verified version of 1.91 or later (architecture §13).
- **First CI run (2026-11-03, workflow run 41733873054 at `9b777fb`; partial, not acceptance).** Setup: `spikes/webrtc`, Linux netns lab and macOS private loopback lab, PCD1 maximum frame of 262,144 bytes in 17 packets, negotiated SCTP message size 16,384.
  | Browser | A direct | B UDP relay (both sides relay-only) | C / D |
  |---|---|---|---|
  | Chrome 156, Linux | Passed: browser prflx, Rust host, UDP | Frame echo passed with relay on both sides and 2 Rust relay candidates; the run then failed in a later B check | Not reached |
  | Firefox, Linux | Failed: browser candidate gathering timed out | Not reached | Not reached |
  | Safari 26.6.2, macOS | Passed: browser prflx, Rust host, UDP | Failed: maximum-frame echo timed out | Not run on macOS |
  - Real Safari, not Playwright WebKit, interoperates with the Rust stack directly.
- **Third CI run (workflow run 41740389496 at `4cea479`; still not acceptance).**
  - **Chrome 156 on Linux passed every case, A to D** (`R4_OK cases=8`, `natCovered=false`). The full 262,144-byte frame and the Unicode echo returned the same hash.
  - **Case C measured the known gap.** With all UDP blocked in the Rust namespace, the Rust peer gathered no relay candidates and the data channel never opened. This confirms by measurement that a computer whose UDP is blocked has no route (architecture §7, D3).
  - **Safari 26.6.2 on macOS.** A passes. B opens the data channel over UDP relay on both sides and the Unicode echo succeeds, but the maximum frame stops at inbound packet 8 of 17 (`bufferedAmount` 144,076). Chrome passes B on Linux with the same Rust code, so the cause is either Safari or the macOS loopback relay fixture. A Chrome run on the same macOS fixture would separate the two.
  - **Firefox on Linux** gathers no candidates (`iceGatheringState` stays `new`). The working hypothesis, unverified, is that Firefox uses only the default-route interface when the page has no `getUserMedia` permission, and the lab namespace has no default route. The next run adds an internal default route in the lab namespace only, under the guards agreed in the development room.
  - NAT traversal is not covered (planned case E). Nothing here exercises production TURN or cloud bootstrap.
- **Fifth CI run (workflow run 41744160624 at `404d5b6`; still not acceptance).**
  - **Firefox 158 on Linux passed every case, A to D** (`R4_OK cases=8`) once the lab namespace had an internal default route. This supports the default-route hypothesis.
  - **Relay results are intermittent.** Chrome on Linux passed B in run 3 but failed B and D in run 5: in B the Rust peer received and re-sent the full frame but the browser got only 2 of its packets; in D the Rust peer received only 8 packets. Chrome on macOS failed B in run 4 and passed it in run 5. Safari's B has failed in every run so far.
  - **No kernel drops.** In every case, both namespaces report zero UDP `InErrors`, `RcvbufErrors` and `SndbufErrors`, and the coturn configuration sets no bandwidth limit.
  - **Current reading (unverified).** The stall is intermittent, appears only on relay paths, varies in direction and causes no kernel drops. That points to user-space loss or flow-control stalls on the relay path: the Rust TURN client, SCTP packet sizing under TURN overhead, or coturn forwarding. It does not point to one browser.
  - **Next diagnostic.** Browser-only relay versus Rust-only relay, each repeated about ten times with success rates; SCTP counters on the Rust side and per-allocation counters on coturn.
  - **Acceptance rule.** B and D must pass repeatedly per browser, not once. If the relay path of `webrtc` 0.21.1 proves unreliable, R4 records that, and the WebRTC stack choice (D15) or the transport route (D3) goes back to the owner.
- **Seventh CI run (workflow run 41746234230 at `6d5256e`; 64 KiB send high-water mark, 10 attempts per relay configuration).**
  | Combination | B both relay | B1 browser relay only | B2 Rust relay only | D restored |
  |---|---|---|---|---|
  | Chrome, Linux | 10/10 | 10/10 | 0/10 | 10/10 |
  | Chrome, macOS | 10/10 | 10/10 | 0/10 | — |
  | Safari, macOS | 3/10 | 10/10 | 0/10 | — |
  - **Bounded backpressure is supported, not proven.** With it, Chrome passed 30 of 30 relay attempts across both OSes; before it, about half of Chrome's single relay attempts stalled. That supports bounded per-packet backpressure as the candidate rule for the v2 data-channel mapping, but it does not prove the cause. Safari's both-relay case still fails 7 of 10 times, with the Rust side reporting `PACKET_SEQUENCE_OR_TIMEOUT` while reassembling the maximum frame.
  - **B2 is not yet valid evidence against the Rust TURN client.** The likely cause, still unverified, is mDNS.
    - With no `getUserMedia` permission, browsers hide host candidates behind `.local` mDNS names. In B2 the browser has no STUN or TURN server, so those names are its only candidates.
    - The browser's checks arrive at coturn, which forwards only from peers that have a permission. Rust can create that permission only from the browser's real IP. If it cannot resolve the `.local` name (multicast in the netns or on loopback), every check is dropped.
    - Direct case A works because the Rust peer learns the browser's address as peer-reflexive from incoming checks.
    - The fixture itself is a second factor: coturn shares the browser's IP on Linux and is loopback on macOS.
    - Fix: give the browser a STUN-only server on a separate IP, so that it emits non-mDNS candidates, as a real phone would. Record mDNS resolution and permission counts on the Rust side and permission drops on coturn.
  - Safari's both-relay result (3/10, against 10/10 for Chrome on the same fixture) is specific to that combination and is followed up separately. Firefox did not run this time because of a harness failure (`webdriver_start`).
- **Eighth CI run (workflow run 41748734723 at `2b2d0a4`, diagnostics only).**
  | Combination | B | B1 | B2 | D |
  |---|---|---|---|---|
  | Chrome, Linux | 10/10 | 10/10 | 0/10 | 10/10 |
  | Firefox, Linux | 9/10 | 10/10 | 6/10 | 9/10 |
  | Chrome, macOS | 9/10 | 10/10 | 0/10 | — |
  | Safari, macOS | 3/10 | 8/10 | 0/10 | — |
  - **The B2 mDNS reading is confirmed.**
    - Chrome's offer held a single `.local` candidate, and it registered no remote candidates or pairs.
    - Firefox offered two non-mDNS candidates, saw the Rust relay candidate, connected and passed 6 of 10.
    - So the Rust TURN client creates permissions and carries a relay path whenever the peer exposes a routable address. Chrome's and Safari's 0/10 come from this fixture (mDNS plus the shared IP), not from a Rust TURN defect.
  - **An intermittent maximum-frame stall remains on relay paths for every browser.** Safari's B1 also fails, and there the Rust peer does not use TURN, so the stall is not explained by the Rust TURN client alone. The remaining suspects are coturn's forwarding path and the Rust SCTP/DTLS stack.
  - **Next controls.** Repeat A ten times as a direct baseline. Run browser-to-browser through the same coturn with the same PCD1 frame, which swaps out the Rust peer: if browsers never stall, the Rust stack is implicated; if they do, coturn or the fixture is.

### R5. v0 identity import in Rust: Confirmed

- **What v0 writes.**
  - The room private key is SEC1 DER (`x509.MarshalECPrivateKey`, `room_identity.go:98`), encoded as base64url without padding (`:31`).
  - Public keys are 65-byte uncompressed points (`:57–63`).
  - Signatures are raw 64-byte r‖s over SHA-256 (`:65–71`). Browser account keys use the same scheme (`room_account.go:51–64`).
- **Rust support.** `p256` 0.14.0 depends on `elliptic-curve` 0.14.1 with `sec1`, which provides `SecretKey::from_sec1_der` (`src/secret_key.rs:266`). ECDSA is a default feature. PBKDF2-SHA256 comes from `pbkdf2` 0.13.0.
- **Tests.** Import tests use identity files and signatures produced by the v0 Go code.

### Other Rust tooling

- **Process trees.** Runner's Rust daemon owns agent trees through Job Objects on Windows. It starts itself with a new process group and job breakaway (`docs/arch/windows.md:103–105`), which matches architecture §5.1.
- **TS types.** `ts-rs` 12.0.1 generates TypeScript from Rust types.
- **Unchanged by D15.** These items do not depend on the daemon's language:
  - T1–T5 (Tauri);
  - A2–A4 (the agent interfaces are process protocols);
  - R1–R3 (facts about v0);
  - C1 (the cloud stays as it is);
  - SEC2.

## Sources

- v0: `cafeshop-ai/pi-packages` `main` @ `3294487` — `relay/internal/{remote,auth,collaboration,managed}`, `scripts/remote/*`, `docs/*`.
- Runner (`yicheng47/runner` @ `9094032`): `docs/features/archive/645-session-host.md`, `docs/arch/process-model.md`, `docs/arch/arch.md`.
- Tauri: [plugins-workspace shell `lib.rs`](https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/shell/src/lib.rs); [Windows installer guide](https://v2.tauri.app/distribute/windows-installer/); [config reference](https://tauri.app/reference/config/); [localhost plugin](https://v2.tauri.app/plugin/localhost/); [CVE-2026-42184](https://nvd.nist.gov/vuln/detail/cve-2026-42184).
- Round 2 (Rust): crate sources from `static.crates.io`: `tokio` 1.54.1, `keyring` 4.2.0, `service-manager` 0.11.0, `webrtc` 0.21.1, `rtc` 0.21.1, `rtc-ice` 0.21.1, `agent-client-protocol` 5.0.0, `rusqlite` 0.41.1, `p256` 0.14.0, `elliptic-curve` 0.14.1. Versions from the crates.io API. [webrtc v0.20.0 announcement](https://webrtc.rs/blog/2026/07/31/announcing-webrtc-v0.20.0.html). Runner `crates/runner-daemon/Cargo.toml`, `crates/runner-daemon/src/ipc.rs`, `docs/arch/linux.md`, `docs/arch/windows.md`. v0 `relay/internal/remote/{room_identity,room_account,rtc}.go`, `docs/REMOTE_ACCESS.md`.
- Go libraries (round 1): [zalando/go-keyring `keyring_darwin.go`](https://github.com/zalando/go-keyring/blob/master/keyring_darwin.go); [kardianos/service](https://github.com/kardianos/service); [microsoft/go-winio `pipe.go`](https://github.com/microsoft/go-winio/blob/main/pipe.go); `proxy.golang.org` for versions.
- Agents: [Pi `docs/rpc.md`, `docs/cli.md`, `docs/rpc-extension-ui.md`](https://github.com/earendil-works/pi/tree/main/packages/coding-agent/docs); [Codex app-server README](https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md); [codex-acp README](https://github.com/agentclientprotocol/codex-acp); [claude-agent-acp README](https://github.com/agentclientprotocol/claude-agent-acp); [ACP schema meta and CHANGELOG](https://github.com/agentclientprotocol/agent-client-protocol/tree/main/schema); the npm registry for versions and deprecations.

## Next round (not yet verified)

- T3 spike, run on macOS, Windows and Linux.
- Claude adapter auth and resume probe.
- `rusqlite` with bundled SQLite, built on every CI target.
- Rust WebRTC spike (R4), including a computer network that blocks UDP.
- Keychain access control on items created by `apple-native-keyring-store`.
- ACP SDK upgrade policy: which fixtures gate a major-version bump.
- Codex app-server attach on Windows: v0's attach was Windows-only, and the macOS/Linux socket paths are untested.
- Tauri updater signing-key custody, plus macOS notarization of a bundled second binary.
- Measuring WebRTC connection-failure rates (D3).
