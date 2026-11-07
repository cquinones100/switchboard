//! A tab's screen (lib/slack-relay/screens.js). Tapping Open on a tab in
//! the Home tab opens a modal for it: where it stands, its last prompt and
//! the end of its last reply, the options of a prompt it is waiting on, and
//! what you can do (message it, run a skill, open its thread, ask its
//! progress, rename, stop, close). Progress and Stop show their result in
//! the modal itself; Message, Skill and Rename open a second screen with a
//! field. Every button names the tab and its Claude pane's process, so a
//! screen left open after the tab has closed can't act on a tab that has
//! since taken its id. While a screen is open, it is drawn again whenever
//! its tab's state changes, so it never shows a prompt that has gone.

use std::collections::HashMap;
use std::sync::LazyLock;

use anyhow::Result;
use regex::{Captures, Regex};
use serde_json::{Value, json};

use super::buttons::{close_button, dialog_buttons, option_buttons, still_showing};
use super::commands::{Reply, close_tab, for_tab, handle, interrupt, rename_tab};
use super::dashboard::{Refresh, home_words, refresh_dashboard};
use super::homes::{home_of, home_thread, thread_url};
use super::post::{Button, PostOpts, button_element, post};
use super::skills::{BUILT_IN, MAX_SKILLS, SKILL_NAME, skills_for};
use super::slack::{Form, Message, Press};
use super::summaries::progress;
use super::tabs::{self, Tab, label, lasted, relative};
use super::{Cx, Relay};
use crate::util;

pub fn clip(text: &str, max: usize) -> String {
    let flat = text.trim();
    if flat.chars().count() > max {
        format!("{}…", flat.chars().take(max - 1).collect::<String>())
    } else {
        flat.to_string()
    }
}

fn running(tab: &Tab) -> bool {
    tab.state == "working" || tab.state == "waiting"
}

/// A screen open now: the tab it is for and the state it was drawn in.
/// Slack says when one is closed; one it never says that of is forgotten
/// after `SCREEN_KEEP_MS`.
#[derive(Clone, Debug)]
struct Open {
    w: String,
    p: String,
    state: String,
    since: String,
    at: i64,
}

pub const SCREEN_KEEP_MS: i64 = 60 * 60_000;

#[derive(Default)]
pub struct Memory {
    open: HashMap<String, Open>,
}

fn remember(cx: &Relay, view: Option<&str>, tab: &Tab) {
    let Some(view) = view.filter(|v| !v.is_empty()) else {
        return;
    };
    cx.mem.borrow_mut().screens.open.insert(
        view.to_string(),
        Open {
            w: tab.id.clone(),
            p: tab.pid.clone(),
            state: tab.state.clone(),
            since: tab.since.clone(),
            at: util::now_ms(),
        },
    );
}

pub fn forget_screen(cx: &Relay, view: &str) {
    cx.mem.borrow_mut().screens.open.remove(view);
}

pub fn open_screens(cx: &Relay) -> usize {
    cx.mem.borrow().screens.open.len()
}

/// A tab's buttons, named for a screen or the Home tab, with the tab's
/// process.
fn renamed(buttons: Vec<Button>, prefix: &str, tab: &Tab) -> Vec<Button> {
    buttons
        .into_iter()
        .map(|mut b| {
            if let Some(v) = b.value.as_mut() {
                let a = v.get("a").and_then(Value::as_str).unwrap_or("").to_string();
                v["a"] = json!(format!("{prefix}{a}"));
                v["p"] = json!(tab.pid);
            }
            b
        })
        .collect()
}

/// The prompt a waiting tab shows, or a dialog before its session starts,
/// as buttons that answer only that prompt.
fn choice_blocks(cx: &Relay, tab: &Tab) -> Vec<Value> {
    let shown = tabs::screen(cx, &tab.id);
    let found = (tab.state == "waiting")
        .then(|| tabs::options_in(&shown))
        .flatten();
    let buttons = match &found {
        Some(f) => option_buttons(tab, f),
        None => tabs::dialog_in(&shown)
            .map(|d| dialog_buttons(tab, &d))
            .unwrap_or_default(),
    };
    if buttons.is_empty() {
        return Vec::new();
    }
    let buttons = renamed(buttons, "screen-", tab);
    vec![
        json!({ "type": "section", "text": { "type": "mrkdwn", "text": "🔴 *It's asking you to choose:*" } }),
        json!({
            "type": "actions",
            "elements": buttons.iter().take(25).enumerate().map(|(i, b)| button_element(b, &i.to_string())).collect::<Vec<_>>(),
        }),
    ]
}

