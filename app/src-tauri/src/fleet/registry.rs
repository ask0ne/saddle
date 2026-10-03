//! `~/.claude/sessions/<pid>.json` — Claude Code's live session registry.
//!
//! This is an INTERNAL format that will change under us. Only `pid` and
//! `session_id` are required; everything else is optional, and a decode
//! failure skips the file rather than taking the app down. Never
//! `deny_unknown_fields` — new fields must be ignored silently.

use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)] // entrypoint/version mirror the real JSON shape; unread until something needs them
pub struct RegistryEntry {
    pub pid: i32,
    pub session_id: String,

    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub status: Option<String>, // observed: "busy", "idle"
    #[serde(default)]
    pub kind: Option<String>, // observed: "interactive"
    #[serde(default)]
    pub entrypoint: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub started_at: Option<f64>, // epoch millis
    #[serde(default)]
    pub status_updated_at: Option<f64>, // epoch millis
}

impl RegistryEntry {
    /// Denylist, not allowlist — matches `fleet::store`'s Notification-type
    /// filtering. Only "busy"/"idle" were originally observed
    /// values; a live re-check (2026-08-31) caught a real, stable third
    /// value, `"shell"` (a session actively running a shell command), on
    /// Claude Code 2.1.251. An allowlist on `"busy"` alone would have shown
    /// that session as `Idle`/`Done` while it was genuinely active. Treat
    /// anything that isn't literally `"idle"` (or missing) as busy — the
    /// internal format drifts (registry.rs's own module doc), so new status
    /// strings should read as "some kind of active", not silently as idle.
    pub fn is_busy(&self) -> bool {
        matches!(self.status.as_deref(), Some(s) if s != "idle")
    }

    /// `claude agents --json` returns "interactive + background" —
    /// subagents spawned by the Task tool register here too, with
    /// `kind:"background"`, and inherit their parent's controlling tty. That
    /// second part means the no-tty `detached_count` filter in `fleet::store`
    /// doesn't catch them: they resolve a real tty, just one already claimed
    /// by another session, so they were slipping through and inflating the
    /// terminal count (a subagent counted as its own terminal).
    pub fn is_interactive(&self) -> bool {
        self.kind.as_deref() != Some("background")
    }

    /// Basename of cwd, used as the display name for a session.
    pub fn repo_name(&self) -> &str {
        match self.cwd.as_deref().filter(|c| !c.is_empty()) {
            Some(cwd) => cwd.rsplit('/').next().unwrap_or(cwd),
            None => self.name.as_deref().unwrap_or("session"),
        }
    }
}

pub(crate) fn registry_dir() -> Option<PathBuf> {
    super::home_dir().map(|home| home.join(".claude/sessions"))
}

/// Read every live session. Dead pids and unparsable files are dropped silently.
#[allow(dead_code)] // the module's natural public entry point; fleet::store uses read_dir directly for testability
pub fn read() -> Vec<RegistryEntry> {
    match registry_dir() {
        Some(dir) => read_dir(&dir),
        None => Vec::new(),
    }
}

/// Same as `read()`, against an explicit directory — the seam `fleet::store`'s
/// tests use to avoid touching the real `~/.claude/sessions`.
pub(crate) fn read_dir(dir: &std::path::Path) -> Vec<RegistryEntry> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };

    entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|e| fs::read(e.path()).ok())
        .filter_map(|bytes| serde_json::from_slice::<RegistryEntry>(&bytes).ok())
        .filter(|entry| is_alive(entry.pid) && !pid_was_reused(entry))
        .collect()
}

/// How far after a registry entry's `startedAt` its pid's process may have
/// started before the pid is considered reused. A real session's process
/// starts a few seconds *before* it writes `startedAt`; a pid recycled after a
/// crash or reboot belongs to a process that started *after* the stale file.
const REUSE_SLACK_SECS: f64 = 30.0;

