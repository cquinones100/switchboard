//! Search everything's reach into each tab's Claude conversation (Settings'
//! `tab_search_conversation`): your prompts and Claude's replies, read from
//! the session's transcript, where Claude Code wrote it, so a tab is found
//! by something said in it long after it scrolled out of the pane's lines.
//! New with the Rust port.
//!
//! Only what was said is kept, never a tool call or its output, which are
//! most of a transcript and were never the conversation. A transcript is
//! read off the server's task, once, and then only as far as it has grown
//! (a whole line at a time, as the history reads it); one over `MOST` bytes
//! is read from its last `MOST`. What was read is kept here, in memory
//! alone, for the transcripts the last search asked about, so a search
//! after it reads only what was added since.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use serde_json::Value;

use crate::agent::transcript::{Cursor, Entry, Kind, by_you};

/// The most of a transcript read: its last 50 MB.
pub const MOST: u64 = 50 * 1024 * 1024;

/// A line of the conversation: when it was said, and its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Said {
    pub at: i64,
    pub line: String,
}

/// What a conversation holds of the text searched for: the latest line
/// holding it, when it was said, and how many lines in all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub at: i64,
    pub line: String,
    pub count: usize,
    /// The lines said around it, for M-F's preview, and where it is
    /// among them (`pane_search::around`).
    pub around: Vec<String>,
    pub around_at: usize,
}

/// A transcript as read so far: its lines said, and where its next read
/// starts.
#[derive(Clone, Debug, Default)]
pub struct Conversation {
    pub said: Vec<Said>,
    pub cursor: Cursor,
}

impl Conversation {
    /// Takes in a line of the transcript.
    fn take(said: &mut Vec<Said>, line: &str) {
        let Some(entry) = Entry::parse(line) else {
            return;
        };
        if entry.sidechain() || entry.meta() {
            return;
        }
        let at = entry.seconds();
        if at == 0 {
            return;
        }
        let text = match entry.kind() {
            Kind::User if by_you(entry.pointer("/origin/kind")) => entry.prompt_text(),
            // A message typed while Claude works, queued as an attachment,
            // as the history counts it a turn of yours.
            Kind::Attachment
                if entry.pointer("/attachment/type").and_then(Value::as_str)
                    == Some("queued_command")
                    && by_you(entry.pointer("/attachment/origin/kind"))
                    && entry
                        .pointer("/attachment/commandMode")
                        .and_then(Value::as_str)
                        .is_none_or(|m| m == "prompt") =>
            {
                entry
                    .pointer("/attachment/prompt")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }
            Kind::Assistant => Some(entry.texts().collect::<Vec<_>>().join("\n")),
            _ => None,
        };
        let Some(text) = text else {
            return;
        };
        // Each line with its indentation, which M-F's preview shows; what
        // is searched and summed up is the same either way.
        for line in text
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.trim().is_empty())
        {
            said.push(Said {
                at,
                line: line.to_string(),
            });
        }
    }

    /// The latest line holding `text` (lowercased), and how many do.
    pub fn search(&self, text: &str) -> Option<Hit> {
        if text.is_empty() {
            return None;
        }
        let hits: Vec<usize> = (0..self.said.len())
            .filter(|&i| self.said[i].line.to_lowercase().contains(text))
            .collect();
        let &i = hits.last()?;
        let last = &self.said[i];
        let lines: Vec<&str> = self.said.iter().map(|s| s.line.as_str()).collect();
        let (around, around_at) = crate::mission::pane_search::around(&lines, i);
        Some(Hit {
            at: last.at,
            line: last.line.trim().to_string(),
            count: hits.len(),
            around,
            around_at,
        })
    }

    /// Reads on in `file`: from its start the first time (its last `MOST`
    /// bytes when it is longer), from where the last read stopped after;
    /// a file rewritten shorter is read again from its start.
    pub fn read_on(&mut self, file: &Path) {
        self.read_within(file, MOST);
    }

    fn read_within(&mut self, file: &Path, most: u64) {
        let said = &mut self.said;
        if let Ok(true) = self.cursor.read(file, most, |l| Self::take(said, l)) {
            said.clear();
            let _ = self.cursor.read(file, most, |l| Self::take(said, l));
        }
    }
}

static READ: LazyLock<Mutex<HashMap<PathBuf, Conversation>>> = LazyLock::new(Mutex::default);

