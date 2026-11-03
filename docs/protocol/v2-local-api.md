# Local API protocol v2 (draft)

Status: M0 draft. Rust types in `crates/protocol` are authoritative; `web/src/protocol/generated/` is produced by `ts-rs`. Run `cargo test -p cafe-space-protocol --locked` to regenerate those bindings; run `pnpm --dir web generate:protocol-fixtures` after editing examples. Rust tests validate the JSON examples in `fixtures/v2/`; TypeScript checks the same examples against the generated types. This daemon API is shared by desktop UI, CLI and remote browsers; it is separate from the legacy v0 host protocol v1. Cloud collaboration HTTP v1 and device-signaling contracts remain unchanged.

## Framing and validation

- Every transport exchanges the same logical frame: one UTF-8 JSON object, bounded by `MAX_FRAME_BYTES`. WebSocket text messages provide frame boundaries; Unix sockets, Windows named pipes and CLI IPC use a four-byte big-endian byte length. Binary WebSocket messages are rejected. Transport delimiters do not change the JSON payload or its byte limit.
- A frame is 1–262,144 bytes. Reject zero-length or oversized frames, invalid UTF-8/JSON, a second JSON value, duplicate fields, unknown fields and unsupported versions. Batches are not supported.
- Field names are `camelCase`; discriminator values are `snake_case`. IDs are non-empty, control-character-free and at most 128 UTF-8 bytes. Labels are at most 256 bytes; text is at most 65,536 bytes. Lists contain at most 1,000 entries, session views at most 1,000 messages and 64 approvals, and each message at most 500 parts. The encoded frame limit still applies to the whole message.
- Protocol types carry IDs and opaque fences, never client-supplied paths, shell commands, environment variables or native agent IDs. `cwdFence` and attach-target keys are not authorization credentials; the daemon must validate every operation independently. Capabilities only affect presentation, never authorization.

## Version handshake

Once a client has selected this API, each connection starts with a client hello before any request:

```json
{"type":"hello","supportedProtocolVersions":[2],"clientVersion":"0.1.0-dev"}
```

If a common version exists, the daemon replies with `type: "welcome"` and the selected `protocolVersion`, then accepts requests for that version. The daemon selects the highest common version. With no common version it sends `type: "version_mismatch"` and its `supportedProtocolVersions`, then closes the connection. A client or server supports 1–8 unique, nonzero versions in its offer; this daemon implements version 2 only. The `type: "request"`, `"response"` and `"event"` messages each carry `protocolVersion: 2`. The connection handler, not the stateless JSON codec, enforces that hello/welcome happens first.

Before opening a data channel, the remote compatibility layer reads the gateway's advertised host-protocol version. A v0 gateway selects legacy protocol v1 and must never receive a v2 hello. `version_mismatch` applies only after both peers implement this handshake.

## Requests and fencing

A request is `{ "type":"request", "protocolVersion":2, "requestId":"…", "request": { "method":"…", "params":{…} } }`. Unit methods omit `params`. Responses echo `requestId`; events have their own stream sequence.

| Method | Params | Result / semantics |
|---|---|---|
| `get_status` | — | Daemon health and readiness. |
| `list_projects` | — | Approved project IDs, labels and `cwdFence`; no paths. |
| `list_runtimes` | — | Runtime profiles and availability. |
| `list_sessions` | — | Session summaries and an event cursor. |
| `subscribe_events` | optional `{ after: { eventStreamId, sequence } }` | Starts event delivery. Send an empty `params` object for a fresh snapshot; a valid cursor resumes. |
| `list_attach_targets` | `projectId`, `runtimeId` | Opaque attach keys; native IDs stay local. The target includes its `cwdFence`. |
| `get_session` | `sessionId` | Session view, bounded transcript/approvals and an event cursor. |
| `create_session` | client-chosen `sessionId`, `projectId`, `runtimeId`, `expectedCwdFence`, `start` | `start` is `{ "mode":"new" }`, `{ "mode":"resume", "params":{"sourceSessionId":"…"} }`, or `{ "mode":"attach", "params":{"targetKey":"…"} }`. Reusing the same `sessionId` is idempotent and reconciles a lost create response; it must not create a second session. |
| `open_session` | `sessionId`, `expectedStreamId`, `expectedCwdFence` | Idempotently opens/reconnects and returns the current view; stale fences reject it. |
| `set_model` | `sessionId`, `expectedStreamId`, `expectedCwdFence`, `provider` (≤128 bytes), `modelId` (≤256 bytes) | Changes the model when the adapter supports it; the daemon enforces capabilities and authorization. |
| `set_thinking` | `sessionId`, `expectedStreamId`, `expectedCwdFence`, `level` | `level` is `off`, `minimal`, `low`, `medium`, `high`, `xhigh` or `max`; the daemon checks adapter support. |
| `prompt` | `sessionId`, `expectedStreamId`, `expectedCwdFence`, `content`, `delivery` | `delivery` is `steer` or `follow_up`; a dispatched result is not completion. |
| `cancel` | `sessionId`, `expectedStreamId`, `expectedCwdFence`, `runId` | Cancels only the named run on that session; authorization is enforced by the daemon. |
| `respond_to_approval` | `sessionId`, `expectedStreamId`, `expectedCwdFence`, `approvalId`, `decision` | `decision` is `allow_once` or `deny`. |
| `close_session` | `sessionId`, `expectedStreamId`, `expectedCwdFence` | Idempotently closes an owned session or detaches an attached one. |

