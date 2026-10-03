//! `~/.claude/projects/<mangled-cwd>/<sessionId>.jsonl` — session transcripts.
//!
//! Transcripts are named by session id, so we glob for the id rather than
//! reconstructing Claude Code's cwd-to-slug mangling. Only the tail is read,
//! and only when the file has actually changed: these files reach hundreds of
//! KB and are appended to constantly, so re-parsing unconditionally on every
//! refresh burns real CPU for nothing.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

const TAIL_BYTES: u64 = 96 * 1024;

/// The handful of things worth lifting out of a session's transcript.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TranscriptInfo {
    pub ai_title: Option<String>,
    pub git_branch: Option<String>,
    pub current_tool: Option<String>,
}

struct Cached {
    size: u64,
    modified: SystemTime,
    info: TranscriptInfo,
}

pub struct TranscriptReader {
    projects_dir: PathBuf,
    paths: HashMap<String, PathBuf>,
    cache: HashMap<String, Cached>,
}

impl TranscriptReader {
    pub fn new() -> Self {
        let projects_dir = super::home_dir()
            .map(|home| home.join(".claude/projects"))
            .unwrap_or_default();
        Self::at(projects_dir)
    }

    fn at(projects_dir: PathBuf) -> Self {
        Self {
            projects_dir,
            paths: HashMap::new(),
            cache: HashMap::new(),
        }
    }

    pub fn info(&mut self, session_id: &str) -> TranscriptInfo {
        let Some(path) = self.path_for(session_id) else {
            return TranscriptInfo::default();
        };

        let Ok(metadata) = fs::metadata(&path) else {
            return TranscriptInfo::default();
        };
        let size = metadata.len();
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);

        if let Some(hit) = self.cache.get(session_id)
            && hit.size == size
            && hit.modified == modified
        {
            return hit.info.clone();
        }

        let info = parse(&path).unwrap_or_default();
        self.cache.insert(
            session_id.to_string(),
            Cached {
                size,
                modified,
                info: info.clone(),
            },
        );
        info
    }

    /// Drop entries for sessions that have gone away.
    pub fn prune(&mut self, live: &std::collections::HashSet<String>) {
        self.paths.retain(|id, _| live.contains(id));
        self.cache.retain(|id, _| live.contains(id));
    }

    fn path_for(&mut self, session_id: &str) -> Option<PathBuf> {
        if let Some(cached) = self.paths.get(session_id)
            && cached.is_file()
        {
            return Some(cached.clone());
        }

        let found = find_transcript(&self.projects_dir, session_id)?;
        self.paths.insert(session_id.to_string(), found.clone());
        Some(found)
    }
}

/// Glob `<projects_dir>/*/​<session_id>.jsonl`.
fn find_transcript(projects_dir: &Path, session_id: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(projects_dir).ok()?;
    let file_name = format!("{session_id}.jsonl");

    entries
        .filter_map(|e| e.ok())
        .map(|e| e.path().join(&file_name))
        .find(|candidate| candidate.is_file())
}

