# M4a — PC test build

Decided 2026-11-12 (architecture §15, order): the PC comes first. Right after M1, the owner gets an
unsigned Windows x64 installer and uses Café Space on their own computer. macOS follows the same
path once Windows passes. Signing, notarization, the updater and the tray stay in M4.

## Tasks

| Task | Owner | What |
|---|---|---|
| #97 | Pi | Turn on the Tauri bundle (NSIS, x64, unsigned). Ship `cafe-spaced` and the `cafe-space` CLI as sidecars. The first start installs the per-user daemon through the existing installation flow. Uninstalling removes the daemon and its login item. |
| #98 | Codex | A manually triggered CI workflow that builds the installer from a fixed commit and uploads it as an artifact with its SHA-256. No signing, no automatic release. |
| #99 | Front end | First run: an install button when the daemon is missing (the Settings installation panel), a guide to approve a first project folder, and a check that says whether Pi and Codex are usable and what is missing. |
| #100 | Claude | This checklist, and review of #97–#99. |
| #101 | Pi | The same for macOS (unsigned dmg, arm64 first), after Windows passes. |

The M1 real-agent smoke through the ordinary UI (#29) is part of the owner's acceptance below.

## Owner acceptance on Windows

Run on the owner's own PC, from the installer the CI built for one fixed commit. Write down the
commit and the installer's SHA-256 before starting.

1. **Install.** Run the installer. Windows warns that the publisher is unknown, because the build is
   unsigned; continue. The app opens and offers to install the daemon. After installing, the status
   in the top right says it is connected.
2. **First project.** Approve one real project folder. It appears in Settings · Projects.
3. **Agents.** The new-session sheet lists Pi, and Codex if it is installed. A missing agent says
   what is missing instead of failing later.
4. **A real Pi session.** Create a session in the project, send a short task, and watch the reply
   stream in. If the agent asks to run a command, the approval card appears; try both Allow once
   and Deny.
5. **Cancel.** Send a longer task and press Stop while it runs. The session goes back to idle and
   says the turn was stopped.
6. **Close.** Close the session. It shows as stopped in the list.
7. **Restart.** Quit the app and sign out of Windows, or restart the daemon from Settings. After
   signing in again, the daemon starts by itself, the app reconnects, and every session shows its
   true state; nothing that already happened is replayed.
8. **Command line.** In PowerShell 5.1, run `cafe-space status` and `cafe-space session list`.
   Chinese titles display correctly.
9. **Uninstall.** Uninstall from Windows Settings. The daemon is no longer running, its login item
   is gone, and the project folders are untouched. Reinstalling keeps or clearly resets the data,
   as the installer says.

The build passes when every step works without a terminal workaround. Anything that needs one is
a finding with its step number, not a pass.
