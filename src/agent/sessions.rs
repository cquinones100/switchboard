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

use std::fs;
use std::path::{Path, PathBuf};

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

/// Whether any of these processes (a pane's foreground) is Claude: has a
/// session file in `dir`. Checked this way rather than by the program's name,
/// since Claude started from a Node script runs as its child and reads as
/// `node`, and any program could be named like a version.
pub fn runs_claude(dir: &Path, pids: &[u32]) -> bool {
    pids.iter()
        .any(|pid| dir.join(format!("{pid}.json")).is_file())
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
        assert!(runs_claude(&dir, &[7, 41]));
        assert!(!runs_claude(&dir, &[7]));
        assert!(!runs_claude(&dir, &[]));
        assert!(read_all(&dir.join("missing")).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
