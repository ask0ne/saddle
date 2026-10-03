//! The `Fleet`: one `Mutex`-guarded snapshot of every live Claude Code session,
//! rebuilt on every `refresh()`.
//!
//! Per ARCHITECTURE.md (Concurrency), several sources (fs watchers, a 1s poll
//! timer, ipc commands) mutate this from different threads — writes are
//! frequent and reads are rare, so one plain `std::sync::Mutex` around
//! everything beats `RwLock` or an actor here. Fleet itself stays
//! Tauri-agnostic; `lib.rs` registers it with `.manage(Fleet::default())`
//! and reaches it through `State<Fleet>`.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use super::events::EventLog;
use super::registry::{self, RegistryEntry};
use super::transcript::TranscriptReader;

/// Derived per ARCHITECTURE.md's Status model:
///
/// | state | derived from |
/// |---|---|
/// | `WaitingInput` | `Notification` hook fired, nothing submitted or seen since |
/// | `Working{tool}` | registry `status == "busy"`, labelled with newest `tool_use` |
/// | `Done` | `Stop` hook fired, user hasn't looked yet |
/// | `Idle` | not busy, already seen |
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum Status {
    WaitingInput(Option<String>), // notification message, when known
    Working(Option<String>),      // tool name, when known
    Done,
    Idle,
}

impl Status {
    /// Sort key, not a total order on the full variant (a derived `Ord` would
    /// also compare the `Option<String>` payload, which would sort two
    /// `WaitingInput` sessions by message text instead of leaving that to the
    /// `since` tiebreak — which is why this is its own method rather than an
    /// `Ord` impl).
    pub fn rank(&self) -> u8 {
        match self {
            Status::WaitingInput(_) => 0,
            Status::Working(_) => 1,
            Status::Done => 2,
            Status::Idle => 3,
        }
    }

    /// The badge counts only this — not called yet; no badge/tray step wired up.
    #[allow(dead_code)]
    pub fn needs_you(&self) -> bool {
        matches!(self, Status::WaitingInput(_))
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub session_id: String,
    pub pid: i32,
    pub repo: String,
    pub cwd: Option<String>,
    /// Claude Code's own derived per-session name ("ask0ne-f4"). Three
    /// sessions in one repo all share the same `repo`, so this is the better
    /// identity when present — `src/lib/fleet.ts`'s `displayName()` is where
    /// the `?? repo` fallback actually happens; kept there, not duplicated
    /// here, since it's a pure presentation choice over already-true fields,
    /// not state Rust needs to own (ARCHITECTURE.md's "Rust owns the truth"
    /// principle is about state, not which string a view picks to show).
    pub session_name: Option<String>,
    pub branch: Option<String>,
    pub title: Option<String>,
    pub status: Status,
    pub since: f64, // epoch seconds; time in the current state, not session age
}

/// What the hooks have told us about one session.
#[derive(Debug, Clone, Default)]
struct HookState {
    notification_at: Option<f64>,
    notification_message: Option<String>,
    stop_at: Option<f64>,
    prompt_at: Option<f64>,
    seen_at: Option<f64>,
}

struct Inner {
    sessions: Vec<Session>,
    /// Live registry entries with no controlling terminal (`ps -o tty=`
    /// returns nothing) — real, running sessions that can never be focused.
    /// Excluded from `sessions` entirely rather than shown as ordinary,
    /// clickable rows: showing them was a real bug once ("shows 4 where only 2
    /// exist"). Not yet surfaced in the UI as "+n detached" — same
    /// computed-but-not-wired-up treatment as `waiting_count` below, ready
    /// for whenever a badge/tray is added.
    detached_count: usize,
    hooks: HashMap<String, HookState>,
    events: EventLog,
    transcripts: TranscriptReader,
    registry_dir: std::path::PathBuf,
}

impl Inner {
    fn new() -> Self {
        Self {
            sessions: Vec::new(),
            detached_count: 0,
            hooks: HashMap::new(),
            events: EventLog::new(),
            transcripts: TranscriptReader::new(),
            registry_dir: registry::registry_dir().unwrap_or_default(),
        }
    }

    /// Every event appended since the last call, folded into hook state, then
    /// a full re-derive — matches `SessionStore.pump()`.
    fn pump(&mut self) {
        self.pump_with(tty_of);
    }

