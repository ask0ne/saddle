<script lang="ts">
  import { onMount, tick } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { LogicalSize } from "@tauri-apps/api/dpi";
  import { Menu } from "@tauri-apps/api/menu";
  import {
    type Session,
    type FocusResult,
    displayName,
    statusKind,
    statusLabel,
    age,
    pillFocus,
    pillHeadline,
    pillDescriptor,
    accentFor,
    WAITING,
    WORKING,
    DONE,
  } from "$lib/fleet";
  import { setShowStablePref } from "$lib/prefs";

  const win = getCurrentWindow(); // one window for this page's whole lifetime

  // The floating pill — a second, borderless, always-on-top-over-fullscreen
  // webview window (see platform::panel for the two NSWindow properties that
  // make "over fullscreen" possible). Deliberately narrow: no drag-position
  // persistence, no outside-click auto-collapse — two things worth adding if
  // this becomes the daily driver. Width-to-content sizing and click/drag
  // arbitration are implemented (see below); a fixed-size blocky pill isn't
  // the same product.

  // Pill size metrics.
  const MIN_WIDTH = 96;
  const MAX_WIDTH = 340;
  const EXPANDED_WIDTH = 340;
  const COLLAPSED_HEIGHT = 30;
  const HEADER_HEIGHT = 30;
  const MIN_LIST = 44;
  const MAX_LIST = 430;

  let sessions = $state<Session[]>([]);
  let expanded = $state(false);
  let headerEl = $state<HTMLDivElement>();
  let listEl = $state<HTMLUListElement>();

  const focusSession = $derived(pillFocus(sessions));
  const headline = $derived(pillHeadline(sessions));
  const descriptor = $derived(pillDescriptor(sessions));
  const accent = $derived(focusSession ? accentFor(focusSession.status) : undefined);
  const needsYou = $derived(accent === WAITING);

  // How loud the whole pill should be, not just its text — collapsed, a
  // colored word alone didn't read as "waiting vs working vs done" at a
  // glance (the actual complaint: all three, and idle, looked the same
  // shape/weight, just a different small color). One name per tier so the
  // template picks one clear class instead of stacking booleans.
  const level = $derived(
    accent === WAITING ? "waiting" : accent === WORKING ? "working" : accent === DONE ? "done" : "none",
  );

  // Resizing this transparent window can make WebKit recreate the webview's
  // backing layer for the new size, and the fresh layer comes back opaque —
  // a real wry/WebKit interaction, not a CSS bug (see
  // platform::panel::keep_transparent's doc comment). Re-assert transparency
  // after every resize rather than guessing which ones triggered it.
  function resize(width: number, height: number) {
    win
      .setSize(new LogicalSize(width, height))
      .then(() => invoke("keep_pill_transparent"))
      .catch(() => {});
  }

  // Sized to exactly its content, never wider than it needs to be (a real DOM
  // element already knows its own natural width). Only while
  // collapsed: the window is `resizable(true)` so once expanded, a manual
  // drag-resize should stick rather than get overwritten on the next tick.
  function fitCollapsedWidth() {
    if (expanded || !headerEl) return;
    const header = headerEl;
    requestAnimationFrame(() => {
      if (expanded || !header.isConnected) return;
      // header.scrollWidth alone under-measures: .headline has its own
      // overflow:hidden (for the steady-state ellipsis), which clips its
      // natural text width before it can ever propagate up into the
      // parent's scrollWidth -- what gets read back is whatever width the
      // window *already* is, not what it needs to be. The window can never
      // grow past its current size this way, which is exactly the "squished
      // on first spawn" bug: at creation the window is COLLAPSED_WIDTH (a
      // guess), .headline immediately truncates to fit that guess, and the
      // "natural width" measurement just confirms the guess right back.
      // Fix: temporarily let .headline render unclipped (see the
      // .measuring override below), measure, put the constraint back --
      // the resize afterward re-applies normal flex-shrink layout at the
      // new, now-correct size.
      header.classList.add("measuring");
      const width = Math.round(Math.min(Math.max(header.scrollWidth, MIN_WIDTH), MAX_WIDTH));
      header.classList.remove("measuring");
      resize(width, COLLAPSED_HEIGHT);
    });
  }

  // One-time size-to-content on expand. After this, the user drags the window edge for
  // whatever height they actually want; we don't fight that on every
  // session-list update the way the collapsed pill's width-fit does.
  function fitExpandedHeight() {
    const list = listEl;
    if (!list) return;
    requestAnimationFrame(() => {
      if (!list.isConnected) return;
      const listHeight = Math.round(Math.min(Math.max(list.scrollHeight, MIN_LIST), MAX_LIST));
      resize(EXPANDED_WIDTH, HEADER_HEIGHT + listHeight);
    });
  }

  async function toggle() {
    // A list exists to disambiguate WHICH session to focus. With exactly one,
    // there's nothing to disambiguate -- expanding just re-shows the same
    // name/status the collapsed header already said, as a redundant second
    // row (the literal bug: "why is this even expandable, seems wasteful").
    // Skip the list entirely and go straight to the one useful action.
    if (sessions.length <= 1) {
      if (sessions.length === 1) focusRow(sessions[0]);
      return;
    }

    expanded = !expanded;
    // Only resizable while expanded — a 30px-tall collapsed pill leaves
    // barely any area that isn't a resize-edge hit zone, which eats clicks.
    win.setResizable(expanded).catch(() => {});
    if (expanded) {
      // `listEl` only binds once the `{#if expanded}` block actually mounts;
      // reading it synchronously here would still see the old (undefined)
      // value and silently skip the resize — the exact bug that made this
      // look "minimized" (full list content, still the old tiny window frame).
      await tick();
      fitExpandedHeight();
    }
  }

  $effect(() => {
    void headline; // re-fit whenever what's rendered changes
    void descriptor;
    void focusSession;
    void expanded;
    fitCollapsedWidth(); // no-op while expanded
  });

  // `data-tauri-drag-region` alone eats the click: it starts a native window
  // drag on mousedown, before the webview can tell a press-that-didn't-move
  // (a click) apart from an actual drag. The fix: track press/move/release
  // state and call `startDragging()` ourselves, only once the pointer
  // has actually moved past a small threshold.
  let pressStart: { x: number; y: number } | null = null;
  let dragging = false;

  function onHeaderPointerDown(e: PointerEvent) {
    if (e.button !== 0) return; // right-click opens the context menu instead
    pressStart = { x: e.clientX, y: e.clientY };
    dragging = false;
  }

  function onHeaderPointerMove(e: PointerEvent) {
    if (!pressStart || dragging) return;
    if (Math.hypot(e.clientX - pressStart.x, e.clientY - pressStart.y) < 3) return;
    dragging = true;
    win.startDragging().catch(() => {});
  }

  function onHeaderPointerUp() {
    if (pressStart && !dragging) toggle();
    pressStart = null;
    dragging = false;
  }

  async function focusRow(session: Session) {
    const result = await invoke<FocusResult>("focus", {
      sessionId: session.sessionId,
      pid: session.pid,
      title: session.title ?? session.sessionName,
    });
    if (result.kind === "Focused" || result.kind === "NotFound") {
      expanded = false; // the $effect resizes back down
    }
  }

  function openStable() {
    setShowStablePref(true);
    invoke("enter_stable_mode").catch(() => {});
  }

  // Right-click for "stable"/"exit" — a visible icon button here was bad UX
  // (unclear, cramped in a 30px header). A context menu is the conventional
  // place to look for this on a small always-on-top widget. "stable" is the
  // full window — where you go to see the whole herd at rest, per
  // design/saddle-exploration.html's confirmed naming.
  async function onContextMenu(e: MouseEvent) {
    e.preventDefault();
    const menu = await Menu.new({
      items: [
        { text: "stable", action: openStable },
        { text: "exit", action: () => invoke("quit") },
      ],
    });
    await menu.popup();
  }

  onMount(() => {
    invoke<Session[]>("sessions").then((s) => (sessions = s));
    const unlisten = listen<Session[]>("fleet://changed", (event) => {
      sessions = event.payload;
    });
    return () => {
      unlisten.then((f) => f());
    };
  });
