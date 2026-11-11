//! Claude Code's session files, `~/.claude/sessions/<pid>.json`: one for each
//! of its processes, written by Claude Code and nothing else, so a process
//! with one is Claude (tmux/claude-pane.sh's `claude_pid`). They also say
//! which sessions are headless (`entrypoint` `sdk-cli`, a `claude -p` that a
//! loop's script starts, whose turns aren't the tab's) and which have been
//! moved into the background (`jobId`), and where a background job came from
//! (the tab's parked Claude names it as `parkedJobId`).
//!
//! They are a handful of small files, read whole each time, as
//! agent-state.sh's `jq` over them was; a file that isn't JSON is passed over.
//!
//! Whether a pane runs Claude is decided here for the whole workspace
//! (`pane_runs_claude`, and `named_like_claude` for a caller that has
//! already looked for the session file): a process in front with a session
//! file, or a program in front named as Claude Code's executable is. The
//! name counts on its own, as lib/slack-relay/tabs.js counted it, because
//! Claude Code asks whether to trust a folder before it writes its session
//! file, and a Claude stopped at that question still has to be alerted on
//! and answered. What needs the session itself (its transcript, its state)
//! still waits for the file.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;

/// `~/.claude/sessions`.
pub fn sessions_dir() -> PathBuf {
    crate::paths::claude_dir().join("sessions")
}

/// What one session file says, as far as the workspace needs it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionFile {
    pub pid: Option<u32>,
    pub session_id: String,
    pub entrypoint: String,
    pub job_id: String,
    pub parked_job_id: String,
}

impl SessionFile {
    fn from_json(v: &Value) -> SessionFile {
        let text = |key: &str| {
            v.get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        // jq compared the pid as a string, so one written either way is read.
        let pid = match v.get("pid") {
            Some(Value::Number(n)) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
            Some(Value::String(s)) => s.trim().parse().ok(),
            _ => None,
        };
        SessionFile {
            pid,
            session_id: text("sessionId"),
            entrypoint: text("entrypoint"),
            job_id: text("jobId"),
            parked_job_id: text("parkedJobId"),
        }
    }
}

/// Every session file in `dir`.
pub fn read_all(dir: &Path) -> Vec<SessionFile> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| crate::util::read_json(e.path()))
        .filter(Value::is_object)
        .map(|v| SessionFile::from_json(&v))
        .collect()
}

/// Whether the session is a headless Claude's.
pub fn is_headless(sessions: &[SessionFile], session_id: &str) -> bool {
    !session_id.is_empty()
        && sessions
            .iter()
            .any(|s| s.session_id == session_id && s.entrypoint.starts_with("sdk"))
}

/// The Claude processes that may hold a backgrounded conversation's tab, in
/// the order to try them: the session's job is named by a parked Claude
/// (`parkedJobId`), which is in a tab, or is itself a job parked in turn into
/// another, whose parked Claude comes next. Empty for a session that isn't a
/// background job. Followed at most five links, as agent-state.sh did, so
/// files that name each other in a ring end.
pub fn parked_chain(sessions: &[SessionFile], session_id: &str) -> Vec<u32> {
    let mut pids = Vec::new();
    if session_id.is_empty() {
        return pids;
    }
    let mut job = sessions
        .iter()
        .find(|s| s.session_id == session_id)
        .map(|s| s.job_id.clone())
        .unwrap_or_default();
    while !job.is_empty() && pids.len() < 5 {
        let Some(pid) = sessions
            .iter()
            .find(|s| s.parked_job_id == job)
            .and_then(|s| s.pid)
        else {
            break;
        };
        pids.push(pid);
        job = sessions
            .iter()
            .find(|s| s.pid == Some(pid))
            .map(|s| s.job_id.clone())
            .unwrap_or_default();
    }
    pids
}

/// Whether any of these processes (a pane's foreground) has a session file
/// of its own in `dir`: certainly Claude, since only Claude Code writes
/// there, and the one way to tell Claude started from a Node script, which
/// runs as its child and reads as `node`.
///
/// Nothing removes a session file when its Claude ends without saying so,
/// and the kernel hands its pid out again (thousands of processes start an
/// hour under autopilot), so a shell that gets a dead Claude's pid back
/// would pass as Claude and keep the state it left, Working included. A
/// file counts only when it was written after its process started.
pub fn runs_claude(dir: &Path, pids: &[u32]) -> bool {
    runs_claude_by(dir, pids, crate::mux::procinfo::started_at)
}

