//! The buttons the relay puts under its messages, so the common answers are
//! a tap (lib/slack-relay/buttons.js). Each carries a value saying what
//! pressing it means; presses.rs checks it and acts. Only you can press
//! them (`slack::press_from`).

use indexmap::IndexMap;
use serde_json::{Value, json};

use super::Relay;
use super::post::Button;
use super::state::Dm;
use super::tabs::{self, Dialog, Tab};
use crate::mission::answer::Prompt;

/// A waiting tab's numbered options. Each records the wait it belongs to
/// (the tab's `since`) and the prompt it was made for, so a press can only
/// answer that same prompt while that same wait is on. A question that
/// takes several answers shows each option ticked or not, and a Submit
/// choices button (option 0) after them. The free option has no button: it
/// needs your text, which you reply with instead.
pub fn option_buttons(tab: &Tab, found: &Prompt) -> Vec<Button> {
    let key = tabs::hash_of(found);
    let value = |n: u32| json!({ "a": "opt", "w": tab.id, "n": n, "s": tab.since, "k": key });
    let mut buttons: Vec<Button> = found
        .options
        .iter()
        .filter(|o| Some(o.n) != found.free)
        .map(|o| {
            let boxed = match o.checked {
                None => "",
                Some(true) => "☑ ",
                Some(false) => "☐ ",
            };
            let b = Button::new(format!("{}. {boxed}{}", o.n, o.label), value(o.n));
            if o.n == 1 && !found.multi {
                b.style("primary")
            } else {
                b
            }
        })
        .collect();
    if found.multi {
        buttons.push(Button::new("Submit choices", value(0)).style("primary"));
    }
    buttons
}

/// The option a choice button stands for: what to reply to the tab to
/// press it (`answer`) and how to name it (`said`).
#[derive(Clone, Debug, PartialEq)]
pub struct Chosen {
    pub n: u32,
    pub answer: String,
    pub said: String,
}

/// The option a choice button stands for, if the tab is still showing the
/// very prompt it was made for (and, for a waiting tab's options, in the
/// same wait), with the tab it is in.
pub fn still_showing(cx: &Relay, value: &Value) -> Option<(Tab, Chosen)> {
    let text = |k: &str| value.get(k).and_then(Value::as_str).unwrap_or("");
    let w = text("w");
    let n = value.get("n").and_then(Value::as_u64)? as u32;
    let tab = tabs::tab(cx, w)?;
    let dialog = text("a").ends_with("dialog");
    let shown = tabs::screen(cx, w);
    let (hash, option) = if dialog {
        let found = tabs::dialog_in(&shown)?;
        let o = found.options.iter().find(|o| o.n == n)?;
        (
            found.hash(),
            Chosen {
                n,
                answer: n.to_string(),
                said: o.label.clone(),
            },
        )
    } else {
        let found = tabs::options_in(&shown)?;
        let option = if n == 0 && found.multi {
            Chosen {
                n: 0,
                answer: "submit".into(),
                said: "Submit choices".into(),
            }
        } else {
            let o = found
                .options
                .iter()
                .filter(|o| Some(o.n) != found.free)
                .find(|o| o.n == n)?;
            Chosen {
                n,
                answer: n.to_string(),
                said: format!("{}. {}", o.n, o.label),
            }
        };
        (tabs::hash_of(&found), option)
    };
    let same = hash == text("k") && (dialog || (tab.state == "waiting" && tab.since == text("s")));
    same.then_some((tab, option))
}

/// A dialog shown before a session starts, answered with the arrow keys.
pub fn dialog_buttons(tab: &Tab, dialog: &Dialog) -> Vec<Button> {
    let key = dialog.hash();
    dialog
        .options
        .iter()
        .map(|o| {
            Button::new(
                o.label.clone(),
                json!({ "a": "dialog", "w": tab.id, "n": o.n, "k": key }),
            )
        })
        .collect()
}

/// Under a finished turn: Skill (a form to run one). Nothing is running,
/// so there is nothing to stop or ask the progress of. There is no "yes, go
/// on": a reply ending in a question is as often a question to answer or a
/// choice as a request to carry on, so you answer it in the thread.
pub fn turn_buttons(tab: &Tab) -> Vec<Button> {
    vec![Button::new(
        "Skill",
        json!({ "a": "skill", "w": tab.id, "p": tab.pid }),
    )]
}

/// Under the warning that a tab's context is nearly full: the Skill form
/// with compact already picked, to add what the summary should keep.
pub fn compact_button(tab: &Tab) -> Button {
    Button::new(
        "Compact",
        json!({ "a": "skill", "w": tab.id, "p": tab.pid, "skill": "compact" }),
    )
    .style("primary")
}

/// While a turn you asked for is running: how it's going, or stop it.
pub fn working_buttons(tab: &Tab) -> Vec<Button> {
    vec![
        Button::new(
            "Progress",
            json!({ "a": "cmd", "w": tab.id, "c": "progress" }),
        )
        .style("primary"),
        Button::new("Stop", json!({ "a": "cmd", "w": tab.id, "c": "esc" })).style("danger"),
    ]
}

