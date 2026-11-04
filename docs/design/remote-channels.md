# Remote channels and the 443 relay (M2 design, task #36)

> Status: **design for review, revision 3**, 2026-11-04. Revised after the server agent's reviews (#40, #41). It implements D3, decided 2026-11-04: WebRTC is the primary browser channel, and a 443 end-to-end encrypted relay is a required M2 deliverable. It must also leave room for P8, an optional native client. Inputs:
> - research R4, closed with unmet items;
> - the server agent's quota investigation (task #39);
> - v0's room identity (`relay/internal/remote/room_identity.go`) and WebRTC answer signing (`rtc.go:116`).
>
> Nothing here is implemented.

## 1. What must hold on every channel

- **Same protocol.** Every channel carries protocol v2 frames: UTF-8 JSON up to 262,144 bytes (`docs/protocol/v2-local-api.md`). Nothing above the channel knows which channel it is on.
- **Same identity and sign-in.**
  - The computer proves it holds the room key named in the link.
  - The browser proves it holds a registered device key.
  - Sign-in with the local-account password and new-device approval (D12, D14) then happen inside the encrypted channel, exactly as on WebRTC today.
- **End to end.** The cloud sees only metadata (room key, sizes, timing, connection counts), never plaintext. A channel the browser cannot authenticate is never used.
- **Bounded.** Frames, chunks, queues, idle time and connection counts all have limits. Backpressure stops reading; it never buffers without bound. The 64 KiB send high-water mark from R4 applies on every channel.
- **Honest.** The UI says which channel is in use (*Direct*, *Relayed*) and why a fallback happened. It never claims a channel worked without a completed handshake.

## 2. Channel interface (daemon side, Rust)

```rust
pub trait Channel: Send {
    /// Sends one protocol-v2 frame, waiting while more than the high-water mark is in flight.
    async fn send(&mut self, frame: Bytes) -> Result<(), ChannelError>;
    /// Receives one complete, reassembled frame; `None` on orderly close.
    async fn recv(&mut self) -> Result<Option<Bytes>, ChannelError>;
    async fn close(&mut self, reason: CloseReason);
    fn kind(&self) -> ChannelKind; // WebRtc | Relay443 | Native(…)
}
```

- **Chunking.** Each channel splits a frame below the interface and reassembles it before `recv` returns. WebRTC and the relay both reuse the PCD1 envelope from v0: 16 KiB packets, a 16-byte header with magic, id, total and offset, and a 10-second assembly deadline. A native channel may use its own.
- **Single writer.** One writer per channel, which is the M0-3 review rule. A channel error closes that channel only; the session layer decides whether to reconnect on another one.
- **Accepting connections.** One acceptor per kind, under one connection limit shared by all kinds.

## 3. The 443 relay

### 3.1 Topology

```text
browser ──WSS /room/relay (443)──► cloud relay ◄──WSS /room/relay-host (443)── daemon (outbound)
          opaque Noise ciphertext                    opaque Noise ciphertext
```

- **Daemon side.** The daemon keeps one outbound host connection per configured cloud, registered with the same room challenge as `/room/host` today (`room_cloud.go:26–32`). It opens a relay stream per browser on request.
- **Cloud side.** The cloud pairs a browser stream with a host stream by connection ID and copies bytes. It never parses the payload. The cloud stays Go (D15), so this is an addition to the existing cloud service, not a new service.
- **Framing.** One Noise message per WebSocket binary message. Text messages are rejected.

### 3.2 Handshake and binding

The relay uses **Noise_NN_25519_ChaChaPoly_SHA256**: ephemeral keys only, which gives forward secrecy. NN on its own authenticates nobody. Authentication comes from signatures over the final handshake hash `h`, which is Noise's channel-binding mechanism (Noise spec §11.2). This is the same principle by which v0 binds WebRTC to the room: the room key signs the answer SDP digest (`cafe-room-answer-v1`).

