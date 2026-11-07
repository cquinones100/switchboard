//! The tabs running Claude, as the relay reads and types into them
//! (lib/slack-relay/tabs.js): each tab's Claude pane, its transcript and
//! last turn, its screen, the prompts and dialogs it shows, and typing an
//! answer or a message into it.
//!
//! A tab can hold other panes beside Claude (a shell, a server, another
//! Claude), so it keeps the pane it had: the first Claude pane the relay
//! sees in a tab is recorded on the tab as `@claude_pane` (a tab started
//! with `new:` records its pane when it is made), and stays the tab's pane
//! for as long as it is open, through /clear or Claude being started again
//! in it. While it isn't running Claude, the tab's first other Claude pane
//! stands in, without being recorded; once it has closed, that pane is
//! recorded instead.
//!
//! Which pane runs Claude, and its transcript, are the server's to say
//! (`list`, from the session in front on each pane, a parked conversation
//! followed to the job that carries it on), where the Node relay asked
//! tmux for each pane's command and walked `ps` for its sessions.
//!
//! The prompts are read with mission control's reading of them
//! (src/mission/answer.rs, which was ported from here).

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

use regex::Regex;
use serde_json::Value;

use super::config::{Paths, SCREEN_LINES};
use super::{Cx, Relay};
use crate::mission::answer::{self, Answer, Press, Prompt, PromptOption};
use crate::util;

pub use answer::{answer_for, answer_said, options_in};

/// The option the relay records a tab's Claude pane in.
pub const CLAUDE_PANE: &str = "@claude_pane";

/// One tab running Claude: its id, number and whole name, its Claude
/// pane's state and since when, whether it is the tab in front of you, its
/// pane and that pane's process, every Claude pane's process, and the
/// transcript of the conversation it shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tab {
    pub id: String,
    pub index: i64,
    pub name: String,
    pub state: String,
    pub since: String,
    pub focused: bool,
    pub loop_tab: bool,
    pub pane: String,
    pub pid: String,
    pub pids: Vec<String>,
    pub transcript: Option<PathBuf>,
    pub own_name: String,
    pub cwd: Option<PathBuf>,
}

/// What this module keeps in memory: each tab's Claude pane as last seen,
/// and each transcript's last activity, read again only when it changes.
#[derive(Default)]
pub struct Memory {
    panes: HashMap<String, String>,
    activity: HashMap<PathBuf, (SystemTime, Option<i64>)>,
}

/// Every tab running Claude, in order, each with the pane the relay reads,
/// follows and types into. Each call also notes every tab's Claude pane for
/// `pane_for`.
pub fn claude_tabs(cx: &Relay) -> Vec<Tab> {
    let Some(windows) = cx.mux.windows() else {
        return Vec::new();
    };
    let mut tabs = Vec::new();
    for w in windows {
        let claude: Vec<_> = w.panes.iter().filter(|p| p.claude).collect();
        if w.relay || claude.is_empty() {
            continue;
        }
        let recorded = w.panes.iter().find(|p| Some(&p.id) == w.recorded.as_ref());
        let chosen = match recorded {
            Some(r) if r.claude => r,
            _ => claude[0],
        };
        if recorded.is_none() {
            let _ = cx.mux.set_option(&w.id, CLAUDE_PANE, Some(&chosen.id));
        }
        tabs.push(Tab {
            id: w.id.clone(),
            index: w.index,
            name: w.name.clone(),
            state: chosen.state.clone(),
            since: chosen.since.clone(),
            focused: w.focused,
            loop_tab: w.loop_tab,
            pane: chosen.id.clone(),
            pid: chosen.pid.clone(),
            pids: claude.iter().map(|p| p.pid.clone()).collect(),
            transcript: chosen.transcript.clone(),
            own_name: w.own_name.clone(),
            cwd: chosen.cwd.clone(),
        });
    }
    let mut mem = cx.mem.borrow_mut();
    mem.tabs.panes = tabs
        .iter()
        .map(|t| (t.id.clone(), t.pane.clone()))
        .collect();
    tabs.sort_by_key(|t| t.index);
    tabs
}

/// One tab, looked up afresh.
pub fn tab(cx: &Relay, id: &str) -> Option<Tab> {
    claude_tabs(cx).into_iter().find(|t| t.id == id)
}

/// The pane to read or type into for a tab: its Claude pane, as the last
/// look saw it (the relay looks every second), else the tab itself, which
/// the server takes as its active pane.
pub fn pane_for(cx: &Relay, id: &str) -> String {
    if !id.starts_with('@') {
        return id.to_string();
    }
    let known = cx.mem.borrow().tabs.panes.get(id).cloned();
    if let Some(pane) = known {
        return pane;
    }
    claude_tabs(cx);
    cx.mem
        .borrow()
        .tabs
        .panes
        .get(id)
        .cloned()
        .unwrap_or_else(|| id.to_string())
}