fn title(tab: &Tab) -> Value {
    // Slack cuts a modal's title at 24 characters; the full name is under it.
    let name = tab.name.split_whitespace().collect::<Vec<_>>().join(" ");
    json!({ "type": "plain_text", "text": clip(&format!("{} · {name}", tab.index), 24) })
}

/// The tab's screen, as Block Kit. `note` is a line at the bottom: what a
/// button just did, or a progress summary.
pub fn tab_screen(cx: &Relay, tab: &Tab, note: Option<&str>) -> Value {
    let file = tab.transcript.as_deref();
    let turn = file.and_then(tabs::last_prompt);
    let said = file.and_then(tabs::last_said);
    let last_at = file.and_then(|f| tabs::last_activity_at(cx, f));
    let context = tabs::context_of(&cx.paths, file);
    let url = home_thread(cx, &tab.id).and_then(|t| thread_url(cx, &t));
    let value = |a: &str| json!({ "a": a, "w": tab.id, "p": tab.pid });
    let now = util::now_ms();
    let mut head = format!("*{}*\n{}", clip(&tab.name, 150), home_words(&tab.state));
    if let Some(since) = tabs::since_ms(&tab.since) {
        head.push_str(&format!(" for {}", lasted(now - since)));
    }
    if let Some(at) = last_at {
        head.push_str(&format!(" · last activity {}", relative(now - at)));
    }
    if let Some(c) = &context {
        head.push_str(&format!(
            "\n{}% context used{}",
            c.percent,
            if c.model.is_empty() {
                String::new()
            } else {
                format!(" · {}", c.model)
            }
        ));
    }
    let mut blocks = vec![json!({ "type": "section", "text": { "type": "mrkdwn", "text": head } })];
    if let Some(t) = &turn {
        blocks.push(json!({
            "type": "section",
            "text": { "type": "plain_text", "text": format!(
                "{}: {}",
                if t.background { "Background task" } else { "You asked" },
                clip(&t.prompt, 400)
            ) },
        }));
    }
    if let Some(s) = &said {
        blocks.push(json!({
            "type": "section",
            "text": { "type": "mrkdwn", "text": format!("*Claude said:*\n{}", as_mrkdwn(&clip(s, 1500))) },
        }));
    }
    blocks.extend(choice_blocks(cx, tab));
    let mut buttons = vec![
        Button::new("Message", value("screen-message")).style("primary"),
        Button::new("Skill", value("screen-skill")),
    ];
    if let Some(url) = url {
        buttons.push(Button::link("Go to thread", url));
    }
    buttons.push(Button::new("Progress", value("screen-progress")));
    buttons.push(Button::new("Rename", value("screen-rename")));
    if running(tab) {
        buttons.push(Button::new("Stop", value("screen-stop")));
    }
    let mut close = close_button(tab);
    close.value = Some(value("screen-close"));
    buttons.push(close);
    blocks.push(json!({
        "type": "actions",
        "elements": buttons.iter().enumerate().map(|(i, b)| button_element(b, &i.to_string())).collect::<Vec<_>>(),
    }));
    if let Some(note) = note {
        blocks.push(json!({ "type": "divider" }));
        blocks.push(
            json!({ "type": "section", "text": { "type": "mrkdwn", "text": clip(note, 2900) } }),
        );
    }
    json!({
        "type": "modal",
        "callback_id": "tab",
        "notify_on_close": true,
        "private_metadata": util::json_compact(&json!({ "w": tab.id, "p": tab.pid })),
        "title": title(tab),
        "close": { "type": "plain_text", "text": "Done" },
        "blocks": blocks,
    })
}

/// A second screen with a field or two. Its title is the tab's, cut as
/// Slack cuts it, so the field's label names the tab in full. `from` says
/// whether it was opened over the tab's screen, which is drawn again with
/// what was done, or from a button in its thread.
fn field_screen(tab: &Tab, callback: &str, submit: &str, blocks: Vec<Value>, from: &str) -> Value {
    json!({
        "type": "modal",
        "callback_id": callback,
        "private_metadata": util::json_compact(&json!({ "w": tab.id, "p": tab.pid, "from": from })),
        "title": title(tab),
        "submit": { "type": "plain_text", "text": submit },
        "close": { "type": "plain_text", "text": if from == "screen" { "Back" } else { "Cancel" } },
        "blocks": blocks,
    })
}

