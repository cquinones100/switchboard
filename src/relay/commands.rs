//! What you send the relay (lib/slack-relay/commands.js): a reply in a
//! tab's thread is typed into that tab; at the top level, the commands
//! below. A message reaches a tab only as a reply in its thread, or through
//! a button or a form made for it: a tab is never named in a message, so a
//! message at the top level can't start work or reach a tab by accident.
//!
//! Typing into a tab takes care with what the tab shows: a tab showing
//! numbered choices gets just the keys for your answer, a dialog before a
//! session starts is answered with the arrow keys, and while either is up
//! anything that isn't one of its options is refused, since Enter after it
//! would accept whichever option is selected. A tab whose prompt box holds
//! text typed and left unsent is asked about first: typed after it, your
//! message and the draft would go as one prompt.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::Result;
use regex::Regex;
use serde_json::{Value, json};

use super::buttons::{Pick, dialog_buttons, option_buttons, picker_buttons, short_id};
use super::cleanup::{cleanup_button, closed_threads};
use super::homes::{home_of, home_thread, thread_link};
use super::post::{Button, PostOpts, offer_choice, post, react, settle_choice};
use super::slack::Message;
use super::state::{Follow, Target};
use super::summaries::{fleet, progress};
use super::tabs::{self, Tab, label, state_words, table};
use super::{Cx, Relay};
use crate::util;

/// What the relay understands, for `help`.
pub const HELP: &str = "**What I understand**\n\
- a reply in a tab's thread: typed into that tab\n\
- in a tab's thread, `progress`, `esc`, `rename <name>` (`rename` alone clears it): just for that tab\n\
- the Home tab: every tab, and Open for one's screen, to message, answer, stop, rename or close it\n\
- `new: …`: a new Claude tab, or `new <repo, ticket or ~/path>: …` to pick where\n\
- `progress`: what every tab is doing\n\
- `cleanup`: delete the threads of closed tabs (asks first)\n\
- `status` or `?`: every tab\n\
- `threads`: each tab's thread\n\
- `help`: this list\n\
- `stop`: end the relay";

/// Where an answer goes: posted in a thread (or the main DM), also sent to
/// the main DM with `broadcast`; or a tab's screen drawn again with it at
/// the bottom.
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    Post {
        thread: Option<String>,
        broadcast: bool,
    },
    Screen {
        view: String,
        meta: Value,
    },
}

impl Reply {
    pub fn to(thread: Option<&str>) -> Reply {
        Reply::Post {
            thread: thread.map(str::to_string),
            broadcast: false,
        }
    }
}

/// Says `body` where `to` says, giving the post's ts.
pub async fn say(cx: &Cx, to: &Reply, body: &str, buttons: Vec<Button>) -> Result<Option<String>> {
    match to {
        Reply::Post { thread, broadcast } => post(
            cx,
            &format!("🤖 {body}"),
            thread.as_deref(),
            PostOpts {
                broadcast: *broadcast,
                buttons,
            },
        )
        .await
        .map(Some),
        Reply::Screen { view, meta } => {
            super::screens::redraw(cx, view, meta, Some(&super::screens::as_mrkdwn(body))).await;
            Ok(None)
        }
    }
}

/// A word quoted for the shell, its line breaks made spaces.
pub fn shell_quote(text: &str) -> String {
    static BREAK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\n\s*").expect("a regex"));
    format!("'{}'", BREAK.replace_all(text, " ").replace('\'', r"'\''"))
}

/// A tab as an alert names it, with what is known of it now.
pub fn named(cx: &Relay, window: &str, index: i64, name: &str) -> Tab {
    let fresh = tabs::tab(cx, window);
    Tab {
        id: window.to_string(),
        index,
        name: name.to_string(),
        ..fresh.unwrap_or_default()
    }
}

fn target(tab: &Tab, thread: Option<&str>) -> Target {
    Target {
        window: tab.id.clone(),
        index: tab.index,
        name: tab.name.clone(),
        thread: thread.map(str::to_string),
    }
}

