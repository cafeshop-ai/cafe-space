# M4b — Usable first

Decided 2026-11-17 by the owner, after trying the M4a preview: the app "is empty, there is nothing to
use". The top goal is that a user can install Café Space on a PC and use it right away; everything
else is improved step by step afterwards. The reference for setup speed is Magpie
(`yetone/magpie`, MIT, read at `be6f8b7`): it lists every coding agent installed on the machine,
uses the agents' own sign-ins, and needs no configuration before the first use.

## Decisions

- **Filesystem isolation is deferred.** `filesystem-isolation.md` stays a draft. No container, ACL
  or proxy trial runs on the owner's PC until the owner reopens it. The owner uses a local proxy, so
  the proxy handshake remains the first gate when it does.
- **No folder step.** The first run no longer asks the user to approve a project. A new session
  starts in the folder chosen in the new-session sheet, which defaults to the last folder used, or the
  user's home folder the first time. "Choose folder…" in the sheet opens the system picker. On this
  computer a chosen folder is recorded as a project without a separate approval; remote clients pick
  only from folders already recorded, and never send a path.
- **Agents come to the user.** Every supported agent installed on the computer appears ready, with no
  registration step. Agents run with their own sign-ins and the owner's full environment (D20).
- **Agents to support**, in order: Claude Code, Codex, Pi (already works), Antigravity CLI (`agy`);
  then the other agents on Magpie's list that offer a machine interface (ACP or a documented JSON
  mode), one at a time.

## Tasks

| Task | Owner | What |
|---|---|---|
| #110 | Front end + Pi | **Open and use.** The first start installs the daemon by itself and shows its progress (no install button; a failure says what failed and offers retrying the install). The folder step is removed; the new-session sheet gets the folder choice described above. A daemon request lets this computer start a session in a chosen folder; remote requests keep using recorded projects only. |
| #111 | Pi | **Find installed agents.** Detect each supported agent from its command on PATH (on Windows, the user and system PATH read from the registry at detection time, since the daemon started at logon can hold an old one), the usual install folders (`~/.local/bin`, npm, pnpm, bun, Volta) and the folder the agent creates (`~/.claude`, `~/.codex`, `~/.pi`, `~/.gemini`, Antigravity's). Read-only, nothing executed (as #102). A found Pi registers itself. |
| #112 | Pi | **One adapter for ACP agents.** #64's adapter boundary first, then one ACP adapter. Adapters are pinned and installed on first use into a private folder, with a registry mirror for users in mainland China (gonggong-space pattern, `docs/research/reference-projects.md`). |
| #113 | Pi | **Claude Code** through `@agentclientprotocol/claude-agent-acp`, using the user's existing Claude Code sign-in. |
| #114 | Pi | **Codex** through `@agentclientprotocol/codex-acp` (replaces #30's plan), using the user's existing Codex sign-in. |
| #115 | Claude | Done: `docs/research/acp-agents.md`. Antigravity CLI speaks ACP natively (`agy acp`); most of Magpie's list does too. |
| #116 | Pi | **Antigravity CLI** through the user's own `agy acp`, after #112; when it is not signed in, the agent row gives `agy`'s own sign-in. |
| #117 | Pi | **The other ACP agents** as rows in a pinned launch table (command, ACP arguments, detection paths), each with one real smoke: start, prompt, stream, cancel, close. Nothing proprietary is downloaded. |

#111 and #112 start in parallel; #113, #114 and #116 follow #112, then #117; #110 is independent. Each lands with
non-author review and green CI on the same commit before it merges.

## Implementation checkpoints

- **#111 backend slice (not the M4b acceptance):** owner-authenticated local IPC `list_runtimes`
  refreshes discovery on a blocking worker; remote `Host::dispatch(ListRuntimes)` and subscription
  snapshots only read the cache. The existing Pi discovery and runtime-list scan share one permit;
  disconnected queued callers do not leave extra scans behind. Commands and profile contents are
  never executed/read, and discovery does not change the D20 child environment. A valid pinned Pi
  is registered only when no private registration record exists; explicit removals and invalid
  records are preserved. Claude Code, Codex, Pi and Antigravity CLI can be detected; without a
  launch adapter they remain unavailable rather than being reported ready. Startup refresh and the
  Agent-row presentation still need integration with #110 and #112–#116.

## Done when

On a Windows PC that already has Claude Code, Codex and Pi installed and signed in: install Café
Space, open it, and without any other setup start a session with each of the three agents in a
folder of choice, prompt it, see the reply stream in, stop a running turn and close the session.
The M4a acceptance (`m4a-pc-test-build.md`) is then run again on that build.
