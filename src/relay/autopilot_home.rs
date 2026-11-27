//! Autopilot's progress on the Home tab (UX 2.0 board 81). No file of the
//! Node relay is behind it.
//!
//! While autopilot works, the Home tab has a section of its own between
//! its Next buttons and Needs you: whether it works or is paused (by you,
//! since when, or by its spend or weekly limit), when the note's ready items should
//! be done, how many items stand in each column of the Autopilot view,
//! today's spend against its limit, its tabs in flight (waiting on you,
//! landing, working), each linking to its thread, and its buttons: Pause
//! (Resume while paused by you, Raise the limit while its spend or weekly
//! limit stopped it) and Settings, which opens board 52's form to change it.
//!
//! It is read from the server's `autopilot-board` at most once a
//! `EVERY_MS`, but at once when the Home tab is opened or one of its
//! buttons pressed, so the Home tab, which is published only when what it
//! shows changes, is edited in place no more than once a minute for it
//! and nothing new is posted; a landing or an item waiting on you still
//! reaches you as its own message by the relay's settings.
//!
//! The note is named only with Settings' "Notes in Slack" on, else it is
//! "your note"; a tab in flight goes by its tab's name, which its thread
//! already says, never by its item, which the board doesn't send. Settings' "Autopilot in Slack" (`relay_autopilot`) off
//! leaves the section out. Its buttons, like every button, are taken
//! only from the owner (`slack::press_from`).

use serde_json::{Value, json};

use super::Cx;
use super::homes::{home_thread, thread_url};
use super::post::{Button, button_element};
use super::tabs::{self, Tab};
use crate::sidebar::text;
use crate::ui::when::{Said, ago};
use crate::util;

/// The setting that turns the section off.
pub const SETTING: &str = "relay_autopilot";

/// How long the board read is kept before it is read again.
pub const EVERY_MS: i64 = 60_000;

/// The most tabs in flight listed.
const MOST_TABS: usize = 10;

/// The board as last read, and when.
#[derive(Default)]
pub struct Memory {
    pub board: Option<Value>,
    /// In ms; 0 before the first read.
    pub at: i64,
}

impl Memory {
    pub fn board(&self) -> Option<&Value> {
        self.board.as_ref()
    }
}

/// Whether the Home tab shows autopilot (on at first).
pub fn shown(cx: &Cx) -> bool {
    (cx.settings)(SETTING) != Some(Value::Bool(false))
}

/// The board read again from the server when `fresh` or `EVERY_MS` have
/// passed since the last read, else kept; nothing with the setting off.
pub fn read(cx: &Cx, fresh: bool) -> Option<Value> {
    if !shown(cx) {
        cx.mem.borrow_mut().autopilot = Memory::default();
        return None;
    }
    let now = util::now_ms();
    let due = {
        let mem = cx.mem.borrow();
        fresh || mem.autopilot.at == 0 || now - mem.autopilot.at >= EVERY_MS
    };
    if due {
        let board = cx.mux.autopilot_board();
        cx.mem.borrow_mut().autopilot = Memory { board, at: now };
    }
    cx.mem.borrow().autopilot.board.clone()
}

/// When the board was last read, in ms.
fn read_at(cx: &Cx) -> i64 {
    cx.mem.borrow().autopilot.at
}

fn mrkdwn(text: &str) -> Value {
    json!({ "type": "section", "text": { "type": "mrkdwn", "text": text } })
}

fn context(text: &str) -> Value {
    json!({ "type": "context", "elements": [{ "type": "mrkdwn", "text": text }] })
}

/// A dollar amount as the Usage view says it.
fn dollars(n: f64) -> String {
    format!("${n:.2}")
}

/// The spend meter: ten cells, filled to the share spent.
fn meter(share: f64) -> String {
    let filled = ((share * 10.0).floor().max(0.0) as usize).min(10);
    format!("{}{}", "▰".repeat(filled), "▱".repeat(10 - filled))
}

