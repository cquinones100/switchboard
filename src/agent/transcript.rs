//! The one reader of Claude Code's transcripts, which every part that
//! reads a conversation goes through: the sidebar, the history, the
//! conversation search, M-y's copy, the Slack relay, the phone app,
//! `claude-here`, `claude-reply`, `wt-revive` and the statusline. Each of those decides
//! what it makes of a conversation; this reads the file and says what
//! each line is.
//!
//! Claude Code keeps each conversation as JSON lines under
//! `~/.claude/projects/`, which isn't documented, so an entry is its JSON
//! (`Entry`, which derefs to it) with what every reader asks of it typed:
//! its kind, its flags, its time, its content's parts and the prompt you
//! typed in it. A line that isn't JSON is passed over, so a change of
//! shape loses what a reader takes from it and nothing else.
//!
//! A transcript can be hundreds of megabytes, so it is read off the
//! server's task in one of two ways: on from where the last read stopped,
//! a whole line at a time (`Cursor`), or back from its end, in a piece that
//! grows until it holds what is looked for (`from_end`).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

/// What an entry is, by its `type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Something you, a hook, another session or a tool's result said to
    /// Claude.
    User,
    /// Something Claude said or called.
    Assistant,
    /// Claude Code's own note, such as a compaction.
    System,
    /// What Claude Code attached, such as a message queued mid-turn.
    Attachment,
    Other,
}

impl Kind {
    fn of(name: &str) -> Kind {
        match name {
            "user" => Kind::User,
            "assistant" => Kind::Assistant,
            "system" => Kind::System,
            "attachment" => Kind::Attachment,
            _ => Kind::Other,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Kind::User => "user",
            Kind::Assistant => "assistant",
            Kind::System => "system",
            Kind::Attachment => "attachment",
            Kind::Other => "",
        }
    }
}

/// One line of a transcript.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry(pub Value);

impl std::ops::Deref for Entry {
    type Target = Value;

    fn deref(&self) -> &Value {
        &self.0
    }
}

impl Entry {
    /// A line read as an entry: a JSON object, else nothing.
    pub fn parse(line: impl AsRef<[u8]>) -> Option<Entry> {
        let line = line.as_ref();
        if !line.starts_with(b"{") {
            return None;
        }
        serde_json::from_slice(line).ok().map(Entry)
    }

    pub fn kind(&self) -> Kind {
        Kind::of(str_of(self.get("type")))
    }

    pub fn is(&self, kind: Kind) -> bool {
        self.kind() == kind
    }

    /// Something said in the conversation, by you or Claude.
    pub fn is_said(&self) -> bool {
        matches!(self.kind(), Kind::User | Kind::Assistant)
    }

    /// Whether `key` is `true`.
    pub fn flag(&self, key: &str) -> bool {
        self.get(key) == Some(&Value::Bool(true))
    }

    /// A subagent's entry, not the conversation's.
    pub fn sidechain(&self) -> bool {
        self.flag("isSidechain")
    }

    /// A note Claude Code adds, such as a slash command's caveat or another
    /// session's message.
    pub fn meta(&self) -> bool {
        self.flag("isMeta")
    }

    /// When it was written, in ms since the epoch; `None` where it says
    /// nothing.
    pub fn time(&self) -> Option<i64> {
        self.get("timestamp")?
            .as_str()?
            .parse::<jiff::Timestamp>()
            .ok()
            .map(|t| t.as_millisecond())
    }

    /// `time`, 0 where it says nothing.
    pub fn ms(&self) -> i64 {
        self.time().unwrap_or(0)
    }

    /// `time` in seconds, 0 where it says nothing.
    pub fn seconds(&self) -> i64 {
        self.ms().div_euclid(1000)
    }

    /// Its origin's kind (`human`, `peer`, `task-notification`), empty
    /// where it has none.
    pub fn origin(&self) -> &str {
        str_of(self.pointer("/origin/kind"))
    }

    pub fn content(&self) -> Option<&Value> {
        self.pointer("/message/content")
    }

    /// Its content's parts, none when the content is a string.
    pub fn parts(&self) -> &[Value] {
        self.content()
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// The parts of one type (`text`, `tool_use`, `tool_result`).
    pub fn parts_of<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Value> + 'a {
        self.parts()
            .iter()
            .filter(move |p| str_of(p.get("type")) == kind)
    }

    /// Its text parts' words.
    pub fn texts(&self) -> impl Iterator<Item = &str> {
        self.parts_of("text").map(|p| str_of(p.get("text")))
    }

    /// Whether its content is a list of tools' results and nothing else.
    pub fn results_only(&self) -> bool {
        self.content()
            .and_then(Value::as_array)
            .is_some_and(|parts| parts.iter().all(|p| str_of(p.get("type")) == "tool_result"))
    }

