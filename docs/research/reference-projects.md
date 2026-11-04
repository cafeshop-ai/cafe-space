# Reference projects

> Date: 2026-11-04. These are public projects we read for design input; nothing here is copied code. Each claim cites the project's own repository at the commit given.
> Earlier reference: Runner (`yicheng47/runner` @ `9094032`), already cited throughout the [architecture](../architecture.md).

| Project | What it is | Stack | License | Commit read |
|---|---|---|---|---|
| [gonggong-space](https://github.com/yoqu/gonggong-space) | Self-hosted team collaboration. Each member connects a machine, and the Claude Code or Codex on it becomes a Bot that the group can @ | Rust daemon (`gg`), Tauri desktop app (macOS), Fastify + PostgreSQL server, React web | Apache-2.0 | `724c1d8` |
| [cospace](https://github.com/JingxuanKang/cospace) | A Mac host runs one Linux VM per "space". Guests reach it with plain `ssh` and use the host's AI quota through a gateway | Go, embedded Tailscale library (tailcat), Apple container or Docker | MIT | `9a580d9` |

## gonggong-space

The closest product to Café Space's M1 and M3: a Rust daemon on each machine drives agents over ACP, and people and bots meet in group chats. Its architecture guide is `website/guide/architecture.md`.

**How it is built**
- **Outbound connections only.** The daemon connects out to the server over WSS (`/ws/daemon` for tasks and events, `/ws/daemon/tunnel` for previews), so no member machine opens a port. The server holds accounts, groups, messages, run cards and audit records, and it stores the free-text run output and diffs encrypted.
- **Agents.** Agents run through pinned ACP adapters (`@agentclientprotocol/claude-agent-acp`, `@agentclientprotocol/codex-acp`), which are installed on first run into `~/.gonggong/adapters/` from the npmmirror registry by default. Each group × bot pair gets one adapter process and one ACP session, and an adapter that is idle for 10 minutes is reclaimed.
- **Built-in MCP.** Each session gets a loopback MCP server named `gonggong`. Through it, the agent can read and search the group's history, ask members a question and wait for the answer, publish previews (web services, static pages, desktop windows, mini-programs) and hand a task to another bot.
- **Permissions.** There are three tiers: read-only, workspace write and full access. Only the bot's owner can approve an operation beyond the tier. Who may trigger a bot is set to the owner only, a named list or any group member.
- **Previews.** Through the daemon tunnel, a local dev server on the member's machine opens for the whole group, with time-limited public links.
- **Daemon crate.** `crates/gonggong` is about 40k lines and depends on `agent-client-protocol` 2.2 with several `unstable_*` features, `rusqlite` 0.40 (bundled), `tokio-tungstenite` and `reqwest` with rustls.

**For Café Space**
- **M3, agent tools.** A loopback MCP server per bound session could sit beside the `cafe-space` CLI and skill (architecture §6). It would give MCP-capable agents structured room tools: read, post, ask, done. Both would enforce the same binding policy.
- **M1-5, adapter installation.** The architecture does not yet say how adapters are installed. gonggong-space pins versions, installs into a private directory and offers a registry mirror, which matters for users in mainland China.
- **M3, resource policy.** One adapter process per binding, with an idle timeout.
- **§8.1 and the future Café-login mode.** Permission tiers, owner-only approval of anything beyond the tier, and per-bot trigger scope are a concrete vocabulary for who may make an agent act.
- **Later, not v1.** A preview tunnel to share a local dev server with the owner's other devices or a room.
- **Do not copy.** Its daemon accepts any server certificate without pinning (`website/guide/architecture.md`, "连接与认证"). That conflicts with architecture §11.

## cospace

A different shape: compute sharing on the host's machine, with an isolated VM per space, ssh access and a credential gateway. Its design document is `DESIGN.md`.

**How it is built**
- **Transport.** tailcat embeds the Tailscale library, with no account and no server run by either side. It uses WireGuard P2P when hole punching works and falls back to Tailscale's public DERP relays, which are rate-limited and come with no SLA, so they are not the only route. An optional bring-your-own-VPS relay forwards encrypted bytes only; the host dials out to it with yamux multiplexing.
- **Credential gateway.**
  - Real provider credentials stay in the host's gateway process. Each space gets its own fake token plus a base URL pointing at the gateway, which swaps in the real token.
  - Revoking a space takes effect at once, and requests are metered on the request path, down to parsing SSE token usage.
  - Every space has a dollar cap and a concurrency cap; over-limit requests get a clean 429.
- **Isolation.** One Linux VM per space (Apple container) or one container (Docker). A space never sees the host's home directory, real credentials, other spaces or the host's git and ssh identities.
- **Pairing.** One-time codes are stored only as hashes. The guest's public key is recorded on the host, and each space's host key is pinned in a dedicated `known_hosts`.
- **Console.** It rejects cross-site writes and unexpected `Host` headers.

**For Café Space**
- **D3, transport.** Café Space's remote clients are browsers, which cannot run WireGuard or Tailscale, so tailcat's P2P path does not transfer directly. The relevant pattern is the dumb encrypted relay: end-to-end encrypted bytes over an outbound connection from the computer. That is the same family as D3's 443 WebSocket relay candidate, which is still unimplemented (research R4).
- **D10, budgets.** Metering at a gateway on the request path, instead of trusting adapter-reported usage, would make cost caps enforceable.
- **The future Café-login mode** (D12, SEC1). If other people may start sessions, they must be isolated. A VM or container per person, plus a credential gateway, is a worked example of that isolation.
- **D14.** Hashed one-time codes and key pinning match the new-device approval design.
- **Caution.** cospace lets guests use the host's subscription quota. Before Café Space ever shares one person's provider access with others, check the provider's terms.

## Summary

Neither project changes a v1 decision. gonggong-space independently made the same core choices as D15: a Rust daemon, Tauri, and agents over ACP. The items above feed M1-5 (adapter installation), M3 (MCP tools, binding permissions, resource policy), D3 (relay design), D10 (metering) and the future Café-login mode (isolation).
