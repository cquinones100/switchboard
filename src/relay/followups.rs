//! Reporting back on a turn the relay typed (lib/slack-relay/followups.js),
//! in the thread the request came from, once the tab is done. A tab with
//! state is done when it moves to finished or waiting after the message was
//! sent; a tab without state is taken as done once its screen has changed
//! and then stayed still for `QUIET_SCREEN_MS`. Either way the reply
//! carries the turn, which is usually Claude's answer. A tab being followed
//! gets no separate alert.
//!
//! The surest signal is the hooks' events: turn ends and waits reported by
//! every Claude session, in the workspace or not, each naming its
//! transcript (`switchboard hook`, src/agent/hook.rs). The hooks write only
//! while the events file exists: the relay creates it, and removes it when
//! stopped on purpose. A relay that crashed leaves it, so the turns that
//! end while it restarts are still there, and how far it had read is kept
//! in the state (`eventsOffset`), so it carries on from there.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

use super::buttons::{compact_button, option_buttons, turn_buttons, working_buttons};
use super::config::{FOLLOW_UP_LIMIT_MS, PICKUP_WARN_MS, QUIET_SCREEN_MS, REPLY_WAIT_MS};
use super::homes::is_home;
use super::post::{PostOpts, offer_choice, post, react, remove, settle_choice, unreact};
use super::state::{Ended, Follow, Target};
use super::tabs::{self, Context, Tab};
use super::{Cx, Relay};
use crate::util;

/// One of the hooks' events.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Event {
    /// prompt, done or waiting.
    pub event: String,
    /// When, in seconds.
    pub time: i64,
    pub transcript: Option<String>,
    pub prompt: Option<String>,
    /// A finished turn's final message, from the Stop hook.
    pub last: Option<String>,
    /// Taken by a follow-up, which reports this turn itself.
    pub handled: bool,
}

impl Event {
    fn from_json(v: &Value) -> Event {
        let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        Event {
            event: text("event").unwrap_or_default(),
            time: v.get("time").and_then(Value::as_f64).unwrap_or(0.0) as i64,
            transcript: text("transcript_path"),
            prompt: text("prompt"),
            last: text("last"),
            handled: false,
        }
    }
}

/// The events written since the last look, whole lines only; a line still
/// being written is read next time. A file shorter than what was read is a
/// new one.
pub fn new_events(cx: &Relay) -> Vec<Event> {
    let file = cx.paths.events_file();
    let Ok(size) = std::fs::metadata(&file).map(|m| m.len()) else {
        return Vec::new();
    };
    let mut state = cx.state.borrow_mut();
    if size < state.events_offset {
        state.events_offset = 0;
    }
    if size == state.events_offset {
        return Vec::new();
    }
    let read = (|| -> std::io::Result<Vec<u8>> {
        let mut f = std::fs::File::open(&file)?;
        f.seek(SeekFrom::Start(state.events_offset))?;
        let mut buf = Vec::new();
        f.take(size - state.events_offset).read_to_end(&mut buf)?;
        Ok(buf)
    })();
    let Ok(buf) = read else {
        return Vec::new();
    };
    let end = buf.iter().rposition(|b| *b == b'\n').map_or(0, |at| at + 1);
    state.events_offset += end as u64;
    String::from_utf8_lossy(&buf[..end])
        .split('\n')
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .map(|v| Event::from_json(&v))
        .collect()
}

/// Under a turn: how full the tab's context is, and its model.
pub fn context_footer(context: Option<&Context>) -> String {
    context
        .map(|c| {
            format!(
                "\n\n_{}{}_",
                tabs::context_words(Some(c)),
                if c.model.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", c.model)
                }
            )
        })
        .unwrap_or_default()
}

/// Once a session's context passes this percent, its thread is told so
/// once, under the turn, with a button to compact it, and again only after
/// it has dropped below, as a compact makes it. The sessions warned are
/// kept, the oldest let go past `CONTEXT_WARNED_KEEP`.
pub const CONTEXT_WARN: i64 = 90;
const CONTEXT_WARNED_KEEP: usize = 50;

