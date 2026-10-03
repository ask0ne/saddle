<p align="center">
  <img src="assets/logo.png" width="96" height="96" alt="Saddle logo">
</p>

<h1 align="center">Saddle</h1>

<p align="center">
  See which <a href="https://claude.com/claude-code">Claude Code</a> session is waiting for you —<br>
  and jump straight to its terminal tab.
</p>

<p align="center">
  <a href="https://github.com/ask0ne/saddle/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/ask0ne/saddle?include_prereleases&color=c89b3c&label=release"></a>
  <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-6b7a4f"></a>
  <img alt="Platform: macOS 13+" src="https://img.shields.io/badge/macOS-13%2B-c1583a">
  <a href="https://github.com/ask0ne/saddle/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/ask0ne/saddle/actions/workflows/ci.yml/badge.svg"></a>
</p>

> **Beta.** Saddle is at v0.1.0. It works day to day, but expect rough edges and
> breaking changes before 1.0. Bug reports are very welcome.

## Why

You run several Claude Code sessions in separate Terminal tabs. Running them is
easy; knowing which one is blocked on you is not. The usual workaround — a
`Stop` hook that raises Terminal — brings the whole app forward without saying
which tab.

Saddle is a small macOS app that watches your sessions and tells you:

- **waiting** — a session needs your input or a permission decision
- **working** — a session is busy (with the tool it is running)
- **done** — a session finished and you haven't looked yet
- **idle** — nothing to report

Click a session and Saddle raises the **exact Terminal tab** it runs in.

Saddle only reads. It never wraps, proxies or intercepts Claude Code — quit
Saddle and every session keeps running exactly as before.

## Two modes

| | |
|---|---|
| **saddle** (default) | A small floating pill that follows you across desktops and sits over fullscreen apps. Collapsed, it shows the one session that matters most; click to expand the list. Right-click for the menu. No Dock icon. |
| **stable** | A regular window with the full list grouped by *needs you / working / idle*, plus setup and about. |

Switch with the **saddle** button in the stable window, or **stable** in the
pill's right-click menu. Your choice is remembered.

## Install

### Download (recommended)

1. Download the `.dmg` from the
   [latest release](https://github.com/ask0ne/saddle/releases/latest).
2. Open it and drag **Saddle** to **Applications**.
3. This beta is **not code-signed or notarized**, so macOS will refuse the
   first launch. Open **System Settings → Privacy & Security**, scroll down to
   the message about Saddle, and click **Open Anyway**. Alternatively, clear
   the quarantine flag from a terminal:

   ```bash
   xattr -dr com.apple.quarantine /Applications/Saddle.app
   ```

   You can verify the download against `SHA256SUMS.txt` on the release page
   with `shasum -a 256 -c SHA256SUMS.txt`.

Requires **macOS 13 or later** and **Claude Code**.

### Build from source

You need [Rust](https://rustup.rs) (1.85+), [Node.js](https://nodejs.org) 20+
and the Xcode command-line tools.

```bash
git clone https://github.com/ask0ne/saddle.git
cd saddle/app
npm install
npm run tauri build
open src-tauri/target/release/bundle/macos/Saddle.app
```

For development with hot reload use `npm run tauri dev`. See
[CONTRIBUTING.md](CONTRIBUTING.md).

## First run

1. **Install the hooks.** On first launch Saddle shows a one-time setup notice.
   Click **install hooks**. This merges three hooks (`Notification`, `Stop`,
   `UserPromptSubmit`) into `~/.claude/settings.json` so Saddle can tell
   *waiting* and *done* from plain *idle*. Your existing settings are preserved
   and a backup is written first. Then **restart any running Claude Code
   sessions** so they pick the hooks up.

   The installer uses `jq`, which ships with macOS 15 and later
   (`brew install jq` on older versions). You can also run it yourself — see
   [Hooks](#hooks).
2. **Allow Terminal control.** The first time you click a session, macOS asks
   whether Saddle may control Terminal. Allow it — that is what lets Saddle
   raise one specific tab. If you decline, Saddle falls back to raising Terminal
   as a whole and tells you so. You can change this later in
   *System Settings → Privacy & Security → Automation*.

## How it works

Claude Code already writes everything Saddle needs to disk:

| Source | What it provides |
|---|---|
| `~/.claude/sessions/<pid>.json` | Live session roster: pid, session id, working directory, name, busy/idle |
| `~/.claude/projects/<slug>/<sessionId>.jsonl` | Session title, git branch and the current tool |
| `~/.devpit/events.jsonl` | `Notification` / `Stop` / `UserPromptSubmit` events, appended by the hooks |
| `ps -o tty=` + AppleScript | Maps a pid to the exact Terminal tab |

Sessions that have exited disappear from the list. Background subagents and
sessions with no controlling terminal are not listed, since they can't be
focused. More detail in [ARCHITECTURE.md](ARCHITECTURE.md).

## Hooks

The hook installer is bundled inside the app and also lives in this repository:

```bash
./install-hooks.sh --dry-run    # print the diff, write nothing
./install-hooks.sh              # install (backup: settings.json.devpit.bak)
./install-hooks.sh --uninstall  # remove only Saddle's hooks
```

From an installed app the script is at
`/Applications/Saddle.app/Contents/Resources/install-hooks.sh`.

It merges rather than replaces, writes atomically, and running it twice is a
no-op.

> **Why does everything still say "devpit"?** Saddle was previously called
> devpit. The event file `~/.devpit/events.jsonl`, the `# devpit` marker in the
> installed hook commands and the `.devpit.bak` backup keep the old name on
> purpose: renaming them would silently silence hooks that were installed by an
> earlier build. They will be migrated in a future release.

## Privacy and security

Saddle makes **no network requests** and collects **no telemetry**. It reads the
Claude Code files listed above, writes only `~/.devpit/events.jsonl` and (when
you ask) `~/.claude/settings.json`, and talks to Terminal through AppleScript.
Hook events contain the data Claude Code passes to hooks (session id, working
directory, notification text); they stay in `~/.devpit/events.jsonl`
(mode `0600`, rotated at 8 MB). To report a vulnerability, see
[SECURITY.md](SECURITY.md).

## Uninstall

```bash
/Applications/Saddle.app/Contents/Resources/install-hooks.sh --uninstall
rm -rf ~/.devpit
```

Then drag Saddle to the Trash.

## Limitations

- macOS only, and **Terminal.app only** for tab focus. iTerm2, Ghostty,
  Warp and tmux are not supported yet; Saddle falls back to raising Terminal.
- The Claude Code session registry and transcript formats are internal and
  undocumented. Saddle tolerates unknown fields, but a Claude Code update can
  still break it. Please file an issue if one does.
- The beta build is unsigned (see [Install](#download-recommended)).

## Contributing

Issues and pull requests are welcome — see [CONTRIBUTING.md](CONTRIBUTING.md)
and the [Code of Conduct](CODE_OF_CONDUCT.md). Release history is in
[CHANGELOG.md](CHANGELOG.md).

## Author and license

Created by [Atharva Kawade](https://github.com/ask0ne) /
[whelmedthinker](https://whelmedthinker.com).

Released under the [MIT License](LICENSE). Saddle is an independent project and
is not affiliated with or endorsed by Anthropic. "Claude" and "Claude Code" are
trademarks of Anthropic, PBC.
