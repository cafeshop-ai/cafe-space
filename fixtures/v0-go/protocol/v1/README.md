# Frozen wire v1 baseline (R02)

Source baseline: `cf20beaf65097173b78a632a79df615b9576e5ce`, captured 2026-09-18 UTC before changing any runtime implementation. Sources are `src/protocol/index.ts`, `src/relay/server.ts`, `src/extension/{index,file-commands}.ts`, and `web/public/app.js`. Source digests and exact limits are in [limits.json](./limits.json). JSON differences are specified in [compatibility.md](./compatibility.md).

Fixtures are synthetic, contain no user session data, and are expectations derived from the unchanged TS source, then executed against it. Go must consume the same checked-in expectations, never regenerate them from its own output. Ordered parts are **not** part of this baseline; R12 adds a separate corpus.

## Field notation and direction

`S(n)` = nonempty string of at most n UTF-16 code units; `E(n)` = string including empty; `N(n)` = null or E(n). `?` = optional/absent, **not** nullable unless explicitly stated. Booleans must be JSON booleans, numbers cannot be strings. Fields below are required unless `?`. Unknown fields are accepted then discarded at known wire/schema levels, but retained within bounded result `data`.

Every frame is a JSON text message ≤262144 UTF-8 bytes (including whitespace and escaped JSON). No binary, compression or model transport. A decoded message alone does not authorize a role, room or command.

| type | Fields after `type` | Direction |
| --- | --- | --- |
| hello | protocolVersion=1, peerRole=host/client, peerId:S(128), roomId:S(128), token:S(4096) | either peer → relay, first message only |
| welcome | protocolVersion=1, connectionId:S(128), peerRole, roomId:S(128), hostConnected:bool | relay → peer |
| host_status | connected:bool, streamId:N(128), sessionId:N(256), hostId?:N(128), hosts?:HostInfo[≤64] | relay → client |
| snapshot | hostId?:S(128), snapshot:SessionSnapshot | host → relay → clients |
| event | hostId?:S(128), streamId:S(128), sessionId:S(256), seq:integer 0..1000000000, emittedAt:S(64), event:CollabEvent | host → relay → clients |
| command | requestId:S(128), expectedStreamId:S(128), expectedSessionId?:S(256), expectedCwd?:S(16384), targetHostId?:S(128), payload:CommandPayload | client → relay |
| routed_command | relayRequestId:S(128), clientRequestId:S(128), sourcePeerId:S(128), expectedStreamId:S(128), expectedSessionId?:S(256), expectedCwd?:S(16384), targetHostId?:S(128), payload | relay → selected host |
| host_command_result | relayRequestId:S(128), status, code:N(128), message:N(2048), data?:bounded JSON | host → relay |
| command_result | hostId?:S(128), requestId:S(128), status, code:N(128), message:N(2048), data?:bounded JSON | relay → originating client connection |
| error | code:S(128), message:S(2048) | relay → peer |

`status = dispatched | applied | rejected`. `dispatched/REQUEST_PENDING` is nonfinal; other dispatched results acknowledge dispatch, not completion of a model run. Relay replaces host-supplied hostId with authenticated identity. Production room ID is stricter than codec: `/^[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}$/`, no surrounding whitespace.

### Nested schemas

- ModelRef: null or `{provider:S(128), id:S(256)}`.
- TranscriptMessage: `{id:S(128), role:user|assistant|tool|system, text:E(65536), thinking:E(65536), timestamp:finite number, status:streaming|complete|error, toolName:N(256), toolCallId:N(256)}`. Negative/fractional timestamp is currently valid; not necessarily an ISO/date range.
- ToolExecution: `{toolCallId:S(256), toolName:S(256), argsText:E(65536), output:E(65536), status:running|complete|error}`.
- HostInfo: `{hostId:S(128), connected:bool, ready?:bool, streamId:N(128), sessionId:N(256), sessionName:N(256), cwd:N(16384)}`. Missing ready is legacy, not false.
- SessionSnapshot: `{protocolVersion:1, streamId:S(128), sessionId:S(256), sessionName:N(256), cwd:E(16384), activeLeafId:N(128), model:ModelRef, thinkingLevel, phase, hasPendingMessages:bool, messages:TranscriptMessage[≤1000], historyTruncated?:bool, tools:ToolExecution[≤500], lastEventSeq:integer 0..1000000000}`. Canonicalization fills absent historyTruncated=false.
- thinkingLevel: off|minimal|low|medium|high|xhigh|max. phase: idle|running|waiting_local_ui.