pub async fn warn_context(cx: &Cx, tab: &Tab, context: Option<&Context>, thread: Option<&str>) {
    if let Some(context) = context.filter(|c| context_due(cx, c)) {
        post_context_warning(cx, tab, context, thread).await;
    }
}

/// Whether a session's context is due a warning: past `CONTEXT_WARN` and
/// not warned of since it last dropped below, noted as warned of now; one
/// below it is forgotten, so it is warned of again once full again. Kept
/// whatever is sent, so a turn or a warning not sent still keeps it.
pub fn context_due(cx: &Relay, context: &Context) -> bool {
    let mut state = cx.state.borrow_mut();
    let warned = &mut state.context_warned;
    if context.percent < CONTEXT_WARN {
        warned.shift_remove(&context.session);
        return false;
    }
    if warned.contains_key(&context.session) {
        return false;
    }
    warned.insert(context.session.clone(), util::now_ms());
    warned.sort_by(|_, a, _, b| b.cmp(a));
    warned.truncate(CONTEXT_WARNED_KEEP);
    true
}

/// A full context said in the tab's thread, and in the main DM, as its
/// kind says; turned off, nothing, the warning counted as given.
pub async fn post_context_warning(cx: &Cx, tab: &Tab, context: &Context, thread: Option<&str>) {
    use super::sending::{self, Kind, Send, Sent};
    let send = sending::send_now(cx, Kind::Context, tab.autopilot);
    if send == Send::Off {
        return;
    }
    let body = format!(
        "🤖 **Tab {}, {}** has used **{}%** of its context. Compact it before it runs out.",
        tab.index, tab.name, context.percent
    );
    let posted = post(
        cx,
        &body,
        thread,
        PostOpts::buttons(vec![compact_button(tab)]),
    )
    .await;
    if let Err(e) = posted {
        cx.log(&format!("context warning failed: {e}"));
        return;
    }
    let sent = Sent {
        kind: Kind::Context,
        tab: Some((tab.index, tab.name.clone())),
        autopilot: tab.autopilot,
        notified: false,
    };
    sending::record(cx, &sent);
    if send == Send::Notify {
        let target = Target {
            window: tab.id.clone(),
            index: tab.index,
            name: tab.name.clone(),
            thread: thread.map(str::to_string),
        };
        let line = format!(
            "🤖 **Tab {}, {}** has used **{}%** of its context.",
            tab.index, tab.name, context.percent
        );
        let sent = super::alerts::notify(
            cx,
            line,
            thread.map(str::to_string),
            target,
            Vec::new(),
            "wait",
            Box::new(|_| true),
            sent,
        )
        .await;
        if let Err(e) = sent {
            cx.log(&format!("context notice failed: {e}"));
        }
    }
}

/// How a follow-up ends.
#[derive(Clone, Copy, PartialEq)]
enum Finish {
    Plain,
    Waiting,
    Done,
}