1. **Open.** The browser opens `/room/relay` with `roomKey` and a fresh 32-byte `browserNonce`. The cloud checks the pre-authentication budget (§4.1) **before** asking the registered host to open a stream for a new `connectionId`. A request over budget is refused without creating any host stream or queue.
2. **Handshake.** The two Noise NN messages pass through the cloud. The handshake must finish within **10 s** of the open, and each message is at most 1 KiB, or the stream is closed.
3. **The computer proves the room first.** The daemon sends `relay_bound` as the first **encrypted transport message**. It carries an ECDSA P-256 signature (raw r‖s, 64 bytes) by the **room key** over:

   ```text
   "cafe-room-relay" 0x00 | version=0x01 | role=0x48 ('H')
   | roomKey (65-byte uncompressed point) | connectionId (16 bytes) | browserNonce (32 bytes) | h (32 bytes)
   ```

   The browser verifies it against the `roomKey` in the link. Until it succeeds, the browser sends nothing else: no device key, no password. On failure the browser closes the channel and the UI shows *This relay could not prove it is your computer*.
4. **The browser proves possession of its device key.** It answers with `device_proof`: an encrypted transport message carrying the device public key and a signature by the non-exportable WebCrypto P-256 **device key** over:

   ```text
   "cafe-device-relay" 0x00 | version=0x01 | role=0x44 ('D') | roomKey | connectionId | browserNonce | h
   ```

   This proves only that the browser holds that key. It is not authorization: the daemon then checks the device list, or starts the new-device flow (D14).
5. **Sign-in.**
   - The browser collects the password **before** opening the stream, so sign-in fits inside the unpromoted window.
   - After `device_proof`, it sends the password sign-in request as a transport message. The host answers with success or a fixed failure code.
   - **On success:**
     - a registered device is promoted to **device** (§4.1);
     - a new device is promoted to **awaiting approval**, and to device only once the owner approves it at the computer (D14).
   - The host refuses every protocol-v2 application frame (sessions, files and so on) until the stream is in the device class.
   - The cloud cannot see frame types. It enforces only bytes, rate and deadlines; the host enforces which messages are allowed in each class.

**Side effects stay idempotent.** WebRTC and the relay may race (§3.3). A sign-in or new-device approval is keyed by `(device key, browserNonce)`, so both channels reaching sign-in create **one** approval request and **one** session. The losing channel is closed without side effects.

**Size limits per layer.**

| Layer | Limit |
|---|---|
| Logical frame (protocol v2) | 262,144 bytes (17 PCD1 packets), unchanged |
| PCD1 packet plaintext | ≤ 16,384 bytes |
| Noise transport message, data | ≤ 16,400 bytes (packet plus the 16-byte AEAD tag), well under Noise's 65,535-byte limit |
| Noise handshake and control messages (`relay_bound`, `device_proof`, sign-in) | ≤ 1,024 bytes |
| WebSocket binary message | Exactly one Noise message, so the same limits apply |
| WebSocket headers | Network and metering overhead, accounted separately and not part of these limits |

- **Send admission.** A packet may be sent only if `in_flight + next_wire_packet ≤ 65,536`. The sender waits **before** exceeding the high-water mark, not after.
- **Queues.** The per-direction queue is a separate bound (§4.2).
- **Tests.** Rust and TS share boundary tests for every limit.

**Libraries.**
- Rust uses `snow` (Runner uses 0.9.6; the version is pinned and verified in M2).
- The browser uses an audited pure-JS X25519, ChaCha20-Poly1305 and SHA-256 implementation (for example `@noble/curves` and `@noble/ciphers`), so that it does not depend on WebCrypto X25519 support in every browser. Ephemeral Noise keys live only in memory.
- Device and room keys stay as they are today.

### 3.3 Choosing a channel

