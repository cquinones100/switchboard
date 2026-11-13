//! What the relay says by itself when a tab needs you or has finished
//! (lib/slack-relay/alerts.js): an alert in the tab's thread carrying what
//! `?` would show, so the answer or the question is readable without asking
//! for it, and for a wait, a line in the main DM, which is what notifies
//! you and which waits while you're at the computer (see presence.rs). A
//! dialog before a session starts, which no hook reports, is read off the
//! screen.

use std::collections::HashMap;
use std::path::PathBuf;

use futures_util::future::LocalBoxFuture;

use super::buttons::{dialog_buttons, option_buttons, turn_buttons};
use super::followups::{Event, context_footer, warn_context};
use super::homes::{close_home, home_of};
use super::post::{Button, PostOpts, offer_choice, post, settle_choice};
use super::sending::{self, Kind, Send, Sent};
use super::state::Target;
use super::tabs::{self, Tab, label};
use super::{Cx, Relay};
use crate::util;

/// A tab with no state is checked for a dialog at most this often: reading
/// its screen is a call to the server, and the relay looks at its tabs
/// every second.
pub const DIALOG_CHECK_MS: i64 = 5_000;

/// What this module keeps: when each tab was last checked for a dialog.
#[derive(Default)]
pub struct Memory {
    pub dialog_checked: HashMap<String, i64>,
}

pub const PROMPT_GONE: &str = "This prompt is no longer showing.";

/// A line in the main DM pointing at a tab's thread, which is what
/// notifies you; it waits while you're at the computer. A notice for a
/// prompt carries the prompt's buttons, so it can be answered from the
/// notice itself, as long as the prompt is still showing when the notice
/// goes out (`still`), since it may have waited for you to step away.
/// `sent` is what the line is about: as it goes out, its kind's setting
/// is read again (it may have changed, or quiet hours begun, while it was
/// held), and it is counted as notifying only once it is posted.
#[allow(clippy::too_many_arguments)]
pub async fn notify(
    cx: &Cx,
    text: String,
    home: Option<String>,
    target: Target,
    buttons: Vec<Button>,
    kind: &'static str,
    still: Box<dyn Fn(&Relay) -> bool>,
    sent: Sent,
) -> anyhow::Result<()> {
    let cx2 = cx.clone();
    let send: LocalBoxFuture<'static, anyhow::Result<()>> = Box::pin(async move {
        if sending::send_now(&cx2, sent.kind, sent.autopilot) != Send::Notify {
            cx2.log(&format!(
                "a held line not sent: relay_send_{} no longer notifies",
                sent.kind.name()
            ));
            return Ok(());
        }
        let live = !buttons.is_empty() && still(&cx2);
        let ts = post(
            &cx2,
            &text,
            home.as_deref(),
            PostOpts {
                broadcast: true,
                buttons: if live { buttons } else { Vec::new() },
            },
        )
        .await?;
        let window = target.window.clone();
        cx2.state.borrow_mut().alerts.insert(ts.clone(), target);
        if live {
            offer_choice(&cx2, &window, kind, &ts, &text);
        }
        cx2.save();
        sending::record(
            &cx2,
            &Sent {
                notified: true,
                ..sent
            },
        );
        Ok(())
    });
    super::presence::deliver(cx, send).await
}

/// A prompt that is no longer showing (answered at the computer, or the
/// tab has moved on or closed) leaves no buttons behind.
pub async fn settle_gone(cx: &Relay, tabs: &[Tab]) {
    let offered: Vec<(String, String)> = cx
        .state
        .borrow()
        .choices
        .iter()
        .map(|(id, o)| (id.clone(), o.kind.clone()))
        .collect();
    for (id, kind) in offered {
        let gone = match tabs.iter().find(|t| t.id == id) {
            None => true,
            Some(t) if kind == "wait" => t.state != "waiting",
            Some(t) => t.state != "idle",
        };
        if gone {
            settle_choice(cx, &id, PROMPT_GONE, None).await;
        }
    }
}

/// A tab to alert about, with what its alert shows when it came from a
/// hook's event rather than the tab's state.
struct Due {
    tab: Tab,
    turn: Option<String>,
    transcript: Option<PathBuf>,
    said: Option<String>,
}

