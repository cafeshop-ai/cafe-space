# Remote channels and the 443 relay (M2 design, task #36)

> Status: **design for review**, 2026-11-04. It implements D3, decided 2026-11-04: WebRTC is the primary browser channel, and a 443 end-to-end encrypted relay is a required M2 deliverable. It must also leave room for P8, an optional native client. Inputs:
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

The relay uses **Noise_NN_25519_ChaChaPoly_SHA256**, giving forward secrecy with ephemeral keys only, plus **signatures over the handshake hash**. This is the same principle by which v0 binds WebRTC to the room: the room key signs the answer SDP digest (`cafe-room-answer-v1`).

1. **Open.** The browser opens `/room/relay` with `roomKey` and a fresh nonce. The cloud asks the registered host to open a stream for that connection ID.
2. **Handshake.** The two sides run the Noise NN handshake (two messages) through the cloud.
3. **The computer proves the room.** The daemon sends `relay_bound`, carrying an ECDSA P-256 signature by the **room key** over `"cafe-room-relay-v1" ‖ roomKey ‖ connectionId ‖ browserNonce ‖ h`, where `h` is the Noise handshake hash. The browser verifies it against the `roomKey` in the link. A mismatch closes the channel, and the UI shows *This relay could not prove it is your computer*.
4. **The browser proves the device.** The browser answers with a signature by its **device key**: the non-exportable WebCrypto P-256 key from v0, over `"cafe-device-relay-v1" ‖ h`. The daemon checks it against the device list, or starts the new-device flow (D14).
5. **Sign-in.** Password sign-in and every protocol-v2 frame follow inside the transport messages.

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

The design follows the server agent's findings (#39).

**Who a quota bucket belongs to.**
- **Host bucket.** The cloud can authenticate only the **computer**, through the room key registration. It cannot see browser device identities, which are verified inside the encrypted channel. So the primary bucket is the room's host.
- **Device buckets.** After sign-in, the daemon may issue a **relay ticket**: `{roomKey, deviceTag, expiry}`, signed by the room key. `deviceTag` is an opaque random value per device. The cloud verifies the ticket with the public room key and applies a per-device bucket without learning who the device is.
- **First connections.** A device's first connection, before it has a ticket, falls under a small anonymous allowance per room.
- **Never trust the client.** A client-chosen device ID is never trusted.

**TURN.**
- Credentials stay REST-style `timestamp:userid`, with `userid` a **stable opaque quota ID** per bucket. The coturn source pinned in #39 counts allocations per extracted userid (`userdb.c:368–397`).
- A fresh random userid is never issued to escape leftover allocations.
- `user-quota` and `total-quota` limit concurrent allocations. `max-bps` is bytes per second per session. None of them is an aggregate device budget.
- The production coturn version is re-checked before relying on this.

**443 relay.**
- **Admission** after host registration and ticket check: a cap on concurrent streams per bucket and per room.
- **Forwarding** per bucket: a byte token bucket (sustained rate and burst), a server-wide capacity, a maximum message size of 64 KiB plus Noise overhead, a bounded per-direction queue (backpressure by not reading), and an idle timeout with a 25-second keepalive. The 25 seconds stays below common proxy idle limits.

**Separate bounds.** TURN and the relay are bounded separately. A single combined hard budget per device is not promised until there is shared accounting. Rate, burst, concurrency and monthly volume are separate limits.

**Numbers.** The numbers are set from cost and load measurements before launch, not guessed here.

**Quota refusals.** On a TURN 486 or a relay quota refusal, the client backs off for at least 60 seconds (RFC 8656) and the UI shows *Relay busy, retrying at 14:02*. It never cycles identities.

**Safari.** R4 observed one Safari allocation still active after 60 seconds. Until real-device evidence explains it (the M2 R4 follow-up), allocation quotas leave headroom for one leftover allocation per device, and the tests measure the actual allocation count per reconnect.

## 5. Native client hook (P8, M6)

A native app or CLI can add a channel kind, for example QUIC, or a WireGuard- or Tailscale-style tunnel, under the same interface, frames, signatures and sign-in. It also gets the relay for free, because the relay does not depend on a browser.

## 6. Security notes

- **Cloud attacks.** A cloud that tampers with or replays the handshake fails step 3: the signature binds the room key to this connection's nonce and handshake hash. A cloud that drops traffic can deny service but cannot read or forge it.
- **Downgrade.** There is no unauthenticated relay mode, and no plaintext fallback.
- **Metadata.** Room key, timing and sizes are visible to the cloud, as they are with TURN today. This is stated in the UI's privacy note.
- **Logs.** Logs record only fixed codes, as today: no handshake hashes, signatures, tickets or payload sizes per message.

## 7. Verification plan (M2)

- **Unit and fixture tests.**
  - Handshake, signature binding and rejection of a forged room signature or replayed nonce.
  - PCD1 reassembly over the relay.
  - The ticket format.
  - Rust and TS run the same fixtures.
- **Cloud tests.**
  - Pairing, an opaque copy (a byte-for-byte check that the payload is never parsed), backpressure, the idle timeout and every limit.
  - Behavior after a quota refusal.
- **End to end, native CI on three OSes.**
  - Browsers with UDP blocked on the computer side, extending R4 case C: the relay must connect where WebRTC does not.
  - A network that allows only 443 and drops all UDP plus TCP 3478. This turns the inferred case into a measured one.
- **Quota tests (#39's checklist).**
  - The same device with rotated TURN credentials stays in one bucket, and devices cannot use each other's buckets.
  - TURN plus relay together stays inside each bound.
  - Reconnects and abnormal closes produce the expected real allocation counts and expiry.
  - 486 back-off works.
