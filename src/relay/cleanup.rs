//! Deleting closed tabs' threads (lib/slack-relay/cleanup.js). Slack
//! doesn't let you delete the app's messages in its DM, only your own, so
//! the relay deletes a closed tab's thread for you: its header and every
//! message the app posted in it, with the files it uploaded there. Your own
//! replies are yours to delete; Slack doesn't let an app delete them, so a
//! thread you replied in keeps them, under a deleted header.
//!
//! Only threads of tabs that have closed can go: `homes::close_home`
//! records each one when its tab closes or starts another session, and
//! nothing else is ever deleted, so a running tab's thread can't be.
//! Deleting is offered as a button that Slack confirms first.

use anyhow::Result;
use serde_json::json;

use super::Relay;
use super::post::{Button, remove};

pub fn closed_threads(cx: &Relay) -> Vec<String> {
    cx.state.borrow().closed_homes.keys().cloned().collect()
}

/// How many threads went, and how many of your replies stayed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Deleted {
    pub deleted: usize,
    pub yours: usize,
}

/// Deletes the closed threads named, replies first and the header last.
pub async fn delete_threads(cx: &Relay, threads: &[String]) -> Deleted {
    let mut result = Deleted::default();
    for ts in threads {
        if !cx.state.borrow().closed_homes.contains_key(ts) {
            continue;
        }
        // Your replies are counted as they are found, whether or not the
        // thread then goes, as cleanup.js counted them.
        let mut yours = 0;
        let done: Result<()> = async {
            let messages = cx.chat.thread_messages(ts).await?;
            for message in messages.iter().filter(|m| m.ts != *ts) {
                if !message.from_app {
                    yours += 1;
                    continue;
                }
                for id in &message.files {
                    if let Err(e) = cx.chat.delete_file(id).await {
                        cx.log(&format!("couldn't delete a file: {e}"));
                    }
                }
                if let Err(e) = remove(cx, &message.ts).await {
                    cx.log(&format!("couldn't delete a message: {e}"));
                }
            }
            if let Err(e) = remove(cx, ts).await
                && !e.to_string().contains("message_not_found")
            {
                return Err(e);
            }
            Ok(())
        }
        .await;
        result.yours += yours;
        match done {
            Ok(()) => {
                cx.chat.forget_thread(ts);
                let mut state = cx.state.borrow_mut();
                state.closed_homes.shift_remove(ts);
                state.threads.shift_remove(ts);
                state.latest.shift_remove(ts);
                state
                    .alerts
                    .retain(|key, a| key != ts && a.thread.as_deref() != Some(ts.as_str()));
                drop(state);
                result.deleted += 1;
                cx.log(&format!("deleted thread {ts}"));
            }
            Err(e) => cx.log(&format!("deleting thread {ts} failed: {e}")),
        }
        cx.save();
    }
    result
}

/// The button that deletes every closed thread, for the Home tab and
/// `cleanup`.
pub fn cleanup_button(count: usize) -> Button {
    let s = if count == 1 { "" } else { "s" };
    Button::new(format!("Delete {count} closed thread{s}"), json!({ "a": "cleanup" }))
        .style("danger")
        .confirm(json!({
            "title": { "type": "plain_text", "text": "Delete closed threads?" },
            "text": {
                "type": "plain_text",
                "text": format!("The relay deletes its messages in the threads of {count} closed tab{s}. Your own replies stay; Slack only lets you delete those. This can't be undone."),
            },
            "confirm": { "type": "plain_text", "text": "Delete" },
            "deny": { "type": "plain_text", "text": "Keep them" },
            "style": "danger",
        }))
}

pub fn cleanup_note(d: Deleted) -> String {
    let mut note = format!(
        "Deleted {} closed thread{}.",
        d.deleted,
        if d.deleted == 1 { "" } else { "s" }
    );
    if d.yours > 0 {
        note.push_str(&format!(
            " {} of your own replies stayed, since only you can delete those.",
            d.yours
        ));
    }
    note
}
