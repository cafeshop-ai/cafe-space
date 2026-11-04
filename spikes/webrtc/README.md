# Isolated R4 WebRTC experiment

**Research only; not a production transport.** This separate workspace never opens Café data, room/device identities, IPC paths or startup registrations. The main and desktop workspaces now align with this probe at Rust 1.95.0, following the architecture decision in room message 67.

## Measured so far

- Windows, actual Chrome 158, fresh isolated browser context: Rust `webrtc = 0.21.1` UDP direct connection passed. PCD1 carried **262144 bytes in 17 packets**, SHA-256 `31a1f9dea0169551092d05e8bf4a446228c8c3eb4c9b713c66adcb7fd53c89be`; Chinese/emoji bytes round-tripped unchanged. Negotiated SCTP message size: 16384. Selected browser pair: prflx/host over UDP. This is loopback interoperability evidence, not WAN/NAT evidence.
- [Run 41744160624](https://github.com/cafeshop-ai/cafe-space/actions/runs/41744160624), commit `404d5b6`: real Firefox 158.0.1 first passed all A/B/C/D with the internally routed fixture. Linux Chrome had relay stalls in both directions; on macOS Safari relay failed while same-runner Chrome relay passed this time. Available kernel UDP error/drop counters stayed zero; that does not identify the cause or prove absence of all network loss. The run still failed.
- [Run 41745384074](https://github.com/cafeshop-ai/cafe-space/actions/runs/41745384074), commit `ccd4ac1`: a 64 KiB send high-water (four messages, including packet headers) let actual Safari A/B pass for the first time. Linux Chrome A–D and same-runner macOS Chrome A/B passed; Firefox B stalled at `SEND_BUDGET` with 65536 bytes queued while its D passed. Each case had only one sample. **Backpressure alone has not fixed relay reliability.** The full 256 KiB/17-packet frame and all previous timeouts remain unchanged.
- [Run 41746234230](https://github.com/cafeshop-ai/cafe-space/actions/runs/41746234230), commit `6d5256e`, completed with all three jobs failed. Its repeated samples are recorded below. **This is not acceptance or proof that backpressure cured the earlier stalls.**
- Next diagnostics leave the network, browser privacy/sandbox, frame sizes and timeouts unchanged: count mDNS names in the browser's offer, remote candidate types and pair states without recording SDP or addresses; retain ICE states on failure; separate WebDriver startup phases with only whitelisted driver milestones and error codes. Synchronous DataChannel send exceptions get a fixed code, not a mangled browser message. Whether mDNS, the shared browser/coturn IP, or a TURN client defect explains B2 remains unproven. All required attempts must pass; missing attempts remain visible.
- Three Rust checks and ten Node codec/diagnostic checks pass locally. Headers, bounds, replay, ID wrap, overlap, order and the 10-second assembly deadline are checked.
- [Run 41740389496](https://github.com/cafeshop-ai/cafe-space/actions/runs/41740389496), commit `4cea479`: actual Linux Chrome 156 passed A/B/C/D, including relay-only maximum-frame echo, measured bidirectional UDP denial with TCP intact, and a fresh successful relay after restoration. Real Safari 26.6.2 passed A but failed B maximum-frame echo; Firefox never gathered candidates. This is **partial evidence, not R4 acceptance**; previous Chrome relay failures remain recorded, not silently treated as reliable interoperation.

## Seventh run: repeated samples, not accepted

Counts are **passed / attempted / required**, read directly from the completed jobs' `R4_SAMPLE_COUNTS` and checked against their case events on 2026-11-04. C passing means the blocked-UDP negative case behaved as expected, not successful connectivity.

| Browser / runner | A | B: both relay | B1: browser relay | B2: Rust relay | C | D |
|---|---|---|---|---|---|---|
| Chrome 156.0.8078.47 / Linux | 1/1/1 | 10/10/10 | 10/10/10 | 0/10/10 | 1/1/1 | 10/10/10 |
| Safari 26.6.2 / macOS | 1/1/1 | 3/10/10 | 10/10/10 | 0/10/10 | not scheduled | not scheduled |
| Chrome 156.0.8078.47 / same macOS runner | 1/1/1 | 10/10/10 | 10/10/10 | 0/10/10 | not scheduled | not scheduled |
| Firefox / Linux (no version returned) | 0/0/1 | 0/0/10 | 0/0/10 | 0/0/10 | 0/0/1 | 0/0/10 |

Sources: Linux Chrome job `126299824886`, Firefox `126299824933`, and macOS `126299824511` (two separately labelled browser summaries).

- All thirty attempted B2 cases failed at `channel_open:OPEN_TIMEOUT`, with one successful allocation but no selected pair. The fixture's shared browser/coturn IP or loopback candidate policy may affect this path; that is **not yet proven**. Preserve these failures even if a later, independently measured topology replaces this fixture.
- Safari B failed seven times during `maximum_frame_echo`. The first failure has a selected relay/relay pair and two allocations. Rust echoed Unicode, then emitted `PACKET_SEQUENCE_OR_TIMEOUT` and `ECHO_FAILED` while receiving maximum-frame fragments, without a maximum-frame `FRAME_RX`/`FRAME_TX`. The browser's old lossy error mapping returned only `T`, so its exact exception is unknown.
- Firefox stopped at `webdriver_start`, code 23, before any case. These are **unexecuted samples**, not ten protocol failures. Startup-stage diagnostics are needed before assigning a cause.
- Chrome's successful repeated samples do not establish causation or cross-browser reliability. Safari remains below the ten-of-ten requirement; R4 remains unaccepted. NAT, production TURN and 443 fallback remain untested.

## Compiler constraint found by the spike

`cargo +1.89.0 test --manifest-path spikes/webrtc/Cargo.toml` fails in upstream `rtc-mdns 0.21.1`, `src/proto/mod.rs:669`: `Ipv4Addr::from_octets` reports E0658 (`ip_from`). Cargo dependency resolution alone did not detect this undeclared compiler requirement.

The research workspace pins the locally tested **Rust 1.95.0**. Following architecture review, the main and desktop `rust-version`, pinned toolchain and CI version are raised together to the same baseline. There is no silent dependency patch, `RUSTC_BOOTSTRAP`, or substitution of the required WebRTC version with the older Tokio implementation. Main-daemon transport integration remains a separate milestone.

## Local direct-only smoke test

From this directory:

```powershell
cargo test --locked
cargo build --locked
node --test pcd1.test.mjs browser.test.mjs udp-stats.test.mjs webdriver.test.mjs
node lab.mjs
```

Open the emitted loopback URL in a **new isolated browser context**, then run `await window.runR4Case({mode: "direct"})` in that page. No TURN service or firewall change is needed. Stop the owned lab process afterward.

## Disposable CI matrix

`.github/workflows/webrtc.yml` runs actual Chrome and Firefox on Linux and actual Safari via `safaridriver` on macOS. Playwright WebKit is not used as a Safari substitute. `run.mjs` refuses non-GitHub-hosted environments.

| Case | Verification |
|---|---|
| A | No TURN, complete ICE/DTLS/SCTP and maximum-frame echo |
| B (10 attempts) | Both peers relay-only, actual selected UDP relay pair, two successful coturn allocations per attempt, maximum-frame echo |
| B1 / B2 (10 each) | Browser-only / Rust-only relay, opposite peer has no TURN servers, verify both selected candidate types and actual allocation count; isolates which TURN client is exercised |
| C, Linux | Close old peers; block **all IPv4/IPv6 UDP in both directions only in the new Rust namespace**; fresh connection must fail to open with no Rust relay candidates |
| D, Linux (10 attempts) | Remove only that namespace table, prove UDP restored, then repeat full relay/frame success with new peers |

Linux uses two newly created namespaces and one veth: lab `10.203.0.1/30`, Rust `10.203.0.2/30`. The lab now has one internal default route via the non-forwarding Rust peer (room decision 76); the Rust namespace has no default IPv4/IPv6 route. Before any browser test, CI records the initial forwarding values, explicitly disables only these two namespace-local switches, asserts both remain off, both namespaces contain only lo/their veth endpoint, and a TEST-NET destination is unreachable. There is no host interface, external route, NAT, host firewall rule, production service, or existing device identity. Both programs run as the non-root runner user. Namespace inode checks prevent accidentally running the test in the host namespace. A separate UDP/TCP probe proves baseline reachability, actual bidirectional UDP drops, unchanged TCP reachability and recovery; both nft drop counters must increase.

macOS runs both real Safari and a same-runner Chrome A/B control with identical code/configuration; either failure fails the job. These cases use a private loopback coturn with an explicit **development-only `allow-loopback-peers` exception**. That exception must never be copied to a public TURN deployment. No macOS pf rule is installed.

TURN credentials are random per run and passed through a private 0700 directory/0600 configuration and in-process signaling, never argv or CI output. Static TURN users do **not** automatically expire: their lifetime is bounded by stopping the owned coturn process and removing its private files. Logs/SDP/credentials are not uploaded. Only sanitized version, candidate-type/protocol, byte/hash, allocation-count, kernel UDP-error and firewall-counter measurements are artifacts. Kernel counters are sampled before/after each case; macOS UDP receive-buffer drops are explicitly runner-global, not attributable to one socket or component. Failed runs preserve explicitly failed partial measurements; only all required cases passing can make CI green. A failed maximum-frame echo does not suppress the independent blocked-UDP probe, but it still fails the entire acceptance gate.

## Limits retained for acceptance

- **No NAT traversal coverage.** A future E must use a separate, measured NAT fixture and prove the actual selected path, including valid prflx outcomes. Receiving an srflx candidate alone is not success.
- A/B do not prove WAN reliability or every symmetric-NAT topology.
- C is a measured failure of this UDP-only computer-side configuration, not an implementation of D3/443 relay or a claim that a remote phone works.
- No cloud/bootstrap authentication, production TURN allocation, real agent session, device import or desktop transport is exercised here.