    /// Same reason as `refresh_with`: `pump()` ends with a `refresh()`, so
    /// testing the hook-drain path with a fake pid needs the same injectable
    /// tty resolver threaded through.
    fn pump_with(&mut self, tty_of: impl Fn(i32) -> Option<String>) {
        let drained = self.events.drain();
        if drained.is_empty() {
            return;
        }
        for event in drained {
            let Some(id) = event.session_id else { continue };
            let state = self.hooks.entry(id).or_default();
            match event.event.as_str() {
                // Claude Code's documented notification_type values (as of
                // 2026-08-31, github.com/anthropics/claude-code#11964):
                // permission_prompt, idle_prompt, auth_success,
                // elicitation_dialog. Only the first and last are a real ask.
                // "idle_prompt" is a "still there?" nudge well after the
                // session already went idle/`Done` (verified false positive).
                // "auth_success" is a plain "you
                // re-authenticated" ping, same non-actionable class.
                // Denylist, not allowlist: anything else, including
                // unknown/future types (and a missing notification_type
                // entirely — a known upstream Claude Code bug, same issue),
                // still counts. Permissive default, same "unknown fields
                // don't break things" tolerance as `registry::RegistryEntry`.
                "Notification"
                    if !matches!(
                        event.notification_type.as_deref(),
                        Some("idle_prompt") | Some("auth_success")
                    ) =>
                {
                    state.notification_at = Some(event.time);
                    state.notification_message = event.message;
                }
                "Stop" => state.stop_at = Some(event.time),
                "UserPromptSubmit" => {
                    state.prompt_at = Some(event.time);
                    state.notification_at = None; // you answered it
                }
                _ => {}
            }
        }
        self.refresh_with(tty_of);
    }

    fn refresh(&mut self) {
        self.refresh_with(tty_of);
    }

    /// The real body, parameterized over tty resolution — same reason
    /// `registry::read_dir` takes a `&Path` instead of hardcoding
    /// `~/.claude/sessions`: a test using a real pid (`std::process::id()`,
    /// so the registry entry looks genuine) can't assume that process has a
    /// controlling terminal — true interactively, not guaranteed in CI. The
    /// real path (`refresh()`) always resolves real ttys; only tests inject
    /// something else.
    fn refresh_with(&mut self, tty_of: impl Fn(i32) -> Option<String>) {
        let entries = registry::read_dir(&self.registry_dir);
        let live_ids: HashSet<String> = entries.iter().map(|e| e.session_id.clone()).collect();

        self.hooks.retain(|id, _| live_ids.contains(id));
        self.transcripts.prune(&live_ids);

        let mut next: Vec<Session> = Vec::with_capacity(entries.len());
        let mut detached = 0usize;
        for entry in entries {
            // A Task-tool subagent, not a terminal of its own — see
            // `RegistryEntry::is_interactive`'s doc comment.
            if !entry.is_interactive() {
                continue;
            }

            // A live registry entry with no controlling terminal is real but
            // can never be focused — count it, don't list it. See the
            // `detached_count` doc comment on `Inner` for why this matters.
            if tty_of(entry.pid).is_none() {
                detached += 1;
                continue;
            }

            let info = self.transcripts.info(&entry.session_id);
            let state = self.hooks.entry(entry.session_id.clone()).or_default();
            let status = derive_status(&entry, state, info.current_tool.as_deref());
            let since = stamp(&entry, state);
            let repo = entry.repo_name().to_string(); // borrows entry — compute before moving its fields below

            next.push(Session {
                session_id: entry.session_id,
                pid: entry.pid,
                repo,
                cwd: entry.cwd,
                session_name: entry.name,
                branch: info.git_branch,
                title: info.ai_title,
                status,
                since,
            });
        }

        sort_sessions(&mut next);
        self.sessions = next;
        self.detached_count = detached;
    }
}

#[cfg(target_os = "macos")]
fn tty_of(pid: i32) -> Option<String> {
    crate::platform::terminal::tty_of(pid)
}

/// No non-macOS build exists yet (ARCHITECTURE.md's Scope section limits this
/// to macOS + Terminal.app deliberately), but treating "can't resolve a tty" as
/// "detached" here, rather than needing a cfg branch at every call site, is
/// the honest answer if this ever runs somewhere `platform::terminal` isn't
/// built: no known tty means no known tty.
#[cfg(not(target_os = "macos"))]
fn tty_of(_pid: i32) -> Option<String> {
    None
}

/// Rank first, then oldest-in-state first within a rank.
fn sort_sessions(sessions: &mut [Session]) {
    sessions.sort_by(|a, b| {
        a.status.rank().cmp(&b.status.rank()).then_with(|| {
            a.since
                .partial_cmp(&b.since)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });
}

/// The whole product, in one function (mirrors `SessionStore.derive`).
fn derive_status(entry: &RegistryEntry, state: &HookState, tool: Option<&str>) -> Status {
    let floor = state
        .prompt_at
        .unwrap_or(f64::NEG_INFINITY)
        .max(state.seen_at.unwrap_or(f64::NEG_INFINITY));

    if let Some(notified) = state.notification_at
        && notified > floor
    {
        return Status::WaitingInput(state.notification_message.clone());
    }
    if entry.is_busy() {
        return Status::Working(tool.map(str::to_string));
    }
    if let Some(stopped) = state.stop_at
        && stopped > state.seen_at.unwrap_or(f64::NEG_INFINITY)
    {
        return Status::Done;
    }
    Status::Idle
}

/// The clock shown on a row: time in the current state, not session age.
fn stamp(entry: &RegistryEntry, state: &HookState) -> f64 {
    if let Some(notified) = state.notification_at {
        return notified;
    }
    if let Some(updated) = entry.status_updated_at {
        return updated / 1000.0;
    }
    if let Some(started) = entry.started_at {
        return started / 1000.0;
    }
    now_epoch_secs()
}

fn now_epoch_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// One `Mutex`-guarded snapshot, safe to share across threads once `.manage()`d.
pub struct Fleet {
    inner: Mutex<Inner>,
}

impl Fleet {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::new()),
        }
    }

