//! Each tab's home thread (lib/slack-relay/homes.js). Each Claude tab has
//! one thread in the DM, its home: its alerts, answers and milestones go
//! there, and whatever you reply there is typed into it. The main DM only
//! gets one-line notices, posted in the home thread and also sent to the
//! conversation (Slack's "also send to channel"), so tapping one opens the
//! thread. A home is started the first time there's something to say about
//! a tab. It belongs to the tab, checked by its Claude pane's process, since
//! tab ids are used again after a restart; a tab that has closed has none,
//! and its news goes to the top level.
//!
//! The thread's first message is its header: the tab's name, what it is
//! doing and since when, and buttons for what you'd ask of it. Slack's list
//! of threads shows it, so that list shows every tab's state; it is edited
//! whenever the tab is renamed, renumbered or changes state, and says so
//! when its session has ended.

use futures_util::FutureExt;
use futures_util::future::LocalBoxFuture;

use super::buttons::{delete_thread_button, header_buttons};
use super::post::{PostOpts, post, update};
use super::state::{ClosedHome, Home, Target};
use super::tabs::Tab;
use super::{Cx, Relay};
use crate::util;

pub fn header_words(state: &str) -> String {
    match state {
        "waiting" => "🔴 **Waiting on you**".into(),
        "done" => "✅ Finished".into(),
        "working" => "🔵 Working".into(),
        "idle" => "⚪ Idle".into(),
        other => other.into(),
    }
}

/// A time as the header says it: the time today, else the day and time.
pub fn when(seconds: f64) -> String {
    let at = util::local_at_ms((seconds * 1000.0) as i64);
    let time = at.strftime("%-I:%M %p").to_string();
    if at.date() == util::local_now().date() {
        time
    } else {
        format!("{}, {time}", at.strftime("%b %-d"))
    }
}

pub fn home_text(tab: &Tab) -> String {
    let since = if !tab.since.is_empty() && tab.since.bytes().all(|b| b.is_ascii_digit()) {
        format!(" since {}", when(tab.since.parse().unwrap_or(0.0)))
    } else {
        String::new()
    };
    format!(
        "🤖 **Tab {}, {}**\n{}{since}\n\nThis is its thread: everything about it goes here, and what you reply here is typed into it.",
        tab.index,
        tab.name,
        header_words(&tab.state)
    )
}

pub fn closed_text(home: &Home) -> String {
    format!(
        "🤖 ~~Tab {}~~\n⚫ Closed at {}. Its Claude session has ended.",
        home.label,
        when(util::now_ms() as f64 / 1000.0)
    )
}

pub fn header_key(tab: &Tab) -> String {
    format!("{}|{}|{}|{}", tab.index, tab.name, tab.state, tab.since)
}

/// A tab's home thread, started if it has none yet: one per tab, however
/// many messages ask for it at once. `None` for a tab that has closed.
pub async fn home_of(cx: &Cx, tab: &Tab) -> Option<String> {
    // Its Claude pane's process, as the tabs report it now.
    let pid = super::tabs::tab(cx, &tab.id).map(|t| t.pid)?;
    {
        let state = cx.state.borrow();
        if let Some(home) = state.homes.get(&tab.id)
            && home.pid == pid
            && state.threads.contains_key(&home.ts)
        {
            return Some(home.ts.clone());
        }
    }
    let existing = cx.homing.borrow().get(&tab.id).cloned();
    let making = match existing {
        Some(making) => making,
        None => {
            let (cx2, tab2) = (cx.clone(), tab.clone());
            let fut: LocalBoxFuture<'static, Option<String>> = Box::pin(async move {
                let made = post(
                    &cx2,
                    &home_text(&tab2),
                    None,
                    PostOpts::buttons(header_buttons(&tab2)),
                )
                .await;
                cx2.homing.borrow_mut().remove(&tab2.id);
                let ts = match made {
                    Ok(ts) => ts,
                    Err(e) => {
                        cx2.log(&format!("starting tab {}'s thread failed: {e}", tab2.index));
                        return None;
                    }
                };
                {
                    let mut state = cx2.state.borrow_mut();
                    state.homes.insert(
                        tab2.id.clone(),
                        Home {
                            ts: ts.clone(),
                            pid,
                            label: format!("{}, {}", tab2.index, tab2.name),
                            shown: header_key(&tab2),
                        },
                    );
                    state.alerts.insert(
                        ts.clone(),
                        Target {
                            window: tab2.id.clone(),
                            index: tab2.index,
                            name: tab2.name.clone(),
                            thread: None,
                        },
                    );
                }
                cx2.save();
                Some(ts)
            });
            let shared = fut.shared();
            cx.homing
                .borrow_mut()
                .insert(tab.id.clone(), shared.clone());
            shared
        }
    };
    making.await
}