/// Closes a tab, which ends its Claude session, so Slack asks first. It
/// names the tab's process as well as its id, which is used again, so a
/// button drawn for a tab that has since closed can't close another.
pub fn close_button(tab: &Tab) -> Button {
    let name: String = tab.name.chars().take(120).collect();
    Button::new("Close", json!({ "a": "close", "w": tab.id, "p": tab.pid }))
        .style("danger")
        .confirm(json!({
            "title": { "type": "plain_text", "text": "Close this tab?" },
            "text": {
                "type": "mrkdwn",
                "text": format!(
                    "*Tab {}, {name}*{} Closing it ends its Claude session; `claude-here` in its directory resumes it later.",
                    tab.index,
                    if tab.state == "working" { " is in the middle of a turn." } else { "." }
                ),
            },
            "confirm": { "type": "plain_text", "text": "Close" },
            "deny": { "type": "plain_text", "text": "Keep it" },
            "style": "danger",
        }))
}

/// Under a note that a tab may be stuck, which is still running.
pub fn stuck_buttons(tab: &Tab) -> Vec<Button> {
    vec![
        Button::new("Stop", json!({ "a": "cmd", "w": tab.id, "c": "esc" })).style("danger"),
        Button::new(
            "Progress",
            json!({ "a": "cmd", "w": tab.id, "c": "progress" }),
        ),
    ]
}

/// A message that named no tab, kept under a short id the picker's buttons
/// carry, for the latest `MAX_PICKS` of them.
#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    pub ts: String,
    pub text: String,
    pub thread_ts: Option<String>,
}

pub const MAX_PICKS: usize = 50;

#[derive(Default)]
pub struct Picks(pub IndexMap<String, Pick>);

/// One button per tab, to send the message there.
pub fn picker_buttons(cx: &Relay, tabs: &[Tab], pick: Pick) -> Vec<Button> {
    let id = short_id();
    {
        let mut mem = cx.mem.borrow_mut();
        mem.picks.0.insert(id.clone(), pick);
        while mem.picks.0.len() > MAX_PICKS {
            mem.picks.0.shift_remove_index(0);
        }
    }
    tabs.iter()
        .take(25)
        .map(|t| {
            Button::new(
                format!("{} {}", t.index, t.name),
                json!({ "a": "to", "w": t.id, "p": id }),
            )
        })
        .collect()
}

/// Eight hex characters no other pick has had in this run.
fn short_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let seed = format!("{}-{}-{n}", crate::util::now_ms(), std::process::id());
    crate::tabs::color::sha256(seed.as_bytes())[..4]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// On a tab's thread header: what you'd ask of the tab without typing into
/// it.
pub fn header_buttons(tab: &Tab) -> Vec<Button> {
    vec![
        Button::new(
            "Progress",
            json!({ "a": "cmd", "w": tab.id, "c": "progress" }),
        ),
        Button::new("Skill", json!({ "a": "skill", "w": tab.id, "p": tab.pid })),
        Button::new(
            "Rename",
            json!({ "a": "rename", "w": tab.id, "p": tab.pid }),
        ),
        close_button(tab),
    ]
}

/// On a closed tab's thread header: delete the thread (see cleanup.rs).
pub fn delete_thread_button(ts: &str) -> Button {
    Button::new("Delete thread", json!({ "a": "delete-thread", "t": ts }))
        .style("danger")
        .confirm(json!({
            "title": { "type": "plain_text", "text": "Delete this thread?" },
            "text": { "type": "plain_text", "text": "The relay deletes its messages here. Your own replies stay; Slack only lets you delete those. This can't be undone." },
            "confirm": { "type": "plain_text", "text": "Delete" },
            "deny": { "type": "plain_text", "text": "Keep it" },
            "style": "danger",
        }))
}

/// The commands the welcome message's buttons stand for.
pub const WELCOME_COMMANDS: &[&str] = &["status", "threads", "progress", "help", "stop"];

/// A link Slack opens in its own app, to the app's Home tab.
pub fn home_link(dm: &Dm) -> String {
    format!(
        "slack://app?team={}&id={}&tab=home",
        dm.team.as_deref().unwrap_or(""),
        dm.app_id.as_deref().unwrap_or("")
    )
}

/// The welcome message's buttons, for what you'd otherwise type: the Home
/// tab first, then the commands. The Home link needs the app's ID, which
/// Slack only gives with an event, so it is left off until the relay has
/// seen one.
pub fn welcome_buttons(dm: &Dm) -> Vec<Button> {
    let mut buttons = Vec::new();
    if dm.app_id.is_some() {
        buttons.push(Button::link("Home", home_link(dm)).style("primary"));
    }
    for (text, c) in [
        ("Status", "status"),
        ("Threads", "threads"),
        ("Progress", "progress"),
    ] {
        buttons.push(Button::new(text, json!({ "a": "do", "c": c })));
    }
    buttons.push(Button::new("New tab", json!({ "a": "new" })));
    buttons.push(Button::new("Help", json!({ "a": "do", "c": "help" })));
    buttons.push(
        Button::new("Stop relay", json!({ "a": "do", "c": "stop" }))
            .style("danger")
            .confirm(json!({
                "title": { "type": "plain_text", "text": "Stop the relay?" },
                "text": { "type": "plain_text", "text": "Slack stops reaching your tabs until you start it again at the computer." },
                "confirm": { "type": "plain_text", "text": "Stop" },
                "deny": { "type": "plain_text", "text": "Keep it running" },
                "style": "danger",
            })),
    );
    buttons
}