    /// What a user entry's content says, its text parts joined with
    /// `join`: never a list holding a tool's result, and nothing when it
    /// says nothing.
    pub fn typed(&self, join: &str) -> Option<String> {
        let text = match self.content()? {
            Value::String(s) => s.clone(),
            Value::Array(_) if self.parts_of("tool_result").next().is_some() => return None,
            Value::Array(_) => self.texts().collect::<Vec<_>>().join(join),
            _ => return None,
        };
        (!text.is_empty()).then_some(text)
    }

    /// The prompt as you typed it: a slash command as `/name args`, pasted
    /// text unwrapped, a background task's report named as one.
    pub fn prompt_text(&self) -> Option<String> {
        static COMMAND: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"(?s)<command-name>(.*?)</command-name>").expect("a regex")
        });
        static ARGS: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"(?s)<command-args>(.*?)</command-args>").expect("a regex")
        });
        static PASTED: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"(?s)<pasted_content\b[^>]*>\n?(.*?)\n?</pasted_content\b[^>]*>")
                .expect("a regex")
        });
        let text = self.typed("\n")?;
        if text.trim().is_empty() {
            return None;
        }
        if self.origin() == "task-notification" {
            return Some("(a background task finished)".into());
        }
        if let Some(command) = COMMAND.captures(&text) {
            let args = ARGS
                .captures(&text)
                .map(|c| c[1].trim().to_string())
                .unwrap_or_default();
            return Some(format!("{} {args}", command[1].trim()).trim().to_string());
        }
        let trimmed = text.trim();
        if trimmed.starts_with("<local-command-") || trimmed.starts_with("<bash-") {
            return None;
        }
        Some(PASTED.replace_all(&text, "$1").trim().to_string())
    }

    /// A prompt: a user entry with `prompt_text`.
    pub fn is_prompt(&self) -> bool {
        self.is(Kind::User) && self.prompt_text().is_some()
    }
}

/// Whether a line may be an entry of `kind`, by its text alone, so a reader
/// skips parsing the lines that can't matter.
pub fn may_be(line: &str, kind: Kind) -> bool {
    line.contains(&format!("\"type\":\"{}\"", kind.name()))
}

/// The entries in a stretch of a transcript's text.
pub fn entries(text: &str) -> impl Iterator<Item = Entry> + '_ {
    text.split('\n').filter_map(Entry::parse)
}

pub fn str_of(v: Option<&Value>) -> &str {
    v.and_then(Value::as_str).unwrap_or("")
}

/// An entry you typed, by its origin: none, or `human`, never a background
/// task's notification or a subagent's report (`peer`), which Claude Code
/// writes as user entries and queues the same way.
pub fn by_you(kind: Option<&Value>) -> bool {
    kind.and_then(Value::as_str).is_none_or(|k| k == "human")
}

/// A tool result's text, its parts' joined with `join` when it is a list.
pub fn result_text(result: &Value, join: &str) -> String {
    match result.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .map(|p| str_of(p.get("text")))
            .collect::<Vec<_>>()
            .join(join),
        Some(v) if !v.is_null() => crate::util::js_string(v),
        _ => String::new(),
    }
}

/// How much is read at once: a transcript read from its start can be tens
/// of megabytes.
const CHUNK: u64 = 16 * 1024 * 1024;

/// How far a transcript has been read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    pub offset: u64,
}

impl Cursor {
    /// Reads on in `file` from where the last read stopped, handing `each`
    /// every whole line it has grown by, a chunk at a time, and stopping
    /// before a line not yet written whole. One that has grown by more
    /// than `most` bytes is read from its last `most`, from the first whole
    /// line in them. A file rewritten shorter is read no further: the
    /// answer is then `true`, and the cursor is back at its start, for
    /// the reader to drop what it took from it before and read again.
    pub fn read(
        &mut self,
        file: &Path,
        most: u64,
        mut each: impl FnMut(&str),
    ) -> std::io::Result<bool> {
        let mut f = File::open(file)?;
        let size = f.metadata()?.len();
        if size < self.offset {
            self.offset = 0;
            return Ok(true);
        }
        let start = self.offset.max(size.saturating_sub(most));
        let mut cut = start > self.offset;
        f.seek(SeekFrom::Start(start))?;
        let mut f = f.take(size - start);
        let mut whole = start;
        let mut pending: Vec<u8> = Vec::new();
        loop {
            if (&mut f).take(CHUNK).read_to_end(&mut pending)? == 0 {
                break;
            }
            let Some(end) = pending.iter().rposition(|b| *b == b'\n') else {
                continue;
            };
            for line in pending[..end].split(|b| *b == b'\n') {
                if !std::mem::take(&mut cut) {
                    each(&String::from_utf8_lossy(line));
                }
            }
            whole += end as u64 + 1;
            pending.drain(..=end);
            self.offset = whole;
        }
        Ok(false)
    }
}

