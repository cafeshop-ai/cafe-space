# Review: v1 architecture and UI design (2026-11-03)

**Scope:**
- [`architecture.md`](../architecture.md)
- [`research/technical-verification.md`](../research/technical-verification.md)
- [`design/ux-ui.md`](../design/ux-ui.md)
- the eight mockups in [`design/mockups/`](../design/mockups/)

**Reviewer:** Claude (author's self-review), for the owner's acceptance.

## Method

1. **Term scan after D12.** Searched all three documents for wording from before D12 (guest, guest projects, Share page, nickname, room password).
2. **Decision integrity.** Every `Dn` referenced anywhere is defined in architecture §14, and each decision has a single status.
3. **Links.** Every relative link and heading anchor between the documents resolves.
4. **Spec against mockups, screen by screen.** Session, Room, Remote access, New session, First run, phone Sign in, phone Session, Foundations.
5. **Accessibility.** Measured WCAG contrast for every token pair used as text (design §7.1) and checked that the mockups use the text-safe variants. Also checked labels, roles and touch targets in the artboard markup.
6. **Feasibility.** The architecture's mechanisms were already verified in round 1 of the research note.

## Findings and fixes

| # | Finding | Severity | Resolution |
|---|---|---|---|
| 1 | The architecture and UX spec still described third-party guests (guest projects, consent sheet, guest approvals) after the owner decided D12 | High | Rewritten. One local account; remote devices sign in as it; computer-only settings; device list and D14 new-device approval. D8 superseded, D11 resolved, D12 decided, D14 added |
| 2 | The research note still showed SEC1 as an open decision | Medium | Marked resolved by D12; the analysis is kept as history and as the constraint for a future Café-login mode |
| 3 | The first-run flow in the spec listed Welcome and Agents as separate steps, while the mockup combines them | Low | Spec §5.1 now follows the mockup: I Agents · II Project · III Remote access (optional, v0 import) · IV Done |
| 4 | The phone sign-in artboard used the dark-theme brand color for its default link, which fails contrast in the light theme | Low | The link now uses the theme token; republished |
| 5 | Remaining "guest" mentions | Info | Only in the research note's round-1 analysis, under a "Resolved by D12" banner. Intentional |

## Checked and consistent

- **P1–P5 and D12 → §7 permissions → UX §5.4 / §5.5 → Remote access and phone mockups.** One local account. "Sign in as Alex". Password, link, cloud address, devices and budgets can only be changed at the computer.
- **D10 budget defaults** (20 turns, 2 h, $5, stop after 3 turns without progress) are the same in architecture §14, UX §5.7, the Room autonomy strip and the Foundations budget card.
- **Capability-shaped controls.**
  - Codex over ACP shows Queue, not Steer, with the reason stated.
  - Stop appears only for your own turn.
  - The model chip is read-only when the agent sets the model.
  - Claude Code is disabled in New session, with the reason and the fix.
- **Truthful states.**
  - Sent never uses a check mark.
  - Unknown is dashed.
  - Delivery marks follow architecture §8: fetched · queued · delivered · handled.
  - Submit for review is disabled with the reason "3 tasks are still open", which matches the task list.
- **Tokens.** The Foundations swatches match design §7.1, including the text-safe ink variants.

## Not verified (honest limits)

- **Rendering.** The mockups were reviewed at the source level. They were not opened in a browser in this review, and they were built in the dark theme only. The light theme is a token switch on each artboard and has not been inspected.
- **Language.** zh-CN copy has not been produced. Chinese serif rendering per platform is still to be checked (design §7.2).
- **Open decisions.** D2, D3, D4, D5, D6, D7, D9, D10, D13 and D14 are recommendations awaiting the owner. The Café-login mode is intentionally undefined.
- **Spikes.** Prototypes and probes are still pending: T3 transport, the Claude adapter's sign-in and resume, SQLite on CI, and Codex attach outside Windows (research note, "Next round").

## Outcome

Ready for the owner's review. Nothing here is implemented yet; M0 can start once D9 (binary names) and D13 (transport spike) are settled.
