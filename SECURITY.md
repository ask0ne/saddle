# Security Policy

## Supported versions

Saddle is in beta. Only the latest 0.1.x release receives security fixes.

| Version | Supported |
|---|---|
| latest 0.1.x | Yes |
| anything older | No |

## Reporting a vulnerability

Please do not open a public issue for security problems. Report them privately using GitHub's private vulnerability reporting:

<https://github.com/ask0ne/saddle/security/advisories/new>

Include what you found, steps to reproduce, and the version affected. You can expect an initial response within a few days.

## What Saddle touches

So you can judge the risk for yourself, this is everything Saddle does on your machine:

- **Reads** `~/.claude/sessions/*.json` (Claude Code's live session registry) and the session transcripts under `~/.claude/projects/` (only the tail, to find the title, git branch and current tool).
- **Reads** `~/.devpit/events.jsonl`, an append-only log written by Saddle's hooks. The path keeps the app's former name on purpose.
- **Edits** `~/.claude/settings.json` only when you run `install-hooks.sh` or click "install hooks" in the app. The change merges three hooks (`Notification`, `Stop`, `UserPromptSubmit`), writes a backup first, and writes atomically. `install-hooks.sh --uninstall` removes them.
- **Runs** `ps` and `osascript` to find a session's Terminal tab and bring it to the front. This uses macOS Automation, so macOS asks for your permission to let Saddle control Terminal.
- **Does not** use the network, collect telemetry, or send any data anywhere. Quitting Saddle never affects running Claude Code sessions.

Release builds are currently unsigned and not notarized.