/// Acts on one of your messages, giving the ts of what it posted for a tab
/// (its confirmation or answer), when that is the thread to follow.
pub async fn handle(cx: &Cx, message: Message) -> Result<Option<String>> {
    let mut text = message.text.trim().to_string();
    // A reply goes where the message was: in its thread if it came in one,
    // at the top level otherwise.
    let thread = message.thread_ts.clone();
    if let Some(t) = &thread {
        cx.state.borrow_mut().watch_thread(t, util::now_ms());
    }
    let here = Reply::to(thread.as_deref());
    let sent_ms = (super::state::number(&message.ts) * 1000.0) as i64;
    let waited = (util::now_ms() - sent_ms) as f64 / 1000.0;
    cx.log(&format!(
        "← {} ({waited:.1}s after sending): {}",
        if thread.is_some() { "thread" } else { "DM" },
        text.chars().take(80).collect::<String>()
    ));

    // Sent while the relay wasn't running: typing it into a tab now could
    // be long after the moment it was meant for.
    if !message.fresh && sent_ms < cx.started_at - 60_000 {
        return say(cx, &here, "I missed this while I wasn't running, so I didn't act on it. Send it again if you still want it.", vec![]).await;
    }

    // Images you attached are downloaded and named at the end of the
    // message, as paths Claude Code can read, and a video by its path and
    // then its stills.
    for file in &message.files {
        let name = file.name.clone();
        match super::files::download(cx, file).await {
            Err(e) => {
                say(
                    cx,
                    &here,
                    &format!(
                        "I couldn't download {}: {e}",
                        name.as_deref().unwrap_or("that file")
                    ),
                    vec![],
                )
                .await?;
            }
            Ok(super::files::Got::Refused(why)) => {
                say(cx, &here, &why, vec![]).await?;
            }
            Ok(super::files::Got::Image(path)) => {
                text = join(&text, &format!("[Image: {}]", path.display()));
            }
            Ok(super::files::Got::Video { path, frames }) => {
                text = join(&text, &format!("[Video: {}]", path.display()));
                if frames.is_empty() {
                    say(
                        cx,
                        &here,
                        &format!(
                            "I couldn't take stills from {}, so the tab gets only its path.",
                            name.as_deref().unwrap_or("that video")
                        ),
                        vec![],
                    )
                    .await?;
                } else {
                    text.push_str(&format!(
                        " Stills from it, in order: {}",
                        frames
                            .iter()
                            .map(|f| format!("[Image: {}]", f.display()))
                            .collect::<Vec<_>>()
                            .join(" ")
                    ));
                }
            }
        }
    }
    if text.is_empty() {
        return Ok(None);
    }

    // A reply under a tab alert is for that tab.
    let alert = thread
        .as_ref()
        .and_then(|t| cx.state.borrow().alerts.get(t).cloned());
    if let Some(alert) = alert {
        let tab = named(cx, &alert.window, alert.index, &alert.name);
        static RENAME: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?is)^rename(?:\s+(.+))?$").expect("a regex"));
        if text.eq_ignore_ascii_case("esc") {
            return interrupt(cx, &tab, &here).await;
        }
        if text.eq_ignore_ascii_case("progress") {
            return progress(cx, &tab, &here, Some(&message.ts)).await;
        }
        if let Some(c) = RENAME.captures(&text) {
            let name = c.get(1).map(|m| m.as_str().to_string());
            return rename_tab(cx, &tab, name.as_deref().unwrap_or(""), &here).await;
        }
        let request = message.request.clone().unwrap_or(message.ts.clone());
        return to_tab(
            cx,
            thread.as_deref(),
            Some(&request),
            &tab,
            &text,
            &here,
            false,
        )
        .await;
    }

    let lower = text.to_lowercase();
    if lower == "stop" {
        say(cx, &here, "**Stopped listening.**", vec![]).await?;
        super::lifecycle::stop_relay(cx).await;
        return Ok(None);
    }
    if lower == "help" {
        return say(cx, &here, HELP, vec![]).await;
    }
    // Deleting can't be undone, so the command only offers the button,
    // which Slack confirms.
    if lower == "cleanup" || lower == "clean up" {
        let count = closed_threads(cx).len();
        if count == 0 {
            return say(
                cx,
                &here,
                "No closed tabs have threads left to delete.",
                vec![],
            )
            .await;
        }
        let body = format!(
            "**{count} closed tab{} a thread.** Delete them? Your own replies in them stay.",
            if count == 1 { " has" } else { "s have" }
        );
        return say(cx, &here, &body, vec![cleanup_button(count)]).await;
    }
    if lower == "status" || lower == "?" {
        let tabs = tabs::claude_tabs(cx);
        if tabs.is_empty() {
            return say(cx, &here, "**Status**\n\nNo Claude tabs are open.", vec![]).await;
        }
        let now = util::now_ms();
        let rows: Vec<Vec<String>> = tabs
            .iter()
            .map(|t| {
                let since = tabs::since_ms(&t.since)
                    .map(|s| tabs::lasted(now - s))
                    .unwrap_or_default();
                let context = tabs::context_of(&cx.paths, t.transcript.as_deref())
                    .map(|c| format!("{}%", c.percent))
                    .unwrap_or_else(|| "—".into());
                let thread = home_thread(cx, &t.id)
                    .map(|h| thread_link(cx, &h, "open"))
                    .unwrap_or_else(|| "—".into());
                vec![
                    format!("**{}**", t.index),
                    t.name.clone(),
                    state_words(&t.state),
                    since,
                    context,
                    thread,
                ]
            })
            .collect();
        let body = format!(
            "**Status**\n\n{}",
            table(&["Tab", "Name", "State", "For", "Context", "Thread"], &rows)
        );
        return say(cx, &here, &body, vec![]).await;
    }
    // Each tab's thread, to find a conversation again.
    if lower == "threads" {
        let tabs = tabs::claude_tabs(cx);
        if tabs.is_empty() {
            return say(cx, &here, "**Threads**\n\nNo Claude tabs are open.", vec![]).await;
        }
        let rows: Vec<Vec<String>> = tabs
            .iter()
            .map(|t| {
                vec![
                    format!("**{}**", t.index),
                    t.name.clone(),
                    state_words(&t.state),
                    home_thread(cx, &t.id)
                        .map(|h| thread_link(cx, &h, "open"))
                        .unwrap_or_else(|| "—".into()),
                ]
            })
            .collect();
        let without: Vec<String> = tabs
            .iter()
            .filter(|t| home_thread(cx, &t.id).is_none())
            .map(|t| format!("tab {}", t.index))
            .collect();
        let mut body = format!(
            "**Threads**\n\n{}",
            table(&["Tab", "Name", "State", "Thread"], &rows)
        );
        if !without.is_empty() {
            body.push_str(&format!(
                "\n\n_No thread yet for {}: open it on the Home tab and send it a Message to start one._",
                without.join(", ")
            ));
        }
        return say(cx, &here, &body, vec![]).await;
    }
    if lower == "progress" || lower.split_whitespace().collect::<Vec<_>>() == ["progress", "all"] {
        return fleet(cx, &here, Some(&message.ts)).await;
    }

    // New work has to be asked for by name: a new tab running Claude with
    // the prompt, in the relay's directory, or the place named before the
    // colon (see places.rs). The reply says where, so a wrong match is
    // plain. Replies under the confirmation go to that tab.
    static NEW: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?is)^new(?:\s+([^:\n]+?))?\s*:\s*(.+)$").expect("a regex"));
    if let Some(c) = NEW.captures(&text) {
        let place = match c.get(1) {
            Some(spec) => super::places::resolve_place(cx, spec.as_str().trim()),
            None => Ok((cx.cwd.clone(), format!("`{}`", cx.paths.short(&cx.cwd)))),
        };
        let (dir, said) = match place {
            Ok(p) => p,
            Err(why) => return say(cx, &here, &why, vec![]).await,
        };
        let (window, pane) = cx.mux.new_tab(&dir, None, None)?;
        // The pane Claude starts in is the tab's, whatever is opened beside
        // it later.
        let _ = cx.mux.set_option(&window, tabs::CLAUDE_PANE, Some(&pane));
        let prompt = c.get(2).map(|m| m.as_str().trim()).unwrap_or("");
        cx.mux
            .type_text(&pane, &format!("claude {}", shell_quote(prompt)))?;
        cx.mux.send_keys(&pane, &["Enter"])?;
        let opened = cx
            .mux
            .windows()
            .unwrap_or_default()
            .into_iter()
            .find(|w| w.id == window);
        let index = opened.as_ref().map(|w| w.index).unwrap_or(0);
        let pid = opened
            .as_ref()
            .and_then(|w| w.panes.first())
            .map(|p| p.pid.clone());
        cx.log(&format!("new: tab {index} in {}", dir.display()));
        let ts = say(
            cx,
            &here,
            &format!("Started a new Claude in tab {index}, in {said}. This is its thread: reply here to talk to it."),
            vec![],
        )
        .await?
        .unwrap_or_default();
        let mut state = cx.state.borrow_mut();
        state.alerts.insert(
            ts.clone(),
            Target {
                window: window.clone(),
                index,
                name: "new Claude".into(),
                thread: None,
            },
        );
        // Started from the top level, this message is the new tab's home.
        if thread.is_none()
            && let Some(pid) = pid
        {
            state.homes.insert(
                window,
                super::state::Home {
                    ts: ts.clone(),
                    pid,
                    ..Default::default()
                },
            );
        }
        return Ok(Some(ts));
    }

    // Anything else is not acted on. With tabs open, a button per tab
    // offers to send it to one; tapping it is the choice of tab.
    let tabs = tabs::claude_tabs(cx);
    if tabs.is_empty() {
        return say(
            cx,
            &here,
            &format!("I didn't recognise that, so **I didn't act on it**, and no Claude tabs are open.\n\n{HELP}"),
            vec![],
        )
        .await;
    }
    let body = format!(
        "{} **Send it to a tab?**\n\n_`help` lists what I understand._",
        if thread.is_some() {
            "This thread isn't linked to a tab."
        } else {
            "That doesn't name a tab or a command."
        }
    );
    let buttons = picker_buttons(
        cx,
        &tabs,
        Pick {
            ts: message.ts.clone(),
            text,
            thread_ts: thread.clone(),
        },
    );
    say(cx, &here, &body, buttons).await
}