/// What each transcript of `files` holds of `text` (lowercased), each read
/// on as far as it has grown; what was read of a transcript not among them
/// is let go. Off the server's task.
pub fn search(files: &[PathBuf], text: &str) -> HashMap<PathBuf, Hit> {
    let mut read = READ.lock().unwrap_or_else(|e| e.into_inner());
    read.retain(|file, _| files.contains(file));
    let mut out = HashMap::new();
    for file in files {
        let conversation = read.entry(file.clone()).or_default();
        conversation.read_on(file);
        if let Some(hit) = conversation.search(text) {
            out.insert(file.clone(), hit);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sb-said-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn take(c: &mut Conversation, text: &str) {
        text.lines()
            .for_each(|l| Conversation::take(&mut c.said, l));
    }

    fn line(entry: Value) -> String {
        format!("{entry}\n")
    }

    fn prompt(at: &str, text: &str) -> String {
        line(json!({"type": "user", "timestamp": at, "message": {"role": "user", "content": text}}))
    }

    fn reply(at: &str, text: &str) -> String {
        line(
            json!({"type": "assistant", "timestamp": at, "message": {"content": [
                {"type": "text", "text": text},
                {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "echo zebra"}}
            ]}}),
        )
    }

    #[test]
    fn keeps_prompts_and_replies_but_not_tool_calls_or_their_output() {
        let mut c = Conversation::default();
        take(
            &mut c,
            &format!(
                "{}{}{}{}{}{}{}",
                prompt("2026-11-15T10:00:00Z", "Where is node installed?\n\nThanks"),
                reply("2026-11-15T10:00:05Z", "Node is under ~/.local"),
                line(
                    json!({"type": "user", "timestamp": "2026-11-15T10:00:06Z", "message": {"content": [
                        {"type": "tool_result", "tool_use_id": "t1", "content": "zebra node output"}
                    ]}})
                ),
                line(
                    json!({"type": "user", "timestamp": "2026-11-15T10:00:07Z", "origin": {"kind": "peer"},
                "message": {"content": "a subagent's node report"}})
                ),
                line(
                    json!({"type": "attachment", "timestamp": "2026-11-15T10:00:09Z", "attachment": {
                "type": "queued_command", "prompt": "also check node's path", "commandMode": "prompt"}})
                ),
                line(
                    json!({"type": "attachment", "timestamp": "2026-11-15T10:00:10Z", "attachment": {
                "type": "queued_command", "prompt": "node task done", "commandMode": "task-notification"}})
                ),
                line(
                    json!({"type": "assistant", "isSidechain": true, "timestamp": "2026-11-15T10:00:08Z",
                "message": {"content": [{"type": "text", "text": "sidechain node"}]}})
                ),
            ),
        );
        let lines: Vec<&str> = c.said.iter().map(|s| s.line.as_str()).collect();
        assert_eq!(
            lines,
            [
                "Where is node installed?",
                "Thanks",
                "Node is under ~/.local",
                "also check node's path"
            ]
        );
        assert!(c.search("zebra").is_none(), "a tool call is left out");
        let hit = c.search("node").unwrap();
        assert_eq!(hit.count, 3);
        assert_eq!(hit.line, "also check node's path");
        assert_eq!(
            hit.at,
            "2026-11-15T10:00:09Z"
                .parse::<jiff::Timestamp>()
                .unwrap()
                .as_second()
        );
    }

    #[test]
    fn a_match_s_lines_around_keep_their_indentation() {
        let mut c = Conversation::default();
        take(
            &mut c,
            &reply("2026-11-15T10:00:05Z", "- parent\n    - zebra child  "),
        );
        let hit = c.search("zebra").unwrap();
        assert_eq!(hit.line, "- zebra child");
        assert_eq!(hit.around, ["- parent", "    - zebra child"]);
        assert_eq!(hit.around_at, 1);
    }

    #[test]
    fn a_grown_transcript_is_read_from_where_it_was() {
        let dir = scratch("grown");
        let file = dir.join("t.jsonl");
        std::fs::write(&file, prompt("2026-11-15T10:00:00Z", "first otter")).unwrap();
        let mut c = Conversation::default();
        c.read_on(&file);
        assert_eq!(c.said.len(), 1);
        let first = c.cursor.offset;
        // A line still being written is left for the next read.
        let mut more = reply("2026-11-15T10:01:00Z", "second otter");
        let partial = prompt("2026-11-15T10:02:00Z", "third otter");
        more.push_str(&partial[..10]);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .and_then(|mut f| std::io::Write::write_all(&mut f, more.as_bytes()))
            .unwrap();
        c.read_on(&file);
        assert_eq!(c.said.len(), 2, "each line read once");
        assert!(c.cursor.offset > first);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .and_then(|mut f| std::io::Write::write_all(&mut f, &partial.as_bytes()[10..]))
            .unwrap();
        c.read_on(&file);
        assert_eq!(c.search("otter").unwrap().count, 3);
        // Rewritten shorter: read again from its start.
        std::fs::write(&file, prompt("2026-11-15T11:00:00Z", "new")).unwrap();
        c.read_on(&file);
        assert_eq!(c.said.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_long_transcript_is_read_from_its_last_part() {
        let dir = scratch("long");
        let file = dir.join("t.jsonl");
        let old = prompt("2026-11-15T10:00:00Z", "old puffin");
        let new = prompt("2026-11-15T10:01:00Z", "new puffin");
        std::fs::write(&file, format!("{old}{new}")).unwrap();
        let mut c = Conversation::default();
        // The cut lands inside the first line, which is passed over.
        c.read_within(&file, new.len() as u64 + 5);
        let lines: Vec<&str> = c.said.iter().map(|s| s.line.as_str()).collect();
        assert_eq!(lines, ["new puffin"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_search_keeps_only_the_transcripts_it_asks_about() {
        let dir = scratch("kept");
        let (a, b) = (dir.join("a.jsonl"), dir.join("b.jsonl"));
        std::fs::write(&a, prompt("2026-11-15T10:00:00Z", "a heron")).unwrap();
        std::fs::write(&b, prompt("2026-11-15T10:00:00Z", "b crane")).unwrap();
        let found = search(&[a.clone(), b.clone()], "heron");
        assert_eq!(found.keys().collect::<Vec<_>>(), [&a]);
        search(std::slice::from_ref(&b), "crane");
        let read = READ.lock().unwrap();
        assert!(!read.contains_key(&a));
        assert!(read.contains_key(&b));
        drop(read);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