pub fn label(index: i64, name: &str) -> String {
    format!("tab {index}, {name}")
}

/// How a tab's state reads in Slack, colored so the one waiting on you
/// stands out in a list.
pub fn state_words(state: &str) -> String {
    match state {
        "waiting" => "🔴 **waiting on you**".into(),
        "done" => "✅ finished".into(),
        "working" => "🔵 working".into(),
        "idle" => "⚪ idle".into(),
        other => other.into(),
    }
}

/// A Markdown table, which Slack's markdown blocks render as a real table.
/// Each cell is one line, with any | in it escaped.
pub fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let cell = |text: &str| {
        text.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .replace('|', "\\|")
    };
    let mut lines = vec![
        format!("| {} |", headers.join(" | ")),
        format!("|{}|", vec!["---"; headers.len()].join("|")),
    ];
    for row in rows {
        lines.push(format!(
            "| {} |",
            row.iter().map(|c| cell(c)).collect::<Vec<_>>().join(" | ")
        ));
    }
    lines.join("\n")
}

/// The tab's Claude pane, to type into, looked up afresh: whichever pane
/// you are using, typing goes to Claude, never to a shell beside it.
pub fn claude_pane(cx: &Relay, id: &str) -> Result<String, String> {
    if let Some(t) = tab(cx, id) {
        return Ok(t.pane);
    }
    let windows = cx.mux.windows().unwrap_or_default();
    match windows.iter().find(|w| w.id == id) {
        None => Err("That tab no longer exists.".into()),
        Some(_) => Err("That tab isn't running Claude, so nothing was sent.".into()),
    }
}

/// The transcript of the conversation a tab shows, as the last look saw it.
pub fn transcript_for(cx: &Relay, id: &str) -> Option<PathBuf> {
    tab(cx, id).and_then(|t| t.transcript)
}

/// How full a session's context is, and its model, from the file the
/// statusline writes for the sidebar (`<percent>\t<model>`). Claude Code
/// gives them only to the statusline, so the figure is as of its last
/// redraw, and there is none until it has drawn once.
#[derive(Clone, Debug, PartialEq)]
pub struct Context {
    pub session: String,
    pub percent: i64,
    pub model: String,
}

pub fn context_of(paths: &Paths, transcript: Option<&Path>) -> Option<Context> {
    let session = transcript?.file_stem()?.to_string_lossy().into_owned();
    let text = paths
        .context_dirs
        .iter()
        .find_map(|dir| util::read_text(dir.join(&session)))?;
    let mut parts = text.splitn(2, '\t');
    let percent = parts.next()?;
    if percent.is_empty() || !percent.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(Context {
        session,
        percent: percent.parse().ok()?,
        model: parts.next().unwrap_or("").trim().to_string(),
    })
}

pub fn context_words(context: Option<&Context>) -> String {
    context
        .map(|c| format!("{}% context used", c.percent))
        .unwrap_or_default()
}

/// How long ago, in words.
pub fn relative(ms: i64) -> String {
    let minutes = (ms as f64 / 60_000.0).round() as i64;
    if minutes < 1 {
        "just now".into()
    } else if minutes < 60 {
        format!("{minutes}m ago")
    } else if minutes < 48 * 60 {
        format!("{}h ago", (minutes as f64 / 60.0).round() as i64)
    } else {
        format!("{} days ago", (minutes as f64 / 1440.0).round() as i64)
    }
}

/// `relative` without its "ago", for how long a state has lasted.
pub fn lasted(ms: i64) -> String {
    let words = relative(ms);
    words
        .strip_suffix(" ago")
        .map(str::to_string)
        .unwrap_or(words)
}

/// When a `since` began, in ms, or `None` for none.
pub fn since_ms(since: &str) -> Option<i64> {
    since.parse::<f64>().ok().map(|s| (s * 1000.0) as i64)
}

pub const NO_REPLY: &str = "(no reply yet)";

fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn same_text(a: Option<&str>, b: Option<&str>) -> bool {
    flat(a.unwrap_or("")) == flat(b.unwrap_or(""))
}

/// The last turn of a tab's conversation: the last prompt and everything
/// Claude has written since, with its tool calls counted rather than shown.
/// The screen when the transcript can't be found.
pub fn last_turn(cx: &Relay, id: &str, expected: Option<&str>) -> String {
    transcript_for(cx, id)
        .and_then(|file| turn_from(&file, expected, None))
        .unwrap_or_else(|| {
            let shown = screen(cx, id);
            format!(
                "```\n{}\n```",
                if shown.is_empty() { "(empty)" } else { &shown }
            )
        })
}

