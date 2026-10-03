<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { type Session, type FocusResult, displayName, statusKind, statusLabel, age, accentFor } from "$lib/fleet";
  import { getShowStablePref, setShowStablePref, hasChosenModePref } from "$lib/prefs";

  // Launch flow: this window is created hidden (tauri.conf.json), so nothing
  // flashes on screen before the mode is known. If you chose "stable" last
  // time — or it's the very first launch, so the setup notice gets seen —
  // the window is shown with a brief splash. Otherwise the whole app
  // switches into saddle mode, the default, without ever showing this
  // window. "Switching" is a real mode change, not just show/hide: the
  // app's activation policy flips (Dock icon vs. none) and the pill window
  // is created/destroyed accordingly. See commands::enter_saddle_mode for
  // why that's necessary rather than cosmetic.
  let phase = $state<"splash" | "app">("splash");
  let sessions = $state<Session[]>([]);
  let permissionDenied = $state(false);
  let version = $state("");
  let hooksInstalled = $state(true); // optimistic until the first check, so the notice doesn't flash
  let installing = $state(false);
  let installError = $state("");
  let justInstalled = $state(false);

  // Three buckets, not five statuses — matches the stat strip and the
  // grouped list below. done/idle both just mean "not blocking you".
  const waiting = $derived(sessions.filter((s) => statusKind(s.status) === "waiting"));
  const working = $derived(sessions.filter((s) => statusKind(s.status) === "working"));
  const idle = $derived(sessions.filter((s) => !["waiting", "working"].includes(statusKind(s.status))));

  async function focus(session: Session) {
    const result = await invoke<FocusResult>("focus", {
      sessionId: session.sessionId,
      pid: session.pid,
      title: session.title ?? session.sessionName,
    });
    permissionDenied = result.kind === "NotPermitted";
  }

  async function installHooks() {
    installing = true;
    installError = "";
    try {
      await invoke("install_hooks");
      hooksInstalled = await invoke<boolean>("hooks_installed");
      justInstalled = hooksInstalled;
    } catch (e) {
      installError = String(e);
    } finally {
      installing = false;
    }
  }

  function switchToSaddle() {
    setShowStablePref(false);
    invoke("enter_saddle_mode").catch(() => {});
  }

  onMount(() => {
    invoke<Session[]>("sessions").then((s) => (sessions = s));
    invoke<string>("app_version").then((v) => (version = v));
    invoke<boolean>("hooks_installed").then((ok) => (hooksInstalled = ok));

    const unlisten = listen<Session[]>("fleet://changed", (event) => {
      sessions = event.payload;
    });

    let splashTimer: ReturnType<typeof setTimeout> | undefined;
    if (getShowStablePref() || !hasChosenModePref()) {
      invoke("enter_stable_mode").catch(() => {});
      splashTimer = setTimeout(() => (phase = "app"), 600);
    } else {
      // This window stays alive hidden. Leave `phase` at "app", not
      // "splash": showing it later (the pill's "stable" menu item) would
      // otherwise reveal nothing but the splash forever.
      phase = "app";
      invoke("enter_saddle_mode").catch(() => {});
    }

    return () => {
      unlisten.then((f) => f());
      clearTimeout(splashTimer);
    };
  });
</script>

