//! The tabs running Claude, as the relay reads and types into them
//! (lib/slack-relay/tabs.js): each tab's Claude pane, its transcript and
//! last turn, its screen, and the prompts and dialogs it shows, which the
//! relay answers, and messages it sends, through the server's `answer`
//! (src/remote).
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
//! The prompts are read with the one reading of Claude's screen
//! (src/agent/screen.rs, which was ported from here).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime};

use regex::Regex;
use serde_json::Value;

use super::buttons::option_text;
use super::config::{Paths, SCREEN_LINES};
use super::mux::TabStage;
use super::post::BUTTON_TEXT;
use super::{Cx, Relay};
#[cfg(test)]
use crate::agent::screen::hash_of;
use crate::agent::screen::{self, Prompt, PromptOption, draft_in, options_in};
use crate::agent::transcript::{self, Entry, Kind};
use crate::ui::when::{Said, ago};
use crate::util;

pub use crate::remote::protocol::CLAUDE_PANE;

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
    /// Opened by autopilot (`@autopilot`), for `relay_send_autopilot`.
    pub autopilot: bool,
    pub pane: String,
    pub pid: String,
    pub pids: Vec<String>,
    pub transcript: Option<PathBuf>,
    pub own_name: String,
    pub cwd: Option<PathBuf>,
    /// Its stage in the sidebar, which the Home groups by.
    pub stage: Option<TabStage>,
    pub remind_at: Option<i64>,
    /// Its Claude's address for another session's messages, or empty.
    pub address: String,
    /// Its direct landing, with the step its Claude pane last named.
    pub landing: Option<super::landing::LandingAt>,
    pub pr: Option<crate::facts::PrFacts>,
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
            autopilot: w.autopilot,
            pane: chosen.id.clone(),
            pid: chosen.pid.clone(),
            pids: claude.iter().map(|p| p.pid.clone()).collect(),
            transcript: chosen.transcript.clone(),
            own_name: w.own_name.clone(),
            cwd: chosen.cwd.clone(),
            stage: w.stage.clone(),
            remind_at: w.remind_at,
            address: chosen.address.clone(),
            landing: w.landing.clone().map(|l| super::landing::LandingAt {
                phase: chosen.phase.clone(),
                ..l
            }),
            pr: w.pr.clone(),
        });
    }
    note_senders(&tabs);
    let mut mem = cx.mem.borrow_mut();
    mem.tabs.panes = tabs
        .iter()
        .map(|t| (t.id.clone(), t.pane.clone()))
        .collect();
    tabs.sort_by_key(|t| t.index);
    tabs
}

/// The tabs' names by their Claude's address, as the last look at the tabs
/// saw them, for naming the tab whose message started another's turn: a
/// transcript names its sender only by address.
static SENDERS: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);

fn note_senders(tabs: &[Tab]) {
    let named = tabs
        .iter()
        .filter(|t| !t.address.is_empty())
        .map(|t| (t.address.clone(), format!("Tab {}, {}", t.index, t.name)));
    if let Ok(mut senders) = SENDERS.lock() {
        senders.extend(named);
    }
}

