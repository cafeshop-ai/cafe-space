# Shared fixtures

- `v0-go/` pins the legacy v0/protocol-v1 baseline. Shared Go/TypeScript fixtures and specs are copied from the source revision; only uncovered room-identity/signature examples are newly recorded from Go. `v0-go/manifest.json` lists source paths and SHA-256 hashes. Do not hand-edit them.
- `v2/` contains draft daemon-local API examples, including the version handshake, event subscription/snapshot and operation outcomes. Rust tests decode/re-encode them; TypeScript checks the same JSON against bindings generated from Rust. `session-lifecycle.json` is an offline trace of independent event frames, not a wire batch; Rust and TS test its observed states, fences and fixed stopped reasons. `session-transitions.json` is the shared offline state-transition matrix; Rust tests all 49 state pairs, and TS checks the trace against the same matrix.
- `migrations/v1.sql` is a synthetic M0 schema with sentinel data for migration/backup tests, not a captured user database. `migrations/v2.sql` is the schema upgrade applied inside the production migration transaction and exercised by those tests.

Legacy v0 device protocol v1 and local API v2 are separate contracts. Do not pass v0 device frames to the v2 decoder.
