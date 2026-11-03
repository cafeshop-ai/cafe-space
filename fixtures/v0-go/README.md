# v0 protocol and room-identity baseline

`manifest.json` records every copied/recorded data file, its source revision/path and SHA-256. The Rust corpus test recomputes those hashes so edits cannot silently change the baseline.

- `protocol/fixtures/**` and `protocol/v1/**` are byte-for-byte copies from `packages/pi-cafe-space` at commit `67237383fef260a8111a14910bc97a56195de349`. These were already shared Go/TypeScript behavior fixtures and protocol specs; they are not the new daemon protocol v2.
- `room-identity.json` and `room-signature.json` are synthetic outputs recorded from the Go code at that same commit. The identity was created through `loadRoomOwner` in a temporary directory and checked with `roomOwner.verify`; the signature was made and checked by the Go signing helpers. Temporary recorder tests were removed from the source checkout after capture.
- The identity uses the public fixture password `fixture-only-123`. Its private key, password verifier and signature are synthetic test values, not credentials. Never use this password or fixture as a real room identity.

Source repository: `github.com/cafecodework/pi-packages/packages/pi-cafe-space`. The pinned Go module is `relay/`. The Go protocol and room-identity tests passed before recording. Protocol v2 types/examples are maintained separately in `crates/protocol`, `web/src/protocol/generated/` and `fixtures/v2/`.
