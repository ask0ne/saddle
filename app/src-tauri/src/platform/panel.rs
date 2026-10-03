//! The exact FFI gap (see ARCHITECTURE.md, The FFI gap): Tauri 2 already exposes
//! `accept_first_mouse`, `focusable(false)`, `always_on_top`,
//! `visible_on_all_workspaces`, `decorations(false)`, `transparent`, `shadow`
//! as plain builder config — no `unsafe` needed for any of that. What it does
//! **not** expose:
//!
//! - `always_on_top` maps to `NSFloatingWindowLevel` (3), which sits *below*
//!   another app's fullscreen window. The pill needs `NSStatusWindowLevel`
//!   (25) — verified empirically, and the one fact that is expensive to
//!   rediscover.
//! - `visible_on_all_workspaces` sets `canJoinAllSpaces` but not
//!   `NSWindowCollectionBehaviorFullScreenAuxiliary` — permission to actually
//!   draw over a fullscreen space, not just follow across desktops.
//!
//! Also sets `.stationary`/`.ignoresCycle` — the verified-working collection
//! behavior is all four together (`behavior=337` ==
//! `CanJoinAllSpaces(1) | Stationary(16) | IgnoresCycle(64) |
//! FullScreenAuxiliary(256)`). Without `.stationary` the window can get
//! reordered by Mission Control / space-switching; without `.ignoresCycle`
//! it participates in window cycling. Both are cheap to include and this is
//! a "match the recipe that was actually verified," not a guess.
//!
//! So: two setters on the raw `NSWindow` handle. Everything else about the
//! pill window (drag via `data-tauri-drag-region`, transparency, sizing) is
//! plain Tauri/web, no `unsafe` involved.

#![allow(unsafe_code)] // this module is the app's one FFI boundary; each block has a SAFETY note

use objc2_app_kit::{NSColor, NSWindowCollectionBehavior};
use objc2_web_kit::WKWebView;

const NS_STATUS_WINDOW_LEVEL: isize = 25;

/// Raise `window` to status-item level and permit it to draw over another
/// app's fullscreen space. Call once, after the window exists.
pub fn float_over_fullscreen(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    let ptr = window.ns_window()?.cast::<objc2_app_kit::NSWindow>();

    // SAFETY: `ns_window()` returns a live NSWindow* owned by `window` for as
    // long as `window` exists; we only dereference it for this call, and
    // NSWindow's level/collectionBehavior setters are safe to call from the
    // main thread, which Tauri guarantees `setup()` (our only caller) runs on.
    unsafe {
        let ns_window = &*ptr;
        ns_window.setLevel(NS_STATUS_WINDOW_LEVEL);
        let behavior = ns_window.collectionBehavior()
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle;
        ns_window.setCollectionBehavior(behavior);
        // Runtime diagnostic (expect level=25 behavior=337) — cheap, and the
        // numbers are exactly what to check first if cross-Space behavior
        // ever regresses.
        eprintln!(
            "saddle: pill shown level={} behavior={}",
            ns_window.level(),
            ns_window.collectionBehavior().0
        );
    }
    Ok(())
}

/// Re-assert full transparency on the pill's WKWebView. `.transparent(true)`
/// in `commands::spawn_pill` only sets this up at window-creation time — on a
/// real machine, resizing the pill (every expand/collapse) can make WebKit
/// recreate the webview's backing layer for the new size, and the fresh
/// layer comes back opaque, painting a plain rectangle behind the CSS
/// rounded corners ("square corners after toggling" — a real, reported
/// wry/WebKit interaction, not just a CSS bug). Call this once after
/// creation AND again after every `setSize()` from the frontend
/// (`commands::keep_pill_transparent`) — cheap, and there's no reliable way
/// to know in advance whether a given resize triggered the layer swap.
pub fn keep_transparent(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    window.with_webview(|webview| {
        // SAFETY: `with_webview` runs this closure on the main thread with a
        // live WKWebView* owned by `webview` for the closure's duration; we
        // only call its public `underPageBackgroundColor` setter.
        unsafe {
            let view: &WKWebView = &*webview.inner().cast();
            view.setUnderPageBackgroundColor(Some(&NSColor::clearColor()));
        }
    })
}