/// Claude's texts after the last prompt.
fn texts_after(after: &[Value]) -> Vec<String> {
    after
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("assistant"))
        .filter_map(|e| e.pointer("/message/content").and_then(Value::as_array))
        .flatten()
        .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .filter(|t| !t.trim().is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether a transcript has caught up with a turn the Stop hook reported:
/// its last text is the turn's final message.
pub fn caught_up(file: &Path, last: &str) -> bool {
    let after = last_prompt(file).map(|p| p.after).unwrap_or_default();
    same_text(texts_after(&after).last().map(String::as_str), Some(last))
}

/// Whether a transcript's last turn has been written to the end: its last
/// entry is Claude's text, with no tool call after it. Claude Code writes
/// the last words a moment after the Stop hook fires, so a turn reported
/// ended can still stop at a tool call, or at text that came before one.
pub fn turn_written(file: &Path) -> bool {
    let after = last_prompt(file).map(|p| p.after).unwrap_or_default();
    let Some(last) = after.iter().rev().find(|e| {
        matches!(
            e.get("type").and_then(Value::as_str),
            Some("assistant" | "user")
        )
    }) else {
        return false;
    };
    let parts = last
        .pointer("/message/content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let kind = |p: &Value| p.get("type").and_then(Value::as_str).map(str::to_string);
    last.get("type").and_then(Value::as_str) == Some("assistant")
        && parts.iter().any(|p| {
            kind(p).as_deref() == Some("text")
                && p.get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|t| !t.trim().is_empty())
        })
        && !parts.iter().any(|p| kind(p).as_deref() == Some("tool_use"))
}

/// Claude Code keeps a long message it was typed (the relay types the whole
/// of yours at once, as a paste) as pasted text, wrapped in a
/// `<pasted_content>` tag; this is the text inside.
pub fn unpasted(text: &str) -> String {
    static PASTED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"<pasted_content\b[^>]*>\n?((?s:.*?))\n?</pasted_content\b[^>]*>")
            .expect("a regex")
    });
    PASTED.replace_all(text, "$1").into_owned()
}

/// Whether a prompt holds the text the relay typed: its first 30
/// characters, ignoring spacing, anywhere in it, since text left unsent in
/// Claude's input box (an earlier message whose Enter was lost, or your own
/// typing) is sent along with it, in front.
pub fn prompt_matches(prompt: &str, expected: &str) -> bool {
    let words = |t: &str| flat(&unpasted(t));
    let wanted: String = words(expected).chars().take(30).collect();
    words(prompt).contains(&wanted)
}

/// A transcript's last prompt: its text, whether it was a background
/// task's or an agent's report rather than yours, when it was sent, and
/// every entry after it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LastPrompt {
    pub prompt: String,
    pub background: bool,
    pub sent_at: Option<i64>,
    pub after: Vec<Value>,
}

/// How much of a transcript's end is read for its last prompt at first:
/// a long session's transcript runs to hundreds of megabytes.
const PROMPT_TAIL: u64 = 1024 * 1024;

/// A prompt's text, when the entry is one you (or the harness) sent.
fn prompt_of(entry: &Value) -> Option<String> {
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

/// The end of a file: its last `tail` bytes, from the start of a line, and
/// whether that is the whole file.
fn tail_of(file: &Path, tail: u64) -> Option<(String, bool)> {
    let mut f = File::open(file).ok()?;
    let size = f.metadata().ok()?.len();
    let length = size.min(tail);
    let whole = length == size;
    f.seek(SeekFrom::Start(size - length)).ok()?;
    let mut buf = Vec::new();
    f.take(length).read_to_end(&mut buf).ok()?;
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if !whole {
        text = text
            .find('\n')
            .map_or(String::new(), |at| text[at + 1..].to_string());
    }
    Some((text, whole))
}

/// When an entry was written, in ms.
pub fn time_of(entry: &Value) -> Option<i64> {
    let ts = entry.get("timestamp")?.as_str()?;
    ts.parse::<jiff::Timestamp>()
        .ok()
        .map(|t| t.as_millisecond())
}

/// A transcript's last prompt, or `None` when there is none yet.
/// Subagents' entries are left out. Read from the end, a tail four times
/// longer each time until it holds a prompt.
pub fn last_prompt(file: &Path) -> Option<LastPrompt> {
    static SUMMARY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)<summary>(.*?)</summary>").expect("a regex"));
    static COMMAND: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"<command-name>([^<]*)</command-name>").expect("a regex"));
    static TAGGED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"<[^>]+>[^<]*</[^>]+>").expect("a regex"));
    static AGENT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"</?agent-message[^>]*>").expect("a regex"));
    let mut tail = PROMPT_TAIL;
    loop {
        let (text, whole) = tail_of(file, tail)?;
        let entries: Vec<Value> = text
            .split('\n')
            .filter(|l| !l.is_empty())
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|e| {
                !e.get("isSidechain").is_some_and(util::truthy)
                    && !e.get("isMeta").is_some_and(util::truthy)
            })
            .collect();
        let start = entries.iter().rposition(|e| {
            e.get("type").and_then(Value::as_str) == Some("user") && prompt_of(e).is_some()
        });
        let Some(start) = start else {
            if whole {
                return None;
            }
            tail = tail.saturating_mul(4);
            continue;
        };
        let entry = &entries[start];
        let raw = prompt_of(entry).unwrap_or_default();
        let after = entries[start + 1..].to_vec();
        let sent_at = time_of(entry);
        let origin = entry.pointer("/origin/kind").and_then(Value::as_str);
        // A subagent's report back starts a turn (origin peer), in the
        // harness's framing: it reads as the report, cut short. Known by its
        // origin alone, never its text, which a prompt of yours may quote.
        if origin == Some("peer") {
            const FOLLOWS: &str = "The report follows:";
            let framed = raw
                .find(FOLLOWS)
                .map(|at| &raw[at + FOLLOWS.len()..])
                .unwrap_or(&raw);
            let report = entry
                .pointer("/origin/handbackReportText")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| AGENT.replace_all(framed, "").into_owned());
            let report = flat(&report);
            let shown = if report.chars().count() > 200 {
                format!("{}…", report.chars().take(200).collect::<String>())
            } else {
                report
            };
            return Some(LastPrompt {
                prompt: format!("An agent reported back: {shown}"),
                background: true,
                sent_at,
                after,
            });
        }
        // A background task's notification (origin task-notification),
        // reads as the task's summary.
        let background = origin == Some("task-notification")
            || raw.trim_start().starts_with("<task-notification>");
        let prompt = if background {
            let summaries: Vec<String> = SUMMARY
                .captures_iter(&raw)
                .map(|c| c[1].trim().to_string())
                .collect();
            if summaries.is_empty() {
                "A background task finished.".to_string()
            } else {
                summaries.join("\n")
            }
        } else {
            // Slash commands are stored with their tags; show what was typed.
            let text = unpasted(&raw);
            let text = COMMAND.replace(&text, "$1 ");
            TAGGED.replace_all(&text, "").trim().to_string()
        };
        return Some(LastPrompt {
            prompt,
            background,
            sent_at,
            after,
        });
    }
}

