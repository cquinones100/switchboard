//! A tab's turns as a channel's messages (board 165): what you
//! typed, what another tab's Claude sent it (board 182), what Claude
//! said, and each tool it ran (which the app folds into a turn's steps,
//! `turns` in assets/phone/app.js), read from the end of its transcript,
//! as the sidebar reads a turn (`sidebar::claude`). A subagent's messages
//! and reports and Claude Code's own are left out. A line of what you typed that is the path of an image the
//! phone sent (`images`) is the image, not words (board 174). Read by
//! `switchboard phone`'s own process, never the server.
//!
//! A channel opens on its last `TURNS` turns and loads the turns before
//! as it is scrolled up (issue 356): `window` reads back from a byte
//! offset (the transcript's end, or the start of the earliest turn shown)
//! in `CHUNK`s until it has found that many turn starts or reached the
//! file's start, however long the tool outputs between them, so a 40 MB
//! transcript still opens on what you said. One read stops at `BOUND`,
//! sending what it found with where it stopped, which the next scroll
//! continues from. Each message carries its line's byte offset, which the
//! app keeps the turns it loaded by as new ones arrive.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use super::images;
use crate::sidebar::claude::{prompt_text, timestamp_ms, tool_text};

/// How many turns a channel opens on, and each scroll up loads.
pub const TURNS: usize = 20;
/// How much of a transcript is read at a time, back from the end.
const CHUNK: u64 = 512 * 1024;
/// The most of a transcript one request reads.
const BOUND: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Message {
    /// `you`, `tab` (another tab's Claude), `claude`, `tool`, `look` for
    /// the pictures Claude showed (`looks`), `task` where Claude Code's
    /// wake-up for a finished background task started a turn, which the
    /// app draws as a dim line, or `turn`, with no words,
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
    /// Where its line starts in the transcript, in bytes; a look has the
    /// offset of the message before it.
    pub offset: u64,
}

impl Message {
    /// Whether it starts a turn the window counts: what you said, another
    /// tab's message that didn't come mid-turn, or a background task's
    /// wake-up.
    fn starts_turn(&self) -> bool {
        matches!(self.who, "you" | "task") || (self.who == "tab" && !self.queued)
    }
}

/// A channel's turns as one request reads them: the messages from `from`,
/// the byte offset the next earlier read ends at, and whether anything of
/// the conversation lies before it.
#[derive(Debug, PartialEq, Eq)]
pub struct Window {
    pub messages: Vec<Message>,
    pub from: u64,
    pub earlier: bool,
}

/// The messages in a stretch of a transcript that starts at a line, `base`
/// bytes into the file, the phone's images found in `images_dir`.
pub fn parse(bytes: &[u8], base: u64, images_dir: &Path) -> Vec<Message> {
    let flag = |e: &Value, k: &str| e.get(k) == Some(&Value::Bool(true));
    let mut out: Vec<Message> = Vec::new();
    // Whether the last reply so far waits on a tool, so its turn runs on
    // until something other than the tool's result is said, as the relay
    // reads it.
    let mut running = false;
    let mut offset = base;
    for line in bytes.split(|b| *b == b'\n') {
        let at_line = offset;
        offset += line.len() as u64 + 1;
        if !line.starts_with(b"{") {
            continue;
        }
        let Ok(entry) = serde_json::from_slice::<Value>(line) else {
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
                    titles: Vec::new(),
                    at,
                    from: entry
                        .pointer("/origin/from")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    queued: running,
                    offset: at_line,
                });
            } else {
                out.push(Message {
                    who: "turn",
                    text: String::new(),
                    images: Vec::new(),
                    titles: Vec::new(),
                    at,
                    from: String::new(),
                    queued: false,
                    offset: at_line,
                });
            }
            continue;
        }
        if flag(&entry, "isMeta") {
            continue;
        }
        match entry.get("type").and_then(Value::as_str) {
            // Claude Code's own wake-up for a finished background task
            // starts a turn, but nobody said it.
            Some("user")
                if entry.pointer("/origin/kind").and_then(Value::as_str)
                    == Some("task-notification") =>
            {
                out.push(Message {
                    who: "task",
                    text: "a background task finished".to_string(),
                    images: Vec::new(),
                    titles: Vec::new(),
                    at,
                    from: String::new(),
                    queued: false,
                    offset: at_line,
                });
            }
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
                        offset: at_line,
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
                                    offset: at_line,
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
                            offset: at_line,
                        }),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The `TURNS` turns of the transcript at `file` that end before the
