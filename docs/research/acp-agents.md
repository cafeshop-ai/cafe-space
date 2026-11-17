# Coding agents with a machine interface (#115)

> Date: 2026-11-17. For M4b (`docs/plans/m4b-usable-first.md`): which agents Café Space can launch and
> talk to. Sources: the official ACP agent list (<https://agentclientprotocol.com/overview/agents>),
> the ACP registry (<https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json>, version
> 1.0.0, 46 agents, read today), Antigravity's CLI docs, and Magpie's README agent list
> (`docs/research/reference-projects.md`). Versions are the registry's on the date above.

## Result

Almost every agent the owner uses speaks the Agent Client Protocol, natively or through a published
adapter. One ACP adapter in the daemon (#112) therefore covers Claude Code, Codex and Antigravity
CLI, and most of Magpie's list after them. Pi keeps its existing RPC adapter.

## The owner's agents

| Agent | How it speaks ACP | Sign-in it uses |
|---|---|---|
| Claude Code | Adapter `@agentclientprotocol/claude-agent-acp` (registry `claude-acp`, 0.107.2), over the Claude Agent SDK | The user's Claude Code sign-in |
| Codex | Adapter `@agentclientprotocol/codex-acp` (registry `codex-acp`, 2.7.0) | The user's Codex sign-in |
| Antigravity CLI (`agy`) | Native: `agy acp` starts an ACP server over stdio ([docs](https://antigravity.google/docs/cli/acp/)). Also a headless mode, `agy -p … --output-format stream-json` ([docs](https://antigravity.google/docs/cli/headless/)) | OAuth cached in the OS keyring by running `agy` once, or `agy acp --login`; also Gemini API key or Google ADC ([docs](https://antigravity.google/docs/getting-started?tab=cli)). Installs to `~/.local/bin/agy`, on Windows `%LOCALAPPDATA%\agy\bin` |
| Pi | Café Space's own RPC adapter (M1). An ACP adapter `pi-acp` also exists (registry 0.0.34) | Pi's own (`~/.pi`) |

The registry's `antigravity-acp` entry is a separate Google-hosted binary (`agy_acp_server`), not
the `agy` the user installs. Café Space should launch the user's own `agy acp`.

## Others on both Magpie's list and the ACP registry

All native ACP. For npm-distributed agents the registry gives the package and its ACP arguments,
not the installed command name; #111 confirms each command name from the package before adding a row.

| Agent | Registry launch |
|---|---|
| Gemini CLI | `@google/gemini-cli` 0.68.0, `--acp` |
| Qwen Code | `@qwen-code/qwen-code` 0.27.4, `--acp --experimental-skills` |
| OpenCode | binary `opencode`, `acp` |
| Goose | binary `goose`, `acp` |
| Kimi Code | `@moonshot-ai/kimi-code` 2.6.2, `acp` |
| GitHub Copilot CLI | `@github/copilot` 1.0.112, `--acp` |
| Cursor CLI | binary `cursor-agent`, `acp` |
| Mistral Vibe | binary `vibe-acp` |
| Factory Droid | `droid` 1.12.1, `exec --output-format acp-daemon` |
| Cline | `cline` 3.0.74, `--acp` |
| Qoder CLI | `@qoder-ai/qodercli` 1.1.90, `--acp` |
| Grok Build | `@xai-official/grok` 1.0.88, `agent stdio` |
| CodeBuddy Code | `@tencent-ai/codebuddy-code` 2.184.0, `--acp` |
| Devin | binary `devin`, `acp` |
| MiniMax Code | `@minimax-ai/code` 0.2.7, `acp` |
| Muse Code | binary `muse`, `acp` |
| Kiro CLI | binary `kiro-cli`, `acp --agent-engine=v3` |

The registry has 46 agents in all; the rest are not on Magpie's list.

## What this means for the design

- **Use what the user installed.** Magpie-style discovery (#111) finds the agent's own command, and
  the daemon launches it with its ACP arguments. Café Space downloads nothing for agents that speak
  ACP natively. Only Claude Code and Codex need an adapter package, pinned and installed on first use
  (#112). Proprietary binaries from the registry are never fetched automatically.
- **One table, not one task per agent.** After Claude Code, Codex and `agy` work, each further agent
  is a row in a pinned launch table (command, ACP arguments, detection paths) plus one real smoke
  test: start, prompt, stream, cancel, close. The registry is a source for that table, read at a
  fixed version, never fetched at run time.
- **Sign-ins stay with the agents.** No agent asks Café Space for a key. When an agent is installed
  but not signed in, the agent row says so and gives the agent's own sign-in command (UX §5.1
  "Sign in needed").
