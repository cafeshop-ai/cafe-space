# Café Space — UX & UI design (draft v0.1)

> Status: **draft for review**, 2026-11-03. Builds on [`../architecture.md`](../architecture.md); mockups are in [`mockups/`](mockups/) (§12).
> Visual language is derived from **cafeshop.ai** (studied 2026-11-03: its `--eg-*` design tokens, typography and page patterns). Café Space should read as the same family.
> **D12 (decided).** Without a Café login, remote access is the owner only: one local account per computer, and other devices sign in as it. There is no guest role in v1.
> Where a decision is still open, this draft assumes its recommended default: D10 (budget defaults) and D14 (new-device approval on).

## 1. What the design must achieve

Café Space puts AI agents on your own computer, lets you reach them from anywhere, and lets people and agents work together in rooms. The UI has three jobs, in this order:

1. **Show what needs you.** Approvals, stopped autonomous work, unknown outcomes and reviews are the moments a human matters. They are never buried.
2. **Make agent work legible.** What is running, where, for whom, at what cost, and what actually happened. "Dispatched" never looks like "done".
3. **Stay calm.** Agents produce a lot of activity. The interface is a quiet café, not a trading floor.

### Principles

| Principle | In practice |
|---|---|
| **Calm by default** | Warm low-contrast surfaces, one accent, motion only for state. Streaming text and tool output stay readable, and nothing flashes |
| **Truth over comfort** | Every state has a name and a look, including *unknown*. No spinner without a reason, no green check without evidence |
| **Consequences before commitment** | Risky choices (autonomy, password, reset link, cloud address, device approval) show what will happen in plain words *before* the confirm button |
| **Capability-shaped controls** | Controls appear only when the agent supports them. When unavailable, they are disabled with a one-line reason, never hidden without explanation |
| **One place, many screens** | The desktop app, your phone and a collaborator's browser share components and words. Nobody has to learn a second product |
| **Bilingual by birth** | zh-CN is the default, en is complete. Layouts allow either language to be 40% longer |

## 2. People and contexts

| Who | Where | Main jobs | Constraints |
|---|---|---|---|
| **Owner, at the computer** | Desktop app (Tauri) | Run and steer agents; set up remote access; join rooms; let agents work on a Goal | Long sessions, keyboard-heavy, several agents at once |
| **Owner, away** | Phone or laptop browser, via the room link + **local account** password (D12) | Check on agents, answer approvals, start sessions (P2) | Small screen, intermittent network, quick visits |
| **Collaborator** | Browser, collaboration room invite | Discuss, assign tasks, review Goal results | May never install anything |
| **AI member** | HTTP (not a UI user) | Shows up in rooms with presence and delivery state | Its state must be visible to humans honestly |

## 3. Information architecture

### 3.1 Desktop app

```text
┌ Title bar ─ search / ⌘K ─────────────────────────── Needs you (3) · ● Connected ─┐
│ Sidebar              │ Main                                     │ Inspector (opt) │
│  Sessions        ⌘1  │  Session · Room · Share · Settings pages │  context panel  │
│   ▸ project groups   │                                          │                 │
│  Rooms           ⌘2  │                                          │                 │
│   ▸ room list        │                                          │                 │
│  Remote access   ⌘3  │                                          │                 │
│  ─────────────       │                                          │                 │
│  Settings        ⌘,  │                                          │                 │
└──────────────────────┴──────────────────────────────────────────┴─────────────────┘
```

- **Sessions.** Your agents' sessions, grouped by project. Each row shows the agent mark, name, state, and the device it was started from when that was not this computer.
- **Rooms.** Collaboration rooms. Each row shows unread count, Goal status and bound agents. There is no limit on rooms (P1).
- **Remote access.** This computer's single device room (P1): the local account, link and QR code, your devices, and new-device approvals (D12, D14).
- **Needs you.** A queue in the title bar, not a page; see §6.4.
- **Inspector.** A context-dependent right panel. For a session it shows details, files, cost and binding; for a room, Goal, tasks and members. Collapsible, and remembered per page.

### 3.2 Owner away (phone first, also laptop)

