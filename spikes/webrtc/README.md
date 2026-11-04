# Isolated R4 WebRTC experiment

**Research only; not a production transport.** This separate workspace never opens Café data, room/device identities, IPC paths or startup registrations. The main and desktop workspaces now align with this probe at Rust 1.95.0, following the architecture decision in room message 67.

## Measured so far

- Windows, actual Chrome 158, fresh isolated browser context: Rust `webrtc = 0.21.1` UDP direct connection passed. PCD1 carried **262144 bytes in 17 packets**, SHA-256 `31a1f9dea0169551092d05e8bf4a446228c8c3eb4c9b713c66adcb7fd53c89be`; Chinese/emoji bytes round-tripped unchanged. Negotiated SCTP message size: 16384. Selected browser pair: prflx/host over UDP. This is loopback interoperability evidence, not WAN/NAT evidence.
- Three Rust checks and six Node codec/diagnostic checks pass locally. Headers, bounds, replay, ID wrap, overlap, order and the 10-second assembly deadline are checked.
- [Run 41740389496](https://github.com/cafeshop-ai/cafe-space/actions/runs/41740389496), commit `4cea479`: actual Linux Chrome 156 passed A/B/C/D, including relay-only maximum-frame echo, measured bidirectional UDP denial with TCP intact, and a fresh successful relay after restoration. Real Safari 26.6.2 passed A but failed B maximum-frame echo; Firefox never gathered candidates. This is **partial evidence, not R4 acceptance**; previous Chrome relay failures remain recorded, not silently treated as reliable interoperation.

## Compiler constraint found by the spike

`cargo +1.89.0 test --manifest-path spikes/webrtc/Cargo.toml` fails in upstream `rtc-mdns 0.21.1`, `src/proto/mod.rs:669`: `Ipv4Addr::from_octets` reports E0658 (`ip_from`). Cargo dependency resolution alone did not detect this undeclared compiler requirement.

The research workspace pins the locally tested **Rust 1.95.0**. Following architecture review, the main and desktop `rust-version`, pinned toolchain and CI version are raised together to the same baseline. There is no silent dependency patch, `RUSTC_BOOTSTRAP`, or substitution of the required WebRTC version with the older Tokio implementation. Main-daemon transport integration remains a separate milestone.

## Local direct-only smoke test

From this directory:

```powershell
cargo test --locked
cargo build --locked
node --test pcd1.test.mjs browser.test.mjs udp-stats.test.mjs
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

Linux uses two newly created namespaces and one veth: lab `10.203.0.1/30`, Rust `10.203.0.2/30`. The lab now has one internal default route via the non-forwarding Rust peer (room decision 76); the Rust namespace has no default IPv4/IPv6 route. Before any browser test, CI records the initial forwarding values, explicitly disables only these two namespace-local switches, asserts both remain off, both namespaces contain only lo/their veth endpoint, and a TEST-NET destination is unreachable. There is no host interface, external route, NAT, host firewall rule, production service, or existing device identity. Both programs run as the non-root runner user. Namespace inode checks prevent accidentally running the test in the host namespace. A separate UDP/TCP probe proves baseline reachability, actual bidirectional UDP drops, unchanged TCP reachability and recovery; both nft drop counters must increase.

macOS runs both real Safari and a same-runner Chrome A/B control with identical code/configuration; either failure fails the job. These cases use a private loopback coturn with an explicit **development-only `allow-loopback-peers` exception**. That exception must never be copied to a public TURN deployment. No macOS pf rule is installed.

TURN credentials are random per run and passed through a private 0700 directory/0600 configuration and in-process signaling, never argv or CI output. Static TURN users do **not** automatically expire: their lifetime is bounded by stopping the owned coturn process and removing its private files. Logs/SDP/credentials are not uploaded. Only sanitized version, candidate-type/protocol, byte/hash, allocation-count, kernel UDP-error and firewall-counter measurements are artifacts. Kernel counters are sampled before/after each case; macOS UDP receive-buffer drops are explicitly runner-global, not attributable to one socket or component. Failed runs preserve explicitly failed partial measurements; only all required cases passing can make CI green. A failed maximum-frame echo does not suppress the independent blocked-UDP probe, but it still fails the entire acceptance gate.

## Limits retained for acceptance

- **No NAT traversal coverage.** A future E must use a separate, measured NAT fixture and prove the actual selected path, including valid prflx outcomes. Receiving an srflx candidate alone is not success.
- A/B do not prove WAN reliability or every symmetric-NAT topology.
- C is a measured failure of this UDP-only computer-side configuration, not an implementation of D3/443 relay or a claim that a remote phone works.
- No cloud/bootstrap authentication, production TURN allocation, real agent session, device import or desktop transport is exercised here.
