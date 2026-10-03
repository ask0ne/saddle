//! Every `#[tauri::command]` — the js boundary (see ARCHITECTURE.md, Modules).
//! Also sets up the watchers + poll fallback that push `fleet://changed`;
//! state is pushed, never polled from Svelte (see ARCHITECTURE.md, Concurrency).

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use tauri::path::BaseDirectory;
use tauri::{AppHandle, Emitter, Manager};

use crate::fleet::events::events_path;
use crate::fleet::registry::registry_dir;
use crate::fleet::store::{Fleet, Session};
use crate::fleet::watch::FileWatcher;
use crate::platform::terminal::{self, FocusResult};

#[tauri::command]
pub fn sessions(fleet: tauri::State<Fleet>) -> Vec<Session> {
    fleet.sessions()
}

#[tauri::command]
pub fn mark_seen(session_id: String, fleet: tauri::State<Fleet>) {
    fleet.mark_seen(&session_id);
}

/// Port of `SessionStore.focus(_:)`: mark the session seen, then raise its tab.
#[tauri::command]
pub fn focus(
    session_id: String,
    pid: i32,
    title: Option<String>,
    fleet: tauri::State<Fleet>,
) -> FocusResult {
    fleet.mark_seen(&session_id);
    terminal::focus(pid, title.as_deref())
}

/// Switch the whole app to accessory (no Dock icon) and show the pill. Not a
/// cosmetic choice: macOS only honors cross-Space collection behavior
/// (`canJoinAllSpaces`) for accessory-policy apps — a regular app's windows
/// have their Space membership pinned no matter what flags you set on them
/// (confirmed against tauri-apps/tauri#11488, closed upstream as a known
/// macOS limitation, not a bug in this code). This is how the pill's
/// "follows you across desktops" actually works, not a toggle layered on top.
/// Every step here runs regardless of whether an earlier one failed — an
/// early `?`-return that skips, say, `board.hide()` (the stable window)
/// because the activation
/// policy call errored is exactly how you'd end up with both windows
/// visible at once, which should be impossible by construction. Each step
/// logs its own outcome instead, so a partial failure is visible in the
/// terminal rather than silently leaving the app in a mixed state.
#[tauri::command]
pub fn enter_saddle_mode(app: AppHandle) {
    #[cfg(target_os = "macos")]
    {
        if let Err(e) = app.set_activation_policy(tauri::ActivationPolicy::Accessory) {
            eprintln!("saddle: enter_saddle_mode: set_activation_policy(Accessory) failed: {e}");
        }
        match app.get_webview_window("main") {
            Some(board) => {
                // A native-fullscreen window owns its own Space; hiding it
                // while still fullscreen strands an empty black Space behind.
                if board.is_fullscreen().unwrap_or(false)
                    && let Err(e) = board.set_fullscreen(false)
                {
                    eprintln!("saddle: enter_saddle_mode: set_fullscreen(false) failed: {e}");
                }
                if let Err(e) = board.hide() {
                    eprintln!("saddle: enter_saddle_mode: board.hide() failed: {e}");
                }
            }
            None => eprintln!("saddle: enter_saddle_mode: no window labeled \"main\""),
        }
        if let Err(e) = spawn_pill(&app) {
            eprintln!("saddle: enter_saddle_mode: spawn_pill failed: {e}");
        }
    }
}

/// Switch back to a normal, Dock-visible app and show the stable window.
/// The pill is
/// closed, not hidden — recreating it fresh the next time `enter_saddle_mode`
/// runs means never having to fix up an *existing* window's Space membership
/// after an activation-policy change, which is the fragile direction; a
/// freshly-created window under the already-correct policy is not. Same
/// run-every-step-regardless shape as `enter_saddle_mode`, same reason.
#[tauri::command]
pub fn enter_stable_mode(app: AppHandle) {
    if let Some(pill) = app.get_webview_window("pill")
        && let Err(e) = pill.close()
    {
        eprintln!("saddle: enter_stable_mode: pill.close() failed: {e}");
    }
    #[cfg(target_os = "macos")]
    if let Err(e) = app.set_activation_policy(tauri::ActivationPolicy::Regular) {
        eprintln!("saddle: enter_stable_mode: set_activation_policy(Regular) failed: {e}");
    }
    match app.get_webview_window("main") {
        Some(board) => {
            // show() alone leaves a window the user minimized sitting in the
            // Dock — "stable" would appear to do nothing.
            if board.is_minimized().unwrap_or(false)
                && let Err(e) = board.unminimize()
            {
                eprintln!("saddle: enter_stable_mode: unminimize() failed: {e}");
            }
            if let Err(e) = board.show().and_then(|_| board.set_focus()) {
                eprintln!("saddle: enter_stable_mode: board.show()/set_focus() failed: {e}");
            }
        }
        None => eprintln!("saddle: enter_stable_mode: no window labeled \"main\""),
    }
}