</script>

<div
  class="pill"
  class:expanded
  class:waiting={level === "waiting"}
  class:working={level === "working" && !expanded}
  class:done={level === "done" && !expanded}
  style:--accent={accent}
  style:--border-strength={level === "waiting" ? "70%" : accent ? "45%" : "22%"}
>
  <!-- Split on purpose: `backdrop-filter` + `border-radius` + `overflow:
       hidden` all on the same element is a known WebKit failure mode on
       macOS — WKWebView can lose the clip after a resize and the element's
       own background fills the window as a plain rectangle ("square
       corners" surviving two earlier fixes aimed at window-level
       transparency, because the real bug was this element-level one).
       `.pill` above owns only shape (radius + clip + the drop
       shadow/glow, which needs the *correct* radius to silhouette right);
       `.pill-surface` below owns the glass background/blur/tint and never
       needs to clip anything itself, since its parent already does. -->
  <div
    class="pill-surface"
    class:waiting={level === "waiting"}
    class:working={level === "working" && !expanded}
    class:done={level === "done" && !expanded}
  >
    <div
      bind:this={headerEl}
      class="header"
      role="button"
      tabindex="0"
      onpointerdown={onHeaderPointerDown}
      onpointermove={onHeaderPointerMove}
      onpointerup={onHeaderPointerUp}
      oncontextmenu={onContextMenu}
    >
      <span class="count">{sessions.length}</span>
      <span class="dp-dot" class:lit={!!accent} class:pulse={needsYou}></span>
      <span class="headline" class:accented={needsYou}>{headline}</span>
      {#if descriptor}
        <span class="descriptor">{descriptor}</span>
      {/if}
      {#if focusSession}
        <span class="age">{age(focusSession.since)}</span>
      {/if}
    </div>

    {#if expanded}
      <ul class="list" bind:this={listEl}>
        {#if sessions.length === 0}
          <li class="empty">no sessions</li>
        {/if}
        {#each sessions as session (session.sessionId)}
          {@const rowAccent = accentFor(session.status)}
          <li>
            <button class="dp-row" style:--accent={rowAccent} onclick={() => focusRow(session)}>
              <span class="dp-dot" class:lit={!!rowAccent}></span>
              <span class="dp-name">{displayName(session)}</span>
              <span class="dp-status">{statusLabel(session.status)}</span>
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</div>

<style>
  :global(html),
  :global(body) {
    background: transparent;
    margin: 0;
    overflow: hidden;
  }

  .pill {
    /* .ultraThinMaterial-ish glass override — this window is genuinely a
       different type (transparent OS window) from the stable window's opaque one,
       so it earns its own --card/--border, but reuses every other token. */
    --card: color-mix(in srgb, canvas 42%, transparent);
    --border: color-mix(in srgb, canvastext 12%, transparent);

    font-size: 12px;
    height: 100vh;
    border-radius: var(--radius-lg);
    overflow: hidden;
    box-shadow:
      0 1px 6px rgba(0, 0, 0, 0.3),
      inset 0 0 0 1px color-mix(in srgb, var(--accent, canvastext) var(--border-strength, 12%), transparent);
  }

  .pill.expanded {
    /* Top corners keep the collapsed capsule's roundness (visual continuity
       with the pill it just opened from); bottom drops to the stable window's
       standard --radius-md now that it's a tall list, not a capsule. A flat
       --radius-md on all four corners read as an arbitrary rounding rather
       than a shape choice. */
    border-radius: var(--radius-lg) var(--radius-lg) var(--radius-md) var(--radius-md);
  }

  /* The pulsing glow needs the *correct* (possibly per-corner) radius to
     silhouette right, so it lives here on the outer, clipping element —
     not on `.pill-surface`, which deliberately has no radius of its own. */
  .pill.waiting {
    animation: dp-glow 1.6s ease-in-out infinite;
  }

  @keyframes dp-glow {
    0%,
    100% {
      box-shadow:
        0 1px 6px rgba(0, 0, 0, 0.3),
        inset 0 0 0 1px color-mix(in srgb, var(--accent) var(--border-strength, 12%), transparent),
        0 0 0 0 color-mix(in srgb, var(--accent) 55%, transparent);
    }
    50% {
      box-shadow:
        0 1px 6px rgba(0, 0, 0, 0.3),
        inset 0 0 0 1px color-mix(in srgb, var(--accent) var(--border-strength, 12%), transparent),
        0 0 14px 4px color-mix(in srgb, var(--accent) 65%, transparent);
    }
  }

  .pill-surface {
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--card);
    backdrop-filter: blur(28px) saturate(1.6);
    -webkit-backdrop-filter: blur(28px) saturate(1.6);
  }

  /* One whole-shell tint per state, each unmistakably different at a glance
     even collapsed — the actual ask: "where is my input required, which is
     processing, which is idle" shouldn't require reading small text.
     Graduated strength: waiting (loudest, plus the glow above) > working >
     done > idle (no tint at all — the one state with nothing to report). */
  .pill-surface.waiting {
    background: color-mix(in srgb, var(--accent) 34%, var(--card));
  }

  .pill-surface.working {
    background: color-mix(in srgb, var(--accent) 20%, var(--card));
  }

  .pill-surface.done {
    background: color-mix(in srgb, var(--accent) 16%, var(--card));
  }

  .pill.waiting .count,
  .pill.working .count,
  .pill.done .count {
    /* Solid fill + white text reads as an actual status badge, not a tinted
       counter — the same jump a real notification badge makes over a plain
       number. */
    background: var(--accent);
    color: white;
    box-shadow: none;
  }

  .header {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 30px;
    padding: 0 11px 0 5px;
    border: none;
    background: transparent;
    color: inherit;
    font: inherit;
    cursor: pointer;
    flex-shrink: 0;
    white-space: nowrap;
  }

  .pill.expanded .header {
    width: 100%;
    box-sizing: border-box;
    border-bottom: 1px solid var(--border);
  }

  .count {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    font-size: 11px;
    font-weight: 700;
    font-variant-numeric: tabular-nums;
    color: var(--accent, var(--fg-dim));
    background: color-mix(in srgb, var(--accent, canvastext) 16%, transparent);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent, canvastext) 45%, transparent);
  }

  .headline {
    font-size: 12px;
    font-weight: 500;
    color: var(--fg-dim);
    min-width: 0;
    flex-shrink: 1;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* Applied to .header only for the duration of one measurement in
     fitCollapsedWidth() -- lets .headline report its true natural width by
     removing the exact constraint that would otherwise clip it first. */
  :global(.header.measuring) .headline {
    flex-shrink: 0;
    overflow: visible;
  }

  .headline.accented {
    color: var(--accent);
    font-weight: 700;
  }

  /* Radar-style ping: a solid dot plus an expanding, fading ring — motion
     reads as "act now" far louder than a color alone does. */
  .dp-dot.pulse {
    position: relative;
  }

  .dp-dot.pulse::after {
    content: "";
    position: absolute;
    inset: -3px;
    border-radius: 50%;
    border: 1.5px solid var(--accent, currentColor);
    animation: dp-ping 1.4s cubic-bezier(0, 0, 0.2, 1) infinite;
  }

  @keyframes dp-ping {
    0% {
      transform: scale(1);
      opacity: 0.9;
    }
    75%,
    100% {
      transform: scale(2.4);
      opacity: 0;
    }
  }

  .descriptor {
    font-size: 11px;
    font-family: var(--font-mono);
    color: var(--fg-faint);
  }

  .age {
    font-size: 11px;
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
    color: var(--fg-faint);
    min-width: 2em;
    text-align: right;
  }

  .list {
    list-style: none;
    margin: 0;
    padding: 4px;
    overflow-y: auto;
    overflow-x: hidden;
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 1px;
    font-size: 12px;
  }

  .list .dp-status {
    font-size: 11px;
  }

  .empty {
    padding: 0.6rem;
    color: var(--fg-faint);
  }
</style>