fn join(text: &str, more: &str) -> String {
    if text.is_empty() {
        more.to_string()
    } else {
        format!("{text} {more}")
    }
}

/// A message for a tab already chosen: by a button or a form made for it,
/// or the picker's tab button. It goes straight to that tab, never through
/// a thread's link to one, which another message may have repointed. Sent
/// from a thread, the conversation stays there; sent at the top level, it
/// goes on in the tab's home thread, and its answer is also sent to the
/// main DM. `?` (or nothing) answers where it was asked.
pub async fn for_tab(
    cx: &Cx,
    tab: &Tab,
    thread: Option<&str>,
    request: Option<&str>,
    text: &str,
) -> Result<Option<String>> {
    if text == "?" || text.is_empty() {
        return to_tab(cx, thread, request, tab, text, &Reply::to(thread), false).await;
    }
    let conversation = match thread {
        Some(t) => Some(t.to_string()),
        None => match home_of(cx, tab).await? {
            Some(home) => Some(home),
            None => request.map(str::to_string),
        },
    };
    if let Some(c) = &conversation {
        cx.state.borrow_mut().watch_thread(c, util::now_ms());
    }
    let broadcast = thread.is_none() && conversation.as_deref() != request;
    let reply = Reply::Post {
        thread: conversation.clone(),
        broadcast,
    };
    to_tab(
        cx,
        conversation.as_deref(),
        request,
        tab,
        text,
        &reply,
        broadcast,
    )
    .await
}