/// Last `TAIL_BYTES` of the file, split into whole lines. A partial first
/// line from seeking into the middle of the file is dropped.
fn tail_lines(path: &Path, tail_bytes: u64) -> Option<Vec<String>> {
    let bytes = fs::read(path).ok()?;
    let start = bytes.len().saturating_sub(tail_bytes as usize);
    let text = String::from_utf8_lossy(&bytes[start..]);

    let mut lines: Vec<&str> = text.split('\n').collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0); // partial first line
    }
    Some(
        lines
            .into_iter()
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// Walk backwards: newest mention of each field wins, stop once full.
fn parse(path: &Path) -> Option<TranscriptInfo> {
    let lines = tail_lines(path, TAIL_BYTES)?;
    let mut info = TranscriptInfo::default();

    for line in lines.iter().rev() {
        let Ok(obj) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let line_type = obj.get("type").and_then(Value::as_str);

        if info.ai_title.is_none()
            && line_type == Some("ai-title")
            && let Some(title) = obj.get("aiTitle").and_then(Value::as_str)
            && !title.is_empty()
        {
            info.ai_title = Some(title.to_string());
        }

        if info.git_branch.is_none()
            && let Some(branch) = obj.get("gitBranch").and_then(Value::as_str)
            && !branch.is_empty()
            && branch != "HEAD"
        {
            info.git_branch = Some(branch.to_string());
        }

        if info.current_tool.is_none()
            && line_type == Some("assistant")
            && let Some(content) = obj
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(Value::as_array)
        {
            for block in content {
                if block.get("type").and_then(Value::as_str) == Some("tool_use")
                    && let Some(name) = block.get("name").and_then(Value::as_str)
                {
                    info.current_tool = Some(name.to_string());
                }
            }
        }

        if info.ai_title.is_some() && info.git_branch.is_some() && info.current_tool.is_some() {
            break;
        }
    }

    Some(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_lines(dir: &Path, name: &str, lines: &[&str]) -> PathBuf {
        let path = dir.join(name);
        let mut f = fs::File::create(&path).unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        path
    }

    #[test]
    fn parse_extracts_newest_of_each_field() {
        let dir = tempdir();
        let path = write_lines(
            dir.path(),
            "s.jsonl",
            &[
                r#"{"type":"ai-title","aiTitle":"old title"}"#,
                r#"{"type":"user","gitBranch":"main"}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#,
                r#"{"type":"ai-title","aiTitle":"new title"}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read"}]}}"#,
            ],
        );
        let info = parse(&path).unwrap();
        assert_eq!(info.ai_title.as_deref(), Some("new title"));
        assert_eq!(info.git_branch.as_deref(), Some("main"));
        assert_eq!(info.current_tool.as_deref(), Some("Read"));
    }

    #[test]
    fn parse_treats_head_branch_as_unknown() {
        let dir = tempdir();
        let path = write_lines(
            dir.path(),
            "s.jsonl",
            &[r#"{"type":"user","gitBranch":"HEAD"}"#],
        );
        let info = parse(&path).unwrap();
        assert_eq!(info.git_branch, None);
    }

    #[test]
    fn parse_skips_unparsable_lines_without_crashing() {
        let dir = tempdir();
        let path = write_lines(
            dir.path(),
            "s.jsonl",
            &["not json at all", r#"{"type":"ai-title","aiTitle":"ok"}"#],
        );
        let info = parse(&path).unwrap();
        assert_eq!(info.ai_title.as_deref(), Some("ok"));
    }

    #[test]
    fn tail_lines_drops_partial_first_line_when_seeking_past_start() {
        let dir = tempdir();
        let path = write_lines(dir.path(), "s.jsonl", &["aaaaaaaaaa", "bb", "ccc"]);
        // "aaaaaaaaaa\n" is 11 bytes; ask for the last 6 bytes, landing mid "bb\n".
        let lines = tail_lines(&path, 6).unwrap();
        assert_eq!(lines, vec!["ccc"]);
    }

    #[test]
    fn find_transcript_globs_by_session_id_across_project_dirs() {
        let dir = tempdir();
        let project = dir.path().join("-Users-x-Workspace-devpit");
        fs::create_dir_all(&project).unwrap();
        let sid = "601fcb5a-0b5e-47e9-a239-c9f53a285e3f";
        write_lines(&project, &format!("{sid}.jsonl"), &[r#"{"type":"system"}"#]);

        let found = find_transcript(dir.path(), sid);
        assert_eq!(found, Some(project.join(format!("{sid}.jsonl"))));
        assert_eq!(find_transcript(dir.path(), "no-such-session"), None);
    }

    #[test]
    fn info_reparses_after_the_file_changes() {
        let dir = tempdir();
        let project = dir.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        let sid = "sess-1";
        write_lines(
            &project,
            &format!("{sid}.jsonl"),
            &[r#"{"type":"ai-title","aiTitle":"first"}"#],
        );

        let mut reader = TranscriptReader::at(dir.path().to_path_buf());
        assert_eq!(reader.info(sid).ai_title.as_deref(), Some("first"));

        // mtime resolution can be coarse; force it forward so the cache sees a change.
        std::thread::sleep(std::time::Duration::from_millis(10));
        write_lines(
            &project,
            &format!("{sid}.jsonl"),
            &[r#"{"type":"ai-title","aiTitle":"second"}"#],
        );
        assert_eq!(reader.info(sid).ai_title.as_deref(), Some("second"));
    }

    #[test]
    fn prune_drops_dead_sessions() {
        let dir = tempdir();
        let project = dir.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        write_lines(
            &project,
            "sess-1.jsonl",
            &[r#"{"type":"ai-title","aiTitle":"x"}"#],
        );

        let mut reader = TranscriptReader::at(dir.path().to_path_buf());
        reader.info("sess-1");
        assert!(reader.cache.contains_key("sess-1"));

        reader.prune(&std::collections::HashSet::new());
        assert!(reader.cache.is_empty());
        assert!(reader.paths.is_empty());
    }

    // minimal temp-dir helper — avoids pulling in a `tempfile` dependency for
    // a handful of throwaway directories cleaned up on drop.
    struct TempDir(PathBuf);
    impl TempDir {
        fn path(&self) -> &Path {
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
            std::env::temp_dir().join(format!("devpit-transcript-test-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}
