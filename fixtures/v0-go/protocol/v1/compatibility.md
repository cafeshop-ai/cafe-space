# v1 JSON and transport compatibility

This is the unchanged TS baseline, not a proposal to loosen Go validation.

| Input/behavior | Current TS outcome | Go requirement |
| --- | --- | --- |
| Object field names | case-sensitive | do not rely on case-insensitive struct decoding |
| Unknown fields | allowed but removed from known schema levels | reconstruct known fields |
| Result data keys | preserved, including __proto__/constructor | ordinary data keys, no unsafe merge |
| Repeated JSON key | last value wins, before validation | preserve same last-key semantics |
| Missing nullable required field | invalid | track presence separately from null |
| Optional field explicitly null | invalid unless schema says nullable | do not conflate nil with absent |
| Result data absent/null/false/0/empty | all distinct; only absent omitted | preserve presence |
| Legacy snapshot historyTruncated | absent → false | insert false canonical value |
| Legacy hostId/host.ready/fences | absent retained as absent | do not invent IDs/true/null |
| UTF-16 string lengths | surrogate pair counts as 2 | Go byte/rune length is not this limit |
| Escaped isolated surrogate | JSON.parse retains a UTF-16 code unit; JSON.stringify re-escapes it | preserve code unit, no U+FFFD replacement |
| Raw invalid UTF-8 | ws rejects before application with close 1007; fatal decoder also rejects | strict full-message UTF-8; no substitution |
| Raw UTF-8 BOM | TextDecoder strips initial BOM | distinguish byte ingress from direct string decode |
| Direct string beginning U+FEFF | JSON.parse fails | no blanket JSON whitespace expansion |
| Large finite JSON number | IEEE-754 binary64 rounding (9007199254740993 → 9007199254740992) | no arbitrary-precision wire value change |
| Overflow number in known data/timestamp | rejected as nonfinite | reject, don't truncate/coerce |
| Overflow in discarded unknown field | accepted then discarded | parse before schema validation without rejecting discarded value |
| Negative zero | finite and valid; outbound JSON.stringify writes 0 | preserve effective outbound semantics |
| seq/offset/limit | safe integer plus field-specific bounds | no float truncation |
| emittedAt | merely S(64), no date parse | do not add ISO-only restriction |
| timestamp | any finite double, negative/fraction allowed | do not add integer/date-only restriction |
| JSON strings with < > & | JSON.stringify does not HTML-escape | Go encoder must not silently inflate budgets with HTML escaping |
| Frame size | serialized UTF-8 bytes including escaping and whitespace | measure full envelope, not decoded field sizes |
| Duplicate object keys ordering | object order not semantic; array order is | compare objects semantically, preserve array order |

A custom Go JSON/string representation is needed to preserve isolated surrogate code units: encoding/json into Go strings substitutes U+FFFD. Any codec must pass these goldens; do not edit expected values to accommodate Go defaults.

Result graph budget is checked from data root: at most 20000 total nodes including containers; array/object widths ≤500; objects/arrays at depth >8 rejected, but scalar/null leaf at depth 9 is accepted because primitives are checked before the container depth guard. Strings and keys accumulate UTF-8 bytes ≤262144; string length ≤262144 UTF-16 units and key length ≤4096. fitCommandResult additionally requires serialized envelope ≤261120 bytes, else rejected/RESULT_TOO_LARGE without data. An applied empty result is not silently substituted for an invalid graph.

Pure reducer is not the inbound validator. It may crop in-memory text, and its sequence check requires exactly next seq even when decode permits seq=0. Cropping avoids splitting a valid surrogate pair at the cutoff; already present isolated surrogate code units are not sanitized. Tool output updates replace rather than append. Protocol cap (1000 messages/500 tools/65536 text) and Node relay compaction (100/24, 8192 message text, 2048 args/4096 output) are separate layers.

R02 first fixture run exposed an authoring arithmetic mistake: 500 arrays of 39 nulls contain 1 + 500×40 = 20001 nodes, not 20000. The expectation was corrected from source budget accounting before freezing; exact 20000 and 20001 cases now exist. No runtime behavior was changed. Later implementations must not auto-update these expectations.

Transport difference intentionally documented: the low-level raw UTF-8 codec adapter reports INVALID_MESSAGE, while actual Node ws terminates malformed UTF-8 at transport with 1007 before any error JSON. Both are independently tested. A Go transport must preserve the latter client-visible behavior.

No unresolved protocol decision is required for R03: these edge cases have fixed expected behavior. New safety quotas/HTTP hardening from CONTRACTS must be documented separately, never claimed as old Node constants. Platform-dependent filesystem/URL/process checks require later layer-specific parity tests.
