//! A tab's recent turns as a channel's messages (board 165): what you
//! typed, what another tab's Claude sent it (board 182), what Claude
//! said, and each tool it ran (which the app folds into a turn's steps,
//! `turns` in assets/phone/app.js), read from the end of its transcript,
//! as the sidebar reads a turn (`sidebar::claude`). A subagent's messages
//! and reports and Claude Code's own are left out. A line of what you typed that is the path of an image the
//! phone sent (`images`) is the image, not words (board 174). Read by
//! `switchboard phone`'s own process, never the server.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use super::images;
use crate::sidebar::claude::{prompt_text, timestamp_ms, tool_text};

/// How much of a transcript's end is read.
const TAIL: u64 = 512 * 1024;
/// How many messages are kept.
pub const MOST: usize = 80;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Message {
    /// `you`, `tab` (another tab's Claude), `claude`, `tool`, `look` for
    /// the pictures Claude showed (`looks`), or `turn`, with no words,
    /// where a subagent's report back started a turn, which the app starts
    /// there without drawing it.
    pub who: &'static str,
    pub text: String,
    /// The names of the images in `images::dir()` a message you sent, or
    /// a look, carries, in order.
    pub images: Vec<String>,
    /// What each image is titled, in order, where its look titled them
    /// (Before and After); empty otherwise.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub titles: Vec<String>,
    /// When, in ms; 0 where the transcript doesn't say.
    pub at: i64,
    /// A `tab` message's sender: its address as the transcript gives it,
    /// which the turns route names by its tab (`web::sender_name`); empty
    /// for every other kind.
    pub from: String,
    /// A `tab` message that came in while Claude worked, which Claude Code
    /// queues into the running turn rather than starting one.
    pub queued: bool,
}