/// byte offset `before` (a turn's start a window answered as its `from`),
/// or at its end, read off the server's task; an error where it can't be
/// read, which the app asks again after rather than take as the start.
pub fn window(file: &Path, before: Option<u64>) -> std::io::Result<Window> {
    let mut f = fs::File::open(file)?;
    let size = f.metadata()?.len();
    read_window(&mut f, size, before, TURNS, CHUNK, BOUND, &images::dir())
}

/// `window` over any file, `turns` turns, read `chunk` bytes at a time
/// back from the end and no more than `bound` bytes in all.
fn read_window<F: Read + Seek>(
    f: &mut F,
    size: u64,
    before: Option<u64>,
    turns: usize,
    chunk: u64,
    bound: u64,
    images_dir: &Path,
) -> std::io::Result<Window> {
    let end = before.unwrap_or(size).min(size);
    // The bytes from `start` to `end`, and the turn starts found in those
    // from `counted` (a line's start) on, oldest first.
    let mut start = end;
    let mut buf: Vec<u8> = Vec::new();
    let mut counted = end;
    let mut starts: Vec<u64> = Vec::new();
    while starts.len() < turns && start > 0 && end - start < bound {
        let take = chunk.min(start).min(bound - (end - start));
        start -= take;
        let mut piece = vec![0; take as usize];
        f.seek(SeekFrom::Start(start))?;
        f.read_exact(&mut piece)?;
        piece.extend_from_slice(&buf);
        buf = piece;
        let new = &buf[..(counted - start) as usize];
        // The first whole line, the one cut by the chunk's start left for
        // the next chunk back.
        let line = match start {
            0 => Some(0),
            _ => new.iter().position(|b| *b == b'\n').map(|i| i + 1),
        };
        let Some(line) = line else { continue };
        let found = parse(&new[line..], start + line as u64, images_dir);
        let mut more: Vec<u64> = found
            .iter()
            .filter(|m| m.starts_turn())
            .map(|m| m.offset)
            .collect();
        more.append(&mut starts);
        starts = more;
        counted = start + line as u64;
    }
    let from = match starts.len().checked_sub(turns) {
        Some(i) => starts[i],
        // No line began in all that was read: the next read ends where
        // this one began.
        None if counted == end => start,
        None => counted,
    };
    let messages = parse(&buf[(from - start) as usize..], from, images_dir);
    Ok(Window {
        messages,
        from,
        earlier: from > 0,
    })
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
        let messages = parse(text.as_bytes(), 0, Path::new("/i"));
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
        assert_eq!(
            messages[1].offset,
            text.find("{\"type\":\"assistant").unwrap() as u64
        );
    }

    #[test]
    fn the_images_you_sent_read_as_images() {
        let a = "0123456789abcdef0123456789abcdef.png";
        let line = serde_json::json!({
            "type": "user",
            "message": {"content": format!("Wraps on my phone too\n/i/{a}")},
        });
        let messages = parse(line.to_string().as_bytes(), 0, Path::new("/i"));
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
        let messages = parse(text.as_bytes(), 0, Path::new("/i"));
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
        let messages = parse(text.as_bytes(), 0, Path::new("/i"));
        let queued: Vec<(&str, bool)> = messages
            .iter()
            .filter(|m| m.who == "tab")
            .map(|m| (m.text.as_str(), m.queued))
            .collect();
        assert_eq!(queued, [("idle", false), ("mid-turn", true)]);
    }

    #[test]
    fn a_background_tasks_wake_up_reads_as_a_task_not_as_you() {
        let text = [
            r#"{"type":"user","origin":{"kind":"task-notification"},"message":{"content":"<task-notification><status>completed</status></task-notification>"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"CI passed."}]}}"#,
        ]
        .join("\n");
        let messages = parse(text.as_bytes(), 0, Path::new("/i"));
        let said: Vec<(&str, &str)> = messages.iter().map(|m| (m.who, m.text.as_str())).collect();
        assert_eq!(
            said,
            [
                ("task", "a background task finished"),
                ("claude", "CI passed.")
            ]
        );
    }

    /// A conversation of `turns` turns, each your ask, a tool call whose
    /// output is `output` bytes, and Claude's reply.
    fn conversation(turns: usize, output: usize) -> Vec<u8> {
        let mut out = String::new();
        for n in 0..turns {
            let lines = [
                serde_json::json!({"type": "user", "message": {"content": format!("ask {n}")}}),
                serde_json::json!({"type": "assistant", "message": {"stop_reason": "tool_use", "content": [{"type": "tool_use", "id": "t", "name": "Bash", "input": {"command": "ls"}}]}}),
                serde_json::json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t", "content": "x".repeat(output)}]}}),
                serde_json::json!({"type": "assistant", "message": {"content": [{"type": "text", "text": format!("reply {n}")}]}}),
            ];
            for line in lines {
                out.push_str(&line.to_string());
                out.push('\n');
            }
        }
        out.into_bytes()
    }

    fn read(bytes: &[u8], before: Option<u64>, chunk: u64, bound: u64) -> Window {
        let mut f = std::io::Cursor::new(bytes);
        read_window(
            &mut f,
            bytes.len() as u64,
            before,
            TURNS,
            chunk,
            bound,
            Path::new("/i"),
        )
        .unwrap()
    }

    fn asks(w: &Window) -> Vec<String> {
        w.messages
            .iter()
            .filter(|m| m.who == "you")
            .map(|m| m.text.clone())
            .collect()
    }

    #[test]
    fn a_transcript_of_long_tool_outputs_opens_on_its_last_turns() {
        // 30 turns of 200 KB tool outputs: 6 MB, nearly all of it output.
        let bytes = conversation(30, 200 * 1024);
        let w = read(&bytes, None, CHUNK, BOUND);
        let want: Vec<String> = (10..30).map(|n| format!("ask {n}")).collect();
        assert_eq!(asks(&w), want);
        assert!(w.earlier);
        assert_eq!(w.messages[0].offset, w.from);
        assert_eq!(&bytes[w.from as usize..][..20], br#"{"type":"user","mess"#);
        assert_eq!(w.messages.last().unwrap().text, "reply 29");
        assert_eq!(w.messages.iter().filter(|m| m.who == "tool").count(), 20);
    }

    #[test]
    fn before_reads_the_turns_before_and_says_where_the_conversation_starts() {
        let bytes = conversation(30, 1000);
        let last = read(&bytes, None, 4096, BOUND);
        let before = read(&bytes, Some(last.from), 4096, BOUND);
        let want: Vec<String> = (0..10).map(|n| format!("ask {n}")).collect();
        assert_eq!(asks(&before), want);
        assert_eq!(before.from, 0);
        assert!(!before.earlier);
        assert_eq!(before.messages.last().unwrap().text, "reply 9");
        assert_eq!(read(&bytes, Some(0), 4096, BOUND).messages, []);
    }

    #[test]
    fn the_bound_sends_what_it_found_and_where_to_read_on() {
        let bytes = conversation(30, 1000);
        // 4 KB at a time, 16 KB in all: some turns, not 20.
        let w = read(&bytes, None, 4096, 16 * 1024);
        let found = asks(&w).len();
        assert!((1..TURNS).contains(&found), "{found}");
        assert!(w.earlier);
        assert!(w.from >= bytes.len() as u64 - 16 * 1024);
        // The next read carries on from there, with nothing twice.
        let next = read(&bytes, Some(w.from), 4096, 16 * 1024);
        assert!(next.messages.iter().all(|m| m.offset < w.from));
        assert_eq!(
            asks(&next).last().unwrap(),
            &format!("ask {}", 30 - found - 1)
        );
        // A line longer than the bound: nothing found, and the next read
        // ends where this one began.
        let mut long = conversation(2, 1000);
        long.extend(conversation(1, 64 * 1024));
        let w = read(&long, None, 4096, 16 * 1024);
        assert_eq!(asks(&w), Vec::<String>::new());
        assert!(w.earlier);
        assert!(w.from < long.len() as u64);
    }
}