### All eight commands

| name | Additional fields |
| --- | --- |
| prompt | content:S(65536), delivery?:steer|followUp |
| abort | none |
| set_thinking | level:thinkingLevel |
| set_model | provider:S(128), modelId:S(256) |
| list_dir | path:E(4096) |
| read_file | path:E(4096), offset?:integer 0..100000000, limit?:integer 1..262144 |
| list_sessions | none |
| get_session | sessionId:S(256) |

Path validation and authorized project access happen in the extension, not the relay. A wire-valid path can still be rejected for security. read_file producer clamps actual bytes to 131072; wire limit is not its actual read size.

### All eleven events

| kind | Fields |
| --- | --- |
| session_state | phase, hasPendingMessages:bool |
| message_started / message_finished | message:TranscriptMessage |
| message_delta | messageId:S(128), channel:text|thinking, delta:E(65536) |
| tool_started / tool_finished | tool:ToolExecution |
| tool_updated | toolCallId:S(256), output:E(65536) |
| model_changed | model:ModelRef |
| thinking_changed | level:thinkingLevel |
| ui_wait | waiting:bool, title:N(512) |
| notice | level:info|warning|error, message:S(16384) |

Reducer first checks matching stream/session and exactly lastEventSeq+1. Message/tool upserts replace by real ID; deltas append to the chosen channel, tool_updated **replaces** output. Missing nonempty message delta marks truncation; missing empty message delta does not; missing tool_updated always marks truncation. ui_wait(false) selects running, not idle. notice only advances seq. Eviction/cropping marks historyTruncated. Pure reducer bounds and Relay snapshot compaction bounds are intentionally different.

## Relay state and failures

Client authentication outputs welcome → host_status → retained ready/offline snapshots (host insertion order). Host authentication sends welcome and publishes online/not-ready status to clients. Snapshot publishes snapshot → ready status. Same-context snapshot seq must not decrease. Duplicate/gapped/wrong-context event clears readiness, sends EVENT_SEQUENCE, closes host 1011 and publishes offline status; no event is silently skipped.

Dedupe key is JSON tuple `[peerId, requestId]`, per host. Route selection considers explicit target, then unique pending/result owner, then matching stream/session; ambiguous owners reject HOST_SELECTION_REQUIRED. Readiness is checked before retained snapshot fences, then stream/session/cwd fences before dedupe/cache. Stale fences send a rejection then a current snapshot. A same-peer browser reconnect can explicitly retry pending ID to transfer result delivery to its new connection; without retry, late reply must not go to that new socket. Host replacement rejects pending HOST_REPLACED and invalidates caches; old close/result cannot affect the replacement.

Only actual list_sessions/get_session requests populate offline history cache. get result must have matching sessionId and kind=session; list requires kind=sessions. Codec validates bounded JSON, Relay validates this minimum association, browser must validate complete result shape. Projection-changing events/snapshots invalidate history and fence pending reads; phase-only events do not. Offline history hits refresh both history and offline-host inactivity timers; file/write requests cannot use offline history cache.