fn runs_claude_by(
    dir: &Path,
    pids: &[u32],
    started_at: impl Fn(u32) -> Option<SystemTime>,
) -> bool {
    pids.iter().any(|&pid| {
        let Ok(modified) = fs::metadata(dir.join(format!("{pid}.json"))).and_then(|m| m.modified())
        else {
            return false;
        };
        written_since(modified, started_at(pid))
    })
}

/// Whether a session file modified at `modified` was written by the
/// process that started at `started`, or by one before it that had its
/// pid. A second's grace covers clocks kept to the second; a start that
/// can't be read leaves the file counted, as before.
fn written_since(modified: SystemTime, started: Option<SystemTime>) -> bool {
    started.is_none_or(|started| modified + Duration::from_secs(1) >= started)
}

/// Whether a pane runs Claude: its program in front is named as Claude
/// Code's executable is, or a process in front has a session file. The name
/// is looked at first, since it is one small read where the session file
/// needs every process in the group found.
pub fn pane_runs_claude(dir: &Path, pane: &crate::mux::pane::Pane) -> bool {
    named_like_claude(pane) || runs_claude(dir, &pane.foreground_pids())
}

/// Whether the program in front on a pane is named as Claude Code's
/// executable is, before (or without) its session file.
pub fn named_like_claude(pane: &crate::mux::pane::Pane) -> bool {
    let Some(front) = pane.foreground_pgid() else {
        return false;
    };
    crate::mux::procinfo::command_of(front)
        .is_some_and(|command| is_claude_name(&command, || crate::mux::procinfo::args_of(front)))
}

