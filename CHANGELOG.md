# Changelog

All notable changes to Vario are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Kiro CLI support (the closed-source successor to Amazon Q Developer CLI): two agent
  JSON files in `~/.kiro/agents/` (`vario-reader.json` with `claude-haiku-4.5`,
  `vario-worker.json` inheriting the session's model), delegation guidance in each
  agent's `prompt` field, and hooks (`agentSpawn` → gauge, `userPromptSubmit` →
  active client). Falls back to `~/.aws/amazonq/cli-agents/` when only the Amazon Q
  CLI path exists. Detected by `kiro-cli`, `q`, `~/.kiro/` or `~/.aws/amazonq/`.
- The gauge switches to the client you are using as soon as you submit a prompt:
  prompt hooks for Codex, Cursor, OpenCode and VS Code (Copilot), and a response-end
  refresh of the usage for OpenCode and Copilot. Selecting another chat without
  sending a prompt does not switch it. Run `vario doctor --fix` (or **Repair**) to
  add the new hooks to an existing setup.
- CodeGeeX in VS Code takes the gauge when a new conversation is saved. Its extension
  has no hook, so Vario reads its conversation history in VS Code's global storage,
  read-only, without keeping any prompt or response.
- The floating gauge opens by itself when an agent's session starts: a session-start
  hook for Claude Code, Codex and VS Code, the OpenCode plugin when OpenCode starts,
  and Cursor's existing hook. It stays up when the agent closes. **Open the gauge
  with each agent**, on the Install page, turns this off. Run `vario doctor --fix`
  (or **Repair**) to add the new hooks to an existing setup.
- The gauge and the main window snap together when released close to each other;
  dragging the gauge brings the docked main window along. The gauge comes back where
  it was left, else in the top right corner of the screen.

### Changed

- The application is drawn with Tauri (the system's web view) instead of Slint, with
  the same windows, pages and looks. On Linux it now needs WebKitGTK
  (`libwebkit2gtk-4.1-dev`) to build. The main window opens at once: it is prepared
  hidden when the interface starts.
- On macOS, the gauge shows only while an agent's application is in front (VS Code,
  Cursor, Terminal, iTerm2, Tabby, Warp, Ghostty) and steps away behind the others.
- Vario.app has an icon.
- The command line and the core need Rust 1.89 (was 1.88).
- The main window has the gauge's look: a dark, frameless panel the height of the
  gauge, with a card of lamps per client and a channel of knobs per sub-agent.
  Closing it leaves the gauge up; on macOS its Dock icon shows only while it is open.
- One interface runs at a time: opening `vario-gui` or `Vario.app` again brings up
  the running one.
- Only Cursor still takes the delegation guidance from the session-start hook; the
  other clients read it from their instructions file.

### Fixed

- Looking at the clients took about 4 s on a machine with Kiro CLI, which answered the
  same `kiro-cli agent list` three times; it is now asked once (about 1.4 s).
- In Claude Code's VS Code extension, where the status line never runs, the gauge
  kept showing the last other client (Copilot's model) while Claude Code was in use.
  Claude Code's prompt hook now takes the gauge back.
- The interface no longer mixes languages when an agent starts it with another `LANG`
  than the system's (OpenCode with `en_US`): its labels now follow the same language
  as its values.
- The gauge no longer shows an old quota as the current one. Claude Code's VS Code
  extension has no status line, so the gauge also reads the quota that Claude Code
  keeps in `.claude.json` (refreshed when a session starts and by `/usage`), and
  shows the more recent reading. Seen more than 15 minutes ago, the quota turns grey
  with how old it is; once its window has passed, the gauge says it expired.
- The gauge keeps the last known quota on screen, greyed, once its window has passed,
  instead of going blank; the thermostat still only lights from a window that is
  current.
- The gauge follows the client you are working in. Only Claude Code has a subscription
  quota; when the session in use is another client, the gauge shows that client instead
  of a stale quota. OpenCode reports its model, tokens and cost, so the gauge shows
  them. Copilot's and Gemini CLI's hooks carry none of that, so the gauge reads the
  model and tokens from the session's log (the Copilot agent's `events.jsonl`, Gemini
  CLI's transcript, through a new `AfterAgent` hook); with VS Code's older built-in
  chat, it shows the model from VS Code's local chat store. Neither reports a cost in
  dollars, so the gauge shows none. Run
  `vario doctor --fix` (or **Repair**) to add the reports to an existing setup.

## [0.2.1] - 2026-10-16

### Fixed

- A delegation is journaled once even when the host reports the sub-agent's end twice.
  Claude Code does so when it makes a background sub-agent hand its report back
  through a tool; `vario report` also leaves out the repeats already in the journal.
- `vario-gui --version` and `--help` answer in the terminal instead of opening the
  window.

## [0.2.0] - 2026-10-16

### Added

- `vario usage`: the tokens used per model, in the main session and in sub-agents, over
  5 hours and 7 days, the share of work delegated, and the MCP servers you connect but
  never use, read from Claude Code's local logs.
- The subscription quota: Vario's status line saves the 5-hour and 7-day limits Claude
  Code passes it. A status line you already have is wrapped, never replaced: it looks
  the same, and removing Vario puts it back.
- The quota thermostat: from 50 % of the most used window the session is asked to
  delegate more, from 80 % you are warned, and at 100 % Vario suggests switching agents
  after saving a Handoff.
- The floating gauge (`vario-gui --gauge`, or **Floating gauge** in the application): a
  slim, dark, borderless window drawn like an audio plugin, above other applications and
  on every desktop, with a needle meter, lit segments, thermostat lamps and knobs.
- Codex, Gemini CLI and VS Code (Copilot) support: sub-agents, delegation guidance, and
  a sub-agent journal hook for Codex and VS Code.
- A **Quota** column in `vario doctor` and the application.

### Changed

- The status line is used only when it is free, and wrapped otherwise.

### Known limits

- The quota is read only while Claude Code runs in a terminal: the VS Code extension
  runs no status line.
- Sub-agents' logs can keep only the start of an answer's output count; Vario then
  estimates it from the answer's length, and says so.

## [0.1.0] - 2026-10-16

First release: `vario` (install, remove, doctor, restore, report) and `vario-gui` for
Claude Code, Cursor and OpenCode, with economical sub-agents, delegation guidance, a
local delegation journal, French and English, and `Vario.app` for macOS.

[0.2.1]: https://github.com/kdev1966/vario/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/kdev1966/vario/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/kdev1966/vario/releases/tag/v0.1.0