/// What `find` makes of the end of `file`: its last `first` bytes, from
/// the first whole line in them, then four times as many while it finds
/// nothing, until it has `most` bytes or the whole file, which it is told
/// it has.
pub fn from_end<T>(
    file: &Path,
    first: u64,
    most: u64,
    mut find: impl FnMut(&str, bool) -> Option<T>,
) -> Option<T> {
    let mut f = File::open(file).ok()?;
    let size = f.metadata().ok()?.len();
    let mut length = first;
    loop {
        // A byte before the piece too, so a piece that starts a line keeps it.
        let start = size.saturating_sub(length);
        let before = start.saturating_sub(1);
        f.seek(SeekFrom::Start(before)).ok()?;
        let mut bytes = Vec::new();
        (&mut f).take(size - before).read_to_end(&mut bytes).ok()?;
        let whole = start == 0;
        let from = match whole {
            true => 0,
            false => bytes
                .iter()
                .position(|b| *b == b'\n')
                .map_or(bytes.len(), |i| i + 1),
        };
        let found = find(&String::from_utf8_lossy(&bytes[from..]), whole);
        if found.is_some() || whole || length >= most {
            return found;
        }
        length = length.saturating_mul(4);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sb-transcript-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("t.jsonl")
    }

    fn read(cursor: &mut Cursor, file: &Path, most: u64) -> (Vec<String>, bool) {
        let mut lines = Vec::new();
        let restarted = cursor
            .read(file, most, |l| lines.push(l.to_string()))
            .unwrap();
        (lines, restarted)
    }

    #[test]
    fn a_cursor_reads_whole_lines_only_and_goes_on_from_there() {
        let file = scratch("cursor");
        std::fs::write(&file, "{\"a\":1}\n{\"b\":").unwrap();
        let mut cursor = Cursor::default();
        assert_eq!(
            read(&mut cursor, &file, u64::MAX),
            (vec!["{\"a\":1}".into()], false)
        );
        assert_eq!(cursor.offset, 8);
        std::fs::write(&file, "{\"a\":1}\n{\"b\":2}\n").unwrap();
        assert_eq!(
            read(&mut cursor, &file, u64::MAX),
            (vec!["{\"b\":2}".into()], false)
        );
        assert_eq!(cursor.offset, 16);
        std::fs::write(&file, "{\"c\":3}\n").unwrap();
        assert_eq!(read(&mut cursor, &file, u64::MAX), (vec![], true));
        assert_eq!(
            read(&mut cursor, &file, u64::MAX),
            (vec!["{\"c\":3}".into()], false)
        );
        let mut fresh = Cursor::default();
        std::fs::write(&file, "{\"a\":1}\n{\"b\":2}\n{\"c\":3}\n").unwrap();
        assert_eq!(read(&mut fresh, &file, 12).0, vec!["{\"c\":3}".to_string()]);
        let _ = std::fs::remove_dir_all(file.parent().unwrap());
    }

    #[test]
    fn the_end_is_read_in_a_growing_piece_from_a_whole_line() {
        let file = scratch("end");
        let text: String = (0..100).map(|i| format!("{{\"n\":{i}}}\n")).collect();
        std::fs::write(&file, &text).unwrap();
        let mut sizes = Vec::new();
        let first = from_end(&file, 20, u64::MAX, |t, _| {
            sizes.push(t.len());
            entries(t).next().map(|e| e["n"].as_i64().unwrap())
        });
        assert_eq!(first, Some(98));
        let found = from_end(&file, 20, u64::MAX, |t, whole| {
            (whole || t.contains("\"n\":3}")).then(|| entries(t).count())
        });
        assert_eq!(found, Some(100));
        assert_eq!(from_end(&file, 20, 80, |_, _| None::<()>), None);
        let _ = std::fs::remove_dir_all(file.parent().unwrap());
    }

    #[test]
    fn an_entry_says_what_it_is() {
        let e = Entry::parse(
            r#"{"type":"user","timestamp":"2026-11-08T13:00:01.500Z","origin":{"kind":"peer"},"message":{"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}}"#,
        )
        .unwrap();
        assert!(e.is(Kind::User) && e.is_said() && !e.meta() && !e.sidechain());
        assert_eq!(e.origin(), "peer");
        assert_eq!(e.typed(" ").as_deref(), Some("a b"));
        assert_eq!(e.seconds(), e.ms() / 1000);
        assert!(!e.results_only());
        let result = Entry::parse(
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t"}]}}"#,
        )
        .unwrap();
        assert!(result.results_only() && result.typed("\n").is_none() && !result.is_prompt());
        assert_eq!(result.time(), None);
        assert!(Entry::parse("not json").is_none());
        assert!(may_be(r#"{"type":"assistant"}"#, Kind::Assistant));
    }
}