/// When a session last did something: the time of its transcript's last
/// conversation entry (a prompt, a reply, a tool call or result), in ms.
/// Not the file's modified time: Claude Code also appends bookkeeping
/// records to an idle session's transcript. Read from the last 256 KB, and
/// again only when the file has changed.
pub fn last_activity_at(cx: &Relay, file: &Path) -> Option<i64> {
    let modified = std::fs::metadata(file).and_then(|m| m.modified()).ok()?;
    if let Some((at, known)) = cx.mem.borrow().tabs.activity.get(file)
        && *at == modified
    {
        return *known;
    }
    let found = activity_in(file);
    let mut mem = cx.mem.borrow_mut();
    if mem.tabs.activity.len() > 500 {
        mem.tabs.activity.clear();
    }
    mem.tabs
        .activity
        .insert(file.to_path_buf(), (modified, found));
    found
}

/// The last conversation entry's time in a transcript's end.
pub fn activity_in(file: &Path) -> Option<i64> {
    let (text, _) = tail_of(file, 256 * 1024)?;
    text.split('\n').rev().find_map(|line| {
        if !line.contains(r#""type":"user""#) && !line.contains(r#""type":"assistant""#) {
            return None;
        }
        let entry: Value = serde_json::from_str(line).ok()?;
        matches!(
            entry.get("type").and_then(Value::as_str),
            Some("user" | "assistant")
        )
        .then(|| time_of(&entry))
        .flatten()
    })
}

/// Claude's last words in a transcript's last turn.
pub fn last_said(file: &Path) -> Option<String> {
    texts_after(&last_prompt(file)?.after).pop()
}

/// The messages queued into a turn while it ran, in order: only what you
/// typed, not a finished background task's notification or a subagent's
/// report, which Claude Code queues the same way.
pub fn queued_in(after: &[Value]) -> Vec<String> {
    after
        .iter()
        .filter_map(|e| {
            let a = e.get("attachment")?;
            let kind = a
                .pointer("/origin/kind")
                .and_then(Value::as_str)
                .unwrap_or("human");
            let typed = kind == "human"
                && a.get("commandMode").and_then(Value::as_str) != Some("task-notification");
            (e.get("type").and_then(Value::as_str) == Some("attachment")
                && a.get("type").and_then(Value::as_str) == Some("queued_command")
                && typed)
                .then(|| a.get("prompt")?.as_str().map(str::to_string))
                .flatten()
        })
        .collect()
}