pub fn rename_screen(tab: &Tab, current: &str, from: &str) -> Value {
    let mut element = json!({ "type": "plain_text_input", "action_id": "name", "max_length": 60 });
    if !current.is_empty() {
        element["initial_value"] = json!(current);
    }
    field_screen(
        tab,
        "rename",
        "Save",
        vec![json!({
            "type": "input",
            "block_id": "name",
            "optional": true,
            "label": { "type": "plain_text", "text": clip(&format!("New name for tab {}, {}", tab.index, tab.name), 2000) },
            "hint": { "type": "plain_text", "text": "Leave it empty for the tab's own name (its ticket or directory)." },
            "element": element,
        })],
        from,
    )
}

pub fn message_screen(tab: &Tab) -> Value {
    field_screen(
        tab,
        "message",
        "Send",
        vec![json!({
            "type": "input",
            "block_id": "text",
            "label": { "type": "plain_text", "text": clip(&format!("Message to tab {}, {}", tab.index, tab.name), 2000) },
            "hint": { "type": "plain_text", "text": "Typed into its Claude, as a reply in its thread would be. The answer goes in its thread." },
            "element": { "type": "plain_text_input", "action_id": "text", "multiline": true, "focus_on_load": true },
        })],
        "screen",
    )
}

/// The Skill form: a menu of the skills the tab can run, grouped the ones
/// you use most, yours, the project's, then the rest, and when a tab has
/// more than a menu holds, a field for any name. Picking a skill draws the
/// form again, so the Arguments field can say what that skill takes.
pub fn skill_screen(cx: &Relay, tab: &Tab, from: &str, chosen: Option<&str>, args: &str) -> Value {
    const GROUPS: &[(&str, &str)] = &[
        ("frequent", "Frequently used"),
        ("yours", "Yours"),
        ("switchboard", "Switchboard"),
        ("project", "This project"),
        ("claude", "Claude Code"),
        ("session", "Plugins and built-in"),
    ];
    // Switchboard's own, from its plugin while the panes load it; the old
    // way they are in ~/.claude/skills, with yours.
    let plugin = (crate::settings::setting(crate::setup::way::SETTING) != Some(Value::Bool(false)))
        .then_some(cx.paths.plugin_dir.as_path());
    let skills = skills_for(
        &cx.paths,
        tab.cwd.as_deref(),
        tab.transcript.as_deref(),
        plugin,
    );
    let shown: Vec<_> = skills.iter().take(MAX_SKILLS).collect();
    let cut = skills.len() > shown.len();
    let option = |s: &super::skills::Skill| {
        let mut o =
            json!({ "text": { "type": "plain_text", "text": clip(&s.name, 75) }, "value": s.name });
        if !s.description.is_empty() {
            o["description"] = json!({ "type": "plain_text", "text": clip(&s.description, 75) });
        }
        o
    };
    let groups: Vec<Value> = GROUPS
        .iter()
        .filter_map(|(source, label)| {
            let options: Vec<Value> = shown
                .iter()
                .filter(|s| s.source == *source)
                .map(|s| option(s))
                .collect();
            (!options.is_empty()).then(
                || json!({ "label": { "type": "plain_text", "text": label }, "options": options }),
            )
        })
        .collect();
    let picked = chosen.and_then(|c| shown.iter().find(|s| s.name == c));
    let mut blocks = Vec::new();
    if !shown.is_empty() {
        let mut element = json!({
            "type": "static_select", "action_id": "skill",
            "placeholder": { "type": "plain_text", "text": "Choose or type to search" },
            "option_groups": groups,
        });
        if let Some(p) = picked {
            element["initial_option"] = option(p);
        }
        blocks.push(json!({
            "type": "input", "block_id": "skill", "optional": cut, "dispatch_action": true,
            "label": { "type": "plain_text", "text": clip(&format!("Skill for tab {}, {}", tab.index, tab.name), 2000) },
            "element": element,
        }));
    }
    if cut || shown.is_empty() {
        let text = if shown.is_empty() {
            clip(&format!("Skill for tab {}, {}", tab.index, tab.name), 2000)
        } else {
            format!(
                "Or another skill ({} more aren't in the menu)",
                skills.len() - shown.len()
            )
        };
        blocks.push(json!({
            "type": "input", "block_id": "typed", "optional": !shown.is_empty(),
            "label": { "type": "plain_text", "text": text },
            "element": { "type": "plain_text_input", "action_id": "typed", "placeholder": { "type": "plain_text", "text": "review-loop" } },
        }));
    }
    let takes = match picked {
        Some(p) if !p.hint.is_empty() => format!("/{} takes {}.", p.name, p.hint),
        Some(p) => format!("/{} doesn't say what it takes.", p.name),
        None => "What you'd type after the skill's name.".into(),
    };
    let mut element = json!({
        "type": "plain_text_input", "action_id": "args", "multiline": true,
        "placeholder": { "type": "plain_text", "text": clip(
            picked.map(|p| p.hint.as_str()).filter(|h| !h.is_empty()).unwrap_or("ABC-123, or a description"),
            150
        ) },
    });
    if !args.is_empty() {
        element["initial_value"] = json!(args);
    }
    blocks.push(json!({
        "type": "input", "block_id": "args", "optional": true,
        "label": { "type": "plain_text", "text": "Arguments" },
        "hint": { "type": "plain_text", "text": clip(&format!("{takes} Runs as a reply in its thread would; the answer goes there."), 2000) },
        "element": element,
    }));
    field_screen(tab, "skill", "Run", blocks, from)
}

