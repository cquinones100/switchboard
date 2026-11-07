//! `claude-reply`: prints the last reply Claude gave in a tab, the final
//! text of its last turn, as the Slack relay reads it from the session's
//! transcript. Ported from `bin/claude-reply` and the parts of
//! `lib/slack-relay/tabs.js` it used (`paneFor`, `transcriptFor`,
//! `lastPrompt`, `lastSaid`).
//!
//! ```text
//! claude-reply <tab>          the reply, to stdout
//! claude-reply --pane <tab>   the pane to type into: the tab's Claude pane,
//!                             or its active pane when it runs none
//! ```
//!
//! `<tab>` is a tab's id (`@12`) or its number in the caller's session (the
//! first session outside a pane). Used by M-p's "Send Claude's last reply to
//! another tab". Exits 1, saying why on stderr, when the tab has no Claude
//! or no reply yet.
//!
//! The tabs come from the server (`switchboard list`). A tab's Claude is the
//! pane under whose process a Claude Code process runs, one with a session
//! file in `~/.claude/sessions`, found by walking the pane's process tree,
//! as the relay walked `ps`'s, and a conversation it moved into the
//! background is followed to its job; the transcript is
//! `~/.claude/projects/*/<session>.jsonl`.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::Value;

use crate::agent::sessions;
use crate::mux::{procinfo, socket};
use crate::paths;

/// How much of a transcript's end is read for its last prompt at first,
/// four times more each time it isn't there.
const PROMPT_TAIL: u64 = 1024 * 1024;

/// The tab `target` names in the server's list, and its session's name.
fn find_tab<'a>(list: &'a Value, target: &str, session: &str) -> Option<&'a Value> {
    let sessions = list.get("sessions")?.as_array()?;
    let tabs = |s: &'a Value| {
        s.get("tabs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
    };
    if target.starts_with('@') {
        return sessions
            .iter()
            .flat_map(tabs)
            .find(|t| t.get("id").and_then(Value::as_str) == Some(target));
    }
    let index: i64 = target.parse().ok()?;
    let own = sessions
        .iter()
        .find(|s| s.get("name").and_then(Value::as_str) == Some(session))
        .or_else(|| sessions.first())?;
    tabs(own).find(|t| t.get("index").and_then(Value::as_i64) == Some(index))
}

/// Every process under `pid`, each before its own children.
fn descendants(pid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut stack = vec![pid];
    while let Some(p) = stack.pop() {
        if out.len() > 4096 {
            break;
        }
        out.push(p);
        stack.extend(procinfo::children_of(p));
    }
    out
}

/// The tab's Claude pane and its session: the first pane with a Claude
/// Code process under it, an interactive one before a headless one.
fn claude_of(tab: &Value) -> Option<(String, String)> {
    let all = sessions::read_all(&sessions::sessions_dir());
    let mut headless = None;
    for pane in tab
        .get("panes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(pid) = pane.get("pid").and_then(Value::as_u64) else {
            continue;
        };
        let id = pane
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        for p in descendants(pid as u32) {
            for s in all
                .iter()
                .filter(|s| s.pid == Some(p) && !s.session_id.is_empty())
            {
                if s.entrypoint.starts_with("sdk") {
                    headless.get_or_insert((id.clone(), carried_on(&all, s)));
                } else {
                    return Some((id, carried_on(&all, s)));
                }
            }
        }
    }
    headless
}

/// The session carrying on the conversation `session` shows: one it moved
/// into the background is followed from its parked Claude (`parkedJobId`)
/// to the job whose `jobId` matches, link after link, as the relay's
/// `transcriptFor` followed it, so the reply is the job's, not the parked
/// Claude's own.
fn carried_on(all: &[sessions::SessionFile], session: &sessions::SessionFile) -> String {
    let mut current = session;
    let mut seen = vec![current.session_id.as_str()];
    while !current.parked_job_id.is_empty() {
        let Some(job) = all.iter().find(|o| {
            o.job_id == current.parked_job_id
                && !o.session_id.is_empty()
                && !seen.contains(&o.session_id.as_str())
        }) else {
            break;
        };
        seen.push(&job.session_id);
        current = job;
    }
    current.session_id.clone()
}

/// The transcript of a session:`~/.claude/projects/<any>/<id>.jsonl`.
pub fn transcript_of(session: &str) -> Option<PathBuf> {
    let projects = paths::claude_dir().join("projects");
    fs::read_dir(projects)
        .ok()?
        .flatten()
        .map(|d| d.path().join(format!("{session}.jsonl")))
        .find(|f| f.is_file())
}

