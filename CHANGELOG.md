# Changelog

All notable changes to Saddle are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-10-03

First public release (beta).

### Added

- Tauri + Svelte macOS app that shows which Claude Code session is waiting for you.
- Live session list, built from what Claude Code already writes to disk.
- Click-to-focus: raises the exact Terminal.app tab a session runs in.
- Two window modes: the floating pill ("saddle" mode) that follows you across Spaces and over fullscreen apps, and the full "stable" window.
- Hook-driven waiting, working and done status.
- In-app hooks installer: one click merges the three hooks into `~/.claude/settings.json`, with a backup written first.
- First-launch setup notice in the stable window, shown until the hooks are installed.
- About section with version, author, license and project links.

### Fixed

- Closed sessions and terminals no longer linger in the list as ghost rows; a session leaves the list as soon as it is gone.
- A stale session file left by a crash or reboot no longer shows up as a live session when its pid has been reused by an unrelated process.
- The stable window is created hidden, so nothing flashes on screen when launching straight into saddle mode.
- Returning to the stable window now restores it if it was minimized.
- Entering saddle mode while the stable window is fullscreen no longer strands an empty Space.
- Added `NSAppleEventsUsageDescription`, so the macOS Automation prompt explains why Saddle needs to control Terminal.
- The hook installer follows a symlinked `settings.json` instead of replacing the link, creates the file on a fresh machine, and `--uninstall` exits 0.
- A missing `jq` now produces an actionable message (`brew install jq`) in the setup notice.
- Set a minimum window size so the stable window layout can't be shrunk until it breaks.
- Set the minimum supported macOS version to 13.

[Unreleased]: https://github.com/ask0ne/saddle/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/ask0ne/saddle/releases/tag/v0.1.0