/// The last turn in a transcript, or `None` when it has no prompt yet or,
/// given `expected`, its last prompt isn't that text (nor one queued into
/// its turn). With `last` (the turn's final message, from the Stop hook), a
/// transcript that hasn't caught up yet still ends with it.
pub fn turn_from(file: &Path, expected: Option<&str>, last: Option<&str>) -> Option<String> {
    let LastPrompt {
        prompt,
        background,
        sent_at,
        after,
    } = last_prompt(file)?;
    let queued = queued_in(&after);
    if let Some(expected) = expected
        && !prompt_matches(&prompt, expected)
        && !queued.iter().any(|q| prompt_matches(q, expected))
    {
        return None;
    }
    let mut tools = 0;
    let mut said: Vec<String> = Vec::new();
    for entry in &after {
        if entry.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        for part in entry
            .pointer("/message/content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match part.get("type").and_then(Value::as_str) {
                Some("tool_use") => tools += 1,
                Some("text") => {
                    let t = part.get("text").and_then(Value::as_str).unwrap_or("");
                    if !t.trim().is_empty() {
                        said.push(t.trim().to_string());
                    }
                }
                _ => {}
            }
        }
    }
    let quoted = std::iter::once(prompt.as_str())
        .chain(queued.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join("\n")
        .split('\n')
        .map(|l| format!("> {l}"))
        .collect::<Vec<_>>()
        .join("\n");
    if let Some(last) = last.filter(|l| !l.trim().is_empty())
        && !same_text(said.last().map(String::as_str), Some(last))
    {
        said.push(last.trim().to_string());
    }
    let reply = if said.is_empty() {
        NO_REPLY.to_string()
    } else {
        said.join("\n\n")
    };
    let ago = sent_at
        .map(|sent| {
            format!(
                "_({} {})_",
                if background {
                    "a background task finished"
                } else {
                    "prompt sent"
                },
                relative(util::now_ms() - sent)
            )
        })
        .unwrap_or_default();
    let tools = if tools > 0 {
        format!(
            "\n\n_(ran {tools} tool{})_",
            if tools == 1 { "" } else { "s" }
        )
    } else {
        String::new()
    };
    Some(
        format!("{ago}\n{quoted}\n\n{reply}{tools}")
            .trim()
            .to_string(),
    )
}

/// Whether a character draws a rule (box-drawing, ═, —, _, =, ~, ., ·, -).
fn rule_char(c: char) -> bool {
    ('\u{2500}'..='\u{257F}').contains(&c) || "═—_=~.·-".contains(c)
}

/// A run of six or more of one rule character cut to three: Claude Code
/// draws its rules as a full-width line of ─ or ╌, 270 of them on a wide
/// tab, which say nothing, and Haiku can't copy hundreds of them exactly.
pub fn cut_rules(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let mut j = i + 1;
        while j < chars.len() && chars[j] == c {
            j += 1;
        }
        let run = j - i;
        if rule_char(c) && run >= 6 {
            out.extend(std::iter::repeat_n(c, 3));
        } else {
            out.extend(std::iter::repeat_n(c, run));
        }
        i = j;
    }
    out
}

/// A tab's screen: its last `SCREEN_LINES` lines with anything on them,
/// rules cut short.
pub fn screen(cx: &Relay, id: &str) -> String {
    let pane = pane_for(cx, id);
    let out = cx.mux.capture(&pane, SCREEN_LINES * 2).unwrap_or_default();
    screen_of(&out)
}

pub fn screen_of(captured: &str) -> String {
    let lines: Vec<String> = captured
        .split('\n')
        .map(|l| cut_rules(l).trim_end().to_string())
        .filter(|l| !l.trim().is_empty())
        .collect();
    lines[lines.len().saturating_sub(SCREEN_LINES)..].join("\n")
}

