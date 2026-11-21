//! A tab's recent turns as a channel's messages (board 165): what you
//! typed, what Claude said, and each tool it ran as one dim line, read
//! from the end of its transcript, as the sidebar reads a turn
//! (`sidebar::claude`). A subagent's messages and Claude Code's own are
//! left out. Read by `switchboard phone`'s own process, never the server.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::sidebar::claude::{prompt_text, timestamp_ms, tool_text};

/// How much of a transcript's end is read.
const TAIL: u64 = 512 * 1024;
/// How many messages are kept.
pub const MOST: usize = 80;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Message {
    /// `you`, `claude` or `tool`.
    pub who: &'static str,
    pub text: String,
    /// When, in ms; 0 where the transcript doesn't say.
    pub at: i64,
}

/// The last `most` messages in a transcript's text (or its end).
pub fn parse(text: &str, most: usize) -> Vec<Message> {
    let flag = |e: &Value, k: &str| e.get(k) == Some(&Value::Bool(true));
    let mut out: Vec<Message> = Vec::new();
    for line in text.split('\n').filter(|l| l.starts_with('{')) {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if flag(&entry, "isSidechain") || flag(&entry, "isMeta") {
            continue;
        }
        let at = timestamp_ms(&entry);
        match entry.get("type").and_then(Value::as_str) {
            Some("user") => {
                if let Some(text) = prompt_text(&entry) {
                    out.push(Message {
                        who: "you",
                        text,
                        at,
                    });
                }
            }
            Some("assistant")
                if entry.pointer("/message/model").and_then(Value::as_str)
                    != Some("<synthetic>") =>
            {
                let parts = entry
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for part in parts {
                    match part.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            let t = part["text"].as_str().unwrap_or("").trim();
                            if !t.is_empty() {
                                out.push(Message {
                                    who: "claude",
                                    text: t.to_string(),
                                    at,
                                });
                            }
                        }
                        Some("tool_use") => out.push(Message {
                            who: "tool",
                            text: tool_text(&part),
                            at,
                        }),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    let skip = out.len().saturating_sub(most);
    out.split_off(skip)
}

/// The last messages of the transcript at `file`, read from its end.
pub fn recent(file: &Path) -> Vec<Message> {
    let Ok(mut f) = fs::File::open(file) else {
        return Vec::new();
    };
    let size = f.metadata().map(|m| m.len()).unwrap_or(0);
    let take = TAIL.min(size);
    if f.seek(SeekFrom::Start(size - take)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::with_capacity(take as usize);
    if f.take(take).read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    parse(&String::from_utf8_lossy(&buf), MOST)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transcript_reads_as_messages() {
        let text = [
            r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","message":{"content":"fix the build"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Looking."},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo build"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1"}]}}"#,
            r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"a subagent"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Fixed."}]}}"#,
        ]
        .join("\n");
        let messages = parse(&text, MOST);
        let said: Vec<(&str, &str)> = messages.iter().map(|m| (m.who, m.text.as_str())).collect();
        assert_eq!(
            said,
            [
                ("you", "fix the build"),
                ("claude", "Looking."),
                ("tool", "Bash: cargo build"),
                ("claude", "Fixed."),
            ]
        );
        assert!(messages[0].at > 0);
        assert_eq!(parse(&text, 1).len(), 1);
    }
}