/// Names a tab, as M-p's "Rename the tab" does; no name gives it its own
/// name again (its ticket's or directory's). The tab bar, the sidebar and
/// the tab's thread header follow.
pub async fn rename_tab(cx: &Cx, tab: &Tab, name: &str, reply: &Reply) -> Result<Option<String>> {
    let new_name: String = name
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(60)
        .collect();
    cx.mux.rename_tab(&tab.id, &new_name)?;
    cx.log(&format!(
        "renamed tab {} to {}",
        tab.index,
        if new_name.is_empty() {
            "its own name"
        } else {
            &new_name
        }
    ));
    let body = if new_name.is_empty() {
        format!("**Tab {}** has its own name again.", tab.index)
    } else {
        format!("Renamed **tab {}** to **{new_name}**.", tab.index)
    };
    say(cx, reply, &body, vec![]).await
}

/// Closes a tab, ending its Claude session, and says so in its thread (or
/// the main DM if it has none). Its thread is marked closed once the tabs
/// are next looked at (see alerts.rs).
pub async fn close_tab(cx: &Cx, tab: &Tab) -> Result<()> {
    cx.mux.kill_tab(&tab.id)?;
    cx.log(&format!("closed {}", label(tab.index, &tab.name)));
    cx.state.borrow_mut().following.shift_remove(&tab.id);
    let home = cx.state.borrow().homes.get(&tab.id).map(|h| h.ts.clone());
    post(
        cx,
        &format!("🤖 Closed **{}**.", label(tab.index, &tab.name)),
        home.as_deref(),
        PostOpts::default(),
    )
    .await?;
    Ok(())
}