/// An ordinal, for a place in the landing queue.
fn ordinal(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (1, 11) | (2, 12) | (3, 13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// A tab in flight, as board 81 lists it: its mark, number and item,
/// where it stands and for how long.
fn flight_words(column: &str, tab: Option<&Tab>, now: i64) -> (&'static str, String) {
    let lasted_for = || {
        tab.and_then(|t| tabs::since_ms(&t.since))
            .map(|s| {
                format!(
                    " {}",
                    ago(Some(s / 1000), now / 1000, Said::Short).replace('<', "&lt;")
                )
            })
            .unwrap_or_default()
    };
    match column {
        "Waiting on you" => ("🟡", format!("waiting on you{}", lasted_for())),
        "Landing" => {
            let words = match tab.and_then(|t| t.landing.as_ref()) {
                Some(l) if l.holding => "landing".to_string(),
                Some(l) => format!("landing, {}", ordinal(l.line)),
                None => "landing".to_string(),
            };
            ("🟣", words)
        }
        _ => ("🔵", format!("working{}", lasted_for())),
    }
}

/// The section's blocks, or none while autopilot is off. `named` lets it
/// name the note; `tabs` are the Claude tabs, for each tab in flight's
/// thread and how long it has stood.
pub fn blocks(cx: &Cx, board: &Value, named: bool, tabs: &[Tab]) -> Vec<Value> {
    if board["on"] != json!(true) {
        return Vec::new();
    }
    let now = util::now_ms();
    let number = |k: &str| board[k].as_f64().filter(|n| n.is_finite()).unwrap_or(0.0);
    let (spent, limit) = (number("spent"), number("limit"));
    let paused = board["paused"] == json!(true);
    let spend_limited = board["limited"] == json!(true);
    // At the weekly limit: its percent and when the week resets (seconds).
    let weekly = board["weekly"].as_object().map(|w| {
        let n = |k: &str| w.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        (n("limit"), n("resetsAt") as i64)
    });
    let limited = spend_limited || weekly.is_some();
    let away = board["away"] == json!(true);
    let note = match board["note"].as_str().filter(|n| named && !n.is_empty()) {
        Some(n) => format!("*{n}*"),
        None => "your note".to_string(),
    };
    let head = if paused {
        "⏸ Autopilot, paused by you"
    } else if limited {
        "⏸ Autopilot, paused by its limit"
    } else if away {
        "⏸ Autopilot, paused while you're away"
    } else {
        "🤖 Autopilot"
    };
    let slots = board["slots"].as_u64().unwrap_or(0);
    let mut first = format!("Working {note}");
    if paused {
        if let Some(at) = board["pausedAt"].as_f64() {
            first.push_str(&format!(" · paused at {}", super::homes::when(at)));
        }
    } else if spend_limited {
        first.push_str(&format!(
            "\n🔴 Paused: today's spend reached {}",
            dollars(limit)
        ));
    } else if let Some((percent, resets_at)) = weekly {
        first.push_str(&format!(
            "\n🔴 Paused: the week reached {percent}% of Claude Code's weekly limit, resets {}",
            util::local_at_ms(resets_at * 1000).strftime("%a %-I:%M %p")
        ));
    } else {
        first.push_str(&format!(
            " · {slots} tab{} at once",
            if slots == 1 { "" } else { "s" }
        ));
    }
    let eta = board["eta"].as_str().unwrap_or("");
    let mut when = String::new();
    if let Some(c) = eta.chars().next() {
        when = c.to_uppercase().chain(eta.chars().skip(1)).collect();
    }
    if paused || limited || away {
        if when.is_empty() {
            when = "No finish time while paused".into();
        } else {
            when.push_str(" · no finish time while paused");
        }
    }
    if !when.is_empty() {
        first.push('\n');
        first.push_str(&when);
    }
    let mut out = vec![
        json!({ "type": "header", "text": { "type": "plain_text", "text": head } }),
        mrkdwn(&first),
    ];
    // The board's columns, two to a row as board 81 sets them; Skipped
    // only while an item is.
    let in_column = |name: &str| {
        board["columns"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|c| c[0] == name)
            .and_then(|c| c[1].as_u64())
            .unwrap_or(0)
    };
    let count = |name: &str| {
        // The Home tab has no Blocked column: its items are counted with
        // those in line, as they were before the board had one.
        if name == "Next up" {
            return ["Next up", "Blocked"].iter().map(|c| in_column(c)).sum();
        }
        if name == "Landed"
            && let Some(n) = board["landedToday"].as_u64()
        {
            return n;
        }
        in_column(name)
    };
    let mut fields = Vec::new();
    for (name, shown_as) in [
        ("Next up", "Next up"),
        ("Landing", "Landing"),
        ("Working", "Working"),
        ("Landed", "Landed today"),
        ("Waiting on you", "Waiting on you"),
        ("Skipped", "Skipped"),
    ] {
        let n = count(name);
        if name == "Skipped" && n == 0 {
            continue;
        }
        fields.push(json!({ "type": "mrkdwn", "text": format!("{shown_as} *{n}*") }));
    }
    out.push(json!({ "type": "section", "block_id": "autopilot-counts", "fields": fields }));
    if limit > 0.0 {
        let share = spent / limit;
        out.push(context(&format!(
            "Today *{}* of {} {} {}%",
            dollars(spent),
            dollars(limit),
            meter(share),
            (share * 100.0).round() as i64
        )));
    }
    let flying: Vec<&Value> = board["tabs"].as_array().into_iter().flatten().collect();
    for f in flying.iter().take(MOST_TABS) {
        let id = f["tab"].as_str().unwrap_or("");
        let tab = tabs.iter().find(|t| t.id == id);
        let column = f["column"].as_str().unwrap_or("");
        let (mark, words) = flight_words(column, tab, now);
        let index = f["index"].as_u64().unwrap_or(0);
        // The tab's name, which its thread already says, never the note's
        // text: the board names no item.
        let name = tab
            .map(|t| text::cut(t.name.trim(), 80))
            .unwrap_or_else(|| "a tab".into());
        let mut block = mrkdwn(&format!("{mark} {index} {name} · {words}"));
        if let Some(url) = home_thread(cx, id).and_then(|t| thread_url(cx, &t)) {
            block["accessory"] = button_element(&Button::link("thread ›", url), "thread");
        }
        out.push(block);
    }
    if paused || limited || away {
        let n = flying.len();
        let mut words = match n {
            0 => "Opens no new tab.".to_string(),
            1 => "Opens no new tab; the one open goes on to its end.".to_string(),
            n => format!("Opens no new tab; these {n} go on to their end."),
        };
        if spend_limited && !paused {
            words.push_str(" Starts again tomorrow, or once the limit is raised.");
        } else if weekly.is_some() && !paused {
            words.push_str(" Starts again once the week resets, or once the limit is raised.");
        }
        out.push(context(&words));
    }
    let settings = Button::new("Settings", json!({ "a": "autopilot" }));
    let first_button = if paused {
        Button::new("Resume", json!({ "a": "autopilot-resume" })).style("primary")
    } else if limited {
        Button::new("Raise the limit", json!({ "a": "autopilot" })).style("primary")
    } else {
        Button::new("Pause", json!({ "a": "autopilot-pause" }))
    };
    out.push(json!({
        "type": "actions", "block_id": "autopilot-actions",
        "elements": [button_element(&first_button, "0"), button_element(&settings, "1")],
    }));
    out.push(context(&format!(
        "As of {}",
        super::lifecycle::clock(read_at(cx))
    )));
    out
}