pub async fn alert_tabs(cx: &Cx, tabs: &[Tab], events: &[Event]) {
    settle_gone(cx, tabs).await;
    // Homes of tabs that have closed are forgotten. Only a home whose tab
    // isn't a Claude tab needs the server asked whether it is still open.
    let unsure = cx
        .state
        .borrow()
        .homes
        .keys()
        .any(|id| !tabs.iter().any(|t| &t.id == id));
    if unsure && let Some(windows) = cx.mux.windows() {
        let gone: Vec<String> = cx
            .state
            .borrow()
            .homes
            .keys()
            .filter(|id| !windows.iter().any(|w| &&w.id == id))
            .cloned()
            .collect();
        for id in gone {
            close_home(cx, &id).await;
        }
    }
    dialogs(cx, tabs).await;
    // A turn already posted from its hook event (below) whose tab is then
    // marked done, by focusing away from it, counts as reported.
    let posted_turn = |t: &Tab| {
        let state = cx.state.borrow();
        t.state == "done"
            && match (t.since.parse::<f64>(), state.turn_posted.get(&t.id)) {
                (Ok(since), Some(at)) => (since - *at as f64).abs() <= 10.0,
                _ => false,
            }
    };
    // A loop tab finishes a turn on every tick, which is nothing to tell you
    // about; only its waits are.
    let mut due: Vec<Due> = tabs
        .iter()
        .filter(|t| {
            let state = cx.state.borrow();
            (t.state == "waiting" || (t.state == "done" && !t.loop_tab))
                && state.reported.get(&t.id) != Some(&t.since)
                && !state.following.contains_key(&t.id)
        })
        .filter(|t| !posted_turn(t))
        .map(|t| Due {
            tab: t.clone(),
            turn: None,
            transcript: None,
            said: None,
        })
        .collect();
    // A turn that ends in the tab you're looking at is never marked done:
    // you saw it end. Its thread still gets it, from the hook's event and the
    // final message the event carries, so the thread is complete.
    let mut unseen: Vec<Due> = Vec::new();
    for e in events
        .iter()
        .filter(|e| e.event == "done" && !e.handled && e.transcript.is_some())
    {
        let file = PathBuf::from(e.transcript.as_deref().unwrap_or(""));
        let Some(tab) = tabs.iter().find(|t| t.transcript.as_ref() == Some(&file)) else {
            continue;
        };
        // Only the tab in front of you: any other is marked done, and
        // alerted from that above.
        if tab.loop_tab
            || !tab.focused
            || tab.state == "done"
            || tab.state == "waiting"
            || cx.state.borrow().following.contains_key(&tab.id)
        {
            continue;
        }
        let since = format!("turn:{}", e.time);
        if cx.state.borrow().reported.get(&tab.id) == Some(&since) {
            continue;
        }
        unseen.retain(|d| d.tab.id != tab.id);
        unseen.push(Due {
            tab: Tab {
                state: "done".into(),
                since,
                ..tab.clone()
            },
            turn: tabs::turn_from(&file, None, e.last.as_deref()),
            transcript: Some(file),
            said: e.last.clone(),
        });
    }
    due.extend(unseen);
    for d in due {
        alert(cx, d).await;
        cx.save();
    }
}

/// Whether a finished turn another tab's message or a background task
/// started, or one that wrote no reply, is posted in its tab's thread.
pub const OTHER_TURNS: &str = "relay_send_other_turns";

/// What becomes of a finished turn: posted, counted as reported without a
/// post, or left for a later tick while its transcript catches up.
enum Turn {
    Post,
    Skip,
    Later,
}

/// A tab waiting on you is always posted; a finished turn only when you
/// started it and it wrote a reply, unless Settings' `OTHER_TURNS` is on.
/// A turn of yours with no reply yet, known from the tab's state, waits, as
/// a follow-up does, for `REPLY_WAIT_MS` after it ended, since Claude Code
/// writes the last words a moment after the Stop hook fires; the next tick
/// looks again. One known from a hook's event (the tab in front of you) has
/// the hook's final message instead, and its event is read only once, so it
/// is decided at once.
fn which_turn(cx: &Cx, due: &Due) -> Turn {
    let tab = &due.tab;
    let posts_every_turn = (cx.settings)(OTHER_TURNS) == Some(serde_json::Value::Bool(true));
    if tab.state == "waiting" || posts_every_turn {
        return Turn::Post;
    }
    let transcript = due.transcript.clone().or_else(|| tab.transcript.clone());
    let Some(found) = transcript.as_deref().and_then(tabs::turn_for) else {
        return Turn::Post;
    };
    let said = due.said.as_deref().is_some_and(|s| !s.trim().is_empty());
    let ended_at = tabs::since_ms(tab.since.trim_start_matches("turn:"));
    let from_state = due.transcript.is_none();
    let may_still_be_written =
        from_state && ended_at.is_some_and(|at| util::now_ms() - at < super::config::REPLY_WAIT_MS);
    match (found.yours, found.replied || said) {
        (true, true) => Turn::Post,
        (true, false) if may_still_be_written => Turn::Later,
        _ => Turn::Skip,
    }
}