/// Presses Escape in a tab, which stops Claude mid-turn, or answers "no"
/// to a prompt it is waiting on. Only then: in an idle Claude, Escape does
/// nothing useful, and a second one opens the rewind menu. The tab is no
/// longer followed, since an interrupted turn may never report that it
/// ended; the reply is linked to the tab, so `?` under it shows where it
/// stopped.
pub async fn interrupt(cx: &Cx, tab: &Tab, reply: &Reply) -> Result<Option<String>> {
    let name = label(tab.index, &tab.name);
    let pane = match tabs::claude_pane(cx, &tab.id) {
        Ok(p) => p,
        Err(e) => return say(cx, reply, &e, vec![]).await,
    };
    let current = tabs::tab(cx, &tab.id);
    if !matches!(
        current.as_ref().map(|t| t.state.as_str()),
        Some("working" | "waiting")
    ) {
        return say(
            cx,
            reply,
            &format!("{name} isn't in the middle of anything, so I didn't press Escape."),
            vec![],
        )
        .await;
    }
    cx.mux.send_keys(&pane, &["Escape"])?;
    cx.state.borrow_mut().following.shift_remove(&tab.id);
    cx.log(&format!("⎋ {name}"));
    let ts = say(
        cx,
        reply,
        &format!("Pressed Escape in {name}. `?` here shows where it stopped."),
        vec![],
    )
    .await?;
    if let Some(ts) = &ts {
        cx.state
            .borrow_mut()
            .alerts
            .insert(ts.clone(), target(tab, None));
    }
    Ok(ts)
}

/// The setting that turns off asking about a draft.
pub const DRAFT_CHECK: &str = "relay_draft_check";

/// What to do with text left unsent in a tab's prompt box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverDraft {
    /// Ask first, with buttons for the others.
    Ask,
    /// Type the message after it, so both go as one prompt.
    After,
    /// Clear it, then type the message.
    Replace,
}

/// A message held while you're asked about a tab's draft, under the id its
/// buttons carry.
#[derive(Clone, Debug)]
pub struct Waiting {
    pub id: String,
    pub thread: Option<String>,
    pub request: Option<String>,
    pub text: String,
    pub reply: Reply,
    pub broadcast: bool,
}

/// The message waiting on each tab's draft, by tab: a second one for the
/// same tab takes the first's place.
pub type Drafts = HashMap<String, Waiting>;

/// Types your message (or answer) into a tab. `request` is your message,
/// which gets 👀 once the tab's Claude takes it (see followups.rs).
pub async fn to_tab(
    cx: &Cx,
    thread: Option<&str>,
    request: Option<&str>,
    tab: &Tab,
    text: &str,
    reply: &Reply,
    broadcast: bool,
) -> Result<Option<String>> {
    send_to_tab(
        cx,
        thread,
        request,
        tab,
        text,
        reply,
        broadcast,
        OverDraft::Ask,
    )
    .await
}

