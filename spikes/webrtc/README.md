# Isolated R4 WebRTC experiment

**Research only; not a production transport.** This separate workspace never opens Café data, room/device identities, IPC paths or startup registrations. The main and desktop workspaces now align with this probe at Rust 1.95.0, following the architecture decision in room message 67.

## Measured so far

- Windows, actual Chrome 158, fresh isolated browser context: Rust `webrtc = 0.21.1` UDP direct connection passed. PCD1 carried **262144 bytes in 17 packets**, SHA-256 `31a1f9dea0169551092d05e8bf4a446228c8c3eb4c9b713c66adcb7fd53c89be`; Chinese/emoji bytes round-tripped unchanged. Negotiated SCTP message size: 16384. Selected browser pair: prflx/host over UDP. This is loopback interoperability evidence, not WAN/NAT evidence.
- Three Rust checks and two Node codec checks pass locally. Headers, bounds, replay, ID wrap, overlap, order and the 10-second assembly deadline are checked.
- TURN, Firefox, real Safari, network blocking and recovery **still require a successful R4 workflow**. The workflow is a test implementation, not evidence that those cases passed.

## Compiler constraint found by the spike

`cargo +1.89.0 test --manifest-path spikes/webrtc/Cargo.toml` fails in upstream `rtc-mdns 0.21.1`, `src/proto/mod.rs:669`: `Ipv4Addr::from_octets` reports E0658 (`ip_from`). Cargo dependency resolution alone did not detect this undeclared compiler requirement.

The research workspace pins the locally tested **Rust 1.95.0**. Following architecture review, the main and desktop `rust-version`, pinned toolchain and CI version are raised together to the same baseline. There is no silent dependency patch, `RUSTC_BOOTSTRAP`, or substitution of the required WebRTC version with the older Tokio implementation. Main-daemon transport integration remains a separate milestone.

## Local direct-only smoke test

From this directory:

```powershell
cargo test --locked
cargo build --locked
node --test pcd1.test.mjs
node lab.mjs
```

Open the emitted loopback URL in a **new isolated browser context**, then run `await window.runR4Case({mode: "direct"})` in that page. No TURN service or firewall change is needed. Stop the owned lab process afterward.

## Disposable CI matrix

`.github/workflows/webrtc.yml` runs actual Chrome and Firefox on Linux and actual Safari via `safaridriver` on macOS. Playwright WebKit is not used as a Safari substitute. `run.mjs` refuses non-GitHub-hosted environments.

| Case | Verification |
|---|---|
| A | No TURN, complete ICE/DTLS/SCTP and maximum-frame echo |
| B | Both peers relay-only, actual selected UDP relay pair, two successful coturn allocations, maximum-frame echo |
| C, Linux | Close old peers; block **all IPv4/IPv6 UDP in both directions only in the new Rust namespace**; fresh connection must fail to open with no Rust relay candidates |
| D, Linux | Remove only that namespace table, prove UDP restored, then repeat full relay/frame success with new peers |

Linux uses two newly created namespaces and one veth: lab `10.203.0.1/30`, Rust `10.203.0.2/30`. There is no host interface, default route, NAT, host firewall rule, production service, or existing device identity. Both programs run as the non-root runner user. Namespace inode checks prevent accidentally running the test in the host namespace. A separate UDP/TCP probe proves baseline reachability, actual bidirectional UDP drops, unchanged TCP reachability and recovery; both nft drop counters must increase.

macOS A/B use a private loopback coturn with an explicit **development-only `allow-loopback-peers` exception**. That exception must never be copied to a public TURN deployment. No macOS pf rule is installed.

TURN credentials are random per run and passed through a private 0700 directory/0600 configuration and in-process signaling, never argv or CI output. Static TURN users do **not** automatically expire: their lifetime is bounded by stopping the owned coturn process and removing its private files. Logs/SDP/credentials are not uploaded. Only sanitized version, candidate-type/protocol, byte/hash, allocation-count and firewall-counter measurements are artifacts.

## Limits retained for acceptance

- **No NAT traversal coverage.** A future E must use a separate, measured NAT fixture and prove the actual selected path, including valid prflx outcomes. Receiving an srflx candidate alone is not success.
- A/B do not prove WAN reliability or every symmetric-NAT topology.
- C is a measured failure of this UDP-only computer-side configuration, not an implementation of D3/443 relay or a claim that a remote phone works.
- No cloud/bootstrap authentication, production TURN allocation, real agent session, device import or desktop transport is exercised here.