pub fn is_home(cx: &Relay, ts: &str) -> bool {
    cx.state.borrow().homes.values().any(|h| h.ts == ts)
}

/// A tab's thread, if it has one yet.
pub fn home_thread(cx: &Relay, id: &str) -> Option<String> {
    let state = cx.state.borrow();
    let home = state.homes.get(id)?;
    state
        .threads
        .contains_key(&home.ts)
        .then(|| home.ts.clone())
}

/// A link that opens a thread in the DM at its newest message, built the
/// way Slack builds a reply's link: the workspace's address, the DM, the
/// message's ts without its dot, and the thread it is in. `None` when the
/// workspace's address isn't known.
pub fn thread_url(cx: &Relay, ts: &str) -> Option<String> {
    let state = cx.state.borrow();
    let dm = state.dm.as_ref()?;
    let url = dm.url.as_ref()?;
    let at = state.latest.get(ts).map(String::as_str).unwrap_or(ts);
    Some(format!(
        "{url}archives/{}/p{}?thread_ts={ts}&cid={}",
        dm.channel,
        at.replace('.', ""),
        dm.channel
    ))
}

pub fn thread_link(cx: &Relay, ts: &str, text: &str) -> String {
    match thread_url(cx, ts) {
        Some(url) => format!("[{text}]({url})"),
        None => text.to_string(),
    }
}

/// A closed tab's thread is recorded as one the relay may delete (see
/// cleanup.rs), and its header offers to.
pub async fn close_home(cx: &Relay, id: &str) {
    let Some(home) = cx.state.borrow_mut().homes.shift_remove(id) else {
        return;
    };
    cx.state.borrow_mut().closed_homes.insert(
        home.ts.clone(),
        ClosedHome {
            label: home.label.clone(),
            at: util::now_ms(),
        },
    );
    cx.save();
    if let Err(e) = update(
        cx,
        &home.ts,
        &closed_text(&home),
        &[delete_thread_button(&home.ts)],
    )
    .await
    {
        cx.log(&format!("marking a thread closed failed: {e}"));
    }
}

/// Each tab's header edited to match the tab, or marked closed when its
/// session has ended (its tab closed, or now runs another session).
pub async fn refresh_homes(cx: &Relay, tabs: &[Tab]) {
    for tab in tabs {
        let home = {
            let state = cx.state.borrow();
            match state.homes.get(&tab.id) {
                Some(h) if state.threads.contains_key(&h.ts) => h.clone(),
                _ => continue,
            }
        };
        if !tab.pids.contains(&home.pid) {
            close_home(cx, &tab.id).await;
            continue;
        }
        let key = header_key(tab);
        if home.shown == key {
            continue;
        }
        if let Some(h) = cx.state.borrow_mut().homes.get_mut(&tab.id) {
            h.label = format!("{}, {}", tab.index, tab.name);
            h.shown = key;
        }
        if let Err(e) = update(cx, &home.ts, &home_text(tab), &header_buttons(tab)).await {
            cx.log(&format!(
                "updating tab {}'s thread header failed: {e}",
                tab.index
            ));
        }
        cx.save();
    }
}
