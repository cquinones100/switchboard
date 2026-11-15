//! Each tab's home thread (lib/slack-relay/homes.js). Each Claude tab has
//! one thread in the DM, its home: its alerts, answers and milestones go
//! there, and whatever you reply there is typed into it. What should notify
//! you is posted in the home thread mentioning you, since Slack notifies
//! no one of a reply in a thread they don't follow, and nothing in a home
//! thread is also sent to the main DM. A home is started the first time there's something to say about
//! a tab. It belongs to the tab, checked by its Claude pane's process, since
//! tab ids are used again after a restart; a tab that has closed has none,
//! and its news goes to the top level.
//!
//! The thread's first message is its header: the tab's name, then its
//! state's one mark, what it is doing and since when, and buttons for what
//! you'd ask of it, nothing more, since Slack's list of threads shows it and
//! so shows every tab's state at a glance; it is edited whenever the tab is
//! renamed, renumbered or changes state, and once its session has ended is
//! one line saying when the tab closed. While the tab lands, its landing
//! takes the state's place, as board 51 draws it (src/relay/landing.rs).

use futures_util::FutureExt;
use futures_util::future::LocalBoxFuture;

use super::buttons::{delete_thread_button, header_buttons};
use super::post::{PostOpts, post, tab_notice, update_noticed};
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
    if let Some(landing) = &tab.landing {
        return format!(
            "**Tab {}, {}**\n{}",
            tab.index,
            tab.name,
            super::landing::header_lines(tab, landing)
        );
    }
    let since = if !tab.since.is_empty() && tab.since.bytes().all(|b| b.is_ascii_digit()) {
        format!(" since {}", when(tab.since.parse().unwrap_or(0.0)))
    } else {
        String::new()
    };
    format!(
        "**Tab {}, {}**\n{}{since}",
        tab.index,
        tab.name,
        header_words(&tab.state)
    )
}

/// The header's notification line: the tab's name and its state.
pub fn home_notice(tab: &Tab) -> String {
    match &tab.landing {
        Some(landing) => tab_notice(&tab.name, &super::landing::notice_words(landing)),
        None => tab_notice(&tab.name, &header_words(&tab.state)),
    }
}

pub fn closed_text(home: &Home) -> String {
    format!(
        "⚫ Tab {}, closed at {}",
        home.label,
        when(util::now_ms() as f64 / 1000.0)
    )
}

/// What the header last sent shows, so it is edited only when that changes.
/// Its leading version is bumped whenever `home_text`'s wording changes, so
/// a thread a relay before it started gets the new header on the next tick.
pub fn header_key(tab: &Tab) -> String {
    let landing = tab
        .landing
        .as_ref()
        .map(|l| super::landing::header_lines(tab, l))
        .unwrap_or_default();
    format!(
        "4|{}|{}|{}|{}|{landing}",
        tab.index, tab.name, tab.state, tab.since
    )
}

/// A tab's home thread, started if it has none yet: one per tab, however
/// many messages ask for it at once. `None` for a tab that has closed, and
/// an error when Slack refused to start the thread, which fails what asked
/// for it (an alert then cleared to go out again on the next tick, as
/// homes.js's rejected promise did) rather than sending it to the top level.
pub async fn home_of(cx: &Cx, tab: &Tab) -> anyhow::Result<Option<String>> {
    // Its Claude pane's process, as the tabs report it now.
    let Some(pid) = super::tabs::tab(cx, &tab.id).map(|t| t.pid) else {
        return Ok(None);
    };
    {
        let state = cx.state.borrow();
        if let Some(home) = state.homes.get(&tab.id)
            && home.pid == pid
            && state.threads.contains_key(&home.ts)
        {
            return Ok(Some(home.ts.clone()));
        }
    }
    let existing = cx.homing.borrow().get(&tab.id).cloned();
    let making = match existing {
        Some(making) => making,
        None => {
            let (cx2, tab2) = (cx.clone(), tab.clone());
            let fut: LocalBoxFuture<'static, Result<String, String>> = Box::pin(async move {
                let made = post(
                    &cx2,
                    &home_text(&tab2),
                    None,
                    PostOpts::buttons(header_buttons(&tab2)).notice(home_notice(&tab2)),
                )
                .await;
                cx2.homing.borrow_mut().remove(&tab2.id);
                let ts = match made {
                    Ok(ts) => ts,
                    Err(e) => {
                        let why = format!("starting tab {}'s thread failed: {e}", tab2.index);
                        cx2.log(&why);
                        return Err(why);
                    }
                };
                {
                    let mut state = cx2.state.borrow_mut();
                    state.homes.insert(
                        tab2.id.clone(),
                        Home {
                            ts: ts.clone(),
                            pid: pid.clone(),
                            label: format!("{}, {}", tab2.index, tab2.name),
                            shown: header_key(&tab2),
                        },
                    );
                    state.alerts.insert(
                        ts.clone(),
                        Target {
                            window: tab2.id.clone(),
                            pid: pid.clone(),
                            index: tab2.index,
                            name: tab2.name.clone(),
                            thread: None,
                            closed: false,
                        },
                    );
                }
                cx2.save();
                Ok(ts)
            });
            let shared = fut.shared();
            cx.homing
                .borrow_mut()
                .insert(tab.id.clone(), shared.clone());
            shared
        }
    };
    making.await.map(Some).map_err(anyhow::Error::msg)
}

pub fn is_home(cx: &Relay, ts: &str) -> bool {
    cx.state.borrow().homes.values().any(|h| h.ts == ts)
}

/// Whether a message posted in `thread` lands in the tab `id`'s own
/// thread and is read only there: the thread's header names the tab, so the
/// message calls it Claude rather than repeating its number and name. One
/// that mentions you is also read as the notification, away from the
/// thread, so it keeps the tab's name.
pub fn in_own_thread(cx: &Relay, id: &str, thread: Option<&str>, mention: bool) -> bool {
    !mention && thread.is_some() && home_thread(cx, id).as_deref() == thread
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
    let name = home
        .label
        .split_once(", ")
        .map_or(home.label.as_str(), |(_, name)| name);
    if let Err(e) = update_noticed(
        cx,
        &home.ts,
        &closed_text(&home),
        &tab_notice(name, "closed"),
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
        if let Err(e) = update_noticed(
            cx,
            &home.ts,
            &home_text(tab),
            &home_notice(tab),
            &header_buttons(tab),
        )
        .await
        {
            cx.log(&format!(
                "updating tab {}'s thread header failed: {e}",
                tab.index
            ));
        }
        cx.save();
    }
}