/// Reports what each followed tab did, as the module's comment says.
pub async fn follow_ups(cx: &Cx, current: &[Tab], events: &mut [Event]) {
    let ids: Vec<String> = cx.state.borrow().following.keys().cloned().collect();
    for id in ids {
        let Some(mut follow) = cx.state.borrow().following.get(&id).cloned() else {
            continue;
        };
        let tab = current.iter().find(|t| t.id == id).cloned();
        let name = tabs::label(follow.index, &follow.name);
        // The surest signal: a hook said a session finished a turn, or
        // started waiting, and that session's last prompt is the text the
        // relay typed.
        let mut answered = None;
        for e in events.iter_mut() {
            let ends = (e.event == "done" || e.event == "waiting")
                && e.time * 1000 >= follow.sent_at - 1000;
            let Some(file) = e.transcript.clone().filter(|_| ends) else {
                continue;
            };
            if tabs::turn_from(Path::new(&file), Some(&follow.text), None).is_some() {
                // This turn is the follow-up's to report.
                e.handled = true;
                answered = Some(e.clone());
            }
        }
        if let Some(e) = answered {
            follow.ended = Some(Ended {
                event: e.event,
                transcript: e.transcript.unwrap_or_default(),
                at: util::now_ms(),
                last: e.last,
            });
        }
        // Claude has taken the message: a session's prompt hook fired with
        // its text, or the tab's transcript ends with it, or the turn already
        // ended. Your message gets 👀. A tab that is idle and still hasn't
        // taken it after PICKUP_WARN_MS is probably showing a dialog, so its
        // screen is posted.
        if let Some(request) = follow.request.clone()
            && !follow.taken
        {
            let file = tab.as_ref().and_then(|t| t.transcript.clone());
            follow.taken = follow.ended.is_some()
                || events.iter().any(|e| {
                    e.event == "prompt"
                        && e.time * 1000 >= follow.sent_at - 1000
                        && e.prompt
                            .as_deref()
                            .is_some_and(|p| !p.is_empty() && tabs::prompt_matches(p, &follow.text))
                })
                || file.is_some_and(|f| tabs::turn_from(&f, Some(&follow.text), None).is_some());
            if follow.taken {
                keep(cx, &id, &follow);
                let _ = unreact(cx, &request, "hourglass_flowing_sand").await;
                if let Err(e) = react(cx, Some(&request), "eyes").await {
                    cx.log(&format!("👀 failed: {e}"));
                }
                // While it runs, a message with Progress and Stop, where you
                // asked; deleted once the answer is posted.
                if follow.ended.is_none()
                    && let Some(t) = &tab
                {
                    match post(
                        cx,
                        "🤖 _Working on it._",
                        follow.thread.as_deref(),
                        PostOpts {
                            broadcast: follow.broadcast,
                            buttons: working_buttons(t),
                        },
                    )
                    .await
                    {
                        Ok(ts) => follow.working = Some(ts),
                        Err(e) => cx.log(&format!("working note failed: {e}")),
                    }
                    keep(cx, &id, &follow);
                }
            } else if !follow.warned
                && tab.as_ref().is_some_and(|t| t.state != "working")
                && util::now_ms() - follow.sent_at > PICKUP_WARN_MS
            {
                follow.warned = true;
                keep(cx, &id, &follow);
                let body = format!(
                    "🤖 {name} hasn't taken your message after {} seconds. It may be showing a prompt or a dialog.{}",
                    PICKUP_WARN_MS / 1000,
                    tabs::on_screen(cx, &id, "")
                );
                if let Err(e) = post(cx, &body, follow.thread.as_deref(), PostOpts::default()).await
                {
                    cx.log(&format!("pickup warning failed: {e}"));
                }
            }
        }
        // A turn reported ended is posted once the transcript has caught up
        // with it (its last text is the final message the Stop hook gave, or
        // without one, it ends with Claude's text), or after REPLY_WAIT_MS,
        // as it is.
        let pending = |file: Option<&Path>, since: i64, last: Option<&str>| {
            file.is_some_and(|f| {
                let caught = match last {
                    Some(l) => tabs::caught_up(f, l),
                    None => tabs::turn_written(f),
                };
                !caught && util::now_ms() - since < REPLY_WAIT_MS
            })
        };
        if let Some(ended) = follow.ended.clone() {
            keep(cx, &id, &follow);
            let transcript = Path::new(&ended.transcript);
            let turn = tabs::turn_from(transcript, Some(&follow.text), ended.last.as_deref())
                .unwrap_or_else(|| tabs::last_turn(cx, &id, Some(&follow.text)));
            if ended.event == "waiting" {
                let shown = if tab.is_some() {
                    tabs::on_screen(cx, &id, &turn)
                } else {
                    String::new()
                };
                let body =
                    format!("{name} is waiting on you. Reply here to answer it:\n{turn}{shown}");
                finish(cx, &id, &follow, tab.as_ref(), &body, Finish::Waiting).await;
            } else if !pending(Some(transcript), ended.at, ended.last.as_deref()) {
                finish(
                    cx,
                    &id,
                    &follow,
                    tab.as_ref(),
                    &format!("{name} is done:\n{turn}"),
                    Finish::Done,
                )
                .await;
            }
            continue;
        }
        let Some(tab) = tab else {
            finish(
                cx,
                &id,
                &follow,
                None,
                &format!("{name} has closed."),
                Finish::Plain,
            )
            .await;
            continue;
        };
        let now = util::now_ms();
        let began = tabs::since_ms(&tab.since);
        if tab.state != "idle" && began.is_some_and(|b| b >= follow.sent_at - 1000) {
            if tab.state == "done" {
                let done_at = *follow.done_at.get_or_insert(now);
                keep(cx, &id, &follow);
                if !pending(tab.transcript.as_deref(), done_at, None) {
                    let turn = tabs::last_turn(cx, &id, Some(&follow.text));
                    finish(
                        cx,
                        &id,
                        &follow,
                        Some(&tab),
                        &format!("{name} is done:\n{turn}"),
                        Finish::Done,
                    )
                    .await;
                }
            } else if tab.state == "waiting" {
                let turn = tabs::last_turn(cx, &id, Some(&follow.text));
                let shown = tabs::on_screen(cx, &id, &turn);
                let body =
                    format!("{name} is waiting on you. Reply here to answer it:\n{turn}{shown}");
                finish(cx, &id, &follow, Some(&tab), &body, Finish::Waiting).await;
            } else {
                keep(cx, &id, &follow);
            }
            continue;
        }
        if tab.state == "idle" {
            let shown = tabs::screen(cx, &id);
            if shown != follow.screen {
                follow.screen = shown;
                follow.changed_at = now;
            } else if follow.changed_at > 0 && now - follow.changed_at >= QUIET_SCREEN_MS {
                let turn = tabs::last_turn(cx, &id, Some(&follow.text));
                let body = format!("{name} looks done (its screen stopped changing):\n{turn}");
                finish(cx, &id, &follow, Some(&tab), &body, Finish::Done).await;
                continue;
            }
        }
        keep(cx, &id, &follow);
        if now - follow.sent_at > FOLLOW_UP_LIMIT_MS {
            let body = format!(
                "{name} is still going after an hour, so I stopped watching it. `?` here shows its screen."
            );
            finish(cx, &id, &follow, Some(&tab), &body, Finish::Plain).await;
        }
    }
}

