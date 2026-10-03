//! `~/.devpit/events.jsonl` — hooks append one line each (`{"e":..,"t":..,"d":..}`),
//! written by `install-hooks.sh`. This tails it by byte offset rather than
//! re-reading the whole file, and survives truncation/rotation.
//!
//! This is used instead of a localhost HTTP listener. An
//! append-only log is simpler: no port, no token, no auth surface, and
//! a dead Saddle simply means the file grows — it can never stall a Claude
//! session waiting on a socket.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// Truncated at launch past this. ~22 KB accumulates in half an hour of heavy
/// use, so an app left running for weeks would otherwise grow without bound.
const ROTATE_AT_LAUNCH: u64 = 1 << 20; // 1 MB
const ROTATE_HARD: u64 = 8 << 20; // 8 MB, safety valve while running

/// One hook firing, as appended by the installed hook command.
#[derive(Debug, Clone, PartialEq)]
pub struct HookEvent {
    pub event: String, // Notification | Stop | UserPromptSubmit | ...
    pub time: f64,     // epoch seconds
    pub session_id: Option<String>,
    pub message: Option<String>, // Notification carries the prompt text
    /// Notification only. Observed values: a real blocking ask (e.g. a
    /// permission request) has some other type; `"idle_prompt"` is just
    /// Claude Code nudging "still there?" after the session already went
    /// idle post-`Stop` — not a real ask. `fleet::store` uses this to avoid
    /// painting an already-`Done` session as needing you (verified false
    /// positive, 2026-08-31).
    pub notification_type: Option<String>,
}

pub struct EventLog {
    path: PathBuf,
    offset: u64,
    partial: String,
    rotate_at_launch: u64,
    rotate_hard: u64,
}

/// `~/.devpit/events.jsonl` — also what `commands.rs` watches for the fast path.
pub(crate) fn events_path() -> Option<PathBuf> {
    super::home_dir().map(|home| home.join(".devpit/events.jsonl"))
}

impl EventLog {
    pub fn new() -> Self {
        Self::at(events_path().unwrap_or_default())
    }

    pub(crate) fn at(path: PathBuf) -> Self {
        let mut log = Self {
            path,
            offset: 0,
            partial: String::new(),
            rotate_at_launch: ROTATE_AT_LAUNCH,
            rotate_hard: ROTATE_HARD,
        };
        log.init();
        log
    }

    fn init(&mut self) {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if !self.path.exists() {
            let _ = fs::write(&self.path, b"");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600));
            }
        }

        let size = fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        // Launch is the safe moment to rotate: we deliberately skip everything
        // already in the file, so nothing is lost by emptying it.
        if size > self.rotate_at_launch {
            let _ = fs::write(&self.path, b"");
        }

        // Start at the end: events from before launch are history, not attention.
        self.offset = fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
    }

    /// Every event appended since the last call.
    pub fn drain(&mut self) -> Vec<HookEvent> {
        let Ok(meta) = fs::metadata(&self.path) else {
            return Vec::new();
        };
        let size = meta.len();

        // Truncated or rotated out from under us — start over.
        if size < self.offset {
            self.offset = 0;
            self.partial.clear();
        }
        if size <= self.offset {
            return Vec::new();
        }

        let Ok(mut file) = fs::File::open(&self.path) else {
            return Vec::new();
        };
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return Vec::new();
        }
        let mut buf = Vec::new();
        if file.read_to_end(&mut buf).is_err() || buf.is_empty() {
            return Vec::new();
        }
        self.offset = size;

        // Hard cap while running. Everything up to `size` has just been
        // consumed, so the only exposure is an append landing in the same instant.
        if size > self.rotate_hard {
            let _ = fs::write(&self.path, b"");
            self.offset = 0;
        }

        let text = format!("{}{}", self.partial, String::from_utf8_lossy(&buf));
        let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        self.partial = lines.pop().unwrap_or_default(); // trailing fragment; wait for the rest

        lines.iter().filter_map(|l| parse_event(l)).collect()
    }
}