/// A message that waited on a tab's draft, sent as the button pressed says;
/// with the draft gone by then (sent or cleared at the keyboard), typed as
/// it is, and said so.
pub async fn send_waiting(
    cx: &Cx,
    tab: &Tab,
    waiting: Waiting,
    over: OverDraft,
) -> Result<Option<String>> {
    // A choice the tab shows now came after the message was held, so the
    // message is never taken as its answer.
    if showing_choice(cx, tab, &tabs::screen(cx, &tab.id)) {
        return say(cx, &waiting.reply, &choice_now(tab), vec![]).await;
    }
    let gone = tabs::claude_pane(cx, &tab.id)
        .ok()
        .and_then(|p| tabs::draft(cx, &p))
        .is_none();
    // Nothing is awaited between the check above and the typing, so a
    // choice can't come up in between; the note on the draft comes after.
    let sent = send_to_tab(
        cx,
        waiting.thread.as_deref(),
        waiting.request.as_deref(),
        tab,
        &waiting.text,
        &waiting.reply,
        waiting.broadcast,
        over,
    )
    .await?;
    // Typing posts nothing, so a post means it said why it didn't type.
    if gone && sent.is_none() {
        let name = label(tab.index, &tab.name);
        say(
            cx,
            &waiting.reply,
            &format!("The draft in {name} was gone, so I typed your message."),
            vec![],
        )
        .await?;
    }
    Ok(sent)
}

/// Whether a tab shows a choice: numbered options while it waits on you,
/// or a dialog before its session starts.
fn showing_choice(cx: &Cx, tab: &Tab, screen: &str) -> bool {
    let waiting_on_you = tabs::tab(cx, &tab.id).is_some_and(|t| t.state == "waiting");
    (waiting_on_you && tabs::options_in(screen).is_some()) || tabs::dialog_in(screen).is_some()
}

fn choice_now(tab: &Tab) -> String {
    format!(
        "{} is showing a choice now, so I didn't type your message. Send it again once that's answered.",
        label(tab.index, &tab.name)
    )
}

/// Asks what to do with the draft in a tab's prompt box, holding the
/// message until a button says.
async fn ask_about_draft(
    cx: &Cx,
    tab: &Tab,
    draft: &str,
    waiting: Waiting,
) -> Result<Option<String>> {
    let name = label(tab.index, &tab.name);
    let shown: String = draft.chars().take(300).collect();
    let quoted = shown
        .lines()
        .map(|l| format!("> {l}"))
        .collect::<Vec<_>>()
        .join("\n");
    let body = format!(
        "**{name}** has text typed and unsent in its prompt:\n{quoted}\nWhat do I do with your message? Sent after it, the two go as one prompt."
    );
    let button = |label: &str, c: &str| {
        Button::new(
            label,
            json!({ "a": "draft", "c": c, "w": tab.id, "p": waiting.id }),
        )
    };
    let buttons = vec![
        button("Replace it", "replace").style("danger"),
        button("Send after it", "after"),
        button("Keep it", "keep"),
    ];
    cx.log(&format!("✋ {name} has a draft; asked before typing"));
    let reply = waiting.reply.clone();
    cx.mem.borrow_mut().drafts.insert(tab.id.clone(), waiting);
    say(cx, &reply, &body, buttons).await
}