    /// Recovers from a poisoned lock instead of propagating it. Without
    /// this, one panic anywhere under the lock (a bug in `refresh`, say)
    /// poisons it permanently — every later call, from the poll timer, both
    /// watcher threads, and every IPC command, would panic too, forever.
    /// `Inner`'s fields have no cross-field invariant a partial mutation
    /// could leave broken, so recovering the guard and carrying on is the
    /// correct choice here, not just a convenient one.
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn refresh(&self) {
        self.lock().refresh();
    }

    pub fn pump(&self) {
        self.lock().pump();
    }

    pub fn sessions(&self) -> Vec<Session> {
        self.lock().sessions.clone()
    }

    /// Badge count: waiting beats everything else. Not exposed as a command
    /// yet — no tray/badge is wired up.
    #[allow(dead_code)]
    pub fn waiting_count(&self) -> usize {
        self.lock()
            .sessions
            .iter()
            .filter(|s| s.status.needs_you())
            .count()
    }

    /// Live sessions with no controlling terminal — real, but unfocusable.
    /// Same not-yet-wired-to-a-command treatment as `waiting_count`: the fix
    /// this accompanies is that they no longer inflate `sessions()`'s count;
    /// surfacing "+n detached" in the UI is a separate, smaller follow-up.
    #[allow(dead_code)]
    pub fn detached_count(&self) -> usize {
        self.lock().detached_count
    }

    pub fn mark_seen(&self, session_id: &str) {
        let mut inner = self.lock();
        inner
            .hooks
            .entry(session_id.to_string())
            .or_default()
            .seen_at = Some(now_epoch_secs());
        inner.refresh();
    }
}

impl Default for Fleet {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(session_id: &str, pid: i32, status: Option<&str>) -> RegistryEntry {
        let json = format!(
            r#"{{"pid":{pid},"sessionId":"{session_id}","status":{}}}"#,
            status.map(|s| format!("\"{s}\"")).unwrap_or("null".into())
        );
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn waiting_input_wins_over_busy() {
        let e = entry("s1", 1, Some("busy"));
        let state = HookState {
            notification_at: Some(10.0),
            notification_message: Some("needs your permission".into()),
            ..Default::default()
        };
        let status = derive_status(&e, &state, Some("Bash"));
        assert_eq!(
            status,
            Status::WaitingInput(Some("needs your permission".into()))
        );
    }

    #[test]
    fn a_prompt_submitted_after_notification_clears_waiting() {
        let e = entry("s1", 1, Some("busy"));
        let state = HookState {
            notification_at: Some(10.0),
            prompt_at: Some(20.0), // you answered it
            ..Default::default()
        };
        assert_eq!(
            derive_status(&e, &state, Some("Bash")),
            Status::Working(Some("Bash".into()))
        );
    }

    #[test]
    fn busy_without_notification_is_working() {
        let e = entry("s1", 1, Some("busy"));
        let state = HookState::default();
        assert_eq!(
            derive_status(&e, &state, Some("Read")),
            Status::Working(Some("Read".into()))
        );
    }