/// What a prompt shows, as Markdown: its question, its options (ticked or
/// not), and how to answer it from Slack.
pub fn prompt_text(found: &Prompt) -> String {
    let boxed = |o: &PromptOption| match o.checked {
        None => "",
        Some(true) => "☑ ",
        Some(false) => "☐ ",
    };
    let hint = if found.multi {
        "Tap options to tick or untick them, then Submit choices. Or reply with their numbers, and `submit` when you're done."
    } else {
        "Tap an option, or reply with its number or the start of it."
    };
    let mut lines = found.question.split('\n');
    let heading = lines.next().unwrap_or("");
    let mut out = String::new();
    if !heading.is_empty() {
        out.push_str(&format!("**{heading}**\n"));
    }
    for line in lines {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(
        &found
            .options
            .iter()
            .map(|o| format!("{}. {}{}", o.n, boxed(o), o.label))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let free = found
        .free
        .map(|n| format!(" Anything else you reply is typed in as option {n}, your own answer."))
        .unwrap_or_default();
    out.push_str(&format!("\n_{hint}{free}_"));
    out
}

/// A short fingerprint of a prompt or dialog: its question and every
/// option. A button records the one it was made for, so a press only ever
/// answers that same prompt.
pub fn prompt_hash(question: &str, options: &[PromptOption]) -> String {
    let text = format!(
        "{question}\n{}",
        options
            .iter()
            .map(|o| format!("{}. {}", o.n, o.label))
            .collect::<Vec<_>>()
            .join("\n")
    );
    crate::tabs::color::sha256(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()[..16]
        .to_string()
}

pub fn hash_of(found: &Prompt) -> String {
    prompt_hash(&found.question, &found.options)
}

/// A dialog Claude Code shows before a session starts ("trust this
/// folder", "new MCP server found"): options listed one per line,
/// unnumbered, the current one marked ❯, above an "Enter to confirm" line.
#[derive(Clone, Debug, PartialEq)]
pub struct Dialog {
    pub question: String,
    pub options: Vec<PromptOption>,
    pub cursor: u32,
}

impl Dialog {
    pub fn hash(&self) -> String {
        prompt_hash(&self.question, &self.options)
    }
}

/// The dialog a screen shows, or `None`. No hook runs before the session
/// starts, so a tab showing one has no state; it is read from the screen.
/// The options are the lines aligned with the text after ❯, so the
/// question and notes above them, set less deep, aren't taken for one.
pub fn dialog_in(text: &str) -> Option<Dialog> {
    let all: Vec<&str> = text.split('\n').collect();
    let lines: Vec<Vec<char>> = all
        .iter()
        .enumerate()
        .filter(|(i, l)| !l.trim().is_empty() && (*i == 0 || all[i - 1] != **l))
        .map(|(_, l)| l.chars().collect())
        .collect();
    let joined = |l: &[char]| l.iter().collect::<String>();
    let footer = lines
        .iter()
        .rposition(|l| joined(l).contains("Enter to confirm"))?;
    if footer + 2 < lines.len() {
        return None;
    }
    let marked = |l: &[char]| {
        l.iter().position(|c| *c == '❯').is_some_and(|at| {
            l[at + 1..].first().is_some_and(|c| c.is_whitespace())
                && l[at + 1..].iter().any(|c| !c.is_whitespace())
        })
    };
    let cursor_line = lines[..footer].iter().rposition(|l| marked(l))?;
    let line = &lines[cursor_line];
    let mark = line.iter().position(|c| *c == '❯')?;
    let column = mark + 1 + line[mark + 1..].iter().position(|c| !c.is_whitespace())?;
    let is_option = |l: &[char]| {
        l.iter().position(|c| *c == '❯') == Some(mark)
            || (l.len() > column
                && l[..column].iter().all(|c| c.is_whitespace())
                && l[column] != ' ')
    };
    let mut start = cursor_line;
    while start > 0 && is_option(&lines[start - 1]) {
        start -= 1;
    }
    let mut end = cursor_line;
    while end + 1 < footer && is_option(&lines[end + 1]) {
        end += 1;
    }
    let options: Vec<PromptOption> = lines[start..=end]
        .iter()
        .enumerate()
        .map(|(i, l)| PromptOption {
            n: i as u32 + 1,
            label: joined(l).replacen('❯', "", 1).trim().to_string(),
            checked: None,
        })
        .collect();
    if options.len() < 2 {
        return None;
    }
    static RULE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[\s\x{2500}-\x{257F}—-]+$").expect("a regex"));
    let mut above: Vec<String> = Vec::new();
    let mut i = start;
    while i > 0 && above.len() < 3 {
        i -= 1;
        let l = joined(&lines[i]);
        if RULE.is_match(&l) {
            break;
        }
        above.insert(0, l.trim().to_string());
    }
    Some(Dialog {
        question: above.join(" "),
        options,
        cursor: (cursor_line - start) as u32 + 1,
    })
}

/// Your answer as one of a dialog's options.
pub fn chosen_option(options: &[PromptOption], text: &str) -> Option<PromptOption> {
    answer::chosen_option(options, text)
}

/// The lines of a screen a post shows under "On screen now".
pub const PROMPT_LINES: usize = 20;

/// What a waiting tab is showing: a permission prompt is not in the
/// transcript, only on the screen, and its numbered options are what you
/// answer with. Nothing when `turn`, what the message already shows, is the
/// screen itself.
pub fn on_screen(cx: &Relay, id: &str, turn: &str) -> String {
    let shown = screen(cx, id);
    if let Some(found) = options_in(&shown) {
        return format!("\n{}", prompt_text(&found));
    }
    if turn.starts_with("```") {
        return String::new();
    }
    let lines: Vec<&str> = shown.split('\n').collect();
    let lines = lines[lines.len().saturating_sub(PROMPT_LINES)..].join("\n");
    if lines.is_empty() {
        String::new()
    } else {
        format!("\nOn screen now:\n```\n{lines}\n```")
    }
}

/// How long a paste is given to land before Enter follows it.
pub const PASTE_SETTLE: Duration = Duration::from_millis(answer::PASTE_SETTLE_MS);

/// Presses what an answer (from `answer_for`) says, in a pane showing
/// `found`.
pub async fn answer_prompt(cx: &Relay, pane: &str, found: &Prompt, answer: &Answer) {
    press(cx, pane, answer::presses(found, answer)).await;
}

/// Presses keys and text in a pane, a paste given a moment to land.
pub async fn press(cx: &Relay, pane: &str, presses: Vec<Press>) {
    for p in presses {
        let _ = match p {
            Press::Key(k) => cx.mux.send_keys(pane, &[&k.notation()]),
            Press::Text(t) => cx.mux.type_text(pane, &t),
            Press::Paste(t) => cx.mux.paste(pane, &t),
            Press::Settle => {
                tokio::time::sleep(PASTE_SETTLE).await;
                Ok(())
            }
        };
    }
}

/// Your message goes in as one paste, then Enter sends it once the paste
/// has landed: typed key by key, each line break reached Claude Code as a
/// keypress, and Enter sent at once can reach it before it has taken the
/// paste in, leaving the text unsent in its input box.
pub async fn type_into(cx: &Relay, pane: &str, text: &str) {
    press(cx, pane, answer::message_presses(text)).await;
}

/// The screen once it has changed from `before`, after an answer, or as it
/// is after `wait` if it hasn't: what comes next takes Claude Code a moment
/// to draw.
pub async fn screen_after(cx: &Cx, id: &str, before: &str, wait: Duration) -> String {
    const SETTLE: Duration = Duration::from_millis(250);
    let until = std::time::Instant::now() + wait;
    while std::time::Instant::now() < until && screen(cx, id) == before {
        tokio::time::sleep(SETTLE).await;
    }
    tokio::time::sleep(SETTLE).await;
    screen(cx, id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write(dir: &Path, name: &str, entries: &[Value]) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let file = dir.join(name);
        let text: Vec<String> = entries.iter().map(|e| e.to_string()).collect();
        std::fs::write(&file, text.join("\n")).unwrap();
        file
    }

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("relay-tabs-{name}-{}", std::process::id()))
    }

    #[test]
    fn a_turn_shows_the_last_prompt_the_reply_and_a_tool_count() {
        let dir = temp("turn");
        let sent = jiff::Timestamp::from_millisecond(util::now_ms() - 5 * 60_000)
            .unwrap()
            .to_string();
        let file = write(
            &dir,
            "turn.jsonl",
            &[
                json!({ "type": "user", "message": { "content": "an earlier prompt" } }),
                json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "earlier reply" }] } }),
                json!({ "type": "user", "timestamp": sent, "message": { "content": [{ "type": "text", "text": "run   the\ntests" }] } }),
                json!({ "type": "user", "isMeta": true, "message": { "content": "meta" } }),
                json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "Running them." }, { "type": "tool_use", "id": "t1" }] } }),
                json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": "ok" }] } }),
                json!({ "type": "assistant", "isSidechain": true, "message": { "content": [{ "type": "text", "text": "a subagent" }] } }),
                json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "All pass." }] } }),
            ],
        );
        assert_eq!(
            turn_from(&file, None, None).unwrap(),
            "_(prompt sent 5m ago)_\n> run   the\n> tests\n\nRunning them.\n\nAll pass.\n\n_(ran 1 tool)_"
        );
        assert!(turn_from(&file, Some("run the tests"), None).is_some());
        assert!(turn_from(&file, Some("something else"), None).is_none());
        assert!(turn_from(&dir.join("missing.jsonl"), None, None).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rules_are_cut_to_a_few_characters() {
        assert_eq!(cut_rules(&"─".repeat(270)), "───");
        assert_eq!(cut_rules("a ----- b ------ c"), "a ----- b --- c");
        assert_eq!(cut_rules("aaaaaaaa"), "aaaaaaaa", "only rule characters");
    }

    #[test]
    fn dialogs_before_a_session_starts_are_read() {
        // Both captured from real tabs.
        let trust = [
            "─".repeat(60).as_str(),
            " Accessing workspace:",
            " /work/new-repo",
            " Quick safety check: Is this a project you created or one you trust?",
            " Claude Code'll be able to read, edit, and execute files here.",
            " Security guide",
            " ❯ No, exit",
            "   Yes, I trust this folder",
            " Enter to confirm · Esc to cancel",
        ]
        .join("\n");
        let found = dialog_in(&trust).unwrap();
        assert_eq!(
            found.question,
            "Quick safety check: Is this a project you created or one you trust? Claude Code'll be able to read, edit, and execute files here. Security guide"
        );
        assert_eq!(
            found
                .options
                .iter()
                .map(|o| o.label.as_str())
                .collect::<Vec<_>>(),
            ["No, exit", "Yes, I trust this folder"]
        );
        assert_eq!(found.cursor, 1);
        let mcp = [
            "  New MCP server found in this project: some-server",
            "  MCP servers may execute code or access system resources. All tool calls require approval.",
            "    Use this MCP server",
            "    Use this and all future MCP servers in this project",
            "  ❯ Continue without using this MCP server",
            "  Enter to confirm · Esc to cancel",
        ]
        .join("\n");
        let found = dialog_in(&mcp).unwrap();
        assert_eq!(
            found
                .options
                .iter()
                .map(|o| o.label.as_str())
                .collect::<Vec<_>>(),
            [
                "Use this MCP server",
                "Use this and all future MCP servers in this project",
                "Continue without using this MCP server"
            ]
        );
        assert_eq!(found.cursor, 3);
        // Not at the bottom of the screen: an old dialog, already answered.
        assert!(dialog_in(&format!("{trust}\n● Working on it\n> next\n✻ Thinking")).is_none());
    }

    #[test]
    fn last_activity_is_the_last_conversation_entry_not_bookkeeping() {
        let dir = temp("activity");
        let file = write(
            &dir,
            "a.jsonl",
            &[
                json!({ "type": "user", "timestamp": "2026-01-01T10:00:00.000Z", "message": { "content": "go" } }),
                json!({ "type": "assistant", "timestamp": "2026-01-01T10:05:00.000Z", "message": { "content": [] } }),
                json!({ "type": "permission-mode", "timestamp": "2026-01-01T11:00:00.000Z" }),
            ],
        );
        assert_eq!(
            activity_in(&file),
            Some(
                "2026-01-01T10:05:00Z"
                    .parse::<jiff::Timestamp>()
                    .unwrap()
                    .as_millisecond()
            )
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_long_message_kept_as_pasted_text_is_still_matched() {
        let typed = "please fix   the login\nbug and add a test for it too";
        let stored = format!("<pasted_content id=\"1\">\n{typed}\n</pasted_content>");
        assert!(prompt_matches(&stored, typed));
        assert!(
            prompt_matches(&format!("left in the box {typed}"), typed),
            "sent along with text left in the input box"
        );
        assert!(!prompt_matches("something else", typed));
    }

    #[test]
    fn a_background_tasks_notification_shows_as_its_summary() {
        let dir = temp("task");
        let file = write(
            &dir,
            "t.jsonl",
            &[
                json!({ "type": "user", "origin": { "kind": "task-notification" }, "message": { "content": "<task-notification><task-id>1</task-id><summary>Tests passed</summary></task-notification>" } }),
                json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "Good." }] } }),
            ],
        );
        let p = last_prompt(&file).unwrap();
        assert_eq!(p.prompt, "Tests passed");
        assert!(p.background);
        let peer = write(
            &dir,
            "p.jsonl",
            &[
                json!({ "type": "user", "origin": { "kind": "peer" }, "message": { "content": "<agent-message>Framing. The report follows: all   done</agent-message>" } }),
            ],
        );
        assert_eq!(
            last_prompt(&peer).unwrap().prompt,
            "An agent reported back: all done"
        );
        let slash = write(
            &dir,
            "s.jsonl",
            &[
                json!({ "type": "user", "message": { "content": "<command-name>/review-loop</command-name><command-args></command-args>" } }),
            ],
        );
        assert_eq!(last_prompt(&slash).unwrap().prompt, "/review-loop");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_message_sent_mid_turn_is_matched_to_that_turn() {
        let dir = temp("queued");
        let file = write(
            &dir,
            "q.jsonl",
            &[
                json!({ "type": "user", "message": { "content": "first thing" } }),
                json!({ "type": "attachment", "attachment": { "type": "queued_command", "prompt": "and the second" } }),
                json!({ "type": "attachment", "attachment": { "type": "queued_command", "prompt": "<task-notification>x</task-notification>", "commandMode": "task-notification" } }),
                json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "Both done." }] } }),
            ],
        );
        let turn = turn_from(&file, Some("and the second"), None).unwrap();
        assert_eq!(turn, "> first thing\n> and the second\n\nBoth done.");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_turn_ending_at_a_tool_call_is_not_written_yet() {
        let dir = temp("written");
        let file = write(
            &dir,
            "w.jsonl",
            &[
                json!({ "type": "user", "message": { "content": "go" } }),
                json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "Looking." }, { "type": "tool_use", "id": "t" }] } }),
            ],
        );
        assert!(!turn_written(&file));
        std::fs::write(
            &file,
            format!(
                "{}\n{}",
                std::fs::read_to_string(&file).unwrap(),
                json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "Done." }] } })
            ),
        )
        .unwrap();
        assert!(turn_written(&file));
        assert!(caught_up(&file, "Done."));
        assert!(!caught_up(&file, "Something else."));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_prompt_reads_with_its_options_and_how_to_answer() {
        let found = options_in("Do you want to proceed?\n❯ 1. Yes\n  2. No").unwrap();
        assert_eq!(
            prompt_text(&found),
            "**Do you want to proceed?**\n1. Yes\n2. No\n_Tap an option, or reply with its number or the start of it._"
        );
        assert_eq!(hash_of(&found).len(), 16);
    }
}