Errors fixed by source/trace assertions include INVALID_MESSAGE, BINARY_UNSUPPORTED, HELLO_REQUIRED, INVALID_ROOM, UNAUTHORIZED, ROOM_LIMIT, CLIENT_LIMIT, HOST_LIMIT, STALE_HOST, STALE_CLIENT, ROLE_VIOLATION, SNAPSHOT_REQUIRED, STALE_SNAPSHOT, EVENT_SEQUENCE, INTERNAL_ERROR; command codes include HOST_NOT_FOUND, HOST_SELECTION_REQUIRED, HOST_OFFLINE, HOST_NOT_READY, STALE_STREAM, STALE_SESSION, REQUEST_PENDING, COMMAND_QUEUE_FULL, COMMAND_TOO_LARGE, HOST_TIMEOUT, HOST_REPLACED, HOST_EXPIRED, RESULT_INVALID, RESULT_TOO_LARGE. Extension errors are forwarded (e.g. PATH_NOT_ALLOWED, SENSITIVE_PATH, BINARY_FILE, SESSION_INVALID). Error code is a bounded string, not a closed enum allowing only this list.

Close codes: normal replacement 1000; shutdown 1001; binary 1003; malformed raw UTF-8 from ws 1007; invalid JSON/auth/hello/timeout 1008; oversized raw frame 1009; seq/snapshot/internal/send failure 1011; capacity/backpressure 1013. Per-socket close backstop is 1000ms. See fixtures for precise reason strings and required per-recipient output order.

## Portable fixture format and runner

Run in package: `npm.cmd run refactor:contracts:test`. Existing `npm.cmd run pi-cafe-space:test` from repo also includes this suite. Tests use only freshly bound loopback port 0 and synthetic credentials, and terminate only their own sockets/listener in finally.

- `../fixtures/v1/codec.json`: decode inputText OR inputBase64 OR inputTemplate; expected accepted/value or code. inputBase64 tests raw byte boundary with fatal UTF-8 decoding. `paddingBytes` right-pads JSON with ASCII spaces to exact byte size.
- `reducer.json`: operation=applyEvent, snapshot/envelope, exact expected output or error message. Inputs may be reducer-only in-memory boundary values, not necessarily valid wire frames.
- `budgets.json`: fitCommandResult input plus exact expected output; encode is the TS canonical decode + JSON.stringify outbound baseline (TS has no public EncodeWire function).
- `values.json`: shared immutable templates. `$ref` refers to a named template, or to a previously bound trace ID; `$merge` shallow-merges expanded objects; `$repeat:{text,count}`, `$array:{value,count}`, `$object:{value,count}` (keys k0..kN), `$nest:{value,depth}` generate bounded boundary inputs. These tags are fixture-only and never protocol fields. Template operator objects have exactly one key.
- `traces/*.json`: connect uses a unique label per socket generation; send/receive carry message templates; close/expectClose assert peer close; advance moves application timeout clock in milliseconds. Every receive compares the next FIFO message, including all keys. `$bind` captures a fresh UUID once; `$ref` reuses it thereafter. No dropping IDs or unordered subset matching. Socket listeners are registered at construction before sending; FIFO retains arrivals before the next receive step. Final ping/pong barriers then assert no extra queued messages.
- Runtime timers are fake only for deterministic traces; assertion deadlines and I/O remain real. Golden tests never run a model or access Pi JSONL.

## Session-control extension (post-baseline)

The original eight-command corpus is unchanged. `session-controls.json` adds three commands and an optional `SessionSnapshot.sessionControl: boolean`. Missing/false capability means unsupported, not implicit permission. Both codecs preserve this field; no protocol-version bump or new transport is introduced.

| name | Additional fields | Native Pi operation |
| --- | --- | --- |
| new_session | none | `ExtensionCommandContext.newSession()` |
| rename_session | title:S(256), nonblank after ECMAScript trim; reject U+0000–001F and U+007F | `pi.setSessionName(title.trim())` on the current session |
| resume_session | sessionId:S(256), opaque ID only | `ExtensionCommandContext.switchSession(validatedPath)` |

All three are authenticated writes, require an online/ready capability-advertising host and explicit matching stream/session/cwd fences, and reject running/local-UI/pending-message states with `SESSION_BUSY`. The extension rechecks native context/idle state after asynchronous history lookup. Resume reuses the bounded project/session-directory validation used for reads, then requires a nonempty matching project. Web and Relay never edit session files or create AgentSessions.

