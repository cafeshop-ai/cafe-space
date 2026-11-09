# Café Space — product positioning (draft)

> Status: **draft for the owner's review**, 2026-11-04. Built from the owner's answers in the product discussion and from the [reference projects](../research/reference-projects.md). Product rules P1–P9 and decisions D1–D16 live in the [architecture](../architecture.md).

## Who it is for

**Individual developers and small teams** who use more than one coding agent: Pi, Codex, Claude Code and others.

## One line

**An open room where your agents and your teammates' agents, from any vendor and on any machine, work together, while your own computer stays yours.**

## Why not the official tools

Agent vendors will ship remote control and team features for their own agents, behind their own accounts. That is not where Café Space competes. It competes where a single vendor structurally will not go:

1. **Vendor-neutral by construction.**
   - Pi, Codex, Claude Code and any ACP agent run side by side, and can work in one room on one Goal.
   - Each agent signs in its own way: an official login, an API key, a regional model through Pi, or a local model.
   - The space itself needs no vendor account (P4, D12).
2. **Local-first and self-hostable.**
   - Code and credentials stay on the owner's computer.
   - Remote access is end-to-end encrypted, so the cloud cannot read device traffic.
   - The cloud is Café-hosted by default and self-hostable (P9).
3. **Truthful automation.**
   - Delivery states are never merged (fetched → queued → delivered → handled).
   - An unknown outcome is never retried.
   - Goal-driven autonomy has budgets and hard stops, and a person accepts the result.
   - When several vendors' agents act on their own, someone has to own "who made it act, what it cost, when it must stop".

## Why not the open-source alternatives

| Kind | Examples | Café Space's difference |
|---|---|---|
| Personal remote clients | Happy, Runner | One daemon covers both personal remote use and multi-person rooms (P6). Agents connect through structured interfaces (ACP, Pi RPC, Codex app-server), not by wrapping a CLI and reading its screen |
| Local orchestration boards | Vibe Kanban, codeg | Work spans machines and people, not one computer |
| Team platforms with a central server | gonggong-space, Multica, Clay | **The room protocol is open.** Any agent joins with plain HTTPS JSON, with no daemon or server of ours to install. Device traffic is end-to-end encrypted; no account is needed in v1 |
| Compute sharing | cospace | Café Space is about agents collaborating, not sharing one machine's quota |

Café Space also treats **Windows and Chinese text as first-class**: native CI on four platforms from M0, and lossless text through Windows shells and code page 936 (architecture §6). Many alternatives are Mac-first.

**Honest gaps.** The alternatives ship features today: mobile apps, previews, worktrees. Café Space is at M0. Room content is plaintext in the cloud, with disclosure, while some alternatives encrypt everything.

## What this means for the roadmap

| Area | Direction |
|---|---|
| **The room protocol as a product** | A versioned spec, conformance tests, small TS, Python and Rust clients, and a loopback MCP server for bound sessions (M3; parts can come earlier) |
| **Adapter breadth** | A generic ACP adapter and the agents common among Chinese developers early, with a registry-mirror option for installing adapters (M1-5) |
| **Parallel agents** | An optional git worktree per session (M1-2 or M3) |
| **Remote access** | Solid, not flashy: a browser path with WebRTC plus a 443 encrypted relay (D3), plus iOS and Android apps (M6) that never replace the browser |
| **Self-hosting as a product** | A one-command cloud deployment and its documentation, not only a configurable address |
| **Team mode (v2)** | Uses the Café account, so team features never need a vendor account. Others who start work on my computer are isolated (P7) |
| **End-to-end encrypted rooms** | A candidate after v1, weighed against server-side search and Goal state |

## The Café ecosystem

Café Space is open source and stands on its own. **Every feature works without any other Café service.** The Café services are optional integrations:

- **Café account.** Single sign-on for the v2 team mode and for hosted features. v1 needs no account (P4).
- **Café model gateway (cafeshop.ai).** One optional, built-in model provider among others. It would:
  - offer one-click setup for agents that accept an OpenAI- or Anthropic-compatible endpoint;
  - give each bound session its own budget-limited key, so that the owner's main key never reaches an agent (SEC1), and spend is metered and capped at the gateway, which D10 needs.

  This needs a key-issuing API on the gateway side.
- **Boundary.** Café Space contains no provider-specific account handling. Users always choose their provider explicitly, and their own official subscriptions are never routed through a third party without explicit consent.

## License (D16)

AGPL-3.0, with a commercial license on offer. A trademark policy protects the name and marks. See D16 in the architecture for what must happen before the LICENSE file is added.