async fn alert(cx: &Cx, due: Due) {
    let tab = &due.tab;
    let decided = which_turn(cx, &due);
    if matches!(decided, Turn::Later) {
        return;
    }
    cx.state
        .borrow_mut()
        .reported
        .insert(tab.id.clone(), tab.since.clone());
    if let Some(at) = tab.since.strip_prefix("turn:") {
        cx.state
            .borrow_mut()
            .turn_posted
            .insert(tab.id.clone(), at.parse().unwrap_or(0));
    }
    let waiting = tab.state == "waiting";
    // Where its kind goes; one turned off, or a turn not for you, still
    // counts as reported.
    let kind = if waiting {
        Kind::Prompts
    } else {
        Kind::Finished
    };
    let send = sending::send_now(cx, kind, tab.autopilot);
    let not_for_you = matches!(decided, Turn::Skip);
    if send == Send::Off || not_for_you {
        let why = if not_for_you {
            format!("not yours or no reply, {OTHER_TURNS} off")
        } else {
            format!("relay_send_{}", kind.name())
        };
        cx.log(&format!(
            "alert: {} {} not sent ({why})",
            label(tab.index, &tab.name),
            tab.state,
        ));
        // A full context is a kind of its own, warned of whatever the turn.
        if !waiting {
            let transcript = due.transcript.clone().or_else(|| tab.transcript.clone());
            // Its bookkeeping kept whatever is sent, and a thread made
            // only for a warning that goes out.
            let context = tabs::context_of(&cx.paths, transcript.as_deref())
                .filter(|c| super::followups::context_due(cx, c));
            if let Some(context) = context
                && sending::send_now(cx, Kind::Context, tab.autopilot) != Send::Off
            {
                match home_of(cx, tab).await {
                    Ok(home) => {
                        super::followups::post_context_warning(cx, tab, &context, home.as_deref())
                            .await
                    }
                    // No thread to warn in: warned of on a later turn.
                    Err(e) => {
                        cx.state
                            .borrow_mut()
                            .context_warned
                            .shift_remove(&context.session);
                        cx.log(&format!("context warning failed: {e}"));
                    }
                }
            }
        }
        return;
    }
    let what = if waiting {
        "is **waiting on you**"
    } else {
        "has finished"
    };
    let turn = due
        .turn
        .clone()
        .unwrap_or_else(|| tabs::last_turn(cx, &tab.id, None));
    let shown = if waiting {
        tabs::on_screen(cx, &tab.id, &turn)
    } else {
        String::new()
    };
    // A wait gets its options as buttons; a finished turn, what you'd send
    // next.
    let found = waiting
        .then(|| tabs::options_in(&tabs::screen(cx, &tab.id)))
        .flatten();
    let buttons = match &found {
        Some(f) => option_buttons(tab, f),
        None if tab.state == "done" => turn_buttons(tab),
        None => Vec::new(),
    };
    let transcript = due.transcript.clone().or_else(|| tab.transcript.clone());
    let result: anyhow::Result<()> = async {
        let home = home_of(cx, tab).await?;
        let target = Target {
            window: tab.id.clone(),
            index: tab.index,
            name: tab.name.clone(),
            thread: home.clone(),
        };
        let context = tabs::context_of(&cx.paths, transcript.as_deref());
        let text = format!(
            "🤖 **Tab {}, {}** {what}. Reply here to answer it.\n\n{turn}{shown}{}",
            tab.index,
            tab.name,
            context_footer(context.as_ref())
        );
        if found.is_some() {
            settle_choice(cx, &tab.id, PROMPT_GONE, None).await;
        }
        let ts = post(
            cx,
            &text,
            home.as_deref(),
            PostOpts::buttons(buttons.clone()),
        )
        .await?;
        cx.state
            .borrow_mut()
            .alerts
            .insert(ts.clone(), target.clone());
        if found.is_some() {
            offer_choice(cx, &tab.id, "wait", &ts, &text);
        }
        // The answers you'd likely tap, added when Haiku has read the turn.
        if tab.state == "done" {
            let said = due
                .said
                .clone()
                .or_else(|| transcript.as_deref().and_then(tabs::last_said));
            super::quickreplies::offer_quick_replies(cx, tab, &ts, said);
        }
        // Images the turn made go under it.
        let under = home.clone().unwrap_or_else(|| ts.clone());
        super::files::share_media(cx, transcript.as_deref(), &under).await;
        warn_context(cx, tab, context.as_ref(), Some(&under)).await;
        let sent = Sent {
            kind,
            tab: Some((tab.index, tab.name.clone())),
            autopilot: tab.autopilot,
            notified: false,
        };
        sending::record(cx, &sent);
        // The line in the main DM, which notifies you, as its kind says.
        if send == Send::Notify {
            let (id, since) = (tab.id.clone(), tab.since.clone());
            let still: Box<dyn Fn(&Relay) -> bool> = Box::new(move |cx: &Relay| {
                tabs::tab(cx, &id).is_some_and(|t| t.state == "waiting" && t.since == since)
            });
            notify(
                cx,
                format!("🤖 **Tab {}, {}** {what}.", tab.index, tab.name),
                home.clone(),
                target,
                if found.is_some() {
                    buttons.clone()
                } else {
                    Vec::new()
                },
                "wait",
                still,
                sent,
            )
            .await?;
        }
        cx.log(&format!(
            "alert: {} {}",
            label(tab.index, &tab.name),
            tab.state
        ));
        Ok(())
    }
    .await;
    if let Err(e) = result {
        cx.state.borrow_mut().reported.shift_remove(&tab.id);
        cx.log(&format!("alert failed: {e}"));
    }
}

