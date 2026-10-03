# Architecture

How Saddle is put together, and why. Read this before changing anything under
`app/src-tauri/src/`.

## Overview

Saddle is a [Tauri 2](https://tauri.app) app: a Rust backend that owns all
state and I/O, and a SvelteKit frontend that only renders what it is told.

```
 ~/.claude/sessions/*.json ─┐
 ~/.claude/projects/**/*.jsonl ─┤   fleet::registry / transcript
 ~/.devpit/events.jsonl ────┘   fleet::events        │
                                                      ▼
                       file watchers + 1 s poll ──▶ Fleet (Mutex)
                                                      │ emit "fleet://changed"
                                    ┌─────────────────┴───────────────┐
                                    ▼                                 ▼
                          stable window (/)                    pill window (/pill)
                                    │  invoke("focus")                │
                                    └──────────▶ platform::terminal ◀─┘
                                                 (ps + osascript → Terminal tab)
```

## Principles

1. **Rust owns the truth.** The `Fleet` lives in the Rust process. Windows are
   views that come and go; a webview can be destroyed and recreated at any time
   without losing state.
2. **State is pushed, never polled.** When a watcher fires, Rust re-derives the
   session list and emits `fleet://changed` with the full snapshot. The frontend
   subscribes; it never polls.
3. **Read-only toward Claude Code.** Saddle reads files Claude Code already
   writes. The only thing it ever writes outside its own state is
   `~/.claude/settings.json`, and only when the user clicks *install hooks*.
4. **Tolerate format drift.** The registry and transcript formats are internal
   to Claude Code. Only `pid` and `sessionId` are required; unknown fields are
   ignored (`deny_unknown_fields` is never used); an unparsable file is skipped,
   never fatal.

## Modules (`app/src-tauri/src/`)

| Module | Responsibility |
|---|---|
| `fleet::registry` | Reads `~/.claude/sessions/<pid>.json`; drops entries whose pid is dead (`kill(pid, 0)`) |
| `fleet::transcript` | Tails the end of a session's `.jsonl` for its AI title, git branch and current tool; cached by `(size, mtime)` |
| `fleet::events` | Byte-offset tail of `~/.devpit/events.jsonl` (the hook output); rotates at 1 MB on launch and 8 MB while running |
| `fleet::watch` | `notify` (FSEvents) watcher with a hand-rolled debounce |
| `fleet::store` | `Fleet`: one `Mutex` around sessions, hook state, event log and transcript cache; derives each session's status |
| `platform::terminal` | pid → tty → AppleScript that selects the matching Terminal tab |
| `platform::panel` | The two `NSWindow`/`WKWebView` properties Tauri does not expose (see below) |
| `commands` | Every `#[tauri::command]`, the watcher/poll wiring, mode switching, hook installer |

## Status model

Derived in `fleet::store::derive_status` from the registry plus hook events:

| Status | When |
|---|---|
| `WaitingInput` | a `Notification` hook fired and nothing was submitted or seen since |
| `Working` | registry status is anything other than `idle` (labelled with the newest `tool_use`) |
| `Done` | a `Stop` hook fired and the user hasn't looked yet |
| `Idle` | none of the above |

Sessions are sorted by status rank, then oldest-in-state first. A session whose
registry entry disappears or whose process dies is removed on the next refresh;
nothing is kept as a ghost row.

Excluded from the list: background subagents (`kind: "background"`, which share
their parent's tty) and sessions with no controlling terminal (they cannot be
focused). `Notification` events of type `idle_prompt` and `auth_success` are not
treated as "waiting" — they are not requests for the user.

## Concurrency

Writers are frequent (every file event, a 1 s poll, IPC commands) and readers
are rare, so one `std::sync::Mutex<Inner>` guards everything mutable. `RwLock`
and an actor/channel design were rejected as machinery without a payoff at this
scale. `Fleet::lock()` recovers from a poisoned mutex instead of propagating the
panic, since `Inner` has no cross-field invariant a partial mutation could break.
Threads are plain `std::thread`; there is no async runtime.

## Windows and modes

- **stable** — the `main` window: a normal, Dock-visible app. Created hidden;
  the frontend decides at launch whether to show it (stable preference, or first
  run) or to enter saddle mode without ever showing it. A watchdog shows it if
  the frontend never decides.
- **saddle** — a borderless, transparent, always-on-top `pill` window.

Switching modes is an **activation-policy change**, not just show/hide: macOS
only honours `canJoinAllSpaces` for accessory-policy apps, so the pill (follows
you across desktops) requires `Accessory`, and the stable window requires
`Regular`. The pill is closed and recreated on each switch rather than
reconfigured, because fixing up an existing window's Space membership after a
policy change is the fragile direction.

## The FFI gap (`platform::panel`)

Tauri exposes `always_on_top`, `visible_on_all_workspaces`, `transparent`,
`accept_first_mouse` and friends as plain config. It does not expose:

- a window level above fullscreen apps: `always_on_top` is
  `NSFloatingWindowLevel` (3); the pill needs `NSStatusWindowLevel` (25);
- `NSWindowCollectionBehaviorFullScreenAuxiliary` (plus `Stationary` and
  `IgnoresCycle`), the permission to draw over a fullscreen Space;
- re-asserting `WKWebView.underPageBackgroundColor` after a resize, which works
  around a WebKit bug where a resized transparent webview paints an opaque
  rectangle behind its rounded corners.

These are the only `unsafe` blocks in the app, isolated to one file behind
`cfg(target_os = "macos")`.

## Dependencies

| Pick | Over | Why |
|---|---|---|
| `notify` | hand-rolled `DispatchSource` | wraps FSEvents, which watches by path and survives file replacement |
| `serde` / `serde_json` | `simd-json` | parsing is not the bottleneck |
| `objc2` | `cocoa`, `objc` | `cocoa` is deprecated; `objc2` is maintained and encodes more safety |
| `std::thread` | `tokio` | a handful of long-lived threads; no async needed |
| `std::sync::Mutex` | `parking_lot` | std's is fine; one less dependency |

## Scope

macOS 13+ and Terminal.app only. Other terminals (iTerm2, Ghostty, Warp, tmux)
would need their own `platform::terminal` implementation; the rest of the code is
terminal-agnostic.