/// A registry file left behind by a crash or reboot can name a pid that now
/// belongs to an unrelated process, which `kill(pid, 0)` happily reports as
/// alive. Detect that by comparing the process's real start time (from
/// `ps -o etime=`) with the entry's `startedAt`. Anything uncertain — no
/// `startedAt`, `ps` failing, an unparsable value — counts as not reused, so
/// a real session is never hidden by this check.
fn pid_was_reused(entry: &RegistryEntry) -> bool {
    let Some(started_at_ms) = entry.started_at else {
        return false;
    };
    let Ok(output) = std::process::Command::new("/bin/ps")
        .args(["-o", "etime=", "-p", &entry.pid.to_string()])
        .output()
    else {
        return false;
    };
    let Some(elapsed) = parse_etime(&String::from_utf8_lossy(&output.stdout)) else {
        return false;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    process_started_after(now - elapsed, started_at_ms / 1000.0)
}

fn process_started_after(process_start_secs: f64, started_at_secs: f64) -> bool {
    process_start_secs > started_at_secs + REUSE_SLACK_SECS
}

/// `ps -o etime=` -> seconds. Formats: `MM:SS`, `HH:MM:SS`,
/// `D-HH:MM:SS`.
fn parse_etime(raw: &str) -> Option<f64> {
    let raw = raw.trim();
    let (days, rest) = match raw.split_once('-') {
        Some((d, rest)) => (d.parse::<f64>().ok()?, rest),
        None => (0.0, raw),
    };
    let parts: Vec<f64> = rest
        .split(':')
        .map(|p| p.parse::<f64>().ok())
        .collect::<Option<_>>()?;
    let secs = match parts.as_slice() {
        [m, s] => m * 60.0 + s,
        [h, m, s] => h * 3600.0 + m * 60.0 + s,
        _ => return None,
    };
    Some(days * 86400.0 + secs)
}

/// `kill(pid, 0)` probes for existence without signalling.
#[allow(unsafe_code)]
pub fn is_alive(pid: i32) -> bool {
    // SAFETY: signal 0 performs only the existence/permission check; no signal
    // is delivered, and `pid` is a plain integer — no memory is read or written.
    let ret = unsafe { libc::kill(pid, 0) };
    ret == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_etime_format() {
        assert_eq!(parse_etime("05:28"), Some(328.0));
        assert_eq!(parse_etime("  05:28:45\n"), Some(19725.0));
        assert_eq!(parse_etime("2-01:02:03"), Some(2.0 * 86400.0 + 3723.0));
        assert_eq!(parse_etime(""), None);
        assert_eq!(parse_etime("garbage"), None);
    }

    #[test]
    fn process_that_started_after_the_entry_is_a_reused_pid() {
        // started 3s before the entry was written: a real session
        assert!(!process_started_after(997.0, 1000.0));
        // started after the entry was written: a recycled pid
        assert!(process_started_after(5000.0, 1000.0));
        // within the slack: not flagged
        assert!(!process_started_after(1020.0, 1000.0));
    }

    fn entry_json(pid: u32, started_at_ms: Option<f64>) -> String {
        let started = started_at_ms
            .map(|ms| format!(r#","startedAt":{ms}"#))
            .unwrap_or_default();
        format!(r#"{{"pid":{pid},"sessionId":"s"{started}}}"#)
    }

    #[test]
    fn a_live_process_matching_its_started_at_is_kept() {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as f64;
        let entry: RegistryEntry =
            serde_json::from_str(&entry_json(std::process::id(), Some(now_ms))).unwrap();
        assert!(!pid_was_reused(&entry));
    }

    #[test]
    fn a_live_pid_whose_entry_predates_the_process_is_dropped() {
        // this test process started seconds ago; an entry claiming to have
        // started an hour before that is a stale file with a recycled pid
        let hour_ago_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as f64
            - 3_600_000.0;
        let entry: RegistryEntry =
            serde_json::from_str(&entry_json(std::process::id(), Some(hour_ago_ms))).unwrap();
        assert!(pid_was_reused(&entry));
    }

    #[test]
    fn an_entry_without_started_at_is_never_dropped() {
        let entry: RegistryEntry =
            serde_json::from_str(&entry_json(std::process::id(), None)).unwrap();
        assert!(!pid_was_reused(&entry));
    }

    #[test]
    fn decodes_only_required_fields() {
        let json = r#"{"pid":7282,"sessionId":"a9c7"}"#;
        let entry: RegistryEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.pid, 7282);
        assert_eq!(entry.session_id, "a9c7");
        assert_eq!(entry.cwd, None);
        assert_eq!(entry.status, None);
        assert!(!entry.is_busy());
    }

    #[test]
    fn decodes_full_entry_and_ignores_unknown_fields() {
        // real shape observed on disk, incl. fields not modeled here
        // (procStart, peerProtocol, messagingSocketPath, peerFeatures, ...)
        let json = r#"{"pid":27341,"sessionId":"601fcb5a","cwd":"/Users/x/Workspace/qna",
            "startedAt":1787808839060.0,"procStart":"Thu Aug 27 05:33:57 2026",
            "version":"2.1.247","peerProtocol":1,"peerFeatures":["notify_idle"],
            "kind":"interactive","entrypoint":"cli","pidDomain":"darwin",
            "messagingSocketPath":"/tmp/cc-socks/27341.sock","name":"qna-ec",
            "nameSource":"derived","nameSince":1787808839060.0,"status":"busy",
            "updatedAt":1787812427581.0,"statusUpdatedAt":1787812427581.0}"#;
        let entry: RegistryEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.pid, 27341);
        assert_eq!(entry.cwd.as_deref(), Some("/Users/x/Workspace/qna"));
        assert_eq!(entry.repo_name(), "qna");
        assert!(entry.is_busy());
    }

    #[test]
    fn is_busy_true_for_a_status_value_not_yet_seen_when_this_was_written() {
        // Verified live, 2026-08-31: Claude Code 2.1.251 emits "shell" for a
        // session actively running a shell command — a real third status
        // value beyond the original busy/idle. The old
        // allowlist-on-"busy" check would have called this session idle
        // while it was genuinely active.
        let e = entry_with_status(Some("shell"));
        assert!(e.is_busy());
    }

    #[test]
    fn is_busy_false_only_for_idle() {
        assert!(!entry_with_status(Some("idle")).is_busy());
        assert!(!entry_with_status(None).is_busy());
    }

    fn entry_with_status(status: Option<&str>) -> RegistryEntry {
        let json = format!(
            r#"{{"pid":1,"sessionId":"s","status":{}}}"#,
            status.map(|s| format!("\"{s}\"")).unwrap_or("null".into())
        );
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn missing_required_field_fails_to_decode() {
        let json = r#"{"pid":1}"#; // no sessionId
        assert!(serde_json::from_str::<RegistryEntry>(json).is_err());
    }

    #[test]
    fn repo_name_falls_back_to_name_then_session() {
        let with_name = RegistryEntry {
            pid: 1,
            session_id: "s".into(),
            cwd: None,
            name: Some("ask0ne-f4".into()),
            status: None,
            kind: None,
            entrypoint: None,
            version: None,
            started_at: None,
            status_updated_at: None,
        };
        assert_eq!(with_name.repo_name(), "ask0ne-f4");

        let bare = RegistryEntry {
            name: None,
            ..with_name
        };
        assert_eq!(bare.repo_name(), "session");
    }

    #[test]
    fn is_alive_true_for_current_process() {
        assert!(is_alive(std::process::id() as i32));
    }

    #[test]
    fn is_alive_false_for_dead_pid() {
        // PIDs are 32-bit signed on Darwin; this one is out of range for any
        // real process and kill(2) returns ESRCH.
        assert!(!is_alive(i32::MAX));
    }
}