#[tauri::command]
pub fn quit(app: AppHandle) {
    app.exit(0);
}

#[tauri::command]
pub fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn claude_settings_path() -> Option<PathBuf> {
    crate::fleet::home_dir().map(|home| home.join(".claude/settings.json"))
}

/// Whether saddle's hooks are already merged into `~/.claude/settings.json`.
/// Without them the registry still gives working/idle, but "needs you" and
/// "done" never fire — the core of the product — so the stable window offers
/// to install them (`install_hooks`) until this is true.
#[tauri::command]
pub fn hooks_installed() -> bool {
    claude_settings_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .is_some_and(|settings| settings.contains(".devpit/events.jsonl"))
}

/// Runs the bundled `install-hooks.sh` (the same script a source checkout
/// uses): merges three hooks into `~/.claude/settings.json`, backs the file
/// up first, writes atomically. `async` so a slow script never blocks the main
/// thread (sync commands run on it). The error is the script's own stderr.
#[tauri::command]
pub async fn install_hooks(app: AppHandle) -> Result<(), String> {
    let script = app
        .path()
        .resolve("install-hooks.sh", BaseDirectory::Resource)
        .map_err(|e| e.to_string())?;
    let output = Command::new("/bin/bash")
        .arg(&script)
        // A Finder-launched app gets a minimal PATH; add Homebrew's so a
        // brew-installed `jq` is found on macOS versions that don't ship one.
        .env(
            "PATH",
            "/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/usr/local/bin",
        )
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.contains("jq is required") {
        // macOS ships jq from 15 on; 13 and 14 need it installed.
        Err("jq is required — install it with `brew install jq`, then try again".into())
    } else {
        Err(stderr)
    }
}

/// Called by the pill frontend right after every `setSize()` (expand,
/// collapse, and the width re-fit on content change) — see
/// `platform::panel::keep_transparent`'s doc comment for why a resize alone
/// can silently make the window opaque again.
#[cfg(target_os = "macos")]
#[tauri::command]
pub fn keep_pill_transparent(app: AppHandle) {
    if let Some(pill) = app.get_webview_window("pill")
        && let Err(e) = crate::platform::panel::keep_transparent(&pill)
    {
        eprintln!("saddle: keep_pill_transparent failed: {e}");
    }
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub fn keep_pill_transparent(_app: AppHandle) {}

/// Kept `.manage()`d so the underlying OS watches live for the app's
/// lifetime — a `FileWatcher` cancels its watch when dropped.
pub struct Watchers(#[allow(dead_code)] Vec<FileWatcher>);

/// Watchers for the fast path + registry lifecycle, and a 1s poll fallback
/// for the thing neither raises an event for: registry status flips
/// (busy↔idle) rewrite `~/.claude/sessions/<pid>.json` in place.
pub fn start(app: &AppHandle) {
    app.state::<Fleet>().refresh();
    emit_sessions(app);

    let mut watchers = Vec::new();

    if let Some(path) = events_path() {
        let handle = app.clone();
        match FileWatcher::new(&path, Duration::from_millis(30), move || {
            handle.state::<Fleet>().pump();
            emit_sessions(&handle);
        }) {
            Ok(w) => watchers.push(w),
            Err(e) => eprintln!("saddle: failed to watch {}: {e}", path.display()),
        }
    }

    if let Some(dir) = registry_dir() {
        let handle = app.clone();
        match FileWatcher::new(&dir, Duration::from_millis(30), move || {
            handle.state::<Fleet>().refresh();
            emit_sessions(&handle);
        }) {
            Ok(w) => watchers.push(w),
            Err(e) => eprintln!("saddle: failed to watch {}: {e}", dir.display()),
        }
    }

    app.manage(Watchers(watchers));

    // The stable window is created hidden and shown by the frontend once it
    // knows which mode to launch in. If that never happens (frontend failed
    // to load), the app would be running with no window at all — show the
    // window rather than leave an invisible process.
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(8));
        let main_hidden = handle
            .get_webview_window("main")
            .is_some_and(|w| !w.is_visible().unwrap_or(true));
        if main_hidden && handle.get_webview_window("pill").is_none() {
            eprintln!("saddle: no window shown after startup; showing the stable window");
            enter_stable_mode(handle);
        }
    });

    let handle = app.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let fleet = handle.state::<Fleet>();
            fleet.pump();
            fleet.refresh();
            emit_sessions(&handle);
        }
    });
}