/// Whether a program is Claude Code by its name: the executable is named
/// after its version (`2.1.285`) where the kernel names a process after the
/// file it ran, `claude` where it names it after the link that was run
/// (Linux) or Claude Code renamed itself, and `node` when an npm install's
/// script runs it, its script then being Claude Code's (`claude`, or a file
/// in `@anthropic-ai/claude-code`), or a shell running a script named
/// `claude` (a wrapper of your own, which a Mac names after its
/// interpreter). Only the script is looked at, never the arguments after
/// it, so `node app.js claude` is no Claude.
pub fn is_claude_name(command: &str, args: impl FnOnce() -> Option<Vec<String>>) -> bool {
    let parts: Vec<&str> = command.split('.').collect();
    let version = parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    if version || command == "claude" {
        return true;
    }
    let node = command == "node";
    if !node && !matches!(command, "sh" | "bash" | "zsh" | "dash") {
        return false;
    }
    let Some(args) = args() else {
        return false;
    };
    // A shell given -c runs a command line, not a script: one that ends by
    // running claude is named claude by then.
    let flags = args.iter().skip(1).take_while(|a| a.starts_with('-'));
    if !node
        && flags
            .into_iter()
            .any(|f| !f.starts_with("--") && f.contains('c'))
    {
        return false;
    }
    let Some(script) = args.iter().skip(1).find(|a| !a.starts_with('-')) else {
        return false;
    };
    let script = Path::new(script);
    script.file_name().is_some_and(|name| name == "claude")
        || (node && script.components().any(|c| c.as_os_str() == "claude-code"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(pid: u32, id: &str, entrypoint: &str, job: &str, parked: &str) -> SessionFile {
        SessionFile {
            pid: Some(pid),
            session_id: id.into(),
            entrypoint: entrypoint.into(),
            job_id: job.into(),
            parked_job_id: parked.into(),
        }
    }

    #[test]
    fn a_session_file_reads_its_pid_either_way() {
        let a = SessionFile::from_json(&serde_json::json!({"pid": 12, "sessionId": "s"}));
        assert_eq!((a.pid, a.session_id.as_str()), (Some(12), "s"));
        let b = SessionFile::from_json(&serde_json::json!({"pid": "13"}));
        assert_eq!(b.pid, Some(13));
        assert_eq!(
            SessionFile::from_json(&serde_json::json!({"pid": -1})).pid,
            None
        );
    }

    #[test]
    fn a_headless_session_is_told_by_its_entrypoint() {
        let sessions = vec![
            file(1, "tab", "cli", "", ""),
            file(2, "loop", "sdk-cli", "", ""),
        ];
        assert!(is_headless(&sessions, "loop"));
        assert!(!is_headless(&sessions, "tab"));
        assert!(!is_headless(&sessions, "unknown"));
        assert!(!is_headless(&sessions, ""));
    }

    #[test]
    fn a_background_job_leads_to_its_parked_claude() {
        let sessions = vec![
            // The tab's Claude parked job a; job a's own Claude parked job b.
            file(10, "tab", "cli", "", "a"),
            file(20, "job-a", "cli", "a", "b"),
            file(30, "job-b", "cli", "b", ""),
        ];
        assert_eq!(parked_chain(&sessions, "job-b"), vec![20, 10]);
        assert_eq!(parked_chain(&sessions, "job-a"), vec![10]);
        assert!(parked_chain(&sessions, "tab").is_empty());
        assert!(parked_chain(&sessions, "").is_empty());
        // Files naming each other in a ring end after five links.
        let ring = vec![file(1, "x", "cli", "a", "b"), file(2, "y", "cli", "b", "a")];
        assert_eq!(parked_chain(&ring, "x").len(), 5);
    }

    #[test]
    fn claude_code_is_known_by_its_name_before_its_session_file() {
        let none = || None;
        assert!(is_claude_name("2.1.285", none));
        assert!(is_claude_name("claude", none));
        assert!(!is_claude_name("zsh", none));
        assert!(!is_claude_name("2.1", none));
        assert!(!is_claude_name("1.2.x", none));
        assert!(!is_claude_name("node", none));
        let args = |a: &[&str]| {
            let a: Vec<String> = a.iter().map(|s| s.to_string()).collect();
            move || Some(a)
        };
        assert!(is_claude_name(
            "node",
            args(&[
                "node",
                "/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js"
            ])
        ));
        assert!(is_claude_name(
            "node",
            args(&["node", "--no-warnings", "/home/me/.npm/bin/claude"])
        ));
        assert!(!is_claude_name("node", args(&["node", "app.js", "claude"])));
        assert!(!is_claude_name("node", args(&["node"])));
        // A wrapper script of your own, as a Mac names it.
        assert!(is_claude_name(
            "sh",
            args(&["/bin/sh", "/home/u/bin/claude"])
        ));
        assert!(is_claude_name("bash", args(&["bash", "-e", "/u/claude"])));
        assert!(!is_claude_name("bash", args(&["bash", "-c", "claude"])));
        assert!(!is_claude_name(
            "sh",
            args(&["sh", "/x/claude-code/run.sh"])
        ));
    }

    #[test]
    fn a_session_file_counts_only_when_written_after_its_process_started() {
        let at = |s| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
        assert!(written_since(at(100), Some(at(90))));
        assert!(written_since(at(100), Some(at(100))));
        // Clocks kept to the second: the same second, or one off, counts.
        assert!(written_since(at(100), Some(at(101))));
        assert!(!written_since(at(100), Some(at(102))));
        assert!(!written_since(at(100), Some(at(5000))));
        assert!(written_since(at(100), None));
    }

    #[test]
    fn session_files_are_read_and_found_by_pid() {
        let dir = std::env::temp_dir().join(format!("sb-sessions-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("41.json"),
            r#"{"pid":41,"sessionId":"s1","entrypoint":"cli"}"#,
        )
        .unwrap();
        fs::write(dir.join("42.json"), "not json").unwrap();
        fs::write(dir.join("41.abc.key"), "x").unwrap();
        let all = read_all(&dir);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].session_id, "s1");
        let long_ago = |_| Some(SystemTime::UNIX_EPOCH);
        assert!(runs_claude_by(&dir, &[7, 41], long_ago));
        assert!(!runs_claude_by(&dir, &[7], long_ago));
        assert!(!runs_claude_by(&dir, &[], long_ago));
        // Pid 41 handed to a process that started after the file was
        // written: a dead Claude's file, not this process's.
        let later = |_| Some(SystemTime::now() + Duration::from_secs(60));
        assert!(!runs_claude_by(&dir, &[41], later));
        assert!(read_all(&dir.join("missing")).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
