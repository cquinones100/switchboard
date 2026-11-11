//! Acting on a press of one of the relay's buttons (lib/slack-relay/
//! presses.js), as the reply it stands for would be acted on. A choice is
//! pressed only if the tab is still showing the very prompt the button was
//! made for; then the message's buttons are replaced by what was chosen, so
//! it can't be pressed twice. What a button made for a tab says goes to
//! that tab: the one the button names, not whichever tab the thread was
//! last linked to.

use anyhow::Result;
use serde_json::Value;

use super::Cx;
use super::buttons::{WELCOME_COMMANDS, still_showing};
use super::cleanup::{cleanup_note, closed_threads, delete_threads};
use super::commands::{OverDraft, Reply, close_tab, for_tab, handle, interrupt, send_waiting};
use super::dashboard::{Refresh, refresh_dashboard};
use super::post::{PostOpts, post, settle_choice, update};
use super::quickreplies::still_asking;
use super::screens::handle_screen_press;
use super::slack::{Message, Press};
use super::state::Target;
use super::summaries::progress;
use super::tabs::{self, label};

pub async fn handle_press(cx: &Cx, press: Press) -> Result<()> {
    let value = &press.value;
    let text = |k: &str| {
        value
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let a = text("a");
    if a == "refresh" {
        return refresh_dashboard(
            cx,
            Refresh {
                opened: true,
                ..Refresh::default()
            },
        )
        .await;
    }
    // Deleting closed tabs' threads: one from its header, or all of them.
    // Only threads recorded as closed are touched.
    if a == "delete-thread" || a == "cleanup" {
        let threads = if a == "cleanup" {
            closed_threads(cx)
        } else {
            vec![text("t")]
        };
        let result = delete_threads(cx, &threads).await;
        if a == "cleanup" || result.yours > 0 {
            post(
                cx,
                &format!("🤖 {}", cleanup_note(result)),
                None,
                PostOpts::default(),
            )
            .await?;
        }
        return refresh_dashboard(cx, Refresh::opened_quick()).await;
    }
    // A command button on the welcome message, answered at the top level
    // as if you had typed it there.
    if a == "do" {
        let c = text("c");
        let Some(message) = &press.message else {
            return Ok(());
        };
        if !WELCOME_COMMANDS.contains(&c.as_str()) {
            return Ok(());
        }
        handle(
            cx,
            Message {
                ts: message.ts.clone(),
                text: c,
                fresh: true,
                ..Message::default()
            },
        )
        .await?;
        return Ok(());
    }
    let thread = press
        .message
        .as_ref()
        .map(|m| m.thread_ts.clone().unwrap_or_else(|| m.ts.clone()));
    // The Home tab's Open and New tab, the buttons on the screens they
    // open, and Skill and Rename under a message, which open screens.
    if matches!(
        a.as_str(),
        "open" | "new" | "skill" | "skill-chosen" | "rename"
    ) || a.starts_with("screen-")
        || a.starts_with("home-")
    {
        return handle_screen_press(cx, &press, thread.as_deref()).await;
    }
    // Everything else is on a message.
    let Some(message) = press.message.clone() else {
        return Ok(());
    };
    let w = text("w");
    let tab = tabs::tab(cx, &w);
    let target = tab.as_ref().map(|t| Target {
        window: t.id.clone(),
        index: t.index,
        name: t.name.clone(),
        thread: thread.clone(),
    });
    let link = |cx: &Cx| {
        if let (Some(th), Some(t)) = (&thread, &target) {
            cx.state
                .borrow_mut()
                .alerts
                .entry(th.clone())
                .or_insert_with(|| t.clone());
        }
    };
    let reply = Reply::to(thread.as_deref());
    let say = |body: &'static str| post(cx, body, thread.as_deref(), PostOpts::default());
    cx.log(&format!(
        "pressed {a} {} for {w}",
        value
            .get("n")
            .or_else(|| value.get("c"))
            .map(crate::util::js_string)
            .unwrap_or_default()
    ));
    match a.as_str() {
        "opt" | "dialog" => {
            let Some((tab, option)) = still_showing(cx, value) else {
                say("🤖 That choice isn't showing any more, so I didn't press anything.").await?;
                return Ok(());
            };
            link(cx);
            // The other messages showing this prompt lose their buttons;
            // this one is edited to say what was chosen.
            settle_choice(
                cx,
                &tab.id,
                &format!("Answered: {}", option.said),
                Some(&message.ts),
            )
            .await;
            if let Err(e) = update(
                cx,
                &message.ts,
                &format!("{}\n\n_Chose **{}**._", message.text, option.said),
                &[],
            )
            .await
            {
                cx.log(&format!("couldn't update the pressed message: {e}"));
            }
            for_tab(
                cx,
                &tab,
                thread.as_deref(),
                Some(&message.ts),
                &option.answer,
            )
            .await?;
            cx.save();
        }
        // Progress and Stop, for the tab the button was made for.
        "cmd" => {
            let Some(tab) = tab else {
                say("🤖 That tab is closed.").await?;
                return Ok(());
            };
            link(cx);
            match text("c").as_str() {
                "progress" => {
                    progress(cx, &tab, &reply, Some(&message.ts)).await?;
                }
                "esc" => {
                    interrupt(cx, &tab, &reply).await?;
                }
                other => {
                    for_tab(cx, &tab, thread.as_deref(), Some(&message.ts), other).await?;
                }
            }
        }
        // A quick reply under a finished turn, typed in as your reply, while
        // it is still the answer to what Claude said last.
        "reply" => {
            let Some(tab) = tab else {
                say("🤖 That tab is closed.").await?;
                return Ok(());
            };
            if !still_asking(&tab, &text("k")) {
                say("🤖 Claude has said more since, so that reply is out of date. I didn't type it.").await?;
                return Ok(());
            }
            link(cx);
            let t = text("t");
            if let Err(e) = update(
                cx,
                &message.ts,
                &format!("{}\n\n_Replied **{t}**._", message.text),
                &[],
            )
            .await
            {
                cx.log(&format!("couldn't update the pressed message: {e}"));
            }
            for_tab(cx, &tab, thread.as_deref(), Some(&message.ts), &t).await?;
        }
        // Only the very tab the button was drawn for: same tab, same
        // process.
        "close" => {
            let Some(tab) = tab.filter(|t| t.pids.contains(&text("p"))) else {
                say("🤖 That tab has already closed.").await?;
                return Ok(());
            };
            close_tab(cx, &tab).await?;
            if let Err(e) = update(
                cx,
                &message.ts,
                &format!("🤖 Closed **{}**.", label(tab.index, &tab.name)),
                &[],
            )
            .await
            {
                cx.log(&format!("couldn't update the close message: {e}"));
            }
            if let Err(e) = refresh_dashboard(cx, Refresh::opened_quick()).await {
                cx.log(&format!("Home tab failed: {e}"));
            }
            cx.save();
        }
        "to" => {
            let pick = cx.mem.borrow().picks.0.get(&text("p")).cloned();
            let Some(pick) = pick else {
                say("🤖 I no longer have that message. Send it again, as a reply in the tab's thread.").await?;
                return Ok(());
            };
            let Some(tab) = tab else {
                say("🤖 That tab is closed.").await?;
                return Ok(());
            };
            cx.mem.borrow_mut().picks.0.shift_remove(&text("p"));
            if let Err(e) = update(
                cx,
                &message.ts,
                &format!("🤖 Sent to **{}**.", label(tab.index, &tab.name)),
                &[],
            )
            .await
            {
                cx.log(&format!("couldn't update the picker: {e}"));
            }
            for_tab(
                cx,
                &tab,
                pick.thread_ts.as_deref(),
                Some(&pick.ts),
                &pick.text,
            )
            .await?;
        }
        // What to do with a draft in the tab's prompt box, for the message
        // that waits on it: only the latest asked about for that tab.
        "draft" => {
            let waiting = cx
                .mem
                .borrow()
                .drafts
                .get(&w)
                .filter(|d| d.id == text("p"))
                .cloned();
            let Some(waiting) = waiting else {
                say("🤖 I no longer have that message. Send it again, as a reply in the tab's thread.").await?;
                return Ok(());
            };
            let Some(tab) = tab else {
                say("🤖 That tab is closed.").await?;
                return Ok(());
            };
            cx.mem.borrow_mut().drafts.remove(&w);
            let (chose, over) = match text("c").as_str() {
                "replace" => ("Replace it", Some(OverDraft::Replace)),
                "after" => ("Send after it", Some(OverDraft::After)),
                _ => ("Keep it", None),
            };
            if let Err(e) = update(
                cx,
                &message.ts,
                &format!("{}\n\n_Chose **{chose}**._", message.text),
                &[],
            )
            .await
            {
                cx.log(&format!("couldn't update the pressed message: {e}"));
            }
            match over {
                Some(over) => {
                    send_waiting(cx, &tab, waiting, over).await?;
                }
                None => {
                    let body = format!(
                        "Kept the draft in {}, and didn't type your message.",
                        label(tab.index, &tab.name)
                    );
                    super::commands::say(cx, &waiting.reply, &body, vec![]).await?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}
