# Optional ordered transcript parts (v1 additive extension)

R12 source/type review: [PARTS_REVIEW.md](../../docs/refactor/PARTS_REVIEW.md).
The original `fixtures/v1` expectations remain frozen. Enhanced expectations are
in `fixtures/parts`; R13 implements them before enabling any new producer.

## Fields and validation

- `TranscriptMessage.parts?`: assistant only, array of at most 500 parts. Missing
  means legacy; `[]` is present and empty. Null is not omission.
- Each part has an integer `index` in 0..499; strictly increasing, never duplicate
  or renumbered after omissions. `text` / `thinking` carry `text` up to 65536 UTF-16
  units; `tool-call` carries nonempty `toolCallId` / `toolName` up to 256 units and
  `argsText` up to 4096 units (empty string allowed). Unknown types fail validation.
- `partsTruncated?`: boolean, including explicit false; omission preserved. May
  accompany a legacy fallback without parts. True contributes to aggregate
  `historyTruncated`. It is not inferred from empty text/empty parts.
- `toolIsError?`: boolean, only on role `tool`; explicit false is preserved.
- `ToolExecution.parentMessageId?`: nonempty string up to 128 units. A wire decoder
  validates its shape, not its referential truth. Producers/UI must verify unique
  actual call ownership in the same host/stream/session/cwd scope.
- `message_delta.partIndex?`: integer 0..499. No new event or command is introduced.
- Optional fields are omitted when absent, never filled with null/false/empty
  arrays. Unknown object fields are stripped by canonicalization, including each
  part. All new strings/arrays count toward existing frame/result/node budgets.

## Reducer rules

1. `message_started` from a new assistant producer starts with empty flat text,
   thinking and parts. Final authoritative `message_finished` replaces by the
   same local message ID; start/update/end object identity is not guaranteed by Pi.
2. Indexed text/thinking delta adds once to the existing flat channel and to the
   part with its actual index. A missing index is inserted in sorted order.
   Existing index with a different type throws before mutation.
3. Indexed delta on legacy or non-assistant content retains the old flat update,
   does not invent earlier structure, and sets partsTruncated/historyTruncated.
4. Unindexed delta on a message that has parts drops its parts entirely and sets
   both truncation markers, rather than displaying a stale structured body.
   Legacy unindexed deltas keep all original v1 behavior.
5. A part exceeding its text bound is not partially presented as complete: omit
   the entire parts array, retain bounded old flat fields, mark incomplete.
   Authoritative final content can repair message-level incompleteness; prior
   aggregate historyTruncated remains sticky under the old reducer rules.
6. tool_updated is replacement output and retains parentMessageId. Tool status is
   independent of assistant message status. Formal toolResult text (even `""`)
   takes precedence over partial output. Explicit error wins over a legacy
   `status=complete`; contradictory explicit terminal evidence is marked conflict.

## Producer and association

Use actual Pi content array indexes, `ToolCall.id/name/arguments`,
`AssistantMessageEvent.contentIndex` and `ToolResultMessage.isError`. Provider
signatures, image bytes, arbitrary details, and usage are not copied into parts.
Unknown/image blocks leave index holes and mark incomplete, without fake text.
Out-of-range source indexes or unrepresentable IDs must never be truncated into
an apparently reliable join key; omit the association and mark incomplete.

`message_end` finishes before tool preflight in both installed versions reviewed.
The producer can build a bounded reverse index from retained assistant parts;
rebuild it on authoritative snapshots/context changes and after message eviction.
A repeated callId, even twice in one message, is ambiguous: do not choose the last
match. parentMessageId is only an optional hint and cannot resolve conflicting
actual call ownership. Fallback toolResult stays at its transcript position; an
execution without any result position appears at the transcript live edge, not in
an independent tools panel. Scope is always part of lookup and UI identity.

Legacy assistant toolCallId may only be used when it is a unique explicit match;
no name/time/nearest-message inference. New parts take rendering precedence over
flat text and the old single toolCallId shortcut, avoiding duplicate rendering.

## Budgets and compatibility

Preserve existing limits at every layer. Compaction can evict old messages/tools
and reduce flat text; if a message's parts cannot remain faithful under a budget,
remove its whole parts array and mark partsTruncated/historyTruncated. Do not
renumber or retain invalid partial JSON. Bounded/invalid argsText is evidence for
plain-text display, not an executable argument object; incomplete messages must
not advertise truncated arguments as a validated `{}` call.

An old Relay removes the new fields; new consumers then use legacy fallback.
Historical get_session messages use the same content projection and actual saved
entry IDs. They are read-only and never replace the current command scope. Keep
custom sessionDir, opened ID/cwd and 64 MiB file defenses unchanged.

## Acceptance fixtures

- `codec.json`: legal/illegal presence, role, index, count, text/ID/args and UTF-8
  envelope boundaries; unknown fields and canonical preservation.
- `reducer.json`: indexed append/insertion, legacy/incomplete recovery, conflict,
  missing index, over-budget drop, authoritative repair and marker propagation.
- `ordered.json`: contractual 0-text/1-t1/2-text/3-t2/4-text sequence, same tool
  name, out-of-order completion/results, empty final output and error persistence.
- `native-events.json`: redacted synthetic input through *installed* 0.84.4 and
  0.85.1 low-level loops. Native final toolResults are source-ordered t1,t2;
  ordered.json deliberately tests reverse result arrival as an extra wire stress
  case, not a claim about Pi's native parallel persistence order.
- `projection-cases.json`: normative producer/association/rebuild cases for R13/14.

Fixture generators are test-only and bounded; `{"$repeat":{"text":"x","count":N}}`
is expanded by runners, never a wire field. No runner updates expected values.
