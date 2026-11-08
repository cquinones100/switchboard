//! Saying the relay is offline (lib/slack-relay/offline.js). When the
//! computer is off, asleep or offline, the relay can't tell Slack anything,
//! so the warning is left with Slack ahead of time: a message scheduled
//! `OFFLINE_AFTER_MS` ahead in the DM, saying the relay is offline. Every
//! `RENEW_MS` a new one is scheduled further out and the old one deleted,
//! so while the relay runs, none is ever sent. If the relay goes quiet,
//! nothing pushes it back and Slack sends it. When the relay is back, it
//! says so, and for how long it was gone. A clean stop deletes it, since
//! you know.

use anyhow::Result;
use serde_json::{Value, json};

use super::Relay;
use super::lifecycle::clock;
use super::post::{PostOpts, as_markdown, post};
use super::state::OfflineNote;
use crate::util;

pub const OFFLINE_AFTER_MS: i64 = 10 * 60_000;
pub const RENEW_MS: u64 = 2 * 60_000;

pub fn offline_text(since: i64) -> String {
    format!(
        "🤖 ⚠️ **The relay is offline.** Your computer last checked in at {}, so it may be off, asleep or without internet. I'll say when it's back.",
        clock(since)
    )
}

fn duration(ms: i64) -> String {
    let minutes = (ms as f64 / 60_000.0).round() as i64;
    if minutes < 60 {
        format!("{minutes}m")
    } else {
        format!("{}h {}m", minutes / 60, minutes % 60)
    }
}

fn channel(cx: &Relay) -> String {
    cx.state.borrow().dm().channel
}

async fn cancel(cx: &Relay, id: &str) {
    if let Err(e) = cx
        .slack
        .api(
            "chat.deleteScheduledMessage",
            json!({ "channel": channel(cx), "scheduled_message_id": id }),
        )
        .await
    {
        // One Slack has already sent can't be deleted, and needn't be.
        if !e.to_string().contains("invalid_scheduled_message_id") {
            cx.log(&format!("couldn't delete the offline note: {e}"));
        }
    }
}

/// Every note still scheduled in the DM but `keep`, and the one the state
/// last knew of, is deleted. A delete that failed, or a relay that stopped
/// between scheduling and deleting, leaves one behind, which Slack would
/// send as a stale warning, and enough of them make Slack refuse to
/// schedule more. Only the relay schedules messages in its DM.
async fn sweep(cx: &Relay, keep: Option<&str>, old: Option<&str>) {
    let listed: Vec<String> = match cx
        .slack
        .api(
            "chat.scheduledMessages.list",
            json!({ "channel": channel(cx) }),
        )
        .await
    {
        Ok(reply) => reply
            .get("scheduled_messages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_string))
            .collect(),
        Err(e) => {
            cx.log(&format!("couldn't list the scheduled offline notes: {e}"));
            Vec::new()
        }
    };
    let mut ids: Vec<String> = old.map(str::to_string).into_iter().collect();
    for id in listed {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    for id in ids.iter().filter(|id| Some(id.as_str()) != keep) {
        cancel(cx, id).await;
    }
}

/// Schedules the next note, then deletes the ones before, so there is
/// always one. Refused for having too many scheduled, it clears them and
/// tries again.
pub async fn renew_offline_note(cx: &Relay) -> Result<()> {
    let now = util::now_ms();
    let schedule = || async {
        let md = as_markdown(&offline_text(now), &[]);
        cx.slack
            .api(
                "chat.scheduleMessage",
                json!({
                    "channel": channel(cx),
                    "post_at": (now + OFFLINE_AFTER_MS) / 1000,
                    "text": md["text"],
                    "blocks": md["blocks"],
                }),
            )
            .await
    };
    let old = cx.state.borrow().offline_note.clone();
    let scheduled = match schedule().await {
        Ok(s) => s,
        Err(e) if e.to_string().contains("restricted_too_many") => {
            sweep(cx, old.as_ref().map(|o| o.id.as_str()), None).await;
            schedule().await?
        }
        Err(e) => return Err(e),
    };
    let id = scheduled
        .get("scheduled_message_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    cx.state.borrow_mut().offline_note = Some(OfflineNote {
        id: id.clone(),
        at: now,
    });
    cx.save();
    sweep(cx, Some(&id), old.as_ref().map(|o| o.id.as_str())).await;
    Ok(())
}

/// After the relay was gone, whether Slack sent the note (it was gone
/// longer than `OFFLINE_AFTER_MS`), and if so, that it is back.
pub async fn say_back_online(cx: &Relay) -> Result<()> {
    let Some(note) = cx.state.borrow().offline_note.clone() else {
        return Ok(());
    };
    let now = util::now_ms();
    if now < note.at + OFFLINE_AFTER_MS {
        return Ok(());
    }
    post(
        cx,
        &format!(
            "🤖 ✅ **Back online**, after {} offline (since {}).",
            duration(now - note.at),
            clock(note.at)
        ),
        None,
        PostOpts::default(),
    )
    .await?;
    Ok(())
}

pub async fn cancel_offline_note(cx: &Relay) {
    let Some(note) = cx.state.borrow().offline_note.clone() else {
        return;
    };
    cancel(cx, &note.id).await;
    cx.state.borrow_mut().offline_note = None;
    cx.save();
}
