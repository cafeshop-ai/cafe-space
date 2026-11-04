# M1-4: Pi RPC adapter

Status: offline implementation in progress, not accepted. #28 process ownership remains awaiting review; this work does not change its acceptance status. No real Pi process or model call is authorized by this plan.

## Pinned boundary

Use `@earendil-works/pi-coding-agent` **1.5.0**, Node **>=22.19.0**, and `dist/bundle/cli.js`. The installed 1.5.0 package metadata and `docs/{cli,rpc,rpc-commands,rpc-extension-ui,json}.md` are the version-specific reference. Passive inspection checks the package name, exact version, entry path and Node executable without executing Pi, reading credentials or probing authentication. No npm install or automatic update is added.

[Contract](../../fixtures/pi-rpc/contract.json) and [synthetic traces](../../fixtures/pi-rpc/traces.json) land before adapter code. Rust and TypeScript consume the same offline fixtures. This stage adds the adapter library and a fake process exercised through M1-3 ownership, not a runnable production profile.

## Launch and observation rules

- Invoke the absolute Node executable directly with the canonical CLI path, not a shell or Windows `.cmd` shim. Match v0's `--mode rpc --offline --no-extensions --no-approve --no-themes --session-dir ... --session-id ...`. No v0 collaboration extension, room tokens or ambient API keys are forwarded. M3 is out of scope.
- Host must provide an owner-only session directory, private native ID and the approved cwd identity, and own the resulting process. Resume uses the same native ID and storage directory; missing identity/history is an error, never a new conversation disguised as Resume. No automatic replay or `--continue`.
- Startup is confirmed by a correlated `get_state` response with the expected private native ID and idle flags. The startup query is not a prompt. Once a run starts, only `agent_settled` is ready; `agent_end`, successful prompt responses and idle-looking intermediate queries do not complete a run. A `handled` prompt starts no run. This distinguishes initial readiness from run completion, because Pi emits no startup `agent_settled`.
- Consume strict LF-framed JSON objects continuously, accept CRLF, preserve U+2028/U+2029 and UTF-8 across read boundaries, and reject invalid, incomplete or oversized records with fixed codes. Limit commands, dialogs and assembled messages. Drain stderr separately without treating it as protocol or sharing raw diagnostics.
- Correlate responses by unique ID and command, not order. A successful prompt/steer/follow-up response is acceptance, not completion. A failed response is rejected with a fixed code, never the raw runtime error. EOF, write failure or a deadline leaves an unacknowledged command unknown; never retry automatically. Host owns deadlines, I/O scheduling, fences and caller/run ownership checks.
- A `confirm` dialog maps to explicit Allow once / Deny. Pi has no built-in tool-policy approval: do not infer permission semantics from arbitrary option labels. Generic `select`, `input` and `editor` require `waiting_for_local_ui`; the current v2 approval decision cannot carry free-form values. `approval_for_session` is false. Writing an extension UI response is dispatched only, because Pi sends no acknowledgement.

## Delivery gates

Synthetic tests cover first turn/retry/settled, handled disposition, reordered steering/follow-up responses, confirmation/local UI, clear-queue-before-abort, resume identity, transcript/tool mapping, malformed and bounded records, and lost outcomes. Production Host integration, persistent private resume identities, full M1-6 transcript replay and an explicitly authorized real smoke remain separate unchecked gates. This note is an implementation proposal, not a new Claude-reviewed architecture decision.
