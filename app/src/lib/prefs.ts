// Persisted user prefs. `localStorage` is shared across all of saddle's
// windows (main/pill both load the same built app, so they're same-origin) —
// no Tauri store plugin needed for a single boolean.

const SHOW_STABLE_KEY = "saddle.showStable";

export function getShowStablePref(): boolean {
  try {
    return localStorage.getItem(SHOW_STABLE_KEY) === "1";
  } catch {
    return false; // defaults to saddle (pill) only
  }
}

// True once the user has picked a mode at least once. Before that — the
// first launch — saddle stays in the stable window so the setup notice is
// seen, instead of vanishing into a pill nobody has been introduced to.
export function hasChosenModePref(): boolean {
  try {
    return localStorage.getItem(SHOW_STABLE_KEY) !== null;
  } catch {
    return true; // storage unavailable: can't remember a choice, don't nag
  }
}

export function setShowStablePref(value: boolean): void {
  try {
    localStorage.setItem(SHOW_STABLE_KEY, value ? "1" : "0");
  } catch {
    // worst case: the choice doesn't survive a restart
  }
}