fn emit_sessions(app: &AppHandle) {
    let sessions = app.state::<Fleet>().sessions();
    let _ = app.emit("fleet://changed", sessions);
}

/// The floating pill: a second, borderless, always-on-top webview window.
/// Everything except the window level / fullscreen collection behavior is
/// plain Tauri config — those two go through `platform::panel` (the only
/// `unsafe` in the app). Positioned top-right every time it's (re)created; the
/// position the user dragged it to is not persisted. Called fresh each time `enter_saddle_mode` runs, not once at
/// startup — `enter_stable_mode` always closes the previous one first, but
/// guard here too in case that invariant is ever violated.
#[cfg(target_os = "macos")]
pub fn spawn_pill(app: &AppHandle) -> tauri::Result<()> {
    use tauri::{LogicalPosition, WebviewUrl, WebviewWindowBuilder};

    if let Some(existing) = app.get_webview_window("pill") {
        return existing.show();
    }

    // Placeholder — the JS side measures its actual content and calls
    // setSize() within the first frame (see pill/+page.svelte's fitToContent).
    const COLLAPSED_WIDTH: f64 = 150.0;
    const COLLAPSED_HEIGHT: f64 = 30.0;
    const MARGIN: f64 = 12.0;

    // Built hidden. Level/collectionBehavior must be set BEFORE the window is
    // ever shown — a window's space membership can get fixed at first-show
    // time, so changing collectionBehavior *after* .build() already displayed
    // it may never retroactively attach it to all Spaces.
    // Set position + the unsafe properties while still hidden, then show.
    let pill = WebviewWindowBuilder::new(app, "pill", WebviewUrl::App("/pill".into()))
        .title("saddle-pill")
        .inner_size(COLLAPSED_WIDTH, COLLAPSED_HEIGHT)
        // Starts non-resizable: a 30px-tall collapsed pill leaves barely any
        // area that ISN'T a resize-edge hit zone, which was eating clicks.
        // The frontend flips this on only while expanded (setResizable).
        .resizable(false)
        .decorations(false)
        .transparent(true)
        // The native window shadow is a plain rectangle bound to the window
        // frame — it doesn't track the webview's rounded corners. Barely
        // visible on the tiny initial placeholder size, but every later
        // setSize() (expand/collapse) recomputes it against the new frame
        // and it snaps square. The pill already draws its own correctly-
        // shaped `box-shadow` in CSS (`.pill` in pill/+page.svelte), so the
        // native one is redundant on top of being wrong-shaped.
        .shadow(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .focused(false)
        .accept_first_mouse(true)
        .visible(false)
        .build()?;

    // `work_area` (not `size`/`position`) already excludes the menu bar —
    // no magic-number top margin needed.
    if let Ok(Some(monitor)) = pill.primary_monitor() {
        let work_area = monitor.work_area();
        let scale = monitor.scale_factor();
        let origin = work_area.position.to_logical::<f64>(scale);
        pill.set_position(LogicalPosition::new(origin.x + MARGIN, origin.y))?;
    }

    crate::platform::panel::float_over_fullscreen(&pill)?;
    crate::platform::panel::keep_transparent(&pill)?;
    pill.show()?;

    Ok(())
}