{#if phase === "splash"}
  <main class="splash">
    <!-- Profile, Bold weight — the confirmed mark, same path + stroke-width
         used everywhere else it appears (the icon source, the menubar).
         Real content now; was an empty 8px dot that faded in. -->
    <svg class="mark" width="52" height="52" viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <path
        d="M3 16c2-5 6-9 10-9 3 0 4.5 1.5 5 3.5.5 2-.5 3.5-2.5 4-3 .8-6-.3-8.5 1.5C5.5 17.3 4 18.5 3 19"
        stroke="var(--waiting)"
        stroke-width="2.4"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  </main>
{:else}
  <main class="app">
    <!-- Was a big full-width bar reading "take off →" (a real placeholder
         that shipped, not a mockup) — a sleek icon button now, matching
         design/saddle-exploration.html's confirmed toggle mockup. No
         custom titlebar exists in the real app (the native macOS one
         provides the traffic lights), so this sits as its own small
         corner control rather than living inside chrome that isn't there. -->
    <div class="toolbar">
      <!-- Icon-only, tucked alone in an otherwise empty bar, was genuinely
           hard to find -- real feedback against the running build, not a
           mockup nit. A labeled button now: it says what it does, and the
           tinted fill reads as clickable at rest instead of only on hover. -->
      <button class="mode-toggle" onclick={switchToSaddle}>
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <rect x="3" y="8" width="18" height="8" rx="4" stroke="currentColor" stroke-width="2" />
          <circle cx="8" cy="12" r="1.4" fill="currentColor" />
        </svg>
        saddle
      </button>
    </div>

    <div class="stats">
      <div class="stat">
        <b style:color="var(--waiting)">{waiting.length}</b>
        <span>needs you</span>
      </div>
      <div class="stat">
        <b style:color="var(--working)">{working.length}</b>
        <span>working</span>
      </div>
      <div class="stat">
        <b>{idle.length}</b>
        <span>idle</span>
      </div>
    </div>

    {#if !hooksInstalled}
      <div class="setup">
        <p>
          <b>one-time setup.</b> saddle needs three hooks in
          <code>~/.claude/settings.json</code> to know when a session is waiting or done. your
          existing settings are kept and a backup is written first.
        </p>
        <button class="setup-btn" onclick={installHooks} disabled={installing}>
          {installing ? "installing…" : "install hooks"}
        </button>
        {#if installError}
          <p class="setup-error">{installError}</p>
        {/if}
      </div>
    {:else if justInstalled}
      <p class="notice">hooks installed. restart any running claude sessions so they pick them up.</p>
    {/if}

    {#if permissionDenied}
      <p class="notice">
        automation permission needed — grant terminal access in system settings, then try again
      </p>
    {/if}

    <div class="groups">
      {#if sessions.length === 0}
        <p class="empty">no sessions</p>
      {:else}
        {#each [{ label: "needs you", items: waiting }, { label: "working", items: working }, { label: "idle", items: idle }] as group (group.label)}
          {#if group.items.length > 0}
            <div class="group-label">{group.label}</div>
            <ul>
              {#each group.items as session (session.sessionId)}
                {@const accent = accentFor(session.status)}
                <li>
                  <button class="dp-row" style:--accent={accent} onclick={() => focus(session)}>
                    <span class="dp-dot" class:lit={!!accent}></span>
                    <span class="dp-name">{displayName(session)}</span>
                    <span class="repo">{session.repo}</span>
                    <span class="dp-status">{statusLabel(session.status)}</span>
                    <span class="dp-age">{age(session.since)}</span>
                  </button>
                </li>
              {/each}
            </ul>
          {/if}
        {/each}
      {/if}
    </div>

    <footer class="about">
      <div class="about-row">
        <span class="about-mark">saddle</span>
        <span class="dp-age">v{version || "0.1.0"} beta</span>
      </div>
      <div class="about-meta">
        <span>© 2026 Atharva Kawade</span>
        <span>MIT license</span>
      </div>
      <div class="about-meta">
        <a href="https://github.com/ask0ne/saddle" target="_blank" rel="noreferrer">github.com/ask0ne/saddle</a>
        <a href="https://whelmedthinker.com" target="_blank" rel="noreferrer">whelmedthinker.com</a>
      </div>
    </footer>
  </main>
{/if}

<style>
  :root {
    font-size: 14px;
  }

  main {
    background: canvas;
  }

  .splash {
    height: 100vh;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .mark {
    opacity: 0;
    animation: fade-in 0.4s ease-out forwards;
  }

  @keyframes fade-in {
    from {
      opacity: 0;
      transform: translateY(2px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }

  /* No gutters: the CTA, stat strip and list all run edge-to-edge. Only the
     about footer gets its own internal padding — it reads as a footer, not
     as part of the flush content above it. */
  .app {
    min-height: 100vh;
    display: flex;
    flex-direction: column;
  }

  .toolbar {
    display: flex;
    justify-content: flex-end;
    padding: var(--space-2) var(--space-3);
    border-bottom: 1px solid var(--border);
  }

  .mode-toggle {
    display: flex;
    align-items: center;
    gap: 7px;
    height: 34px;
    padding: 0 var(--space-3);
    /* A visible tint at rest (not just on hover) plus an actual word --
       "sleek" meant not-a-giant-bar, not invisible-until-you-stumble-on-it.
       Working's brass, not a neutral border: this is the one real action
       on the whole screen, it should look like one. */
    border: 1px solid color-mix(in srgb, var(--working) 40%, transparent);
    border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--working) 14%, var(--card));
    color: var(--working);
    font: inherit;
    font-size: 0.85rem;
    font-weight: 600;
    cursor: pointer;
    transition: background 0.15s ease-out;
  }

  .mode-toggle:hover {
    background: color-mix(in srgb, var(--working) 22%, var(--card));
    color: var(--fg);
    border-color: var(--border);
  }

  .mode-toggle:active {
    opacity: 0.85;
  }

  .stats {
    display: flex;
    border-bottom: 1px solid var(--border);
  }

  .stat {
    flex: 1;
    padding: var(--space-3) var(--space-4);
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .stat + .stat {
    border-left: 1px solid var(--border);
  }

  .stat b {
    font-size: 1.4rem;
    font-family: var(--font-mono);
    font-weight: 700;
  }

  .stat span {
    font-size: 0.68rem;
    color: var(--fg-faint);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .empty {
    color: var(--fg-faint);
    margin: 0;
    padding: var(--space-4);
  }

  .setup {
    padding: var(--space-3) var(--space-4);
    background: color-mix(in srgb, var(--working) 12%, transparent);
    border-bottom: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-2);
  }

  .setup p {
    margin: 0;
    font-size: 0.85rem;
    line-height: 1.45;
    color: var(--fg-dim);
  }

  .setup b {
    color: var(--fg);
  }

  .setup code {
    font-family: var(--font-mono);
    font-size: 0.78rem;
  }

  .setup-btn {
    height: 30px;
    padding: 0 var(--space-3);
    border: 1px solid color-mix(in srgb, var(--working) 40%, transparent);
    border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--working) 14%, var(--card));
    color: var(--working);
    font: inherit;
    font-size: 0.85rem;
    font-weight: 600;
    cursor: pointer;
  }

  .setup-btn:hover:not(:disabled) {
    background: color-mix(in srgb, var(--working) 22%, var(--card));
    color: var(--fg);
  }

  .setup-btn:disabled {
    opacity: 0.6;
    cursor: default;
  }

  .setup .setup-error {
    color: var(--waiting);
    font-family: var(--font-mono);
    font-size: 0.75rem;
  }

  .notice {
    padding: var(--space-3) var(--space-4);
    margin: 0;
    background: color-mix(in srgb, var(--waiting) 15%, transparent);
  }

  .groups {
    flex: 1;
    padding-bottom: var(--space-2);
  }

  .group-label {
    padding: var(--space-3) var(--space-4) var(--space-1);
    font-size: 0.68rem;
    font-weight: 700;
    color: var(--fg-faint);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0 var(--space-2);
    display: flex;
    flex-direction: column;
  }

  .dp-row {
    border-radius: var(--radius-sm);
    padding: var(--space-2) var(--space-2);
  }

  .repo {
    color: var(--fg-faint);
    white-space: nowrap;
  }

  .dp-age {
    min-width: 2.5em;
    text-align: right;
  }

  .about {
    padding: var(--space-4);
    border-top: 1px solid var(--border);
  }

  .about-row {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
  }

  .about-mark {
    /* wordmark only, per the theme exploration's font decision — nowhere
       else in this file uses this family. */
    font-family: "IBM Plex Sans", -apple-system, BlinkMacSystemFont, sans-serif;
    font-weight: 700;
    text-transform: lowercase;
    letter-spacing: -0.005em;
  }

  .about-meta {
    margin-top: var(--space-2);
    display: flex;
    align-items: center;
    gap: var(--space-3);
    font-family: var(--font-mono);
    font-size: 0.7rem;
    color: var(--fg-faint);
  }

  .about-meta a {
    color: var(--fg-faint);
    text-decoration: none;
  }

  .about-meta a:hover {
    color: var(--fg-dim);
    text-decoration: underline;
  }
</style>