1. **Try WebRTC first**, as today.
2. **Start the relay** if WebRTC has not authenticated within **4 seconds**, or if the browser saw no relay or host candidate at all. Use whichever channel authenticates first and close the other. Two channels are never active for one connection ID.
3. **Remember the outcome.** The daemon records which channel won per device (local only) and may start with the relay on that device's next attempt. A setting lets the owner force *Relay only*.
4. **v0 gateways have no relay.** The UI says so, instead of waiting.

The 4-second figure is a starting point, measured and tuned in M2.

## 4. Quotas and rate limits (P9)

The design follows the server agent's findings (#39) and review (#40).

**What the cloud can know.**
- The cloud authenticates a **registered room identity**: whoever proves the room key. That is not a physical computer. A computer can register another room key and get another bucket.
- So limits are **per registered room identity**, plus a service-wide capacity, plus anti-abuse limits on new room registrations. Per-source limits only help; they never identify anyone.
- Nothing fingerprints machines or collects local data to approximate "one computer".

### 4.1 Stream classes, assigned by the host

Every relay stream starts **unpromoted**. Only the daemon, over its authenticated host connection, can change a stream's class, by sending `relay_promote {connectionId, class, deviceTag?}`. The browser presents nothing the cloud trusts. There are no bearer tickets: a copied value cannot buy a bucket.

| Class | Set by | Allows | Limits |
|---|---|---|---|
| **Unpromoted** | Default at open | Handshake, room proof, device proof, and the password sign-in request with its response or failure | Must be promoted within **30 s** of opening. At most 64 KiB in each direction. At most **2** per room, plus a service-wide pending cap, both checked before pairing |
| **Awaiting approval** | Host, after a successful sign-in from a new device (D14) | Approval status frames only | ~1 KiB/s. At most **5 min**, measured from the first time the stream enters this class. At most **2** per room |
| **Device** | Host, after sign-in, with a stable opaque `deviceTag` that the daemon assigns per device and stores in the device list | All protocol-v2 traffic | The device bucket (§4.2) |

- Every stream, whatever its class, also counts against its **room parent bucket** and the **service total**. An unpromoted stream therefore never bypasses a device or room limit. A signed-in device that withholds sign-in on a fresh stream stays in the unpromoted class, with its 30-second and 64 KiB caps.
- Revoking a device makes the daemon close its streams and stop promoting it.
- **Promotions are conserved.** The cloud handles `relay_promote` under the same lock that owns the stream table, and:
  - **Binding.** A `connectionId` is bound to its room and to the **generation** of the host connection that opened it. A promotion for another room, a closed stream or an older host generation is rejected.
  - **Atomic counts.** Checking the target class's concurrency and moving the stream's count from source to target happen atomically. Parent-bucket counts are not reset by the move, so two concurrent promotions cannot both take the last slot.
  - **No resets.** A promotion only moves forward: unpromoted → awaiting approval → device, or unpromoted → device. A repeated or backward promotion is rejected and resets nothing: no byte allowance, token bucket or deadline.
  - **Host disconnect.** When a host connection drops, its streams close and their counts are released. A failed or timed-out sign-in or approval releases its slot immediately and never holds a permanent pending entry.
- The first-connection and pre-authentication cost is bounded by the unpromoted limits **before** the cloud asks the host to open a stream (§3.2 step 1), so an attacker cannot exhaust host streams or pending approvals before any password.

### 4.2 Buckets

**443 relay.**
- **Device bucket:** concurrent streams, a byte token bucket (sustained rate and burst), and a bounded per-direction queue. Backpressure works by not reading.
- **Room parent bucket:** the same limits, set higher, shared by every class in the room.
- **Service:** total streams and total bytes per second.
- **Per message:** the limits in §3.2 (16,400 bytes for data, 1,024 bytes for control).
- **Idle:** a timeout, with a 25-second keepalive, which stays below common proxy idle limits.