fn parse_event(line: &str) -> Option<HookEvent> {
    if line.is_empty() {
        return None;
    }
    let obj: Value = serde_json::from_str(line).ok()?; // interleaved or half-written — skip
    let event = obj.get("e").and_then(Value::as_str)?.to_string();

    let payload = obj.get("d");
    let session_id = payload
        .and_then(|p| p.get("session_id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let message = payload
        .and_then(|p| p.get("message"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let notification_type = payload
        .and_then(|p| p.get("notification_type"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let time = obj.get("t").and_then(Value::as_f64).unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    });

    Some(HookEvent {
        event,
        time,
        session_id,
        message,
        notification_type,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn append(path: &std::path::Path, text: &str) {
        let mut f = fs::OpenOptions::new().append(true).open(path).unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn drain_skips_history_and_returns_only_new_events() {
        let dir = tempdir();
        let path = dir.path().join("events.jsonl");
        fs::write(&path, b"{\"e\":\"Stop\",\"t\":1.0,\"d\":{}}\n").unwrap(); // pre-launch history

        let mut log = EventLog::at(path.clone());
        assert!(log.drain().is_empty()); // history is skipped, not attention

        append(
            &path,
            "{\"e\":\"Notification\",\"t\":2.0,\"d\":{\"session_id\":\"s1\",\"message\":\"hi\"}}\n",
        );
        let events = log.drain();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "Notification");
        assert_eq!(events[0].session_id.as_deref(), Some("s1"));
        assert_eq!(events[0].message.as_deref(), Some("hi"));
    }

    #[test]
    fn drain_parses_notification_type() {
        let dir = tempdir();
        let path = dir.path().join("events.jsonl");
        fs::write(&path, b"").unwrap();
        let mut log = EventLog::at(path.clone());

        append(
            &path,
            "{\"e\":\"Notification\",\"t\":1.0,\"d\":{\"session_id\":\"s1\",\"message\":\"still there?\",\"notification_type\":\"idle_prompt\"}}\n",
        );
        let events = log.drain();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].notification_type.as_deref(), Some("idle_prompt"));
    }

    #[test]
    fn drain_holds_back_a_partial_trailing_line() {
        let dir = tempdir();
        let path = dir.path().join("events.jsonl");
        fs::write(&path, b"").unwrap();
        let mut log = EventLog::at(path.clone());

        append(&path, "{\"e\":\"Stop\",\"t\":1.0,\"d\":{}}\n{\"e\":\"Notif");
        let events = log.drain();
        assert_eq!(events.len(), 1); // the partial second line isn't emitted yet

        append(&path, "ication\",\"t\":2.0,\"d\":{}}\n");
        let events = log.drain();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "Notification");
    }

    #[test]
    fn drain_skips_lines_missing_the_required_e_field() {
        let dir = tempdir();
        let path = dir.path().join("events.jsonl");
        fs::write(&path, b"").unwrap();
        let mut log = EventLog::at(path.clone());

        append(
            &path,
            "{\"t\":1.0,\"d\":{}}\nnot json\n{\"e\":\"Stop\",\"t\":2.0,\"d\":{}}\n",
        );
        let events = log.drain();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "Stop");
    }

    #[test]
    fn drain_resets_offset_when_file_shrinks() {
        let dir = tempdir();
        let path = dir.path().join("events.jsonl");
        fs::write(&path, b"").unwrap();
        let mut log = EventLog {
            path: path.clone(),
            offset: 500, // pretend we'd already read past the truncated file's end
            partial: "stale".into(),
            rotate_at_launch: ROTATE_AT_LAUNCH,
            rotate_hard: ROTATE_HARD,
        };

        fs::write(&path, b"{\"e\":\"Stop\",\"t\":1.0,\"d\":{}}\n").unwrap();
        let events = log.drain();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "Stop");
    }

    #[test]
    fn init_rotates_an_oversized_file_at_launch() {
        let dir = tempdir();
        let path = dir.path().join("events.jsonl");
        fs::write(&path, "x".repeat(100)).unwrap();

        let mut log = EventLog {
            path: path.clone(),
            offset: 0,
            partial: String::new(),
            rotate_at_launch: 50, // small threshold so the test stays cheap
            rotate_hard: ROTATE_HARD,
        };
        log.init();

        assert_eq!(fs::metadata(&path).unwrap().len(), 0);
        assert_eq!(log.offset, 0);
    }

    #[test]
    fn drain_rotates_hard_cap_while_running() {
        let dir = tempdir();
        let path = dir.path().join("events.jsonl");
        fs::write(&path, b"").unwrap();
        let mut log = EventLog {
            path: path.clone(),
            offset: 0,
            partial: String::new(),
            rotate_at_launch: ROTATE_AT_LAUNCH,
            rotate_hard: 20, // small threshold so the test stays cheap
        };

        append(&path, "{\"e\":\"Stop\",\"t\":1.0,\"d\":{}}\n"); // > 20 bytes
        let events = log.drain();
        assert_eq!(events.len(), 1);
        assert_eq!(fs::metadata(&path).unwrap().len(), 0); // rotated away
        assert_eq!(log.offset, 0);
    }

    struct TempDir(PathBuf);
    impl TempDir {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn tempdir() -> TempDir {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("devpit-events-test-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}