/// The last `most` messages in a transcript's text (or its end), the
/// phone's images found in `images_dir`.
pub fn parse(text: &str, most: usize, images_dir: &Path) -> Vec<Message> {
    let flag = |e: &Value, k: &str| e.get(k) == Some(&Value::Bool(true));
    let mut out: Vec<Message> = Vec::new();
    // Whether the last reply so far waits on a tool, so its turn runs on
    // until something other than the tool's result is said, as the relay
    // reads it (`relay::tabs`).
    let mut running = false;
    for line in text.split('\n').filter(|l| l.starts_with('{')) {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if flag(&entry, "isSidechain") {
            continue;
        }
        let at = timestamp_ms(&entry);
        match entry.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                running = entry
                    .pointer("/message/stop_reason")
                    .and_then(Value::as_str)
                    == Some("tool_use");
            }
            Some("user") if !flag(&entry, "isMeta") => {
                running &= entry
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .is_some_and(|parts| {
                        parts
                            .iter()
                            .all(|p| p.get("type").and_then(Value::as_str) == Some("tool_result"))
                    });
            }
            _ => {}
        }
        // Another session's message (`SendMessage`) is a meta entry with
        // its words in `origin.body`, never the framed content; one of
        // peer origin with no body is a subagent's report, known by its
        // origin alone as the relay knows it, kept only as where a turn
        // starts, so what Claude does after it isn't folded into the turn
        // before.
        if entry.pointer("/origin/kind").and_then(Value::as_str) == Some("peer") {
            if let Some(body) = entry.pointer("/origin/body").and_then(Value::as_str) {
                out.push(Message {
                    who: "tab",
                    text: body.trim().to_string(),
                    images: Vec::new(),
                    at,
                    from: entry
                        .pointer("/origin/from")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    queued: running,
                });
            } else {
                out.push(Message {
                    who: "turn",
                    text: String::new(),
                    images: Vec::new(),
                    at,
                    from: String::new(),
                    queued: false,
                });
            }
            continue;
        }
        if flag(&entry, "isMeta") {
            continue;
        }
        match entry.get("type").and_then(Value::as_str) {
            Some("user") => {
                if let Some(text) = prompt_text(&entry) {
                    let (text, images) = images::split(&text, images_dir);
                    out.push(Message {
                        who: "you",
                        text,
                        images,
                        titles: Vec::new(),
                        at,
                        from: String::new(),
                        queued: false,
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
                                    images: Vec::new(),
                                    titles: Vec::new(),
                                    at,
                                    from: String::new(),
                                    queued: false,
                                });
                            }
                        }
                        Some("tool_use") => out.push(Message {
                            who: "tool",
                            text: tool_text(&part),
                            images: Vec::new(),
                            titles: Vec::new(),
                            at,
                            from: String::new(),
                            queued: false,
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

/// The last messages of the transcript at `file`, read from its end, and
/// whether the read began past its start, so older messages were left out.
pub fn recent(file: &Path) -> (Vec<Message>, bool) {
    let Ok(mut f) = fs::File::open(file) else {
        return (Vec::new(), false);
    };
    let size = f.metadata().map(|m| m.len()).unwrap_or(0);
    let take = TAIL.min(size);
    if f.seek(SeekFrom::Start(size - take)).is_err() {
        return (Vec::new(), false);
    }
    let mut buf = Vec::with_capacity(take as usize);
    if f.take(take).read_to_end(&mut buf).is_err() {
        return (Vec::new(), false);
    }
    let messages = parse(&String::from_utf8_lossy(&buf), MOST, &images::dir());
    (messages, take < size)
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
        let messages = parse(&text, MOST, Path::new("/i"));
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
        assert_eq!(parse(&text, 1, Path::new("/i")).len(), 1);
    }

    #[test]
    fn the_images_you_sent_read_as_images() {
        let a = "0123456789abcdef0123456789abcdef.png";
        let line = serde_json::json!({
            "type": "user",
            "message": {"content": format!("Wraps on my phone too\n/i/{a}")},
        });
        let messages = parse(&line.to_string(), MOST, Path::new("/i"));
        assert_eq!(messages[0].text, "Wraps on my phone too");
        assert_eq!(messages[0].images, [a]);
    }

    #[test]
    fn another_tabs_message_reads_as_a_tab_message_and_a_report_stays_out() {
        let text = [
            r#"{"type":"user","message":{"content":"land it"}}"#,
            r#"{"type":"user","isMeta":true,"message":{"content":"Caveat: the messages below were generated by the user"}}"#,
            r#"{"type":"user","origin":{"kind":"peer"},"message":{"content":"<agent-message>Framing. The report follows: all done</agent-message>"}}"#,
            r#"{"type":"user","isMeta":true,"timestamp":"2026-11-12T04:44:00Z","origin":{"kind":"peer","from":"uds:/tmp/tab-six.sock","name":"proposer","body":"Which fixture?\n"},"message":{"content":"Another Claude session sent a message:\n<cross-session-message>Which fixture?</cross-session-message>"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"The two-commit one."}]}}"#,
        ]
        .join("\n");
        let messages = parse(&text, MOST, Path::new("/i"));
        let said: Vec<(&str, &str, &str)> = messages
            .iter()
            .map(|m| (m.who, m.text.as_str(), m.from.as_str()))
            .collect();
        assert_eq!(
            said,
            [
                ("you", "land it", ""),
                ("turn", "", ""),
                ("tab", "Which fixture?", "uds:/tmp/tab-six.sock"),
                ("claude", "The two-commit one.", ""),
            ]
        );
        assert!(messages[2].at > 0);
    }

    #[test]
    fn another_tabs_message_mid_turn_reads_as_queued() {
        let peer = |body: &str| {
            format!(
                r#"{{"type":"user","isMeta":true,"origin":{{"kind":"peer","from":"uds:/tmp/six.sock","body":"{body}"}},"message":{{"content":"framed"}}}}"#
            )
        };
        let text = [
            r#"{"type":"user","message":{"content":"/model"}}"#.to_string(),
            peer("idle"),
            r#"{"type":"user","message":{"content":"fix it"}}"#.to_string(),
            r#"{"type":"assistant","message":{"stop_reason":"tool_use","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#.to_string(),
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1"}]}}"#.to_string(),
            peer("mid-turn"),
        ]
        .join("\n");
        let messages = parse(&text, MOST, Path::new("/i"));
        let queued: Vec<(&str, bool)> = messages
            .iter()
            .filter(|m| m.who == "tab")
            .map(|m| (m.text.as_str(), m.queued))
            .collect();
        assert_eq!(queued, [("idle", false), ("mid-turn", true)]);
    }
}
