//! Tabs that may be stuck (lib/slack-relay/stuck.js). A tab marked working
//! whose transcript hasn't changed for `STUCK_MS` may be stuck (or running
//! something long, which the screen usually shows), so its thread gets a
//! note with what it did last and its screen, and a line mentioning you. Once
//! per stall: it is noted again only after it has moved on and stalled
//! anew. The quiet is counted from the later of its last activity and when
//! it was marked working: the prompt hook marks a tab working a moment
//! before Claude Code writes the prompt to the transcript, so a tab idle for
//! an hour that has just been sent a prompt would otherwise look stuck for
//! an hour.

use super::Cx;
use super::alerts::notify;
use super::buttons::stuck_buttons;
use super::homes::{home_of, in_own_thread};
use super::post::{PostOpts, post, tab_notice};
use super::sending::{self, Kind, Send, Sent};
use super::state::Target;
use super::summaries::tab_facts;
use super::tabs::{self, Tab, label, lasted};
use crate::util;

pub const STUCK_MS: i64 = 10 * 60_000;

pub async fn stuck_tabs(cx: &Cx, tabs: &[Tab]) {
    // Just after the computer wakes, every working tab's transcript looks
    // old: it was asleep too. Stuck means stuck while awake.
    if util::now_ms() - cx.life.borrow().woke_at < STUCK_MS {
        return;
    }
    for tab in tabs {
        if tab.state != "working" {
            cx.state.borrow_mut().stuck.shift_remove(&tab.id);
            continue;
        }
        // When it last did anything first, which is cheap (kept until the
        // file changes): reading the whole turn every second for every
        // working tab cost more than anything else the relay did.
        let began = tabs::since_ms(&tab.since).unwrap_or(0);
        let Some(last_done) = tab
            .transcript
            .as_deref()
            .and_then(|f| tabs::last_activity_at(cx, f))
        else {
            continue;
        };
        let now = util::now_ms();
        if now - last_done.max(began) < STUCK_MS
            || cx.state.borrow().stuck.get(&tab.id) == Some(&last_done)
        {
            continue;
        }
        let facts = tab_facts(cx, tab);
        let Some(last_at) = facts.last_at else {
            continue;
        };
        if now - last_at.max(began) < STUCK_MS
            || cx.state.borrow().stuck.get(&tab.id) == Some(&last_at)
        {
            continue;
        }
        cx.state.borrow_mut().stuck.insert(tab.id.clone(), last_at);
        cx.save();
        // Noted either way, so turning the kind back on posts no old stall.
        let send = sending::send_now(cx, Kind::Stuck, tab.autopilot);
        if send == Send::Off {
            continue;
        }
        let idle = lasted(now - last_at.max(began));
        let lastly = if facts.activity.is_empty() {
            String::new()
        } else {
            format!(
                "\nWhat it did last:\n{}",
                facts.activity[facts.activity.len().saturating_sub(3)..]
                    .iter()
                    .map(|l| format!("• {l}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };
        let name = label(tab.index, &tab.name);
        let result: anyhow::Result<()> = async {
            let home = home_of(cx, tab).await?;
            let target = Target {
                window: tab.id.clone(),
                pid: tab.pid.clone(),
                index: tab.index,
                name: tab.name.clone(),
                thread: home.clone(),
                closed: false,
            };
            let who = if in_own_thread(cx, &tab.id, home.as_deref(), false) {
                "Claude".to_string()
            } else {
                name.clone()
            };
            // The tab's name and its state once each, so the facts line,
            // which would say both again, gives only its turn.
            let so_far = facts
                .turn_so_far
                .as_deref()
                .map(|turn| format!(" {}{}.", turn[..1].to_uppercase(), &turn[1..]))
                .unwrap_or_default();
            let body = format!(
                "🤖 {who} may be stuck: working, but nothing has happened for {idle}.{so_far}{lastly}{}",
                tabs::on_screen(cx, &tab.id, "")
            );
            let notice = tab_notice(&tab.name, &format!("may be stuck, nothing new for {idle}"));
            let note = post(
                cx,
                &body,
                home.as_deref(),
                PostOpts::buttons(stuck_buttons(tab)).notice(notice.clone()),
            )
            .await?;
            cx.state.borrow_mut().alerts.insert(note, target.clone());
            let sent = Sent {
                kind: Kind::Stuck,
                tab: Some((tab.index, tab.name.clone())),
                autopilot: tab.autopilot,
                notified: false,
            };
            sending::record(cx, &sent);
            if send == Send::Thread {
                cx.log(&format!("stuck: {name}"));
                return Ok(());
            }
            // The note is in the thread at once; the line mentioning you,
            // which notifies you, waits while you're at the computer.
            notify(
                cx,
                format!("🤖 {name} may be stuck: nothing new for {idle}."),
                notice,
                home,
                target,
                Vec::new(),
                "wait",
                Box::new(|_| true),
                sent,
            )
            .await?;
            cx.log(&format!("stuck: {name}"));
            Ok(())
        }
        .await;
        if let Err(e) = result {
            cx.log(&format!("stuck note failed: {e}"));
        }
        cx.save();
    }
}