/// A prompt's text, when the entry is one you typed: a string, or text
/// parts with no tool result among them.
fn prompt_text(entry: &Value) -> Option<String> {
    match entry.pointer("/message/content")? {
        Value::String(s) => (!s.is_empty()).then(|| s.clone()),
        Value::Array(parts) => {
            if parts
                .iter()
                .any(|p| p.get("type").and_then(Value::as_str) == Some("tool_result"))
            {
                return None;
            }
            let text = parts
                .iter()
                .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

/// The entries after the transcript's last prompt, read from a tail of it
/// that grows until it holds one; `None` when it has no prompt yet.
fn after_last_prompt(path: &Path) -> Option<Vec<Value>> {
    let mut file = File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let mut tail = PROMPT_TAIL;
    loop {
        let length = size.min(tail);
        let whole = length == size;
        file.seek(SeekFrom::Start(size - length)).ok()?;
        let mut buf = Vec::new();
        (&mut file).take(length).read_to_end(&mut buf).ok()?;
        let mut text = String::from_utf8_lossy(&buf).into_owned();
        // A tail starts partway through a line.
        if !whole {
            text = text
                .find('\n')
                .map_or(String::new(), |at| text[at + 1..].to_string());
        }
        let entries: Vec<Value> = text
            .split('\n')
            .filter(|l| !l.is_empty())
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|e| {
                !e.get("isSidechain").is_some_and(crate::util::truthy)
                    && !e.get("isMeta").is_some_and(crate::util::truthy)
            })
            .collect();
        let start = entries.iter().rposition(|e| {
            e.get("type").and_then(Value::as_str) == Some("user") && prompt_text(e).is_some()
        });
        match start {
            Some(at) => return Some(entries[at + 1..].to_vec()),
            None if whole => return None,
            None => tail = tail.saturating_mul(4),
        }
    }
}

/// The last thing Claude said in its last turn: the last text part of an
/// assistant message after the last prompt.
pub fn last_said(path: &Path) -> Option<String> {
    after_last_prompt(path)?
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("assistant"))
        .filter_map(|e| e.pointer("/message/content").and_then(Value::as_array))
        .flatten()
        .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .rfind(|t| !t.trim().is_empty())
        .map(str::to_string)
}

pub fn main(args: Vec<String>) -> ExitCode {
    let pane_wanted = args.first().map(String::as_str) == Some("--pane");
    let target = if pane_wanted {
        args.get(1)
    } else {
        args.first()
    };
    let Some(target) = target.cloned() else {
        eprintln!("usage: claude-reply [--pane] <tab>");
        return ExitCode::from(2);
    };
    let session = socket::ask_ok(&["display", "session"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let list = socket::ask_ok(&["list"])
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .unwrap_or(Value::Null);
    let Some(tab) = find_tab(&list, &target, &session) else {
        eprintln!("No tab {target}.");
        return ExitCode::from(1);
    };
    let claude = claude_of(tab);

    if pane_wanted {
        let pane = claude.map(|(p, _)| p).unwrap_or_else(|| {
            tab.get("panes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|p| p.get("active").and_then(Value::as_bool) == Some(true))
                .and_then(|p| p.get("id").and_then(Value::as_str))
                .unwrap_or("")
                .to_string()
        });
        print!("{pane}");
        let _ = std::io::stdout().flush();
        return ExitCode::SUCCESS;
    }
    let Some((_, session_id)) = claude else {
        eprintln!("This tab isn't running Claude.");
        return ExitCode::from(1);
    };
    let Some(reply) = transcript_of(&session_id).and_then(|f| last_said(&f)) else {
        eprintln!("Claude hasn't replied in this tab yet.");
        return ExitCode::from(1);
    };
    print!("{}", reply.trim());
    let _ = std::io::stdout().flush();
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_last_reply_is_the_last_text_after_the_last_prompt() {
        let dir = std::env::temp_dir().join(format!("claude-reply-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("s.jsonl");
        let lines = [
            json!({ "type": "user", "message": { "content": "first" } }),
            json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "old reply" }] } }),
            json!({ "type": "user", "message": { "content": [{ "type": "text", "text": "second" }] } }),
            json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "looking" }, { "type": "tool_use" }] } }),
            json!({ "type": "user", "message": { "content": [{ "type": "tool_result" }] } }),
            json!({ "type": "assistant", "isSidechain": true, "message": { "content": [{ "type": "text", "text": "a subagent's" }] } }),
            json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "  done  " }, { "type": "text", "text": " " }] } }),
            json!({ "type": "user", "isMeta": true, "message": { "content": "meta" } }),
        ];
        let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        fs::write(&file, text.join("\n")).unwrap();
        assert_eq!(last_said(&file).as_deref(), Some("  done  "));

        fs::write(&file, format!("{}\n", lines[2])).unwrap();
        assert_eq!(last_said(&file), None, "a prompt with no reply yet");
        fs::write(&file, "").unwrap();
        assert_eq!(last_said(&file), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tabs_by_id_or_number_in_the_session() {
        let list = json!({ "sessions": [
            { "name": "main", "tabs": [{ "id": "@1", "index": 1 }, { "id": "@2", "index": 2 }] },
            { "name": "other", "tabs": [{ "id": "@5", "index": 1 }] }
        ] });
        let id = |t: Option<&Value>| {
            t.and_then(|t| t.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        assert_eq!(id(find_tab(&list, "@5", "main")).as_deref(), Some("@5"));
        assert_eq!(id(find_tab(&list, "1", "other")).as_deref(), Some("@5"));
        assert_eq!(id(find_tab(&list, "2", "")).as_deref(), Some("@2"));
        assert_eq!(id(find_tab(&list, "9", "main")), None);
    }

    #[test]
    fn a_conversation_moved_to_the_background_is_followed_to_its_job() {
        let file = |session: &str, job: &str, parked: &str| sessions::SessionFile {
            pid: Some(1),
            session_id: session.into(),
            entrypoint: "cli".into(),
            job_id: job.into(),
            parked_job_id: parked.into(),
        };
        let all = vec![
            file("parked", "", "j1"),
            file("first-job", "j1", "j2"),
            file("second-job", "j2", ""),
            file("ring", "j3", "j3"),
        ];
        assert_eq!(carried_on(&all, &all[0]), "second-job");
        assert_eq!(carried_on(&all, &all[2]), "second-job");
        // Files that name each other in a ring end.
        assert_eq!(carried_on(&all, &all[3]), "ring");
    }
}
