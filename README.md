# Vario

[![CI](https://github.com/kdev1966/vario/actions/workflows/ci.yml/badge.svg)](https://github.com/kdev1966/vario/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://github.com/kdev1966/vario/blob/main/LICENSE)

**English** · [Français](https://github.com/kdev1966/vario/blob/main/README.fr.md)

**Vario** makes your AI coding agents delegate the simple work to cheaper models. It gives each agent two economical sub-agents and short delegation guidance, through the agent's own mechanisms: no proxy, no API key, nothing sent anywhere. Your main session keeps its strong model, and its prompt cache, for design, debugging and review; reading, summarizing and well-defined tasks go to a sub-agent on a cheaper model.

It runs on **macOS, Linux and Windows**, as a command (`vario`) and as an application (`vario-gui`).

> Vario is young (phase 2 of its [roadmap](vario-document-de-conception.md), in French). Delegation has been tried for real with Claude Code and OpenCode. Cursor, Codex, Gemini CLI and VS Code follow their documentation and are covered by tests; Gemini CLI and VS Code were installed on a real machine, without a delegation tried yet.

## How it works

Vario installs two sub-agents in every client it finds:

| Sub-agent | Role | Tools |
| --- | --- | --- |
| `vario-reader` | read and summarize: locate code, extract values, summarize a file or a long tool output | read only |
| `vario-worker` | carry out a bounded task that comes with a check: rename, apply a known pattern, run known commands | read, edit, shell |

Each sub-agent ends its answer with a short report (`RESULT`, `CHECKS`, `UNCERTAINTIES`, `ESCALATE`). A failed check sends the task one level up, once; the main session decides the rest.

| Client | Sub-agents and model | Delegation guidance | Hooks |
| --- | --- | --- | --- |
| **Claude Code** | `~/.claude/agents/`, `haiku` and `sonnet` | a block in `~/.claude/CLAUDE.md` | session start → gauge; end of each sub-agent → journal; before each prompt → quota thermostat; a status line that reads the quota |
| **Cursor** | `~/.cursor/agents/`, Composer 2.5 (Cursor's own model, no third-party billing) | injected at session start (Cursor has no global rules file) | prompt submit → active client and gauge; session start → guidance and gauge; end of each sub-agent → journal |
| **OpenCode** | `~/.config/opencode/agents/`, your `small_model` for the reader, else the caller's model | the rules file OpenCode reads (`AGENTS.md`, or `~/.claude/CLAUDE.md`) | a plugin: prompt submit → active client; response end → usage refresh; start → gauge; end of each sub-agent → journal |
| **Codex** | `~/.codex/agents/*.toml`, `gpt-6-luna` for the reader (no shell), the session's model for the worker | a block in `~/.codex/AGENTS.md` (Codex delegates only when its instructions ask) | prompt submit → active client and gauge; session start → gauge; end of each sub-agent → journal (approve them once with `/hooks`) |
| **Gemini CLI** | `~/.gemini/agents/`, `flash-lite` for the reader (reading tools only), the session's model for the worker | a block in `~/.gemini/GEMINI.md` | none: Gemini CLI has no sub-agent end event |
| **VS Code (Copilot)** | `~/.copilot/agents/*.agent.md`, Claude Haiku 4.5 for the reader (`read`, `search`), the chat's model for the worker | a block in `~/.copilot/copilot-instructions.md` | `~/.copilot/hooks/vario.json`: prompt submit → active client; response end → usage refresh; session start → gauge; end of each sub-agent → journal (hooks are a preview: `chat.useHooks`) |
| **Kiro CLI** | `~/.kiro/agents/*.json` (or `~/.aws/amazonq/cli-agents/` for Amazon Q CLI), `claude-haiku-4.5` for the reader (`fs_read`, `fs_write`), the session's model for the worker | the agent's `prompt` field | `agentSpawn` → gauge; `userPromptSubmit` → active client |

Codex, Gemini CLI, VS Code and Kiro CLI follow their documentation and are covered by tests. Sub-agents need a recent Gemini CLI (they are not in 0.1.x), and Google moved part of its users to Antigravity CLI. Kiro CLI is the closed-source successor to Amazon Q Developer CLI; Vario installs into `~/.kiro/cli-agents/` and falls back to `~/.aws/amazonq/cli-agents/` when only the older path exists.

**VS Code agent switching.** Vario switches the gauge when a supported agent reports a prompt: Claude Code, Codex, OpenCode, GitHub Copilot and Kiro CLI. For CodeGeeX, it watches the extension's local conversation history in VS Code global storage (read-only; prompt content is not retained). Enable VS Code's `chat.useHooks` setting for Copilot hooks; approve Codex hooks once with `/hooks`. VS Code's public extension API does not expose a universal event for selecting another extension's chat conversation, so selecting a chat without sending a prompt cannot switch the gauge.

The hooks note every delegation to Vario's sub-agents in a local journal; `vario report` and the application's **Delegations** tab summarize it: delegations, escalations and answers without a report.

**Quota and thermostat (Claude Code).** Claude Code tells only the status line how much of your subscription you have used. Vario's status line saves it, and shows `Vario · 5h 23% · 7d 41%`; if you already have a status line, Vario wraps it instead: your line looks exactly the same. Before each prompt, the thermostat looks at the most used window: from 50 % it asks the main session to delegate more, from 80 % it also warns you, and at 100 % it suggests switching agents after saving a [Handoff](https://github.com/kdev1966/Handoff-CLI). `vario usage` shows the quota, the tokens per model (main session and sub-agents) over 5 hours and 7 days, the share of work delegated, and the MCP servers you connect but never use.

Claude Code runs status lines in the terminal only: the quota is read while you use Claude Code in a terminal, not from the VS Code extension. The rest (sub-agents, guidance, journal, `vario usage`) works everywhere.

**Floating gauge.** `vario-gui --gauge`, or **Floating gauge** in the application, opens a slim (156 px), dark, floating and borderless system window drawn like an audio plugin, narrow enough to sit beside your editor: above other windows, on every desktop (and over full-screen apps on macOS), with no Dock icon or taskbar entry in `--gauge` mode. On macOS it follows the agents' applications: it shows while VS Code, Cursor or a terminal (Terminal, iTerm2, Tabby, Warp, Ghostty) is in front, and steps away behind any other application. It shows a needle meter for the 5-hour quota, a row of lit segments for the 7-day quota, the thermostat's state as lamps, and knobs for the share delegated and the escalation rate. Drag it from anywhere.

It opens by itself when an agent's session starts, in every client whose hooks Vario installed (all but Gemini CLI), and stays up when the agent closes; untick **Open the gauge with each agent** on the **Install** page to open it only when you ask. Where a client exposes a prompt-submit hook, the gauge switches to that client as soon as you submit a prompt; otherwise it follows the last client to report activity. Merely selecting another chat tab without submitting a prompt may not trigger a switch. It comes back where you left it, else in the top right corner of the screen. Only one runs: opening it again, or opening the application, brings up the running one. Its **open Vario** link opens the main window beside it. Released close to each other, the two windows snap together, tops aligned; dragging the gauge then brings the main window along, and dragging the main window away undocks it. Closing the main window leaves the gauge up.

## Install

**Prebuilt**, from the [Releases](https://github.com/kdev1966/vario/releases) page:

- **macOS application**: `Vario-macos-aarch64.zip` (Apple silicon) or `Vario-macos-x86_64.zip` (Intel). Unzip it and move `Vario.app` to Applications. It is not notarized: open it the first time with a right click, **Open**. The command is inside the app.
- **Command, macOS and Linux**:
  ```sh
  curl --proto '=https' --tlsv1.2 -LsSf https://github.com/kdev1966/vario/releases/latest/download/vario-installer.sh | sh
  ```
- **Command, Windows** (PowerShell):
  ```powershell
  powershell -ExecutionPolicy Bypass -c "irm https://github.com/kdev1966/vario/releases/latest/download/vario-installer.ps1 | iex"
  ```
- **Application on Linux and Windows**: the same way with `vario-gui-installer.sh` or `vario-gui-installer.ps1`.

**From source** (Rust 1.89 or later; on Linux the application also needs WebKitGTK: `sudo apt install libwebkit2gtk-4.1-dev libxdo-dev libssl-dev librsvg2-dev` on Debian and Ubuntu):

```sh
cargo install --git https://github.com/kdev1966/vario vario
cargo install --git https://github.com/kdev1966/vario vario-gui   # the application, optional
```

Then let Vario configure the AI clients it finds:

```sh
vario install            # shows the changes, then asks before applying them
```

To build `Vario.app` yourself: `scripts/macos-app.sh`.

## Use

```sh
vario install --dry-run          # what would change, without changing anything
vario install --manual           # choose the clients one by one
vario install --client opencode  # configure one client, even if it is not detected
vario doctor                     # what is installed where; --fix repairs it
vario report                     # the delegation journal, per sub-agent
vario usage                      # quota, thermostat and tokens used, from local logs
vario restore --list             # the backups; `vario restore` puts back the latest
vario remove                     # remove Vario from every client
```

Without a terminal to confirm, Vario changes nothing unless you pass `--yes`.

The application (`vario-gui`, or `Vario.app`) offers the same in a dark panel the height of the gauge, in four pages: **Status** (a card of lamps per client, with Repair and Remove), **Install** (automatic or manual, the changes shown before they are applied, and the gauge setting), **Delegations** (a channel of knobs per sub-agent) and **About**.

Messages follow your system's language, French or English; `VARIO_LANG=en` or `VARIO_LANG=fr` overrides it.

## Safety

- **Nothing changes without a preview** and your confirmation (or `--yes`).
- **Every file is backed up** before its first change, in Vario's data folder (`~/Library/Application Support/vario` on macOS, `~/.local/share/vario` on Linux, `%LOCALAPPDATA%\vario` on Windows; `VARIO_HOME` overrides it). `vario restore` puts a backup back.
- **Your settings and other tools' are kept**: Vario edits only its own block, its own hook entries and its own files, recognized by a marker, and refuses to rewrite a configuration it cannot parse exactly. A test installs then removes Vario over a real-looking [Handoff](https://github.com/kdev1966/Handoff-CLI) setup in all three clients and checks every file comes back byte for byte.
- **Local**: Vario reads and writes files on your machine, and sends nothing to anyone.

## Companion tool

[**Handoff**](https://github.com/kdev1966/Handoff-CLI), by the same author, keeps each project's memory when you switch from one AI agent to another. Vario detects it and says whether it is connected to each client; they are installed and work independently.

## Develop

```sh
cargo test                  # the whole workspace
cargo clippy --all-targets -- -D warnings
cargo run -p vario -- doctor
cargo run -p vario-gui
```

The workspace has three crates: `vario-core` (all the logic), `vario` (the command) and `vario-gui` (the application, built with [Tauri](https://tauri.app): its windows are drawn by the system's web view, from the pages in `crates/vario-gui/ui/`). The design and the roadmap are in [vario-document-de-conception.md](vario-document-de-conception.md), in French.

## License

MIT. The application is built with [Tauri](https://tauri.app) (MIT or Apache-2.0).