/// A tab with no state showing a dialog is waiting on you too, once per
/// dialog.
async fn dialogs(cx: &Cx, tabs: &[Tab]) {
    for tab in tabs.iter().filter(|t| t.state == "idle") {
        if cx.state.borrow().following.contains_key(&tab.id) {
            continue;
        }
        let now = util::now_ms();
        let checked = cx
            .mem
            .borrow()
            .alerts
            .dialog_checked
            .get(&tab.id)
            .copied()
            .unwrap_or(0);
        if now - checked < DIALOG_CHECK_MS {
            continue;
        }
        cx.mem
            .borrow_mut()
            .alerts
            .dialog_checked
            .insert(tab.id.clone(), now);
        let Some(dialog) = tabs::dialog_in(&tabs::screen(cx, &tab.id)) else {
            let was_dialog = cx
                .state
                .borrow()
                .reported
                .get(&tab.id)
                .is_some_and(|r| r.starts_with("dialog:"));
            if was_dialog {
                cx.state.borrow_mut().reported.shift_remove(&tab.id);
            }
            if cx.state.borrow().choices.contains_key(&tab.id) {
                settle_choice(cx, &tab.id, PROMPT_GONE, None).await;
            }
            continue;
        };
        let key = format!(
            "dialog:{}",
            dialog
                .options
                .iter()
                .map(|o| o.label.as_str())
                .collect::<Vec<_>>()
                .join("|")
        );
        if cx.state.borrow().reported.get(&tab.id) == Some(&key) {
            continue;
        }
        cx.state
            .borrow_mut()
            .reported
            .insert(tab.id.clone(), key.clone());
        let result: anyhow::Result<()> = async {
            let home = home_of(cx, tab).await?;
            let target = Target {
                window: tab.id.clone(),
                index: tab.index,
                name: tab.name.clone(),
                thread: home.clone(),
            };
            let mut text = format!(
                "🤖 **Tab {}, {}** is showing a dialog before its session starts, **waiting on you**:\n\n",
                tab.index, tab.name
            );
            if !dialog.question.is_empty() {
                text.push_str(&format!("**{}**\n", dialog.question));
            }
            text.push_str(
                &dialog
                    .options
                    .iter()
                    .map(|o| {
                        format!(
                            "{}. {}{}",
                            o.n,
                            o.label,
                            if o.n == dialog.cursor { " (selected)" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            text.push_str("\n_Tap an option, or reply with its number or the start of it, and I'll move to it and press Enter._");
            settle_choice(cx, &tab.id, PROMPT_GONE, None).await;
            let ts = post(
                cx,
                &text,
                home.as_deref(),
                PostOpts::buttons(dialog_buttons(tab, &dialog)),
            )
            .await?;
            cx.state.borrow_mut().alerts.insert(ts.clone(), target.clone());
            offer_choice(cx, &tab.id, "dialog", &ts, &text);
            let send = sending::send_now(cx, Kind::Dialogs, tab.autopilot);
            let sent = Sent {
                kind: Kind::Dialogs,
                tab: Some((tab.index, tab.name.clone())),
                autopilot: tab.autopilot,
                notified: false,
            };
            sending::record(cx, &sent);
            let (id, wanted) = (tab.id.clone(), key.clone());
            if send == Send::Notify {
            notify(
                cx,
                format!(
                    "🤖 **Tab {}, {}** is showing a dialog, **waiting on you**.",
                    tab.index, tab.name
                ),
                home,
                target,
                dialog_buttons(tab, &dialog),
                "dialog",
                Box::new(move |cx: &Relay| cx.state.borrow().reported.get(&id) == Some(&wanted)),
                sent,
            )
            .await?;
            }
            cx.log(&format!("alert: {} dialog", label(tab.index, &tab.name)));
            Ok(())
        }
        .await;
        if let Err(e) = result {
            cx.state.borrow_mut().reported.shift_remove(&tab.id);
            cx.log(&format!("dialog alert failed: {e}"));
        }
        cx.save();
    }
}