    #[test]
    fn stop_after_seen_does_not_reopen_done() {
        let e = entry("s1", 1, Some("idle"));
        let state = HookState {
            stop_at: Some(10.0),
            seen_at: Some(20.0),
            ..Default::default()
        };
        assert_eq!(derive_status(&e, &state, None), Status::Idle);
    }

    #[test]
    fn stop_before_seen_is_done() {
        let e = entry("s1", 1, Some("idle"));
        let state = HookState {
            stop_at: Some(20.0),
            seen_at: Some(10.0),
            ..Default::default()
        };
        assert_eq!(derive_status(&e, &state, None), Status::Done);
    }

    #[test]
    fn neither_busy_nor_stopped_is_idle() {
        let e = entry("s1", 1, Some("idle"));
        assert_eq!(derive_status(&e, &HookState::default(), None), Status::Idle);
    }

    fn session(id: &str, status: Status, since: f64) -> Session {
        Session {
            session_id: id.into(),
            pid: 1,
            repo: "r".into(),
            cwd: None,
            session_name: None,
            branch: None,
            title: None,
            status,
            since,
        }
    }

    #[test]
    fn sort_ranks_before_age() {
        let mut sessions = vec![
            session("idle-old", Status::Idle, 1.0),
            session("waiting-new", Status::WaitingInput(None), 100.0),
            session("working", Status::Working(None), 50.0),
        ];
        sort_sessions(&mut sessions);
        let ids: Vec<&str> = sessions.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(ids, vec!["waiting-new", "working", "idle-old"]);
    }

    #[test]
    fn sort_breaks_ties_oldest_first() {
        let mut sessions = vec![
            session("newer", Status::Idle, 100.0),
            session("older", Status::Idle, 1.0),
        ];
        sort_sessions(&mut sessions);
        let ids: Vec<&str> = sessions.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(ids, vec!["older", "newer"]);
    }

    #[test]
    fn fleet_recovers_from_a_poisoned_lock() {
        // Prove the recovery, not just that it compiles: actually poison the
        // mutex (a thread panicking while it holds the lock is the only way
        // std ever marks one poisoned), then confirm a later call succeeds
        // instead of panicking too.
        let fleet = std::sync::Arc::new(Fleet::new());
        let poisoner = std::sync::Arc::clone(&fleet);

        let result = std::thread::spawn(move || {
            let _guard = poisoner.inner.lock().unwrap();
            panic!("deliberate — poisons the lock for the assertion below");
        })
        .join();
        assert!(result.is_err(), "the spawned thread should have panicked");

        // Before the fix this unwrap()'d the Err(PoisonError) and panicked
        // here too — a second, unrelated thread paying for the first one's bug.
        let sessions = fleet.sessions();
        assert!(sessions.is_empty());
    }

    #[test]
    fn fleet_pump_transitions_notification_to_waiting_input() {
        let dir = tempdir();
        let events_path = dir.path().join("events.jsonl");
        std::fs::write(&events_path, b"").unwrap();

        // A registry entry matching the event's session_id, or refresh()
        // (which pump() always ends with) prunes the hook state right back
        // out again — the real ~/.claude/sessions doesn't know about "s1".
        let registry_dir = dir.path().join("sessions");
        std::fs::create_dir_all(&registry_dir).unwrap();
        std::fs::write(
            registry_dir.join("1.json"),
            format!(
                r#"{{"pid":{},"sessionId":"s1","status":"idle"}}"#,
                std::process::id()
            ),
        )
        .unwrap();

        let mut inner = Inner {
            sessions: Vec::new(),
            detached_count: 0,
            hooks: HashMap::new(),
            events: EventLog::at(events_path.clone()),
            transcripts: TranscriptReader::new(),
            registry_dir,
        };

        std::fs::write(
            &events_path,
            b"{\"e\":\"Notification\",\"t\":42.0,\"d\":{\"session_id\":\"s1\",\"message\":\"hi\"}}\n",
        )
        .unwrap();
        // Not testing tty resolution here — a fixed resolver keeps this test
        // from depending on whether the process running it happens to have a
        // controlling terminal, which is not guaranteed in CI.
        inner.pump_with(|_| Some("ttys999".into()));

        assert_eq!(inner.sessions.len(), 1);
        assert_eq!(
            inner.sessions[0].status,
            Status::WaitingInput(Some("hi".into()))
        );
    }