/// The Home tab's New tab: where, and the prompt, as `new <place>:
/// <prompt>`. Where is a menu of every directory you have run Claude in,
/// and a field beside it takes anything else `new <place>:` does.
pub fn new_screen(cx: &Relay) -> Value {
    let groups: Vec<Value> = super::places::place_choices(cx)
        .into_iter()
        .map(|(group, choices)| {
            json!({
                "label": { "type": "plain_text", "text": clip(&group, 75) },
                "options": choices.iter().map(|c| json!({
                    "text": { "type": "plain_text", "text": clip(&c.label, 75) },
                    "value": c.value,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let mut blocks = Vec::new();
    let any = !groups.is_empty();
    if any {
        blocks.push(json!({
            "type": "input", "block_id": "place", "optional": true,
            "label": { "type": "plain_text", "text": "Where" },
            "hint": { "type": "plain_text", "text": "Every directory you have run Claude in, most recent first. Empty: the relay's directory." },
            "element": { "type": "static_select", "action_id": "place", "placeholder": { "type": "plain_text", "text": "Choose or type to search" }, "option_groups": groups },
        }));
    }
    blocks.push(json!({
        "type": "input", "block_id": "typed", "optional": true,
        "label": { "type": "plain_text", "text": if any { "Or somewhere else" } else { "Where" } },
        "hint": { "type": "plain_text", "text": "A repository, a ticket or branch with a worktree, or a path. Used instead of the menu." },
        "element": { "type": "plain_text_input", "action_id": "typed", "max_length": 200 },
    }));
    blocks.push(json!({
        "type": "input", "block_id": "prompt",
        "label": { "type": "plain_text", "text": "Prompt" },
        "element": { "type": "plain_text_input", "action_id": "prompt", "multiline": true, "focus_on_load": true },
    }));
    json!({
        "type": "modal",
        "callback_id": "new",
        "title": { "type": "plain_text", "text": "New Claude tab" },
        "submit": { "type": "plain_text", "text": "Start" },
        "close": { "type": "plain_text", "text": "Cancel" },
        "blocks": blocks,
    })
}

/// A tab that has closed, or was just closed.
pub fn gone_screen(note: &str) -> Value {
    json!({
        "type": "modal",
        "callback_id": "gone",
        "title": { "type": "plain_text", "text": "Tab" },
        "close": { "type": "plain_text", "text": "Done" },
        "blocks": [{ "type": "section", "text": { "type": "mrkdwn", "text": note } }],
    })
}

/// The tab a button or field was made for, if it is still the same tab.
pub fn same_tab(cx: &Relay, meta: &Value) -> Option<Tab> {
    let w = meta.get("w").and_then(Value::as_str)?;
    let p = meta.get("p").map(util::js_string)?;
    tabs::tab(cx, w).filter(|t| t.pids.contains(&p))
}

/// Markdown, as Claude and the relay's replies are written, in Slack's
/// older mrkdwn, the only formatting a modal renders: headings and **bold**
/// become *bold*, *italic* _italic_, [links](…) <…|links>, and list items
/// bullets. mrkdwn has no tables, so a table becomes a bullet a row, its
/// first cell in bold and the rest after it, its header row dropped. Code
/// is left as it is (mrkdwn has the same backticks), and &, < and > are
/// escaped everywhere, as mrkdwn needs.
pub fn as_mrkdwn(markdown: &str) -> String {
    static FENCED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)```.*?```").expect("a regex"));
    static INLINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`[^`\n]+`").expect("a regex"));
    let escape = |t: &str| {
        t.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let mut out = String::new();
    let mut at = 0;
    for m in FENCED.find_iter(markdown) {
        out.push_str(&outside_code(&markdown[at..m.start()]));
        out.push_str(&escape(m.as_str()));
        at = m.end();
    }
    out.push_str(&outside_code(&markdown[at..]));
    return out;

    fn outside_code(text: &str) -> String {
        let text = rows(text);
        let escape = |t: &str| {
            t.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        };
        let mut out = String::new();
        let mut at = 0;
        for m in INLINE.find_iter(&text) {
            out.push_str(&prose(&text[at..m.start()]));
            out.push_str(&escape(m.as_str()));
            at = m.end();
        }
        out.push_str(&prose(&text[at..]));
        out
    }
}

/// A table as a bullet a row.
fn rows(text: &str) -> String {
    static TABLE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^[ \t]*\|.*\|[ \t]*\n[ \t]*\|?[ \t]*:?-+:?[ \t]*(?:\|[ \t]*:?-+:?[ \t]*)*\|?[ \t]*((?:\n[ \t]*\|.*)*)").expect("a regex")
    });
    TABLE
        .replace_all(text, |c: &Captures| {
            c[1].split('\n')
                .filter(|l| !l.trim().is_empty())
                .map(|line| {
                    let line = line.trim();
                    let line = line.strip_prefix('|').unwrap_or(line);
                    let line = line.strip_suffix('|').unwrap_or(line);
                    let cells: Vec<&str> = line
                        .split('|')
                        .map(str::trim)
                        .filter(|c| !c.is_empty())
                        .collect();
                    let first = cells.first().copied().unwrap_or("");
                    let rest = if cells.len() > 1 {
                        format!(" · {}", cells[1..].join(" · "))
                    } else {
                        String::new()
                    };
                    format!("- **{first}**{rest}")
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .into_owned()
}

fn prose(text: &str) -> String {
    static HEADING: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^#{1,6}[ \t]+(.+?)[ \t]*#*$").expect("a regex"));
    static BOLD: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\*\*([^*\n]+?)\*\*").expect("a regex"));
    static UNDER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"__([^_\n]+?)__").expect("a regex"));
    static STRIKE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"~~([^~\n]+?)~~").expect("a regex"));
    static LINK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\[([^\]\n]+)\]\((https?:[^)\s]+)\)").expect("a regex"));
    static ITEM: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^([ \t]*)[-*+][ \t]+").expect("a regex"));
    let text = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let text = HEADING.replace_all(&text, "**$1**");
    let text = italics(&text);
    let text = BOLD.replace_all(&text, "*$1*");
    let text = UNDER.replace_all(&text, "*$1*");
    let text = STRIKE.replace_all(&text, "~$1~");
    let text = LINK.replace_all(&text, "<$2|$1>");
    ITEM.replace_all(&text, "$1• ").into_owned()
}

/// `*italic*` as `_italic_`: a lone `*` not after a word character or
/// another `*`, its text on one line with no `*` in it, and a closing `*`
/// not followed by a word character or `*`.
fn italics(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let opens = c == '*'
            && (i == 0 || !(chars[i - 1] == '*' || word(chars[i - 1])))
            && chars.get(i + 1).is_some_and(|n| *n != '*');
        if opens {
            let close = (i + 1..chars.len()).find(|&j| chars[j] == '*' || chars[j] == '\n');
            if let Some(j) = close
                && chars[j] == '*'
                && j > i + 1
                && chars.get(j + 1).is_none_or(|n| !(*n == '*' || word(*n)))
            {
                out.push('_');
                out.extend(&chars[i + 1..j]);
                out.push('_');
                i = j + 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Draws a tab's screen again, or says the tab has gone.
pub async fn redraw(cx: &Cx, view: &str, meta: &Value, note: Option<&str>) {
    let tab = same_tab(cx, meta);
    let screen = match &tab {
        Some(t) => tab_screen(cx, t, note),
        None => gone_screen("That tab has closed."),
    };
    match cx
        .slack
        .api("views.update", json!({ "view_id": view, "view": screen }))
        .await
    {
        Ok(_) => match &tab {
            Some(t) => remember(cx, Some(view), t),
            None => forget_screen(cx, view),
        },
        Err(e) => {
            // A screen you have closed is gone from Slack (not_found), which
            // is nothing to report: it is only forgotten.
            if !e.to_string().contains("not_found") {
                cx.log(&format!("screen update failed: {e}"));
            }
            forget_screen(cx, view);
        }
    }
}

/// The tab's thread, made if it has none yet, and linked to the tab.
async fn thread_for(cx: &Cx, tab: &Tab) -> Option<String> {
    let home = home_of(cx, tab).await.ok().flatten()?;
    let mut state = cx.state.borrow_mut();
    state.watch_thread(&home, util::now_ms());
    state
        .alerts
        .entry(home.clone())
        .or_insert_with(|| super::state::Target {
            window: tab.id.clone(),
            index: tab.index,
            name: tab.name.clone(),
            thread: None,
        });
    Some(home)
}

/// What you send from a screen is said in the tab's thread, then handled
/// as a reply there, straight to the tab the screen is for: never read as a
/// command, and never through the thread's link to a tab.
async fn send_from_screen(cx: &Cx, tab: &Tab, text: &str, said: &str) -> Result<Option<String>> {
    let Some(home) = thread_for(cx, tab).await else {
        return Ok(None);
    };
    let ts = post(cx, &format!("🤖 {said}"), Some(&home), PostOpts::default()).await?;
    for_tab(cx, tab, Some(&home), Some(&ts), text).await?;
    Ok(Some(home))
}

fn link_to(cx: &Relay, home: Option<&str>) -> String {
    match home.and_then(|h| thread_url(cx, h)) {
        Some(url) => format!("<{url}|its thread>"),
        None => "its thread".into(),
    }
}

/// A press on the Home tab's Open or New tab, on a button in a tab's
/// screen, or on Skill or Rename under a message in a tab's thread.
pub async fn handle_screen_press(cx: &Cx, press: &Press, thread: Option<&str>) -> Result<()> {
    let value = &press.value;
    let a = value.get("a").and_then(Value::as_str).unwrap_or("");
    let trigger = press.trigger_id.clone().unwrap_or_default();
    if a == "new" {
        cx.slack
            .api(
                "views.open",
                json!({ "trigger_id": trigger, "view": new_screen(cx) }),
            )
            .await?;
        return Ok(());
    }
    let tab = same_tab(cx, value);
    let view = press.view.clone().unwrap_or_default();
    // A skill picked in the Skill form: drawn again to say what it takes.
    // The hash makes Slack refuse it if the form has changed since.
    if a == "skill-chosen" {
        let Some(tab) = tab else {
            return Ok(());
        };
        let text = |k: &str| value.get(k).map(util::js_string).unwrap_or_default();
        let from = text("from");
        let mut params = json!({
            "view_id": view.id,
            "view": skill_screen(cx, &tab, &from, Some(&text("skill")), &text("args")),
        });
        if let Some(hash) = &view.hash {
            params["hash"] = json!(hash);
        }
        if let Err(e) = cx.slack.api("views.update", params).await {
            cx.log(&format!("couldn't redraw the Skill form: {e}"));
        }
        return Ok(());
    }
    // An option on the Home tab, answered as a reply in its thread would be;
    // the Home tab is then drawn again, without it.
    if a == "home-opt" || a == "home-dialog" {
        if let Some(tab) = &tab {
            let mut plain = value.clone();
            plain["a"] = json!(a.trim_start_matches("home-"));
            if let Some((_, chosen)) = still_showing(cx, &plain) {
                send_from_screen(
                    cx,
                    tab,
                    &chosen.answer,
                    &format!("_From the Home tab:_ chose **{}**", chosen.said),
                )
                .await?;
            }
        }
        return refresh_dashboard(cx, Refresh::opened_quick()).await;
    }
    // Skill and Rename on a message in a tab's thread open their form on
    // its own.
    if a == "skill" || a == "rename" {
        let Some(tab) = tab else {
            post(
                cx,
                "🤖 That tab has closed, or is running a different session now.",
                thread,
                PostOpts::default(),
            )
            .await?;
            return Ok(());
        };
        let skill = value.get("skill").and_then(Value::as_str);
        let screen = if a == "skill" {
            skill_screen(cx, &tab, "thread", skill, "")
        } else {
            rename_screen(&tab, &tab.own_name, "thread")
        };
        cx.slack
            .api(
                "views.open",
                json!({ "trigger_id": trigger, "view": screen }),
            )
            .await?;
        return Ok(());
    }
    if a == "open" {
        let Some(tab) = tab else {
            return refresh_dashboard(cx, Refresh::opened_quick()).await;
        };
        let opened = cx
            .slack
            .api(
                "views.open",
                json!({ "trigger_id": trigger, "view": tab_screen(cx, &tab, None) }),
            )
            .await?;
        remember(cx, opened.pointer("/view/id").and_then(Value::as_str), &tab);
        return Ok(());
    }
    let Some(tab) = tab else {
        redraw(cx, &view.id, value, None).await;
        return Ok(());
    };
    let meta = json!({ "w": tab.id, "p": tab.pid });
    let push = |screen: Value| {
        let trigger = trigger.clone();
        async move {
            cx.slack
                .api(
                    "views.push",
                    json!({ "trigger_id": trigger, "view": screen }),
                )
                .await
                .map(|_| ())
        }
    };
    match a {
        "screen-rename" => push(rename_screen(&tab, &tab.own_name, "screen")).await,
        "screen-message" => push(message_screen(&tab)).await,
        "screen-skill" => push(skill_screen(cx, &tab, "screen", None, "")).await,
        "screen-opt" | "screen-dialog" => {
            let mut plain = value.clone();
            plain["a"] = json!(a.trim_start_matches("screen-"));
            let Some((_, chosen)) = still_showing(cx, &plain) else {
                redraw(
                    cx,
                    &view.id,
                    &meta,
                    Some("That choice isn't showing any more, so I didn't press anything."),
                )
                .await;
                return Ok(());
            };
            let home = send_from_screen(
                cx,
                &tab,
                &chosen.answer,
                &format!("_From the Home tab:_ chose **{}**", chosen.said),
            )
            .await?;
            let note = format!(
                "Chose *{}*. What happens next goes in {}.",
                chosen.said,
                link_to(cx, home.as_deref())
            );
            redraw(cx, &view.id, &meta, Some(&note)).await;
            Ok(())
        }
        "screen-progress" => {
            redraw(cx, &view.id, &meta, Some("_Asking what it's doing…_")).await;
            let reply = Reply::Screen {
                view: view.id.clone(),
                meta,
            };
            progress(cx, &tab, &reply, None).await.map(|_| ())
        }
        "screen-stop" => {
            let reply = Reply::Screen {
                view: view.id.clone(),
                meta,
            };
            interrupt(cx, &tab, &reply).await.map(|_| ())
        }
        "screen-close" => {
            close_tab(cx, &tab).await?;
            cx.save();
            forget_screen(cx, &view.id);
            let _ = cx
                .slack
                .api(
                    "views.update",
                    json!({ "view_id": view.id, "view": gone_screen(&format!("Closed {}.", label(tab.index, &tab.name))) }),
                )
                .await;
            refresh_dashboard(cx, Refresh::opened_quick()).await
        }
        _ => Ok(()),
    }
}

/// Send or Save on a field's screen, or Start on New tab. Slack closes
/// that screen when the relay acknowledges it, which it does at once, so
/// the tab's screen under it is drawn again with what was done.
pub async fn handle_submission(cx: &Cx, form: &Form) -> Result<()> {
    if form.kind == "new" {
        static GAP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\s:]+").expect("a regex"));
        let chosen = if form.value("typed").trim().is_empty() {
            form.value("place")
        } else {
            form.value("typed").trim()
        };
        let place = GAP.replace_all(chosen, " ").trim().to_string();
        let prompt = form.value("prompt").trim();
        if prompt.is_empty() {
            return Ok(());
        }
        let text = if place.is_empty() {
            format!("new: {prompt}")
        } else {
            format!("new {place}: {prompt}")
        };
        handle(
            cx,
            Message {
                ts: format!("{:.6}", util::now_ms() as f64 / 1000.0),
                text,
                fresh: true,
                ..Message::default()
            },
        )
        .await?;
        return Ok(());
    }
    let tab = same_tab(cx, &form.meta);
    let over = form.meta.get("from").and_then(Value::as_str) != Some("thread");
    let root = form.root_view.clone().unwrap_or_default();
    let Some(tab) = tab else {
        if over {
            redraw(cx, &root, &form.meta, None).await;
        }
        return Ok(());
    };
    match form.kind.as_str() {
        "rename" => {
            let reply = if over {
                Reply::Screen {
                    view: root,
                    meta: form.meta.clone(),
                }
            } else {
                Reply::to(home_thread(cx, &tab.id).as_deref())
            };
            rename_tab(cx, &tab, form.value("name"), &reply).await?;
            refresh_dashboard(cx, Refresh::opened_quick()).await
        }
        "skill" => {
            let typed = form.value("typed").trim();
            let name = if typed.is_empty() {
                form.value("skill").trim()
            } else {
                typed
            };
            let name = name.trim_start_matches('/');
            if name.is_empty() {
                if over {
                    redraw(
                        cx,
                        &root,
                        &form.meta,
                        Some("No skill was chosen, so I didn't run anything."),
                    )
                    .await;
                }
                return Ok(());
            }
            if !SKILL_NAME.is_match(name) {
                if over {
                    redraw(
                        cx,
                        &root,
                        &form.meta,
                        Some(&format!(
                            "`{name}` isn't a skill name, so I didn't run anything."
                        )),
                    )
                    .await;
                }
                return Ok(());
            }
            let args = form.value("args").trim();
            let text = if args.is_empty() {
                format!("/{name}")
            } else {
                format!("/{name} {args}")
            };
            let home = send_from_screen(cx, &tab, &text, &format!("_Skill:_ `{text}`")).await?;
            // Claude Code answers its own commands in the tab, not as a turn.
            let where_ = if BUILT_IN.iter().any(|(n, _, _)| *n == name) {
                "Claude Code shows what it did in the tab, not here.".to_string()
            } else {
                format!("The answer goes in {}.", link_to(cx, home.as_deref()))
            };
            if over {
                redraw(
                    cx,
                    &root,
                    &form.meta,
                    Some(&format!("Ran `{}`. {where_}", as_mrkdwn(&text))),
                )
                .await;
            }
            Ok(())
        }
        "message" => {
            let text = form.value("text").trim();
            if text.is_empty() {
                return Ok(());
            }
            let home =
                send_from_screen(cx, &tab, text, &format!("_From the Home tab:_ {text}")).await?;
            let note = format!("Sent. The answer goes in {}.", link_to(cx, home.as_deref()));
            redraw(cx, &root, &form.meta, Some(&note)).await;
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Every tick: an open screen whose tab has changed state is drawn again,
/// and one whose tab has closed says so.
pub async fn refresh_screens(cx: &Cx, tabs: &[Tab]) {
    let open: Vec<(String, Open)> = cx
        .mem
        .borrow()
        .screens
        .open
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (view, o) in open {
        if util::now_ms() - o.at > SCREEN_KEEP_MS {
            forget_screen(cx, &view);
            continue;
        }
        let tab = tabs.iter().find(|t| t.id == o.w && t.pid == o.p);
        if tab.is_some_and(|t| t.state == o.state && t.since == o.since) {
            continue;
        }
        redraw(cx, &view, &json!({ "w": o.w, "p": o.p }), None).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claudes_markdown_reads_as_the_mrkdwn_a_modal_renders() {
        assert_eq!(
            as_mrkdwn(
                "## Done\n**All** *tests* pass, see [the PR](https://github.com/o/r/pull/1).\n- one & <two>\n`a **b**`"
            ),
            "*Done*\n*All* _tests_ pass, see <https://github.com/o/r/pull/1|the PR>.\n• one &amp; &lt;two&gt;\n`a **b**`"
        );
        assert_eq!(
            as_mrkdwn("```\n**x** <y>\n```"),
            "```\n**x** &lt;y&gt;\n```"
        );
        assert_eq!(
            as_mrkdwn("a_b*c* x*y"),
            "a_b*c* x*y",
            "a star inside a word is no italic"
        );
    }

    #[test]
    fn a_markdown_table_reads_as_a_bullet_a_row() {
        assert_eq!(
            as_mrkdwn("| Tab | State |\n|---|---|\n| 1 | waiting |\n| 2 | done |"),
            "• *1* · waiting\n• *2* · done"
        );
    }
}
