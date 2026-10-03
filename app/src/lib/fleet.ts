// Mirrors fleet::store::{Session, Status} and platform::terminal::FocusResult
// on the Rust side. Shared by the stable window and the pill — both render the same
// session data.

// serde's default external tagging: a unit variant (Done/Idle) is a
// plain string, a data-carrying one (WaitingInput/Working) is a single-key
// object whose value is the payload (or null).
export type Status =
  | { WaitingInput: string | null }
  | { Working: string | null }
  | "Done"
  | "Idle";

export type Session = {
  sessionId: string;
  pid: number;
  repo: string;
  cwd: string | null;
  sessionName: string | null;
  branch: string | null;
  title: string | null;
  status: Status;
  since: number;
};

// #[serde(tag = "kind", content = "detail")]
export type FocusResult =
  | { kind: "Focused" }
  | { kind: "NotFound" }
  | { kind: "NotPermitted" }
  | { kind: "Failed"; detail: string };

// Prefer the AI-derived title ("Class Link Notifications for BIM") over
// Claude's own generic per-session name ("workspace-e1") over the raw repo
// folder name. The generic name exists specifically for sessions sharing one
// cwd, but in practice it's just a dir name plus a short
// disambiguating suffix — meaningless until you already know which session
// is which. The title only populates once Claude derives one from the
// actual conversation (fleet::transcript's `ai-title` line), so early in a
// session's life this still falls back correctly.
export function displayName(s: Session): string {
  return s.title ?? s.sessionName ?? s.repo;
}

// "WaitingInput" 's own lowercase ("waitinginput") doesn't match the
// "waiting" every caller here and in +page.svelte compares against — that
// mismatch meant a waiting session never matched anywhere (accentFor never
// went red, pillFocus never surfaced it, the stable window's "waiting" list was
// always empty) and silently fell in with plain "idle" instead. Root-caused
// once here rather than patched at each of those call sites.
export function statusKind(status: Status): string {
  if (typeof status === "string") return status.toLowerCase();
  const key = Object.keys(status)[0]; // "WaitingInput" | "Working"
  return key === "WaitingInput" ? "waiting" : key.toLowerCase();
}

// saddle's status accents — Tack, from design/saddle-exploration.html.
// Named by ROLE, not by color: the previous version (RED/TEAL/TAN) named
// itself after whelmedthinker.com's literal hues, and the first thing that
// broke when the palette changed to Tack was TEAL silently holding a brass
// value — a color-named constant can't help but drift the moment the color
// does. Must match src/app.css's --waiting / --working / --done tokens.
// "idle" stays unaccented on purpose: it's the only state with nothing to
// report.
export const WAITING = "#c1583a";
export const WORKING = "#c89b3c";
export const DONE = "#6b7a4f";

export function accentFor(status: Status): string | undefined {
  const kind = statusKind(status);
  if (kind === "waiting") return WAITING;
  if (kind === "working") return WORKING;
  if (kind === "done") return DONE;
  return undefined;
}

// "waiting" | "working" | "done" | "idle" plus, when known, the
// notification message / tool name that came with it.
export function statusLabel(status: Status): string {
  if (typeof status === "string") return status.toLowerCase();
  if ("WaitingInput" in status) return status.WaitingInput ? `waiting · ${status.WaitingInput}` : "waiting";
  if ("Working" in status) return status.Working ? `working · ${status.Working}` : "working";
  return "unknown";
}

export function age(since: number): string {
  const seconds = Math.max(0, Date.now() / 1000 - since);
  if (seconds < 60) return `${Math.floor(seconds)}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  return `${Math.floor(seconds / 3600)}h`;
}

// Sessions already arrive rank-sorted (waiting beats working beats done beats
// everything else — fleet::store::sort_sessions), so the first non-idle
// session IS "the one that matters". "done" used
// to be excluded here, so a fleet that had only just-finished sessions still
// read as "all idle" in the collapsed pill — the exact "can't tell waiting
// from working from done at a glance" complaint.
export function pillFocus(sessions: Session[]): Session | undefined {
  return sessions.find((s) => {
    const kind = statusKind(s.status);
    return kind === "waiting" || kind === "working" || kind === "done";
  });
}

export function pillHeadline(sessions: Session[]): string {
  const focus = pillFocus(sessions);
  if (focus) return displayName(focus);
  return sessions.length > 0 ? "all idle" : "no sessions";
}

// "needs you" / the tool name, when there's a focus session — else nothing.
export function pillDescriptor(sessions: Session[]): string | null {
  const focus = pillFocus(sessions);
  if (!focus) return null;
  const status = focus.status;
  if (typeof status !== "string" && "WaitingInput" in status) return "needs you";
  if (typeof status !== "string" && "Working" in status) return (status.Working ?? "working").slice(0, 14);
  if (status === "Done") return "done";
  return null;
}