    #[test]
    fn idle_prompt_notification_does_not_trigger_waiting_input() {
        // Verified false positive (2026-08-31): Claude Code
        // fires a Notification with notification_type "idle_prompt" as a
        // generic "still there?" nudge, not a real ask — an already-idle
        // session shouldn't flip to WaitingInput because of it.
        let dir = tempdir();
        let events_path = dir.path().join("events.jsonl");
        std::fs::write(&events_path, b"").unwrap();

        let registry_dir = dir.path().join("sessions");
        std::fs::create_dir_all(&registry_dir).unwrap();
        std::fs::write(
            registry_dir.join("1.json"),
            format!(
                r#"{{"pid":{},"sessionId":"s1","status":"idle"}}"#,
                std::process::id()
            ),
        )
        .unwrap();

        let mut inner = Inner {
            sessions: Vec::new(),
            detached_count: 0,
            hooks: HashMap::new(),
            events: EventLog::at(events_path.clone()),
            transcripts: TranscriptReader::new(),
            registry_dir,
        };

        std::fs::write(
            &events_path,
            b"{\"e\":\"Notification\",\"t\":42.0,\"d\":{\"session_id\":\"s1\",\"message\":\"Claude is waiting for your input\",\"notification_type\":\"idle_prompt\"}}\n",
        )
        .unwrap();
        inner.pump_with(|_| Some("ttys999".into()));

        assert_eq!(inner.sessions.len(), 1);
        assert_eq!(inner.sessions[0].status, Status::Idle);
    }

    #[test]
    fn auth_success_notification_does_not_trigger_waiting_input() {
        // Same false-positive class as idle_prompt above (2026-08-31):
        // "you re-authenticated" is informational, not a real ask.
        let dir = tempdir();
        let events_path = dir.path().join("events.jsonl");
        std::fs::write(&events_path, b"").unwrap();

        let registry_dir = dir.path().join("sessions");
        std::fs::create_dir_all(&registry_dir).unwrap();
        std::fs::write(
            registry_dir.join("1.json"),
            format!(
                r#"{{"pid":{},"sessionId":"s1","status":"idle"}}"#,
                std::process::id()
            ),
        )
        .unwrap();

        let mut inner = Inner {
            sessions: Vec::new(),
            detached_count: 0,
            hooks: HashMap::new(),
            events: EventLog::at(events_path.clone()),
            transcripts: TranscriptReader::new(),
            registry_dir,
        };

        std::fs::write(
            &events_path,
            b"{\"e\":\"Notification\",\"t\":42.0,\"d\":{\"session_id\":\"s1\",\"message\":\"re-authenticated\",\"notification_type\":\"auth_success\"}}\n",
        )
        .unwrap();
        inner.pump_with(|_| Some("ttys999".into()));

        assert_eq!(inner.sessions.len(), 1);
        assert_eq!(inner.sessions[0].status, Status::Idle);
    }

    #[test]
    fn permission_prompt_and_elicitation_dialog_still_trigger_waiting_input() {
        // The two genuinely-blocking notification_type values must keep
        // working — this is the regression guard against over-broadening the
        // denylist above into something that swallows real asks too.
        for kind in ["permission_prompt", "elicitation_dialog"] {
            let dir = tempdir();
            let events_path = dir.path().join("events.jsonl");
            std::fs::write(&events_path, b"").unwrap();

            let registry_dir = dir.path().join("sessions");
            std::fs::create_dir_all(&registry_dir).unwrap();
            std::fs::write(
                registry_dir.join("1.json"),
                format!(
                    r#"{{"pid":{},"sessionId":"s1","status":"idle"}}"#,
                    std::process::id()
                ),
            )
            .unwrap();

            let mut inner = Inner {
                sessions: Vec::new(),
                detached_count: 0,
                hooks: HashMap::new(),
                events: EventLog::at(events_path.clone()),
                transcripts: TranscriptReader::new(),
                registry_dir,
            };

            std::fs::write(
                &events_path,
                format!(
                    "{{\"e\":\"Notification\",\"t\":42.0,\"d\":{{\"session_id\":\"s1\",\"message\":\"hi\",\"notification_type\":\"{kind}\"}}}}\n"
                )
                .into_bytes(),
            )
            .unwrap();
            inner.pump_with(|_| Some("ttys999".into()));

            assert_eq!(inner.sessions.len(), 1);
            assert_eq!(
                inner.sessions[0].status,
                Status::WaitingInput(Some("hi".into())),
                "notification_type {kind} should still trigger WaitingInput"
            );
        }
    }