/// The tab whose Claude has this address, as the tab list names it.
fn sender_named(address: &str) -> Option<String> {
    SENDERS.lock().ok()?.get(address).cloned()
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
    let Some(window) = windows.iter().find(|w| w.id == id) else {
        return Err("That tab no longer exists.".into());
    };
    // What the tab's active pane runs, as tmux's `pane_current_command`
    // for the window named it.
    let command = window
        .panes
        .iter()
        .find(|p| p.active)
        .or(window.panes.first())
        .map(|p| p.command.as_str())
        .unwrap_or("");
    if command.is_empty() {
        Err("That tab isn't running Claude, so nothing was sent.".into())
    } else {
        Err(format!(
            "That tab is running {command}, not Claude, so nothing was sent."
        ))
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
fn texts_after(after: &[Entry]) -> Vec<String> {
    after
        .iter()
        .filter(|e| e.is(Kind::Assistant))
        .flat_map(Entry::texts)
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
    let Some(last) = after.iter().rev().find(|e| e.is_said()) else {
        return false;
    };
    last.is(Kind::Assistant)
        && last.texts().any(|t| !t.trim().is_empty())
        && last.parts_of("tool_use").next().is_none()
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
    pub after: Vec<Entry>,
}

/// How much of a transcript's end is read for its last prompt at first:
/// a long session's transcript runs to hundreds of megabytes.
const PROMPT_TAIL: u64 = 1024 * 1024;

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
    transcript::from_end(file, PROMPT_TAIL, u64::MAX, |text, _| {
        // Whether the last reply so far waits on a tool, so its turn runs on,
        // until something other than the tool's result is said (Esc's
        // interruption, a prompt of yours).
        let mut running = false;
        let entries: Vec<Entry> = transcript::entries(text)
            .filter(|e| !e.sidechain())
            .filter(|e| {
                match e.kind() {
                    Kind::Assistant => {
                        running = e.pointer("/message/stop_reason").and_then(Value::as_str)
                            == Some("tool_use");
                    }
                    Kind::User if !e.meta() => running &= e.results_only(),
                    _ => {}
                }
                // Another session's message arrives as a meta entry once a
                // turn has ended, and starts one all the same (one that comes
                // in while a turn runs is queued into it).
                let starts_a_turn = !running && e.origin() == "peer";
                starts_a_turn || !e.meta()
            })
            .collect();
        let start = entries
            .iter()
            .rposition(|e| e.is(Kind::User) && e.typed("\n").is_some())?;
        let entry = &entries[start];
        let raw = entry.typed("\n").unwrap_or_default();
        let after = entries[start + 1..].to_vec();
        let sent_at = entry.time();
        let origin = Some(entry.origin()).filter(|o| !o.is_empty());
        // A subagent's report back starts a turn (origin peer), in the
        // harness's framing: it reads as the report, cut short. Known by its
        // origin alone, never its text, which a prompt of yours may quote.
        if origin == Some("peer") {
            let cut = |text: &str| {
                let text = flat(text);
                if text.chars().count() > 200 {
                    format!("{}…", text.chars().take(200).collect::<String>())
                } else {
                    text
                }
            };
            // Another session's message (SendMessage), by its sender.
            if let Some(body) = entry.pointer("/origin/body").and_then(Value::as_str) {
                let sender = entry
                    .pointer("/origin/from")
                    .and_then(Value::as_str)
                    .and_then(sender_named)
                    .unwrap_or_else(|| "Another tab".to_string());
                return Some(LastPrompt {
                    prompt: format!("{sender} sent: {}", cut(body)),
                    background: true,
                    sent_at,
                    after,
                });
            }
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
            return Some(LastPrompt {
                prompt: format!("An agent reported back: {}", cut(&report)),
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
        Some(LastPrompt {
            prompt,
            background,
            sent_at,
            after,
        })
    })
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
    let tail = 256 * 1024;
    transcript::from_end(file, tail, tail, |text, _| {
        text.split('\n').rev().find_map(|line| {
            if !transcript::may_be(line, Kind::User) && !transcript::may_be(line, Kind::Assistant) {
                return None;
            }
            Entry::parse(line).filter(Entry::is_said)?.time()
        })
    })
}

/// Who a transcript's last turn is for: whether you started it (a prompt
/// typed in the pane or sent from Slack, or one of yours queued into it),
/// rather than another tab's message or a background task, and whether
/// Claude wrote anything in it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TurnFor {
    pub yours: bool,
    pub replied: bool,
}

pub fn turn_for(file: &Path) -> Option<TurnFor> {
    let last = last_prompt(file)?;
    Some(TurnFor {
        yours: !last.background || !queued_in(&last.after).is_empty(),
        replied: !texts_after(&last.after).is_empty(),
    })
}

/// Claude's last words in a transcript's last turn.
pub fn last_said(file: &Path) -> Option<String> {
    texts_after(&last_prompt(file)?.after).pop()
}

/// The messages queued into a turn while it ran, in order: only what you
/// typed, not a finished background task's notification or a subagent's
/// report, which Claude Code queues the same way.
pub fn queued_in(after: &[Entry]) -> Vec<String> {
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
            (e.is(Kind::Attachment)
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
    for entry in after.iter().filter(|e| e.is(Kind::Assistant)) {
        for part in entry.parts() {
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
                ago(Some(sent / 1000), util::now_ms() / 1000, Said::Ago)
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

/// A turn (`turn_from`'s) without the prompt it quotes: an alert for a
/// tab waiting on you leads with what Claude said and what it asks, not
/// your own words back. A turn read from the screen is left as it is.
pub fn unquoted(turn: &str) -> String {
    let mut lines = turn.split('\n').peekable();
    let mut out: Vec<&str> = Vec::new();
    if let Some(first) = lines.next_if(|l| l.starts_with("_(")) {
        out.push(first);
    }
    let mut quoted = false;
    while lines
        .next_if(|l| l.starts_with("> ") || *l == ">")
        .is_some()
    {
        quoted = true;
    }
    if quoted {
        lines.next_if(|l| l.is_empty());
    }
    out.extend(lines);
    out.join("\n").trim().to_string()
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

/// What is typed and unsent in a pane's prompt box (`draft_in`).
pub fn draft(cx: &Relay, pane: &str) -> Option<String> {
    draft_in(&cx.mux.capture(pane, 0)?, &cx.mux.capture_typed(pane)?)
}

/// What a prompt shows, as Markdown, over its buttons: its question and
/// one line on how to answer it from Slack. The options are the buttons
/// alone, each with its number; only one whose label is too long for its
/// button is written out here too, in full.
pub fn prompt_text(found: &Prompt) -> String {
    let hint = if found.multi {
        "Tap options to tick or untick them, then Submit choices. Or reply with their numbers, and `submit` when you're done."
    } else {
        "Tap one, or reply with its number."
    };
    let too_long = |o: &&PromptOption| option_text(o).chars().count() > BUTTON_TEXT;
    prompt_with(found, too_long, hint)
}

/// What a prompt shows in a message without its buttons (a warning that a
/// tab hasn't taken your message, or may be stuck): its question, every
/// option, and how to answer it by replying.
pub fn prompt_listed(found: &Prompt) -> String {
    let hint = if found.multi {
        "Reply with their numbers, and `submit` when you're done."
    } else {
        "Reply with its number or the start of it."
    };
    prompt_with(found, |_| true, hint)
}

/// A prompt's question, the options `shown` picks, and a line saying how
/// to answer it.
fn prompt_with(found: &Prompt, shown: impl Fn(&&PromptOption) -> bool, hint: &str) -> String {
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
    for o in found
        .options
        .iter()
        .filter(|o| Some(o.n) != found.free)
        .filter(shown)
    {
        out.push_str(&format!("{}\n", option_text(o)));
    }
    let free = found
        .free
        .map(|_| " Anything else you reply is typed in as your own answer.")
        .unwrap_or_default();
    out.push_str(&format!("_{hint}{free}_"));
    out
}

/// Your answer as one of a dialog's options.
pub fn chosen_option(options: &[PromptOption], text: &str) -> Option<PromptOption> {
    screen::chosen_option(options, text)
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
        return format!("\n\n{}", prompt_listed(&found));
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

/// Text from Slack as it may be typed into a pane, as the phone's is.
pub use crate::remote::places::typeable;

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
        let message = |from: &str| json!({ "type": "user", "isMeta": true, "timestamp": "2026-11-12T04:44:00Z", "origin": { "kind": "peer", "from": from, "name": "proposer", "body": "Here is   my\nproposal" }, "message": { "content": "Another Claude session sent a message:\n<cross-session-message>Here is my proposal</cross-session-message>" } });
        let messaged = write(
            &dir,
            "m.jsonl",
            &[
                json!({ "type": "user", "message": { "content": "walk me through it" } }),
                json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "The walkthrough." }] } }),
                message("uds:/tmp/unknown.sock"),
                json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "The proposal looks right." }] } }),
            ],
        );
        let p = last_prompt(&messaged).unwrap();
        assert_eq!(p.prompt, "Another tab sent: Here is my proposal");
        assert!(p.background);
        let turn = turn_from(&messaged, None, None).unwrap();
        assert!(turn.contains("The proposal looks right."), "{turn}");
        assert!(!turn.contains("The walkthrough."), "{turn}");
        assert!(turn_from(&messaged, Some("walk me through it"), None).is_none());
        // One that comes in while a turn waits on a tool stays in that turn.
        let midway = write(
            &dir,
            "mid.jsonl",
            &[
                json!({ "type": "user", "message": { "content": "walk me through it" } }),
                json!({ "type": "assistant", "message": { "stop_reason": "tool_use", "content": [{ "type": "text", "text": "Reading." }, { "type": "tool_use", "id": "t1" }] } }),
                message("uds:/tmp/unknown.sock"),
                json!({ "type": "assistant", "message": { "stop_reason": "end_turn", "content": [{ "type": "text", "text": "The walkthrough." }] } }),
            ],
        );
        let turn = turn_from(&midway, None, None).unwrap();
        assert!(turn.contains("> walk me through it"), "{turn}");
        assert!(turn.contains("Reading."), "{turn}");
        // One after Esc interrupted the call starts a turn of its own.
        let interrupted = write(
            &dir,
            "esc.jsonl",
            &[
                json!({ "type": "user", "message": { "content": "walk me through it" } }),
                json!({ "type": "assistant", "message": { "stop_reason": "tool_use", "content": [{ "type": "tool_use", "id": "t1" }] } }),
                json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": "denied" }] } }),
                json!({ "type": "user", "message": { "content": [{ "type": "text", "text": "[Request interrupted by user for tool use]" }] } }),
                message("uds:/tmp/unknown.sock"),
                json!({ "type": "assistant", "message": { "stop_reason": "end_turn", "content": [{ "type": "text", "text": "The proposal looks right." }] } }),
            ],
        );
        assert_eq!(
            last_prompt(&interrupted).unwrap().prompt,
            "Another tab sent: Here is my proposal"
        );
        note_senders(&[Tab {
            index: 3,
            name: "design".into(),
            address: "uds:/tmp/tab-three.sock".into(),
            ..Tab::default()
        }]);
        let named = write(&dir, "n.jsonl", &[message("uds:/tmp/tab-three.sock")]);
        assert_eq!(
            last_prompt(&named).unwrap().prompt,
            "Tab 3, design sent: Here is my proposal"
        );
        let meta = write(
            &dir,
            "meta.jsonl",
            &[
                json!({ "type": "user", "message": { "content": "typed" } }),
                json!({ "type": "user", "isMeta": true, "message": { "content": "a reminder" } }),
            ],
        );
        assert_eq!(last_prompt(&meta).unwrap().prompt, "typed");
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
    fn a_turn_unquoted_leaves_out_only_the_prompt_it_quotes() {
        assert_eq!(
            unquoted(
                "_(prompt sent 2m ago)_\n> first\n> \n> second\n\nDone.\n> a quote of Claude's"
            ),
            "_(prompt sent 2m ago)_\nDone.\n> a quote of Claude's"
        );
        assert_eq!(unquoted("> deploy it\n\nDeployed."), "Deployed.");
        assert_eq!(unquoted("```\nscreen\n```"), "```\nscreen\n```");
    }

    #[test]
    fn a_prompt_reads_with_its_options_and_how_to_answer() {
        let found = options_in("Do you want to proceed?\n❯ 1. Yes\n  2. No").unwrap();
        assert_eq!(
            prompt_text(&found),
            "**Do you want to proceed?**\n_Tap one, or reply with its number._"
        );
        let long =
            "No, and tell Claude what to do differently, since this path is not the one meant";
        let found = options_in(&format!("Do you want to proceed?\n❯ 1. Yes\n  2. {long}")).unwrap();
        assert_eq!(
            prompt_text(&found),
            format!("**Do you want to proceed?**\n2. {long}\n_Tap one, or reply with its number._"),
            "only an option too long for its button is written out"
        );
        assert_eq!(hash_of(&found).len(), 16);
    }
}
