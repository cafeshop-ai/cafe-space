# Isolated R4 WebRTC experiment

**Research only; not a production transport.** This separate workspace never opens Café data, room/device identities, IPC paths or startup registrations. The main and desktop workspaces now align with this probe at Rust 1.95.0, following the architecture decision in room message 67.

## Measured so far

- Windows, actual Chrome 158, fresh isolated browser context: Rust `webrtc = 0.21.1` UDP direct connection passed. PCD1 carried **262144 bytes in 17 packets**, SHA-256 `31a1f9dea0169551092d05e8bf4a446228c8c3eb4c9b713c66adcb7fd53c89be`; Chinese/emoji bytes round-tripped unchanged. Negotiated SCTP message size: 16384. Selected browser pair: prflx/host over UDP. This is loopback interoperability evidence, not WAN/NAT evidence.
- [Run 41744160624](https://github.com/cafeshop-ai/cafe-space/actions/runs/41744160624), commit `404d5b6`: real Firefox 158.0.1 first passed all A/B/C/D with the internally routed fixture. Linux Chrome had relay stalls in both directions; on macOS Safari relay failed while same-runner Chrome relay passed this time. Available kernel UDP error/drop counters stayed zero; that does not identify the cause or prove absence of all network loss. The run still failed.
- [Run 41745384074](https://github.com/cafeshop-ai/cafe-space/actions/runs/41745384074), commit `ccd4ac1`: a 64 KiB send high-water (four messages, including packet headers) let actual Safari A/B pass for the first time. Linux Chrome A–D and same-runner macOS Chrome A/B passed; Firefox B stalled at `SEND_BUDGET` with 65536 bytes queued while its D passed. Each case had only one sample. **Backpressure alone has not fixed relay reliability.** The full 256 KiB/17-packet frame and all previous timeouts remain unchanged.
- [Run 41746234230](https://github.com/cafeshop-ai/cafe-space/actions/runs/41746234230), commit `6d5256e`, completed with all three jobs failed. Its repeated samples are recorded below. **This is not acceptance or proof that backpressure cured the earlier stalls.**
- [Run 41748734723](https://github.com/cafeshop-ai/cafe-space/actions/runs/41748734723), commit `2b2d0a4`, also failed. Its diagnostics and exact counts are below. Rust's disabled mDNS handling is consistent with B2's candidate failures; the large-frame stalls remain unresolved. These are separate failure paths.
- The next fixture-first diagnostic adds ten direct attempts, ten same-browser relay/relay controls without Rust, and per-attempt coturn lifecycle counters. It does not change the topology, existing browser settings, transport implementation, 64 KiB send high-water, frame sizes, protocol deadlines or quotas. The controls are **not yet real-browser evidence**.
- Five Rust checks and fifteen Node codec/diagnostic/lab checks cover headers, bounds, replay, ID wrap, overlap, order, the 10-second assembly deadline, browser-control route proofs, safe coturn/SCTP counters and explicit daemon reset/snapshot.
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

## Eighth run: diagnostics, still not accepted

Counts remain **passed / attempted / required**; all jobs completed with failure.

| Browser / runner | A | B | B1 | B2 | C | D |
|---|---|---|---|---|---|---|
| Chrome 156.0.8078.47 / Linux | 1/1/1 | 10/10/10 | 10/10/10 | 0/10/10 | 1/1/1 | 10/10/10 |
| Firefox 158.0.1 / Linux | 1/1/1 | 9/10/10 | 10/10/10 | 6/10/10 | 1/1/1 | 9/10/10 |
| Safari 26.6.2 / macOS | 1/1/1 | 3/10/10 | 8/10/10 | 0/10/10 | not scheduled | not scheduled |
| Chrome 156.0.8078.47 / same macOS runner | 1/1/1 | 9/10/10 | 10/10/10 | 0/10/10 | not scheduled | not scheduled |

Sources: completed job logs `126308203506` (Chrome), `126308203380` (Firefox), `126308203931` (macOS), with each `R4_SAMPLE_COUNTS` checked against the case events.

- Every Chrome/Safari B2 offer contained one candidate, a `.local` name. There was no successful candidate pair. Rust is configured with mDNS disabled; `rtc-ice 0.21.1` ignores such remote host candidates. Firefox B2 offered two non-mDNS candidates and opened every channel in the same Linux topology. This supports the candidate explanation and shows a shared browser/coturn IP does not necessarily prevent B2. It does not directly measure TURN permission failures or prove all other hypotheses false. Preserve the old B2 failures; a future STUN-only comparison must be separately labelled.
- Firefox started successfully this time. B failed once, B2 four times and D once, all during maximum-frame sending with `SEND_BUDGET`, 65536 bytes buffered and a connected/open channel.
- Safari B had five `DATA_CHANNEL_SEND_FAILED` maximum-frame failures, accompanied by Rust `PACKET_SEQUENCE_OR_TIMEOUT`; two other B attempts failed to open. The last two B1 attempts also failed to open. Late B/B2 attempts had zero Rust relay candidates/allocations. Quota exhaustion or retained allocations are hypotheses, not measured conclusions.
- macOS Chrome B had one maximum-frame `DATA_CHANNEL_SEND_FAILED`, also accompanied by Rust `PACKET_SEQUENCE_OR_TIMEOUT`. Kernel receive-drop counters stayed zero; this is not proof of no loss or of a particular component's fault.

## Ninth run: controls passed in three combinations, unresolved Rust relay stalls

[Run 41757101762](https://github.com/cafeshop-ai/cafe-space/actions/runs/41757101762), commit `a0960a4`, completed with failure. Counts are **passed / attempted / required**.

| Browser / runner | K | A | B | B1 | B2 | C | D |
|---|---|---|---|---|---|---|---|
| Chrome 156.0.8078.47 / Linux | 10/10/10 | 10/10/10 | 10/10/10 | 10/10/10 | 0/10/10 | 1/1/1 | 10/10/10 |
| Firefox 158.0.1 / Linux | 10/10/10 | 10/10/10 | 10/10/10 | 10/10/10 | 5/10/10 | 1/1/1 | 8/10/10 |
| Chrome 156.0.8078.47 / macOS | 10/10/10 | 10/10/10 | 10/10/10 | 10/10/10 | 0/10/10 | not scheduled | not scheduled |
| Safari 26.6.2 / same macOS runner | 1/1/10 | 0/0/10 | 0/0/10 | 0/0/10 | 0/0/10 | not scheduled | not scheduled |

Sources: complete job logs `126335675499`, `126335675249` and `126335675583`, summaries at lines 840, 840 and 578/681 respectively, checked against case and release events.

- Every executed Firefox/Chrome case had a successful observed-zero release check and no quota error (61 releases in each Linux job, 50 in macOS Chrome). Firefox still failed five B2 attempts and two D attempts at `maximum_frame_echo:SEND_BUDGET`, with 65536 bytes buffered and an open/connected channel. This run does not support quota exhaustion as the explanation for those stalls.
- Chrome B2 still offered one mDNS-only host candidate. Its allocation succeeded but the coturn permission-success count did not increase in that case. No candidate pair succeeded. The independently labelled STUN-only comparison remains outstanding.
- Safari K's first Unicode/maximum-frame echo **passed**, but only one of two allocations had a final-close record within the strict five-second check. Observed active allocations remained one, quota errors zero. The fixture stopped; all its Rust cases are **unexecuted**, not failed transports. There is no proof yet of an actual persistent allocation leak or its owner.
- K/A's thirty successes each cover this observation window only. They narrow the investigation but neither prove universal reliability nor uniquely identify a Rust-library defect. R4 and task #8 remain unaccepted. The local user explicitly chose to continue R4 rather than defer it and start M1.
- [Native desktop 41757101867](https://github.com/cafeshop-ai/cafe-space/actions/runs/41757101867) passed on all three OSes. [Base CI 41757101977](https://github.com/cafeshop-ai/cafe-space/actions/runs/41757101977) failed on macOS in `local::tests::stale_socket_is_recovered_but_a_regular_file_is_never_deleted` (`AddrInUse` at `local.rs:501`); the other three platforms passed. That failure has no measured root cause yet. A diagnostic-only IPC change distinguishes a live endpoint, a timed-out probe and another OS error while keeping the same conservative refusal-to-unlink policy.

The next run adds numeric observations, not a transport fix:

- A private `log` subscriber counts only pinned `rtc_sctp::association` DATA reception, SACK construction/reception, T3 timeouts, fast retransmit and receive-buffer drops. It retains the last numeric TSN and advertised window from the known log formats. No raw library log is printed or uploaded. SACK **construction is not proof of on-wire delivery**; trace counters can affect timing. Missing fields remain null. Explicit snapshots do not close the peer. A fresh isolated Chrome context on Windows completed the unchanged 256 KiB/17-packet direct echo and expected SHA-256 with this observer; the actual snapshot contained 245 DATA chunks, 51 built SACKs, 122 received SACKs, local window 524288, and zero observed T3/fast-retransmit/buffer-drop events. This one direct smoke verifies that the observer fires, not relay reliability.
- A failed five-second cleanup check gets up to sixty additional seconds of observation, recording whether/when the log-derived count clears. The original gate **still fails**, even if it later clears; later cases remain unexecuted. This does not extend frame, send or echo deadlines, raise quotas or restart coturn.

## Fixture-first diagnostic

The locally selected next experiment checks the fixture before changing the Rust stack:

1. **K: browser to browser**, ten relay/relay maximum-frame round trips, replacing only the Rust endpoint with a second `RTCPeerConnection` in the same browser process/page. This is not a second device or browser process, and success would narrow the investigation, not uniquely prove a Rust-library bug. Both selected paths must be UDP relay/relay; the PCD1 codec, Unicode sample, 256 KiB/17-packet maximum frame, 64 KiB high-water and deadlines are shared with the Rust cases. K runs first so an earlier Rust allocation cannot contaminate this control.
2. **A: direct**, now ten attempts. B/B1/B2 and Linux D retain ten attempts each; all old failures remain recorded.
3. **Allocation release**, checked before and after each attempt. Rust gets an explicit acknowledged `reset` after each result rather than waiting until the next offer to close the old peer. Private coturn logs produce only aggregate unique successful allocation, final-close, permission-success, channel-bind-success and quota-error counters. Active counts are **observed from logs**, not an independent live-server census. Unrecognized allocation records stay unknown, never zero. Counters use coturn's session IDs internally but never publish IDs, addresses, users or raw logs.
4. **Fixture failure is distinct.** Each attempt must return observed active allocations to zero within a separate five-second cleanup bound. `TURN_ALLOCATION_NOT_RELEASED` stops admission to the contaminated fixture and preserves remaining cases as unexecuted, not protocol failures. `TURN_QUOTA_EXCEEDED` is recorded separately. Quotas remain 8 per user / 16 total; the test does not raise them or restart coturn to hide retained allocations. A normal frame failure still permits later cases if release is proven.

The lifecycle parser follows the [coturn 4.6.1 method and final-close log formats](https://github.com/coturn/coturn/blob/4.6.1/src/server/ns_turn_server.c), with support for suffix session IDs in newer versions. Raw logs remain private and are deleted with the owned coturn process/configuration. No production service is involved.

## Compiler constraint found by the spike

`cargo +1.89.0 test --manifest-path spikes/webrtc/Cargo.toml` fails in upstream `rtc-mdns 0.21.1`, `src/proto/mod.rs:669`: `Ipv4Addr::from_octets` reports E0658 (`ip_from`). Cargo dependency resolution alone did not detect this undeclared compiler requirement.

The research workspace pins the locally tested **Rust 1.95.0**. Following architecture review, the main and desktop `rust-version`, pinned toolchain and CI version are raised together to the same baseline. There is no silent dependency patch, `RUSTC_BOOTSTRAP`, or substitution of the required WebRTC version with the older Tokio implementation. Main-daemon transport integration remains a separate milestone.

## Local direct-only smoke test

From this directory:

```powershell
cargo test --locked
cargo build --locked
node --test pcd1.test.mjs browser.test.mjs udp-stats.test.mjs webdriver.test.mjs turn-stats.test.mjs lab.test.mjs
node lab.mjs
```

Open the emitted loopback URL in a **new isolated browser context**, then run `await window.runR4Case({mode: "direct"})` in that page. No TURN service or firewall change is needed. Stop the owned lab process afterward.

## Disposable CI matrix

`.github/workflows/webrtc.yml` runs actual Chrome and Firefox on Linux and actual Safari via `safaridriver` on macOS. Playwright WebKit is not used as a Safari substitute. `run.mjs` refuses non-GitHub-hosted environments.

| Case | Verification |
|---|---|
| K (10 attempts) | Two peers in the same browser, no Rust signaling; both selected paths UDP relay/relay, two distinct coturn allocations and the same maximum-frame echo |
| A (10 attempts) | No TURN, complete ICE/DTLS/SCTP and maximum-frame echo |
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