After signing in as the local account, you see **Sessions**, **New** and **Needs you**, plus a **Device** sheet (this device's name, sign out). Tabs sit at the bottom on phones and become a sidebar on laptops.

Settings that could lock you out stay on the computer: password, link, cloud address, devices, projects and budgets. Collaboration rooms are not in the remote web app for v1; the owner uses the room's own browser page instead.

### 3.3 Collaborator (browser)

The collaborator sees a room page only (§5.6), reachable from an invite. The cloud's public home keeps the two-door entry from v0's redesign: *Remote device* and *Collaboration room*, plus a single "paste a link" box.

### 3.4 Objects and their names

Use one name per object, everywhere, in both languages.

| Object | en | zh-CN | Notes |
|---|---|---|---|
| Agent runtime | Agent | 智能体 | Pi, Codex, Claude Code, each with its own color mark |
| Session | Session | 会话 | One running or stopped conversation with an agent |
| Project | Project | 项目 | An approved folder |
| Device room | Remote access | 远程访问 | Exactly one per computer |
| Local account | Account (on this computer) | 本机账号 | One per computer; remote devices sign in as it (D12) |
| Device | Device | 设备 | A browser signed in as the local account |
| Collaboration room | Room | 协作房间 | Any number |
| Goal / Task | Goal / Task | 目标 / 任务 | Room-level |
| Binding | Agent in room | 房间中的智能体 | A session acting as a room member |
| Binding modes | Respond · Autonomous | 仅回复 · 自主推进 | Respond is the default |
| Needs you | Needs you | 待你处理 | The human queue |

## 4. Global chrome

- **Title bar.** On macOS: traffic lights and a draggable area. On Windows and Linux: custom controls. It holds global search / command palette (⌘K or Ctrl K), the **Needs you** button with a count, and a connection chip: `● Connected`, `● Cloud unreachable`, or `● Daemon restarting`. Clicking the chip explains it and offers fixes.
- **Command palette** (⌘K). Jump to any session, room or page; actions such as New session, Share link, Pause all autonomy and Toggle theme; and recent items. It can do everything the menus can.
- **Tray / menu bar.** Shows connection state, the Needs-you count, *New session*, *Copy remote link*, *Pause all autonomous work* and *Open Café Space*. Quitting the window never stops agents, and the tray says so.
- **Notifications (OS).** They cover: approval needed, autonomy stopped, a room mention of you or your agent, a review requested, and (opt-in) session finished. Each opens the exact place it refers to.
- **Theme.** Dark by default, like cafeshop.ai; also Light and System. Language: zh-CN / en.

## 5. Key flows

### 5.1 First run (≤ 2 minutes; steps III and IV skippable)

The welcome and agent detection share the first screen, so the user sees value before any setup. Steps are numbered with italic serif roman numerals, as on cafeshop.ai.

1. **I · Agents.** Beside one line of what Café Space is and the café line-art, detection runs immediately and states that nothing is sent to any model.
   - If the computer has a proxy configured, the step shows it with its credentials masked: "Agents need this proxy to reach their models? **Use it**." Agents get proxy, certificate and SSH-agent access only when the owner chooses it here or in Settings, never by default.
   - Each agent shows a version and a sign-in state: *Ready*, *Sign in needed* (with the agent's own sign-in command, copyable) or *Not installed* (with a link).
   - At least one Ready agent is required to continue; otherwise the screen explains how to install one.
2. **II · Project.** Choose a folder; it becomes the first approved project.
3. **III · Remote access (optional).** It creates the **local account**: a display name and a password. One sentence explains: "Only you can sign in — other devices use this account."
   - **Import from v0** appears here only if v0 is detected: "Keep your existing room link". Your old room password becomes the account password (R1 import, with confirmation).
4. **IV · Done.**

The app ends on an empty Sessions page with **New session** in focus.

### 5.2 Start a session (⌘N)

Opens as a sheet over the current page, in one step:

- **Project** (search list with paths).
- **Agent** (Ready ones first; others disabled with the reason).
- **Start as:** *New* · *Resume a previous session* (shows the agent's own list) · *Attach to a running session* (only for agents that support it; under D12 every user is the owner, so it is available remotely too).
- **Permission profile** for that agent: e.g. *Read-only*, *Ask before changes*, *Full auto*. These are the agent's own terms, shown with one line each on what they allow.
  - When an agent has no permission modes of its own, the sheet says so instead of offering a choice. The mark follows `toolApproval` (architecture D18), on the computer and remotely, in the sheet and in the session header (truthful automation):
    - `never_asks`: *Runs tools without asking*, with one line: "This agent edits files and runs commands in this project without confirmation." Pi 1.5 is launched without extensions, so it never asks.
    - `may_ask`: *May ask before some tools*. It never claims every tool is confirmed.
    - `unknown`: *Doesn't say whether it asks*. Never shown as safe.
- **Name** (optional).

**Start.** The new session appears in the sidebar immediately as *Starting…*, with a three-step progress strip (Launching → Connecting → Ready). On failure the strip names the failed step and the reason code in words, with **Try again**, which is safe because creation is idempotent. *Unknown* outcomes say "Check the list before starting another".

### 5.3 Work in a session (the core screen)

- **Header.** It shows:
  - the agent mark and name, plus the session name (editable);
  - project chip;
  - model chip, read-only with a tooltip when the agent fixes its own model;
  - state pill;
  - origin badge, only when the session was started elsewhere (*From iPhone*);
  - room binding chip, if any (*In "Launch plan" · Autonomous 6/20 turns*).
- **Transcript.**
  - User and assistant messages are typeset for reading: 15 px sans, 68ch measure.
  - **Tool calls** are compact cards: name, a one-line argument summary, status and duration, expandable to full input and output.
  - Thinking is collapsed by default.
  - Plans render as checklists.
  - **Approval requests** appear inline as a highlighted card (§6.3).
  - Errors are plain sentences, with the raw code behind *Details*.
- **Composer.**
  - Enter sends; Shift+Enter adds a new line; @ references files; / runs the agent's commands (when supported).
  - While running, it offers **Steer** and **Queue** only if the agent supports them. Otherwise the composer stays editable with "Send when the agent is idle" (draft kept).
  - **Stop** appears only for turns you started (an attached turn shows "Started in the native app — stop it there").
- **Inspector.** Details (agent, version, cwd, started by), cost and usage if the agent reports them, files touched, and room binding with its controls.

### 5.4 Remote access (at the computer)

D12: only the owner, signed in as this computer's **local account**, can use it remotely. The page has two states.

1. **Not set up.**
   - One sentence: "Use your agents from your phone or another computer. Only you can sign in."
   - The cloud address (default `space.cafecode.work`, P5, with a *Change* link).
   - **Set up**, which creates the local account: display name, plus a password with a strength hint (6–20 characters, for v0 compatibility). Then it creates the link.
2. **Live.** Left column:
   - the account card ("Signed in as **Alex** on other devices");
   - a large QR code and the link with **Copy**;
   - status with menu-style dotted leaders: *Cloud … reachable*, *Last connection … direct (P2P) · 14:02*.

   Right column:
   - **Your devices**: device name, browser, *Connected now* or *Last seen*, and **Sign out** on each;
   - the **New devices need approval** switch (on by default, D14);
   - **Pending sign-ins**: "Pixel 9 · Chrome · 1 min ago", with **Approve** and **Deny**. The same item also appears in Needs you and as an OS notification.

Danger zone (bottom of the page; each action shows its consequence first):
- **Change password.** Every other device is signed out.
- **Reset link.** Old links stop working and every device is signed out; the password is kept.
- **Change cloud address.** The room moves and old links stop working; collaboration rooms on the old address are unaffected.

These three actions, device approval and revocation exist **only at the computer**, never on a remote device.

### 5.5 Signing in from your phone

1. **Room preview.** Opening the link shows the computer's name and online state, and "Private computer · only Alex can sign in".
2. **Sign in.** The screen says **Sign in as Alex**, with one field: Alex's password.
   - A wrong password shows the attempts remaining and a short wait when rate-limited.
   - Copy for someone else holding the link: "Not Alex? Ask for an invite to a collaboration room instead."
3. **New-device approval (D14).** On a device's first sign-in, the phone shows *Waiting for approval on Alex's MacBook Pro*. The computer gets a Needs-you item and an OS notification, and the phone continues as soon as it is approved.
4. **Sessions.** Inside, the phone shows all sessions. You can prompt, steer, stop your own turns and answer approvals.
5. **New.** Start a session in any project, exactly as at the computer.
6. **Sign out.** It is in the Device sheet. The computer's device list always shows this phone until it signs out or is revoked.

### 5.6 Collaboration rooms

- **Rooms list.** Each row shows the title, unread count, Goal status pill, member faces (humans as initials, AIs with agent marks) and an expiry hint ("expires in 3 days").
- **Room page.** Feed in the center; the right inspector has tabs **Goal · Tasks · Members**.
  - **Feed.** Messages with @mentions, replies and *To:* markers. Messages addressed to *your* bound agents show small delivery marks (§6.2).
  - **Goal card.** Title, status (active · paused · review · done), revision, reviewer and criteria (collapsed). Actions depend on role: Pause/Resume for anyone; Accept/Reject for the reviewer only; Submit when all tasks are done.
  - **Tasks.** A three-column board (To do · Doing · Done) with assignees and links to the result message.
  - **Members.** Presence for each member, and for AIs, a truthful mode: *Manual*, *Listening*, or *Wake confirmed*. Your own bound agents also show listener health: *Listening*, *Reconnecting since 14:02*, or *Stopped: room closed*. Listening is held by the daemon, so it does not stop when an agent's turn ends.
- **Create room.** Title and your name, then the room opens with an **Invite** sheet: a link for people, and a ready-to-send instruction for an AI, carried over from v0.
- **Join room.** Paste the invite, see the preview, choose a name, then **Join**. Joining always asks for confirmation.

### 5.7 Put an agent in a room (binding)

From a room (*Add an agent*) or from a session (*Put in a room*):

1. **Choose session.** Pick an existing session or start a new one in a project.
2. **Mode.**
   - **Autonomous** (default, P10). The agent works on the room's active Goal, step by step, and keeps going until the Goal ends, the budget is spent or it reports a blocker.
   - **Respond.** The agent takes a turn only when a room message is delivered to it.
   - One plain line states the consequence, with the agent's tool-approval mark beside it (D18). For a profile without isolation: *Anyone with this room's link can direct this agent. Its tools run as you on this computer, not only inside the project.* If the agent *Runs tools without asking*, the line adds that this includes editing files and running commands without confirmation. Never call the project a sandbox (architecture §8).
3. **For Autonomous, a budget card in menu style.**
   - It reads like the cafeshop.ai price list, with dotted leaders:
     ```text
     Turns .............. 20
     Time ............... 2 h
     Cost cap ........... $5 (if the agent reports usage)
     Stops after ........ 3 turns without progress
     ```
   - Defaults follow D10, and every value can be edited.
   - Below it, a plain list of **when it stops**: the Goal is paused, in review, done or deleted; AI is paused in the room; the budget is spent; you press Stop.
4. **Confirm** creates the binding. The session header and the room's member list both show it.

**While autonomous**, a slim progress strip on the bound session and in the room shows "Turn 6 of 20 · 41 min · $1.20", the last reported status (*Next: run the migration tests*), and **Pause** and **Stop**. A turn that ended without a status shows *No status — asking it to continue* while the one follow-up turn runs. When it stops, one Needs-you item explains why ("Stopped: no progress for 3 turns. Last step: …") and offers **Resume** (if the Goal is active) or **Review**.

### 5.8 Goal review

- The **reviewer** gets a Needs-you item when work is submitted. The review sheet shows:
  - the criteria;
  - the submitted result message;
  - linked tasks;
  - **Accept** or **Reject with note**. Reject returns the Goal to active.
- **Other members** see *In review*, and autonomous agents stop.

### 5.9 Updates

- **App-only update.** The title bar shows a dot and "Update ready", and restarting the window never stops agents.
- **Update that includes the daemon.** The dialog says "Running sessions will stop and can be resumed" and offers **Update when idle** (default) or **Update now**. After the update, stopped sessions show **Resume**.
- **Waiting for idle.** The title bar reads *Update waits for idle* while work continues. Quitting the app cancels a waiting update; the next launch offers it again.
- **Update now.** Before confirming, the dialog lists the sessions that will stop.

### 5.10 Recovering from the unexpected

- **Daemon restarting.** A connection chip plus a non-blocking banner. The UI keeps the last state, clearly marked *Last known*.
- **Cloud unreachable.** Local sessions are unaffected. Share and Rooms show *Offline* with the last sync time, and nothing pretends to be live.
- **Unknown outcome.** An inline notice where the action happened, plus a Needs-you item: *Check* (opens the relevant view) and *Mark as done / Retry*, which only appears after checking.
- **Update not confirmed.** A new daemon whose update was never confirmed accepts no work (architecture §6). Every client, the CLI included, shows *Update not finished*, with two actions: **Finish update** and **Go back to the previous version**. They follow the handover rules, so recovery never depends on the desktop app alone; the CLI gets `cafe-space update finish` and `cafe-space update rollback`.
- **Recovery needed.** When the daemon refuses to start because the state on disk does not match any version it can safely run, the app explains this in plain words and lists the files involved. The only action offered is an explicit restore, such as the backup taken before a migration. Nothing is downgraded or overwritten automatically.
- **Backup restored.** After a restore the daemon still accepts no work: the restored data belongs to the previous version (`ROLLBACK_REQUIRED`). The screen says *The data from before the update is back. This version cannot use it; go back to the previous version. You can update again later.* Its one action opens **Go back to the previous version**, which is not blocked in this state. Remote clients see the reason and are asked to finish on the computer itself; the CLI shows the same state and points to `cafe-space update rollback`.

## 6. Interaction patterns

### 6.1 States and their looks

| State | Pill | Color token | Motion |
|---|---|---|---|
| Starting | `Starting…` | info | 3-step strip |
| Idle | `Idle` | fg-2 | none |
| Running | `Running` | brand | **steam** above the agent's cup mark, slow, 2.4 s loop; static under reduced motion |
| Waiting for you | `Needs you` | warn | none (no blinking); counts toward the queue |
| Stopped | `Stopped` | fg-3 | none |
| Failed | `Failed · reason` | bad | none |
| Unknown | `Unknown — check` | warn, dashed outline | none |

Command outcomes use the same vocabulary everywhere:
- *Sent* (dispatched; a clock icon);
- *Done* (applied; a check);
- *Not done · reason* (rejected);
- *Unknown — check* (dashed).

"Sent" never uses a check mark.

### 6.2 Delivery marks (room messages to your agents)

These are four small dots, shown on hover and on a tap target for touch: **fetched · queued · delivered · handled**. A filled dot marks a reached state and a dashed dot an unknown one. They appear only on messages addressed to agents you bound, in the order of the architecture's §8.

### 6.3 Approval requests

- **The card.** Inline in the transcript, highlighted with the warn-soft background (not a left border). It shows:
  - what the agent wants to do, in one sentence;
  - the exact command or file, in mono;
  - **Allow once**, **Deny**, and, only when the agent supports it, *Allow for this session*.
- **Elsewhere.** It also appears in Needs you and as an OS notification.
- **Any device.** Whichever of your devices sees the card first can answer it. The answer clears it everywhere, and a second answer is refused.
- **Timeout.** An unanswered request never auto-approves; on timeout it shows *Expired — denied*.

### 6.4 Needs you

A popover from the title bar, grouped and ordered by urgency:
1. Approvals.
2. Autonomy stopped.
3. Reviews.
4. Unknown outcomes.
5. New-device sign-ins (D14).

Each item has the primary action inline and a link to the exact place it belongs. Items clear themselves when resolved elsewhere. The count is the only badge in the app.

### 6.5 Confirmations

Three levels; buttons always name the action ("Reset link", never "OK"):
- **None.** Reversible, local actions.
- **Simple confirm.** Close session, leave room.
- **Consequence sheet.**
  - Starting autonomy, approving a new device, resetting the link, changing the password or the cloud address.
  - The sheet states the consequence, and needs an explicit checkbox where trust is extended (autonomy, new device).

### 6.6 Empty, loading and offline

- **Empty states** show one line-art illustration (a cup, a table, an open door), one sentence and one action.
- **Loading** uses skeleton rows; nothing spins for more than 400 ms without saying what it waits for.
- **Offline data** carries *Last updated 14:02*.

### 6.7 Keyboard

| Keys | Action |
|---|---|
| ⌘K | Command palette |
| ⌘N | New session |
| ⌘1 / ⌘2 / ⌘3 | Sessions / Rooms / Share |
| ⌘. | Stop own turn |
| ⌘⇧A | Open Needs you |
| ⌘\ | Toggle inspector |
| ⌘, | Settings |

Every control is reachable with Tab, and focus rings use the brand color (2 px, offset 2). Windows and Linux use Ctrl in place of ⌘.

## 7. Visual system

### 7.1 Tokens

Café Space adopts cafeshop.ai's semantic tokens unchanged, renaming the prefix from `--eg-*` to `--cs-*`, and adds only what Space needs.

| Token | Dark (default) | Light | Use |
|---|---|---|---|
| `canvas` | `#251d15` | `#f5f3f0` | Window background |
| `panel` / `panel-2` / `panel-3` | `#2d261f` / `#332c25` / `#3c352e` | `#ffffff` / `#faf8f6` / `#f0ece7` | Surfaces, raised 1–3 |
| `hair` / `hair-2` | `#fff0e1` 7% / 14% | `#3c2814` 8% / 15% | Hairlines and borders, warm |
| `fg` / `fg-2` / `fg-3` | `#f3eee8` / `#baafa4` / `#988d83` | `#1d1915` / `#675e55` / `#998f85` | Text, primary to tertiary |
| `brand` / `brand-2` / `brand-soft` | `#dca36a` / `#e8b783` / 14% | `#9f5f29` / `#85501f` / `#f6ece1` | Accent, links, focus |
| `fill` / `on-fill` | `#a3632d` / `#fff8f0` | `#9f5f29` / `#fffaf4` | Primary buttons |
| `ok` `warn` `bad` `info` (+ `-soft`) | `#5dbd88` `#e1aa4b` `#ec7a68` `#7ea8da` | `#2e8756` `#b0700b` `#c23f2e` `#3b6ea5` | Status |
| `c1…c7` | cafeshop categorical set | same | Agent marks and member faces |
| `shadow-1…3`, `glass` | cafeshop values | cafeshop values | Elevation and floating bars |

**Space additions: text-safe variants.** I measured contrast for all pairs (WCAG 2.2). These fail 4.5:1 for body-size text, so Space adds `*-ink` variants for text use only; fills and icons keep the originals.

| Fails | Ratio | Text variant (passes ≥ 4.6 on canvas, panel and panel-3) |
|---|---|---|
| Light `fg-3` text | 2.7–3.2 | `fg-3-ink` `#71675e` |
| Light `ok` / `warn` text | 3.8–4.0 / 3.5–4.1 | `ok-ink` `#28764b` / `warn-ink` `#935e09` |
| Light `brand` text on panel-3 | 4.31 | `brand-ink` `#995b27` |
| Light `bad` / `info` text on panel-3 | 4.42 / 4.51 | `bad-ink` `#bc3d2d` / `info-ink` `#3a6ba1` |
| Dark `fg-3` text on panel-3 | 3.72 | `fg-3-ink` `#a89f97` |
| Dark `bad` text on panel-3 | 4.34 | `bad-ink` `#ed8271` |

Primary button text passes as is: 4.55 dark, 4.88 light.

### 7.2 Typography

| Role | Face | Size / line | Notes |
|---|---|---|---|
| Display | Serif stack: `"Iowan Old Style", "Palatino Linotype", Palatino, "Book Antiqua", Georgia, serif` | 28–34 / 1.3 | Page titles, empty states, the wordmark's voice |
| Eyebrow | Serif *italic*, brand color | 13–15, +0.03em | "The bar", "Your rooms", section numerals `I II III` |
| UI | `Inter, -apple-system, BlinkMacSystemFont, "Segoe UI", "PingFang SC", "Microsoft YaHei", sans-serif` | 13 / 1.45 (controls), 14 (lists), 15 / 1.6 (reading) | Never below 12 px for text |
| Numbers | Serif for menu-style figures (budgets, prices); tabular sans for live counters | — | As on cafeshop.ai's price list |
| Code | `"SF Mono", ui-monospace, Menlo, Consolas, "Liberation Mono", monospace` | 12.5 / 1.55 | Tool input and output, paths |

The CJK fallback is PingFang SC or Microsoft YaHei for UI. Display serif headlines in Chinese fall back to the system Song or Ming face, so check the rendering per platform before ship.

### 7.3 Space, shape and depth

- **Grid.** A 4 px base: spacing steps 4, 8, 12, 16, 24, 32, 48.
- **Width.** The sidebar is 248 px and the inspector 320 px; main content is fluid, with reading text capped at 68ch.
- **Radii** follow cafeshop.ai: 6 px for small controls and chips, 8 px for inputs and buttons, 12 px for panels and cards, 999 px for pills and avatars.
- **Borders and depth.** Hairlines rather than heavy borders. Elevation is `shadow-1` for cards, `shadow-2` for popovers and `shadow-3` for sheets and dialogs. Floating title and composer bars use `glass` with a backdrop blur.

### 7.4 Iconography and illustration

- **Icons.** 1.5 px stroke line icons on a 20 px grid, rounded joins, `currentColor`, with no filled icon sets mixed in.
- **Agent marks.**
  - Each runtime gets a monogram tile in its categorical color (`c1…c7`).
  - Vendor logos are not used inside the product UI unless licensed.
  - The cup mark with **steam** is Space's activity motif, as in v0's `CoffeeActivityMark`.
- **Illustration.**
  - Fine gold line-art in `brand`, on a faint dotted grid, as in cafeshop.ai's "Made for your everyday" slide. It appears in onboarding, empty states and the Remote access page.
  - The cafeshop.ai character art may appear on the welcome screen only, if brand-cleared. The product screens stay calm.

### 7.5 Motion

cafeshop.ai's timing tokens: press 90 ms, fast 140 ms, base 220 ms and slow 320 ms, on `cubic-bezier(.2,0,0,1)`.

- **Use motion for:** state changes, sheet entry, list insertion, and the steam on running agents.
- **Never animate:** streaming text, counters, or attention-seeking pulses on Needs you.
- `prefers-reduced-motion`: the steam becomes a static cup and sheets fade instead of sliding.

## 8. Content and voice

- **Warm and plain.** Short sentences, no hype, no blame. Say what happened, then what to do.
- **Honest.**
  - "Sent to Codex. It hasn't confirmed yet."
  - "We couldn't confirm this. Check the session before trying again."
  - "Only you can sign in — other devices use this account."
- **Café flavor lives only in eyebrows and empty states** ("Freshly brewed", "The bar is quiet"), never in buttons or errors.
- **Language.** zh-CN is primary. Never machine-translate: both languages are written by hand and kept in one catalog.

## 9. Accessibility

- **Contrast.** Text meets 4.5:1 using the token table in §7.1, and UI component boundaries meet 3:1. Status is never color alone: it always has a word and an icon.
- **Interaction.** Fully keyboard operable, with visible focus. The transcript is an ARIA log region; state changes and new approvals are announced politely; approvals use an alert.
- **Targets.** Touch targets are ≥ 44 px on phone layouts. Pointer targets are ≥ 28 px with ≥ 8 px spacing on desktop.
- **Motion.** Reduced motion is honored, and no information is conveyed only by motion.
- **Scaling.** Text scales to 200% without loss; the inspector moves below the main content.

## 10. Platforms and sizes

- **macOS.** Native traffic lights, vibrancy off (solid canvas), ⌘ shortcuts, menu-bar item.
- **Windows and Linux.** Custom title-bar controls on the right, Ctrl shortcuts, a tray icon. WebView differences, such as IME and clipboard, are in the QA matrix.
- **Window sizes.** Minimum window is 900×600. Below 1100 px the inspector becomes an overlay sheet, and below 760 px (remote web) the sidebar becomes bottom tabs.
- **Phone (remote).** Layouts start at 390 px. Body text is 16 px in inputs (avoids iOS zoom), sheets come up from the bottom, and the composer stays above the keyboard.

## 11. Open UX questions

1. **Brand assets.** May Space use the cafeshop.ai wordmark treatment and character art, or only the token system?
2. **Agent marks.** Monogram tiles (current proposal), or licensed vendor logos?
3. **Remembered sign-in on your phone (D12).** How long should a device stay signed in? Proposal: 30 days since last use, and always ending on a password change or revocation.
4. **Autonomy budget defaults (D10).** Are 20 turns, 2 h and $5 right for most owners?
5. **Rooms list scale.** With unlimited rooms (P1), when do we add folders or archive? Proposal: archive automatically on expiry, folders later.

## 12. Mockups

The mockups are kept in [`mockups/`](mockups/): eight artboards, laid out by `canvas.json`. The names in them ("Alex", "Alex's MacBook Pro") are placeholders:

- the visual foundations;
- desktop **Session**, **Room**, **Remote access** and **New session**;
- **Welcome**;
- phone **Sign in** and **Session**.

They implement this document's decisions in the dark theme. Light theme and zh-CN copy are produced from the same tokens and catalog.
