# M1-5: Codex ACP adapter

Status: **in progress**. The private stable-v1 SDK connection and owned synthetic-process boundary are implemented. Codex is not registered or selectable in the production Host yet. This is not M1-5 acceptance.

## Implemented boundary

- Runtime target `@agentclientprotocol/codex-acp` 2.5.0; exact official Rust SDK `agent-client-protocol = 5.0.0`, without `process`, `stdio` or unstable features. The daemon remains the only process owner. This component never authenticates, installs software, serves filesystem/terminal requests or starts an agent.
- Correlated initialize/new/load, prompt and cancel; exact native identity; no new-session fallback after a failed load. Only the prompt response settles a turn. Cancellation closes outstanding permissions and waits for that response; a lost response, abandoned prompt future or failed cancellation is unknown, never replayed.
- Permission decisions select a unique native option ID by kind. `allow_always` is not treated as session-only permission. New permission requests after cancellation are cancelled without displaying a fresh approval.
- Stable model/effort configuration uses advertised `session/set_config_option` selectors (including grouped options), validates the returned current value, and preserves later config notifications. Modes use `session/set_mode`. Unknown/ambiguous choices are rejected; one malformed optional config does not discard valid siblings.
- LF-only records, strict UTF-8, 1 MiB pre-allocation read bound, 64 KiB commands, eight-item transport/event queues, 32 pending permissions and 30-second request/cancel deadlines. The bridge bounds the SDK incoming channel and stops reading input while its output is backpressured. Partial writes are unknown. Native SDK tracing is disabled within the connection future, without changing the host subscriber, so native IDs and third-party error data cannot enter host diagnostics.
- `fixtures/acp/contract.json` is shared by Rust and TS tests. `test/fake-acp.mjs` uses no agent package, network or credentials. The managed harness runs it inside the existing Windows Job / POSIX supervisor ownership boundary, with real bounded pipes, and verifies cleanup.

## Verification

```sh
cargo test --locked -p cafe-space-adapters --lib acp::
cargo test --locked -p cafe-spaced --test managed -- --acp-fixture
pnpm --dir web exec vitest run test/acp-fixtures.test.ts
```

The SDK callback is boxed once at the connection boundary to avoid recompiling its full generic driver for each Host/test callback. After this change the expanded local test compilation fell from exceeding a 120-second bound to about 16 seconds. This is build-cost evidence, not a runtime performance claim.

## Required before production admission

1. Explicit local runtime registration and passive pin validation; no downloaded or implicitly launched agent. Preserve private native identities transactionally through Host New/Resume and crash recovery.
2. Normalize native updates/tool/plan/content into bounded shared observations, including unknown/omitted content. Native SDK types and IDs never become public wire fields. Add the bounded Host-owned Queue (not Steer), fencing, truthful operation outcomes and stopped-session retention.
3. Integrate model/mode controls and Queue explanation into CLI/UI; cover real IPC, replay/gap, update handover and native WebView with the synthetic agent.
4. Run final-SHA platform CI and one separately authorized real Codex smoke. The earlier Pi smoke authorization is consumed and does not authorize Codex. No real Codex session has been started by this stage.

Full daemon wire stays 7, SQLite schema stays 6 and frozen control stays 2. The remaining M1 gates and the M2–M4 roadmap are still part of the overall project objective.