/// The follow-up as it now stands, kept while it is still there.
fn keep(cx: &Relay, id: &str, follow: &Follow) {
    if let Some(f) = cx.state.borrow_mut().following.get_mut(id) {
        *f = follow.clone();
    }
    cx.save();
}

/// Reports a followed turn and stops following it.
async fn finish(cx: &Cx, id: &str, follow: &Follow, tab: Option<&Tab>, body: &str, how: Finish) {
    cx.state.borrow_mut().following.shift_remove(id);
    if let Some(working) = &follow.working
        && let Err(e) = remove(cx, working).await
    {
        cx.log(&format!("couldn't delete the working note: {e}"));
    }
    // What the tab shows now, not when this pass began: the turn has just
    // ended, and the state that says so is the one the alerts would report.
    let now = tabs::tab(cx, id);
    if let Some(since) = now
        .as_ref()
        .map(|t| t.since.clone())
        .or_else(|| tab.map(|t| t.since.clone()))
        .filter(|s| !s.is_empty())
    {
        cx.state.borrow_mut().reported.insert(id.to_string(), since);
    }
    cx.save();
    let waiting = how == Finish::Waiting;
    let done = how == Finish::Done;
    // A wait gets its options as buttons; a finished turn, what you'd send
    // next. Nothing for a tab that has closed or is still going.
    let found = (waiting && now.is_some())
        .then(|| tabs::options_in(&tabs::screen(cx, id)))
        .flatten();
    let buttons = match (&now, &found) {
        (None, _) => Vec::new(),
        (Some(n), Some(f)) => option_buttons(n, f),
        (Some(n), None) if done => turn_buttons(n),
        _ => Vec::new(),
    };
    let ended_transcript = follow.ended.as_ref().map(|e| e.transcript.clone());
    let result: anyhow::Result<()> = async {
        if found.is_some() {
            settle_choice(cx, id, "This prompt is no longer showing.", None).await;
        }
        let context = if (waiting || done) && now.is_some() {
            tabs::context_of(
                &cx.paths,
                now.as_ref().and_then(|n| n.transcript.as_deref()),
            )
        } else {
            None
        };
        let ts = post(
            cx,
            &format!("🤖 {body}{}", context_footer(context.as_ref())),
            follow.thread.as_deref(),
            PostOpts {
                broadcast: follow.broadcast,
                buttons: buttons.clone(),
            },
        )
        .await?;
        if found.is_some() {
            offer_choice(cx, id, "wait", &ts, &format!("🤖 {body}"));
        }
        // The answers you'd likely tap, added when Haiku has read the turn.
        if done && let Some(n) = &now {
            let said = follow
                .ended
                .as_ref()
                .and_then(|e| e.last.clone())
                .or_else(|| {
                    ended_transcript
                        .as_deref()
                        .map(Path::new)
                        .map(Path::to_path_buf)
                        .or_else(|| n.transcript.clone())
                        .and_then(|f| tabs::last_said(&f))
                });
            super::quickreplies::offer_quick_replies(cx, n, &ts, said);
        }
        // Your message's reaction: ✅ once its turn is answered.
        if done && let Some(request) = &follow.request {
            let _ = unreact(cx, request, "eyes").await;
            if let Err(e) = react(cx, Some(request), "white_check_mark").await {
                cx.log(&format!("✅ failed: {e}"));
            }
        }
        cx.state.borrow_mut().alerts.insert(
            ts.clone(),
            Target {
                window: id.to_string(),
                index: follow.index,
                name: follow.name.clone(),
                thread: follow.thread.clone(),
            },
        );
        // Images the turn made go under it.
        let transcript = ended_transcript
            .clone()
            .map(std::path::PathBuf::from)
            .or_else(|| tab.and_then(|t| t.transcript.clone()));
        let under = follow.thread.clone().unwrap_or_else(|| ts.clone());
        super::files::share_media(cx, transcript.as_deref(), &under).await;
        if let Some(n) = &now {
            warn_context(cx, n, context.as_ref(), Some(&under)).await;
        }
        // Otherwise, in a tab's home thread, a wait gets a line in the main
        // DM.
        if waiting
            && !follow.broadcast
            && let Some(thread) = &follow.thread
            && is_home(cx, thread)
        {
            let notice = format!(
                "🤖 {} is **waiting on you**.",
                tabs::label(follow.index, &follow.name)
            );
            let posted = post(
                cx,
                &notice,
                Some(thread),
                PostOpts {
                    broadcast: true,
                    buttons: if found.is_some() {
                        buttons.clone()
                    } else {
                        Vec::new()
                    },
                },
            )
            .await?;
            cx.state.borrow_mut().alerts.insert(
                posted.clone(),
                Target {
                    window: id.to_string(),
                    index: follow.index,
                    name: follow.name.clone(),
                    thread: Some(thread.clone()),
                },
            );
            if found.is_some() {
                offer_choice(cx, id, "wait", &posted, &notice);
            }
        }
        cx.save();
        Ok(())
    }
    .await;
    if let Err(e) = result {
        cx.log(&format!("follow-up failed: {e}"));
    }
}