**TURN.**
- Browser TURN credentials are issued at signaling time, before any device is known. Their REST-style `timestamp:userid` uses a `userid` that the **server derives from the room identity**, such as a keyed hash of `roomKey`. It is never chosen by the client.
- The coturn source pinned in #39 counts allocations per extracted userid (`userdb.c:368–397`), so TURN quotas are **per room identity**. Per-device TURN buckets are not offered.
- `user-quota` and `total-quota` limit concurrent allocations. `max-bps` is bytes per second per session. None of them is a room or device budget.
- The production coturn version is re-checked before relying on this.

**Separate bounds.**
- TURN and the relay are bounded separately. A single combined hard budget is not promised until there is shared accounting.
- Rate, burst, concurrency and monthly volume are separate limits.
- Numbers are set from cost and load measurements before launch, not guessed here.

**Quota refusals.** On a TURN 486 or a relay quota refusal, the client backs off for at least 60 seconds (RFC 8656) and the UI shows *Relay busy, retrying at 14:02*. It never cycles identities.

**Safari.**
- R4 saw one Safari allocation still active after 60 seconds, from a log-derived count. It is not a proven upper bound: if every reconnect leaves one, a device could accumulate several before they expire.
- The hard caps stay. A leftover allocation may lower the concurrency Safari has left. Any margin is set after measuring reconnect frequency against the real allocation lifetime with a real census, and the 486 back-off and earlier failure records are kept.

## 5. Native client hook (P8, M6)

A native app or CLI can add a channel kind, for example QUIC, or a WireGuard- or Tailscale-style tunnel, under the same interface, frames, signatures and sign-in. It also gets the relay for free, because the relay does not depend on a browser.

## 6. Security notes

- **Cloud attacks.** A cloud that tampers with or replays the handshake fails step 3: the signature binds the room key to this connection's nonce and handshake hash. A cloud that drops traffic can deny service but cannot read or forge it.
- **Downgrade.** There is no unauthenticated relay mode, and no plaintext fallback.
- **Promotion trust.** The cloud accepts `relay_promote` only on the host connection, which the room key authenticated at registration. A host can only move streams within its own room's buckets, so a dishonest host gains nothing beyond its own limits.
- **Metadata.** Room key, timing and sizes are visible to the cloud, as they are with TURN today. This is stated in the UI's privacy note.
- **Logs.** Logs record only fixed codes, as today: no handshake hashes, signatures, device tags or payload sizes per message.

## 7. Verification plan (M2)

- **Unit and fixture tests (same fixtures in Rust and TS).**
  - The exact signed-byte encodings.
  - Rejection of a forged room signature, and of a room proof or device proof replayed across rooms or across connections.
  - The host-first order: no device proof or password before the room proof verifies.
  - A first-time new device.
  - Idempotent sign-in and approval when WebRTC and the relay race.
  - PCD1 reassembly over the relay.
  - Promotion messages are accepted only from the host. Tests cover duplicate, concurrent, backward, cross-room, closed-stream and stale-host-generation promotions.
  - Sign-in through the unpromoted class works for both a registered device and a new device. A device that is not yet approved cannot call any session API. A failure or timeout frees its slot.
  - Rust and TS run the same fixtures.
- **Cloud tests.**
  - Pairing, an opaque copy (a byte-for-byte check that the payload is never parsed), backpressure, the idle timeout and every limit.
  - Behavior after a quota refusal.
- **End to end, native CI on three OSes.**
  - Browsers with UDP blocked on the computer side, extending R4 case C: the relay must connect where WebRTC does not.
  - A network that allows only 443 and drops all UDP plus TCP 3478. This turns the inferred case into a measured one.
- **Quota tests (#39's checklist).**
  - Rotated TURN credentials stay in the room's bucket.
  - Device buckets cannot be entered without host promotion.
  - Unpromoted and awaiting-approval caps hold under floods.
  - Room-key rotation, parallel rooms and delete-and-recreate are counted as separate registered identities, not as one computer.
  - TURN plus relay together stays inside each bound.
  - Reconnects and abnormal closes produce the expected real allocation counts and expiry.
  - 486 back-off works.