Pi's cancellable before-switch hooks remain authoritative. Cancellation returns `rejected/SESSION_CANCELLED`. Accepted new/resume commands return **dispatched**, not applied, from the old host's native shutdown hook before closing its socket; the replacement Pi extension supplies a fresh snapshot. That acknowledgement is handoff, not proof of successful replacement. Disconnect/timeout/ambiguous replies remain unknown writes and must not be auto-retried. Rename uses the existing `session_info_changed` projection path and does not rotate the stream. A one-use private command-context bridge is never populated from arbitrary client text; an input-hook fallback consumes its reserved text rather than allowing it to become a model request on incompatible runtimes.

## Input-assistance extension (post-baseline)

`input-assist.json` adds twelve shared TS/Go fixtures without altering the original corpus. `SessionSnapshot.inputAssist?: boolean` advertises command discovery/execution and file references. Absent/false means unsupported; do not silently drop references when talking to old hosts.

- `list_commands`: no additional fields. Online read; result `{kind:"commands", truncated:boolean, commands:[{name:S(128), description:E(160), source:"extension"|"prompt"|"skill"}]}` has at most 200 entries. Names contain no whitespace, slashes, backslashes or ASCII controls. The host uses `pi.getCommands()`, preserves first/native precedence for duplicate names, and excludes private `collab-*` commands. No `sourceInfo` or absolute resource paths are transmitted.
- `run_command`: `command:S(65536)` beginning with `/`; controls other than tab/LF/CR are rejected. Authenticated write, online/ready, explicit stream/session/cwd fences, idle/no-pending only. The host validates the command against its current bounded inventory before `pi.sendUserMessage(..., {expandPromptTemplates:true})`. Unknown, private and interactive-only commands reject `COMMAND_UNAVAILABLE`; accepted results are **dispatched**, never a claim that async command work has succeeded. Native local UI still needs the terminal. No automatic write retry.
- `prompt.files?: S(4096)[≤8]`: explicit project-relative UTF-8 text references. The host reuses `readProjectFile` guards (including sensitive paths, broad roots, traversal, link/junction/device names, binary/UTF-8 and descriptor identity checks), deduplicates exact paths, and rejects partial/oversized reads. Maximum 64 KiB per file and 128 KiB total. It appends bounded JSON file data to the original message, then rechecks the native context and delivery state before dispatch. Failure sends no partial prompt; references are read by Pi, not Relay or Web.

All three require the capability and explicit matching session/project fences in Relay and extension. Web completion reads use isolated transient Gateway slots and do not overwrite the file browser; cancellation/view/connection fences still apply. Slash commands and file references share no unbounded cache. The UI maps `/new`, `/name`, `/resume`, `/model` and `/thinking` to existing confirmed session controls/history/settings; these are not additional wire commands. In slash arguments `@` completes paths only; normal messages explicitly send the `files` array. `#` remains ordinary text.

## Coverage and remaining layers

All ten wire variants, eight commands, eleven events, required/null/unknown fields, UTF-16 vs bytes, exact raw/result budgets, duplicate keys, raw invalid UTF-8/BOM, surrogate/number behavior, bounded result graph, sequence/reducer behavior are executable fixtures. Traces cover auth/role/binary, hello/snapshot timeouts, host replacement, client generations, multi-host ambiguity, room isolation, cwd fence, stale snapshot/event, pending/result dedupe, result association, history poisoning/revision/offline inactivity and command timeout.

Nonportable JS getter/proxy/cycle/prototype/Map regression tests remain in `src/protocol/index.test.ts`; source paths and further existing security regressions are mapped in limits.json. Filesystem, process ownership, HTTP static path/Origin tests stay in their existing suites and later Go transport/HTTP tests; pure fixtures do not pretend to test OS behavior. New Go global quotas are CONTRACTS additions, not old TS constants.
