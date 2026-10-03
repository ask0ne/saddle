//! Raises the exact Terminal.app tab a Claude session is running in.
//!
//! The chain, verified by hand:
//!   pid 7282 -> `ps -o tty= -p 7282` -> "ttys000"
//!   -> AppleScript tab whose `tty` is "/dev/ttys000" -> window 266
//!
//! Both legs run as `std::process::Command` (`ps`, then `osascript -e`) —
//! the sync-subprocess style (see ARCHITECTURE.md, Concurrency), same as `ps`
//! elsewhere in `fleet`. No `objc2`/`unsafe` needed for this; that's reserved
//! for `platform::panel`, which needs `NSWindow` properties Tauri doesn't
//! expose. AppleScript over a shelled `osascript` is plenty for a `tell
//! application` script — `osascript`'s stderr text (`-1743`/`-1744`) gives
//! the error codes needed.

use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", content = "detail")]
pub enum FocusResult {
    Focused,
    /// Terminal was raised but the tab couldn't be singled out.
    NotFound,
    /// macOS Automation permission denied (AppleScript error -1743/-1744).
    NotPermitted,
    Failed(String),
}

/// `ps -o tty= -p <pid>` -> "ttys000" (with padding), or `None` when the
/// process has no controlling terminal (`ps` prints `??`).
pub fn tty_of(pid: i32) -> Option<String> {
    let raw = run("/bin/ps", &["-o", "tty=", "-p", &pid.to_string()]);
    parse_tty(&raw)
}

fn parse_tty(raw: &str) -> Option<String> {
    let name = raw.trim();
    if name.is_empty() || name == "??" || name == "?" {
        return None;
    }
    Some(if name.starts_with("/dev/") {
        name.to_string()
    } else {
        format!("/dev/{name}")
    })
}

/// Terminal was raised but we could not single out the tab, if `tty_of`
/// can't resolve one (`fallback_title` covers tmux/ssh/re-exec — Claude Code
/// writes a status glyph + AI title into the tab's custom title).
pub fn focus(pid: i32, fallback_title: Option<&str>) -> FocusResult {
    match tty_of(pid) {
        Some(tty) => run_script(&focus_script(&tty, fallback_title)),
        None => {
            if raise_terminal_only() {
                FocusResult::NotFound
            } else {
                FocusResult::Failed("could not resolve tty".into())
            }
        }
    }
}

fn focus_script(tty: &str, title: Option<&str>) -> String {
    let title_clause = match title.filter(|t| !t.is_empty()) {
        Some(title) => {
            let escaped = title.replace('"', "\\\"");
            format!(
                r#"
                repeat with w in windows
                    repeat with t in tabs of w
                        if (custom title of t as string) contains "{escaped}" then
                            set selected tab of w to t
                            set frontmost of w to true
                            activate
                            return "ok"
                        end if
                    end repeat
                end repeat
            "#
            )
        }
        None => String::new(),
    };

    format!(
        r#"tell application "Terminal"
    repeat with w in windows
        repeat with t in tabs of w
            if (tty of t as string) is "{tty}" then
                set selected tab of w to t
                set frontmost of w to true
                activate
                return "ok"
            end if
        end repeat
    end repeat
{title_clause}
    activate
    return "notfound"
end tell"#
    )
}

fn run_script(source: &str) -> FocusResult {
    let output = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(source)
        .output();
    match output {
        Ok(out) => classify(
            out.status.success(),
            &String::from_utf8_lossy(&out.stdout),
            &String::from_utf8_lossy(&out.stderr),
        ),
        Err(e) => FocusResult::Failed(e.to_string()),
    }
}

/// The osascript exit/stdout/stderr -> `FocusResult` mapping, split out so
/// the interesting logic (permission-error detection) is testable without
/// actually running AppleScript or touching Terminal.app.
fn classify(success: bool, stdout: &str, stderr: &str) -> FocusResult {
    if success {
        return match stdout.trim() {
            "ok" => FocusResult::Focused,
            _ => FocusResult::NotFound,
        };
    }
    // errAEEventNotPermitted / needs user consent.
    if stderr.contains("-1743") || stderr.contains("-1744") {
        FocusResult::NotPermitted
    } else {
        FocusResult::Failed(stderr.trim().to_string())
    }
}

/// Last resort — what the old shell hooks did: bring Terminal forward wholesale.
fn raise_terminal_only() -> bool {
    Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(r#"tell application "Terminal" to activate"#)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn run(path: &str, args: &[&str]) -> String {
    Command::new(path)
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tty_handles_no_controlling_terminal() {
        assert_eq!(parse_tty("??\n"), None);
        assert_eq!(parse_tty("?"), None);
        assert_eq!(parse_tty(""), None);
        assert_eq!(parse_tty("   \n"), None);
    }

    #[test]
    fn parse_tty_prefixes_dev_when_missing() {
        assert_eq!(parse_tty("ttys000\n"), Some("/dev/ttys000".to_string()));
    }

    #[test]
    fn parse_tty_leaves_an_already_prefixed_path_alone() {
        assert_eq!(
            parse_tty("/dev/ttys003\n"),
            Some("/dev/ttys003".to_string())
        );
    }

    #[test]
    fn tty_of_dead_pid_is_none() {
        assert_eq!(tty_of(i32::MAX), None);
    }

    #[test]
    fn tty_of_launchd_is_none() {
        assert_eq!(tty_of(1), None); // launchd has no controlling terminal
    }

    #[test]
    fn tty_of_current_process_is_none_or_a_dev_path() {
        // Shape check, not a specific value — this sandbox's own tty (if any) varies.
        match tty_of(std::process::id() as i32) {
            None => {}
            Some(tty) => assert!(tty.starts_with("/dev/")),
        }
    }

    #[test]
    fn focus_script_without_title_has_no_title_clause() {
        let script = focus_script("/dev/ttys000", None);
        assert!(script.contains(r#"is "/dev/ttys000""#));
        assert!(!script.contains("custom title"));
    }

    #[test]
    fn focus_script_with_title_escapes_quotes() {
        let script = focus_script("/dev/ttys000", Some(r#"fix "the" bug"#));
        assert!(script.contains("custom title"));
        assert!(script.contains(r#"fix \"the\" bug"#));
    }

    #[test]
    fn classify_ok_stdout_is_focused() {
        assert_eq!(classify(true, "ok\n", ""), FocusResult::Focused);
    }

    #[test]
    fn classify_notfound_stdout_is_not_found() {
        assert_eq!(classify(true, "notfound\n", ""), FocusResult::NotFound);
    }

    #[test]
    fn classify_permission_error_codes_are_not_permitted() {
        assert_eq!(
            classify(false, "", "execution error: Not authorized (-1743)"),
            FocusResult::NotPermitted
        );
        assert_eq!(
            classify(false, "", "... -1744 ..."),
            FocusResult::NotPermitted
        );
    }

    #[test]
    fn classify_other_failure_is_failed_with_stderr() {
        assert_eq!(
            classify(false, "", "  syntax error  \n"),
            FocusResult::Failed("syntax error".to_string())
        );
    }
}