    #[test]
    fn detached_sessions_are_excluded_and_counted_not_listed() {
        let dir = tempdir();
        let registry_dir = dir.path().join("sessions");
        std::fs::create_dir_all(&registry_dir).unwrap();
        std::fs::write(
            registry_dir.join("1.json"),
            format!(
                r#"{{"pid":{},"sessionId":"attached","status":"idle"}}"#,
                std::process::id()
            ),
        )
        .unwrap();
        // pid 1 (launchd) is always alive, so it survives registry::read_dir's
        // own liveness filter same as any real detached session would — the
        // point being tested is tty resolution, not liveness. kill(1, 0) as a
        // non-root user hits EPERM, which is_alive() correctly reads as
        // "exists" (confirmed: errno EPERM here, not ESRCH).
        std::fs::write(
            registry_dir.join("2.json"),
            r#"{"pid":1,"sessionId":"detached","status":"idle"}"#,
        )
        .unwrap();

        let events_path = dir.path().join("events.jsonl");
        std::fs::write(&events_path, b"").unwrap();

        let mut inner = Inner {
            sessions: Vec::new(),
            detached_count: 0,
            hooks: HashMap::new(),
            events: EventLog::at(events_path),
            transcripts: TranscriptReader::new(),
            registry_dir,
        };

        // Resolver keyed by pid, standing in for `ps -o tty=`: the "attached"
        // entry's pid resolves, the "detached" one's doesn't — same shape a
        // real registry produces.
        inner.refresh_with(|pid| {
            if pid == std::process::id() as i32 {
                Some("ttys999".into())
            } else {
                None
            }
        });

        assert_eq!(inner.sessions.len(), 1);
        assert_eq!(inner.sessions[0].session_id, "attached");
        assert_eq!(inner.detached_count, 1);
    }

    #[test]
    fn background_subagents_are_excluded_even_with_a_resolvable_tty() {
        let dir = tempdir();
        let registry_dir = dir.path().join("sessions");
        std::fs::create_dir_all(&registry_dir).unwrap();
        std::fs::write(
            registry_dir.join("1.json"),
            format!(
                r#"{{"pid":{},"sessionId":"interactive","status":"idle","kind":"interactive"}}"#,
                std::process::id()
            ),
        )
        .unwrap();
        // Shares the same resolvable tty as the interactive session above —
        // a subagent running inside that terminal, not a terminal of its own.
        std::fs::write(
            registry_dir.join("2.json"),
            r#"{"pid":1,"sessionId":"subagent","status":"busy","kind":"background"}"#,
        )
        .unwrap();

        let events_path = dir.path().join("events.jsonl");
        std::fs::write(&events_path, b"").unwrap();

        let mut inner = Inner {
            sessions: Vec::new(),
            detached_count: 0,
            hooks: HashMap::new(),
            events: EventLog::at(events_path),
            transcripts: TranscriptReader::new(),
            registry_dir,
        };

        inner.refresh_with(|_| Some("ttys999".into()));

        assert_eq!(inner.sessions.len(), 1);
        assert_eq!(inner.sessions[0].session_id, "interactive");
        assert_eq!(inner.detached_count, 0); // excluded outright, not miscounted as detached either
    }

    #[test]
    fn a_session_whose_registry_entry_is_removed_leaves_the_list() {
        let dir = tempdir();
        let registry_dir = dir.path().join("sessions");
        std::fs::create_dir_all(&registry_dir).unwrap();
        let entry_path = registry_dir.join("1.json");
        std::fs::write(
            &entry_path,
            format!(
                r#"{{"pid":{},"sessionId":"closing","status":"idle"}}"#,
                std::process::id()
            ),
        )
        .unwrap();

        let events_path = dir.path().join("events.jsonl");
        std::fs::write(&events_path, b"").unwrap();

        let mut inner = Inner {
            sessions: Vec::new(),
            detached_count: 0,
            hooks: HashMap::new(),
            events: EventLog::at(events_path),
            transcripts: TranscriptReader::new(),
            registry_dir,
        };

        inner.refresh_with(|_| Some("ttys999".into()));
        assert_eq!(inner.sessions.len(), 1);

        // Terminal closed: Claude Code removes its registry file (or the pid
        // dies). The row must disappear, not linger as a ghost.
        std::fs::remove_file(&entry_path).unwrap();
        inner.refresh_with(|_| Some("ttys999".into()));
        assert!(inner.sessions.is_empty());
        assert!(inner.hooks.is_empty());
    }

    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn tempdir() -> TempDir {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("devpit-store-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}