/// Clears a prompt box's draft with Ctrl-U, once a line and once more, `i`
/// first in vim's Normal mode, giving whether it is gone.
async fn clear_draft(cx: &Cx, pane: &str, draft: &str) -> bool {
    let screen = cx.mux.capture(pane, 0).unwrap_or_default();
    if crate::mission::answer::normal_mode(&screen) {
        let _ = cx.mux.type_text(pane, "i");
    }
    for _ in 0..=draft.lines().count() {
        let _ = cx.mux.send_keys(pane, &["C-u"]);
    }
    let until = std::time::Instant::now() + Duration::from_secs(2);
    while tabs::draft(cx, pane).is_some() {
        if std::time::Instant::now() >= until {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    true
}

#[allow(clippy::too_many_arguments)]
async fn send_to_tab(
    cx: &Cx,
    thread: Option<&str>,
    request: Option<&str>,
    tab: &Tab,
    text: &str,
    reply: &Reply,
    broadcast: bool,
    over: OverDraft,
) -> Result<Option<String>> {
    let name = label(tab.index, &tab.name);
    if text == "?" || text.is_empty() {
        let turn = tabs::last_turn(cx, &tab.id, None);
        return say(cx, reply, &format!("Last turn in {name}:\n{turn}"), vec![]).await;
    }
    // A newer message for the tab takes the place of one waiting on its
    // draft, whether or not it is asked about in turn.
    cx.mem.borrow_mut().drafts.remove(&tab.id);
    let pane = match tabs::claude_pane(cx, &tab.id) {
        Ok(p) => p,
        Err(e) => return say(cx, reply, &e, vec![]).await,
    };
    let mut before = tabs::screen(cx, &tab.id);
    let current = tabs::tab(cx, &tab.id);
    let aimed = target(tab, thread);
    // A tab showing numbered choices gets just the keys for your answer: an
    // option's number picks it at once, and an Enter after it would land on
    // whatever comes next.
    let prompt = current
        .as_ref()
        .filter(|t| t.state == "waiting")
        .and_then(|_| tabs::options_in(&before));
    let answer = prompt.as_ref().and_then(|p| tabs::answer_for(p, text));
    if let (Some(prompt), Some(answer)) = (&prompt, &answer) {
        tabs::answer_prompt(cx, &pane, prompt, answer).await;
        let did = tabs::answer_said(answer);
        cx.log(&format!("→ {name}: {did}"));
        settle_choice(cx, &tab.id, &format!("{did}."), None).await;
        if let Some(t) = thread {
            cx.state
                .borrow_mut()
                .alerts
                .insert(t.to_string(), aimed.clone());
        }
        // Claude Code may still be asking, in the same wait: the next of
        // several questions, the one you ticked an option in, or the review
        // of your answers. No hook says so, so it is posted here, with its
        // buttons.
        let after = tabs::screen_after(cx, &tab.id, &before, Duration::from_secs(2)).await;
        let now = tabs::tab(cx, &tab.id);
        let same_wait = now.as_ref().is_some_and(|n| {
            n.state == "waiting" && Some(&n.since) == current.as_ref().map(|c| &c.since)
        });
        let next = same_wait.then(|| tabs::options_in(&after)).flatten();
        // The same prompt still up after a pick that takes one answer only
        // means Claude Code hasn't drawn what comes next yet.
        if let (Some(next), Some(now)) = (&next, &now)
            && (prompt.multi || tabs::hash_of(next) != tabs::hash_of(prompt))
        {
            let moved = next.question != prompt.question;
            let body = format!(
                "{did} in {name}.{}\n\n{}",
                if moved { " It's asking next:" } else { "" },
                tabs::prompt_text(next)
            );
            let ts = say(cx, reply, &body, option_buttons(now, next)).await?;
            if let Some(ts) = &ts {
                cx.state
                    .borrow_mut()
                    .alerts
                    .insert(ts.clone(), aimed.clone());
                offer_choice(cx, &tab.id, "wait", ts, &format!("🤖 {body}"));
            }
            return Ok(ts);
        }
        // The turn carries on, so it is followed as the prompt it belongs to.
        let asked = tabs::transcript_for(cx, &tab.id)
            .and_then(|f| tabs::last_prompt(&f))
            .map(|p| p.prompt)
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| text.to_string());
        cx.state.borrow_mut().following.insert(
            tab.id.clone(),
            Follow {
                thread: thread.map(str::to_string),
                index: tab.index,
                name: tab.name.clone(),
                text: asked,
                request: request.map(str::to_string),
                sent_at: util::now_ms(),
                screen: before.clone(),
                taken: true,
                ..Follow::default()
            },
        );
        let ts = say(
            cx,
            reply,
            &format!("{did} in {name}. I'll tell you when it's done."),
            vec![],
        )
        .await?;
        if let Some(ts) = &ts {
            cx.state.borrow_mut().alerts.insert(ts.clone(), aimed);
        }
        return Ok(ts);
    }
    // A dialog before the session starts is answered with the arrow keys:
    // up or down to the chosen option, then Enter.
    let dialog = tabs::dialog_in(&before);
    let picked = dialog
        .as_ref()
        .and_then(|d| tabs::chosen_option(&d.options, text));
    if let (Some(dialog), Some(picked)) = (&dialog, &picked) {
        let moves = picked.n as i64 - dialog.cursor as i64;
        let key = if moves > 0 { "Down" } else { "Up" };
        for _ in 0..moves.abs() {
            cx.mux.send_keys(&pane, &[key])?;
        }
        cx.mux.send_keys(&pane, &["Enter"])?;
        cx.log(&format!("→ {name}: chose \"{}\" in a dialog", picked.label));
        if let Some(t) = thread {
            cx.state
                .borrow_mut()
                .alerts
                .insert(t.to_string(), aimed.clone());
        }
        settle_choice(cx, &tab.id, &format!("Answered: {}", picked.label), None).await;
        let ts = say(
            cx,
            reply,
            &format!("Chose \"{}\" in {name}.", picked.label),
            vec![],
        )
        .await?;
        if let Some(ts) = &ts {
            cx.state.borrow_mut().alerts.insert(ts.clone(), aimed);
        }
        return Ok(ts);
    }
    // Anything else typed at a choice would be followed by Enter, which
    // accepts whichever option is selected: "cancel" would approve the
    // command. So only an answer that names an option goes through; the
    // refusal carries the choice's buttons, so the answer is a tap away.
    if prompt.is_some() || dialog.is_some() {
        let (shown, buttons, kind) = match (&prompt, &dialog) {
            (Some(p), _) => (
                tabs::prompt_text(p),
                option_buttons(current.as_ref().unwrap_or(tab), p),
                "wait",
            ),
            (None, Some(d)) => (
                format!(
                    "{}\n_Tap one, or reply with its number or the start of it._",
                    d.options
                        .iter()
                        .map(|o| format!("{}. {}", o.n, o.label))
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
                dialog_buttons(current.as_ref().unwrap_or(tab), d),
                "dialog",
            ),
            (None, None) => unreachable!("one of them is there"),
        };
        let body = format!(
            "{name} is showing a choice, and that isn't one of its options, so I didn't type anything.\n\n{shown}"
        );
        let ts = say(cx, reply, &body, buttons).await?;
        if let Some(ts) = &ts {
            offer_choice(cx, &tab.id, kind, ts, &format!("🤖 {body}"));
        }
        return Ok(ts);
    }
    // Text typed in the prompt box and left unsent would go with the
    // message as one prompt, so you're asked first.
    match (over, tabs::draft(cx, &pane)) {
        (OverDraft::Ask, Some(draft)) if !cx.setting_off(DRAFT_CHECK) => {
            let waiting = Waiting {
                id: short_id(),
                thread: thread.map(str::to_string),
                request: request.map(str::to_string),
                text: text.to_string(),
                reply: reply.clone(),
                broadcast,
            };
            // The thread belongs to the tab already, so a reply while you
            // decide reaches it, taking the waiting message's place.
            if let Some(t) = thread {
                cx.state
                    .borrow_mut()
                    .alerts
                    .insert(t.to_string(), aimed.clone());
            }
            return ask_about_draft(cx, tab, &draft, waiting).await;
        }
        (OverDraft::Replace, Some(draft)) => {
            if !clear_draft(cx, &pane, &draft).await {
                let body =
                    format!("I couldn't clear the draft in {name}, so I didn't type your message.");
                return say(cx, reply, &body, vec![]).await;
            }
            // A choice that came up while the draft was cleared would take
            // the message as its answer.
            before = tabs::screen(cx, &tab.id);
            if showing_choice(cx, tab, &before) {
                return say(cx, reply, &choice_now(tab), vec![]).await;
            }
        }
        _ => {}
    }
    // Typed while Claude is mid-turn, the message waits in Claude Code's
    // queue until the turn ends.
    let busy = current.as_ref().is_some_and(|t| t.state == "working");
    tabs::type_into(cx, &pane, text).await;
    cx.log(&format!(
        "→ {name}: {}",
        text.chars().take(80).collect::<String>()
    ));
    // The conversation now belongs to the tab: replies in the thread it
    // came from go there, and when the tab is done the result is posted in
    // the same place. Nothing is posted: your message's reaction says where
    // it is (⏳ queued, 👀 taken, ✅ answered).
    if let Some(t) = thread {
        cx.state.borrow_mut().alerts.insert(t.to_string(), aimed);
    }
    cx.state.borrow_mut().following.insert(
        tab.id.clone(),
        Follow {
            thread: thread.map(str::to_string),
            index: tab.index,
            name: tab.name.clone(),
            text: text.to_string(),
            request: request.map(str::to_string),
            broadcast,
            sent_at: util::now_ms(),
            screen: before,
            ..Follow::default()
        },
    );
    if busy && let Err(e) = react(cx, request, "hourglass_flowing_sand").await {
        cx.log(&format!("⏳ failed: {e}"));
    }
    Ok(None)
}