`cwdFence` is an opaque per-project generation, rotated when its approved root changes. It is a stale-context check, not an authorization token. Create requests are fenced by the client-chosen session ID and the current project `cwdFence`. Operations against an existing session carry its `sessionId`, expected stream and expected cwd fence. The daemon rejects stale or mismatched fences before acting. Session summaries carry mode (`new`, `resume`, `attach`), state (`starting`, `ready`, `running`, `waiting_for_local_ui`, `stopping`, `stopped`, or `failed` with a code), the current cwd fence and stream ID, optional active run ID, current model/thinking level/title, and presentation-only capability flags (`modelControl`, `thinkingControl`, `deliveryControl`, `sessionControl`). Clients use the active run ID for `cancel`; agent-native session IDs never leave the daemon.

## Results

A response has exactly one status: `applied`, `rejected`, `dispatched` or `unknown`.

- `applied`: the operation finished and includes its typed result.
- `rejected`: the operation was not applied; includes a stable error code and display-safe message.
- `dispatched`: accepted for asynchronous work and includes an `operationId`; this is not a completion signal.
- `unknown`: the operation may have run, but its outcome cannot be proven. **Do not retry the command.** Reconcile by reading state; for creates, query the same client-chosen `sessionId` with `get_session` or `list_sessions`.

## Events, resume and snapshots

Events carry a daemon `eventStreamId` and monotonically increasing `sequence` from 1 to 1,000,000,000. A cursor may use sequence 0. The daemon rotates to a new `eventStreamId` before the sequence limit, and also on restart. Events cover session changes/removal, transcript messages, approval requests/resolutions and notices.

`subscribe_events` atomically installs the live subscription and chooses one of two responses:

- `resumed`, with no snapshot, when `after` names the current stream and the daemon can replay every event after that cursor. The response's `(eventStreamId, sequence)` is the replay high-water mark; the daemon replays strictly increasing events through it, up to `MAX_REPLAY_EVENTS` (4,096), then continues live.
- `snapshot_required`, with a full snapshot, when no cursor was supplied, the stream changed, the cursor is ahead, or the missing range is unavailable/too large. The response's `(eventStreamId, sequence)` is the snapshot cursor. The subscription stays live; the daemon delivers later events strictly after that cursor.

The subscription response precedes replay/live event messages. The daemon installs the live queue before capturing either a snapshot cursor or a replay high-water mark. It sends the response, replays through that high-water mark when resuming, then delivers queued/live events; snapshot mode delivers events newer than its snapshot cursor. Clients apply only events newer than their last applied cursor; on a sequence gap or disconnect they subscribe again from that cursor. `get_session` and `list_sessions` also return their snapshot cursor for clients that fetch those views while subscribed. Model and thinking changes update the session snapshot and arrive as `session_changed` events.

A snapshot contains project, runtime and session summaries; transcript history is fetched through `get_session`. A single response or snapshot that cannot fit the frame limit is rejected with a stable size error. History may be truncated to fit and reports `historyTruncated`; pagination is deferred until adapter histories establish cursor semantics.

## Scope

This draft defines wire types, codecs and validation—not transport handlers, access-token checks, persistence or session execution. The loopback WebSocket still requires the host allowlist, exact Origin and per-install token described in the architecture; owner-only socket/pipe permissions remain transport responsibilities. Any implementation change to these semantics updates this document, Rust tests, generated TypeScript and the v2 examples together.
