//! Posting in the DM (lib/slack-relay/post.js): messages split to fit,
//! written as Markdown, with buttons; reactions, edits, deletions and
//! uploads; and the buttons a prompt is offered as, cleared once it is
//! answered or gone.
//!
//! Everything the relay posts is standard Markdown, the way Claude writes,
//! sent as a markdown block, which Slack renders: headings, **bold**,
//! [links](url), lists, tables and code. Slack's older mrkdwn would show
//! most of that as raw symbols. `text` is the same words, for
//! notifications and previews.

use anyhow::Result;
use indexmap::IndexMap;
use serde_json::{Value, json};

use super::Relay;
use super::config::MAX_POST_CHARS;
use super::state::{Offered, OfferedMessage};
use crate::util;

/// A button: what pressing it means to the relay (see buttons.rs), or a
/// link, with an optional style and a dialog Slack shows before the press
/// counts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Button {
    pub text: String,
    pub value: Option<Value>,
    pub url: Option<String>,
    pub style: Option<&'static str>,
    pub confirm: Option<Value>,
}

impl Button {
    pub fn new(text: impl Into<String>, value: Value) -> Button {
        Button {
            text: text.into(),
            value: Some(value),
            ..Button::default()
        }
    }

    pub fn link(text: impl Into<String>, url: impl Into<String>) -> Button {
        Button {
            text: text.into(),
            url: Some(url.into()),
            ..Button::default()
        }
    }

    pub fn style(mut self, style: &'static str) -> Button {
        self.style = Some(style);
        self
    }

    pub fn confirm(mut self, confirm: Value) -> Button {
        self.confirm = Some(confirm);
        self
    }

    /// The button's action, as `a` in its value.
    pub fn action(&self) -> &str {
        self.value
            .as_ref()
            .and_then(|v| v.get("a"))
            .and_then(Value::as_str)
            .unwrap_or("")
    }
}

/// A button as Block Kit wants it, its action named by its place.
pub fn button_element(b: &Button, id: &str) -> Value {
    let text: String = if b.text.chars().count() > 75 {
        format!("{}…", b.text.chars().take(74).collect::<String>())
    } else {
        b.text.clone()
    };
    let mut element = json!({
        "type": "button",
        "action_id": format!("relay-{id}"),
        "text": { "type": "plain_text", "text": text, "emoji": true },
    });
    match (&b.url, &b.value) {
        (Some(url), _) => element["url"] = json!(url),
        (None, Some(value)) => element["value"] = json!(util::json_compact(value)),
        _ => {}
    }
    if let Some(style) = b.style {
        element["style"] = json!(style);
    }
    if let Some(confirm) = &b.confirm {
        element["confirm"] = confirm.clone();
    }
    element
}

/// A message's `text` and `blocks`: the Markdown, and its buttons in a row
/// under it.
pub fn as_markdown(body: &str, buttons: &[Button]) -> Value {
    let mut blocks = vec![json!({ "type": "markdown", "text": body })];
    if !buttons.is_empty() {
        blocks.push(json!({
            "type": "actions",
            "elements": buttons
                .iter()
                .enumerate()
                .map(|(i, b)| button_element(b, &i.to_string()))
                .collect::<Vec<_>>(),
        }));
    }
    json!({ "text": body, "blocks": blocks })
}

/// Text longer than Slack shows in one message, as several, split at
/// paragraph breaks (or lines, or anywhere as a last resort), each under
/// `MAX_POST_CHARS`. A code block split across two is closed in the first
/// and opened again in the second.
pub fn chunks(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut rest: Vec<char> = text.chars().collect();
    while rest.len() > MAX_POST_CHARS {
        let window = &rest[..MAX_POST_CHARS];
        let last = |pattern: &[char]| {
            (0..=window.len().saturating_sub(pattern.len()))
                .rev()
                .find(|&i| window[i..].starts_with(pattern))
        };
        let cut = [last(&['\n', '\n']), last(&['\n'])]
            .into_iter()
            .flatten()
            .find(|&i| i > MAX_POST_CHARS / 2)
            .unwrap_or(MAX_POST_CHARS);
        let mut part: String = rest[..cut].iter().collect();
        let mut after: Vec<char> = rest[cut..].to_vec();
        while after.first() == Some(&'\n') {
            after.remove(0);
        }
        if part.matches("```").count() % 2 == 1 {
            part.push_str("\n```");
            let mut opened: Vec<char> = "```\n".chars().collect();
            opened.extend(after);
            after = opened;
        }
        parts.push(part);
        rest = after;
    }
    parts.push(rest.into_iter().collect());
    parts
}

/// What this module keeps: the last part of each recent post, by its first
/// part's ts, where its buttons are, for adding more later (see
/// quickreplies.rs).
#[derive(Default)]
pub struct Memory {
    pub last_parts: IndexMap<String, (String, String)>,
}

const LAST_PARTS_KEPT: usize = 100;

fn channel(cx: &Relay) -> String {
    cx.state.borrow().dm().channel
}

/// Posts one message and gives its ts. Links aren't expanded into
/// previews, which would bury a short reply under a PR's card.
/// `broadcast` also shows a thread reply in the main DM, linked to its
/// thread.
pub async fn post_one(
    cx: &Relay,
    body: &str,
    thread: Option<&str>,
    broadcast: bool,
    buttons: &[Button],
) -> Result<String> {
    let mut params = json!({ "channel": channel(cx) });
    let md = as_markdown(body, buttons);
    params["text"] = md["text"].clone();
    params["blocks"] = md["blocks"].clone();
    if let Some(t) = thread {
        params["thread_ts"] = json!(t);
        if broadcast {
            params["reply_broadcast"] = json!(true);
        }
    }
    params["unfurl_links"] = json!(false);
    params["unfurl_media"] = json!(false);
    let sent = cx.slack.api("chat.postMessage", params).await?;
    Ok(sent
        .get("ts")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string())
}

/// Options for `post`.
#[derive(Clone, Debug, Default)]
pub struct PostOpts {
    pub broadcast: bool,
    pub buttons: Vec<Button>,
}

impl PostOpts {
    pub fn buttons(buttons: Vec<Button>) -> PostOpts {
        PostOpts {
            broadcast: false,
            buttons,
        }
    }

    pub fn broadcast(broadcast: bool) -> PostOpts {
        PostOpts {
            broadcast,
            buttons: Vec::new(),
        }
    }
}

/// Posts `text`, in parts if it is long, and gives the first part's ts. A
/// message at the top level is a thread you may reply under, so it is
/// watched. Buttons go under the last part, where you finish reading.
pub async fn post(cx: &Relay, text: &str, thread: Option<&str>, opts: PostOpts) -> Result<String> {
    let parts = chunks(text);
    let only = parts.len() == 1;
    let ts = post_one(
        cx,
        &parts[0],
        thread,
        opts.broadcast,
        if only { &opts.buttons } else { &[] },
    )
    .await?;
    let mut last = ts.clone();
    for (i, part) in parts[1..].iter().enumerate() {
        let buttons: &[Button] = if i == parts.len() - 2 {
            &opts.buttons
        } else {
            &[]
        };
        last = post_one(cx, part, thread, false, buttons).await?;
    }
    {
        let mut mem = cx.mem.borrow_mut();
        let kept = &mut mem.post.last_parts;
        kept.insert(
            ts.clone(),
            (last.clone(), parts.last().cloned().unwrap_or_default()),
        );
        while kept.len() > LAST_PARTS_KEPT {
            kept.shift_remove_index(0);
        }
    }
    {
        let mut state = cx.state.borrow_mut();
        match thread {
            Some(t) => state.newest_in(t, &last),
            None => state.watch_thread(&ts, util::now_ms()),
        }
    }
    // Where it went says whether Slack notified you: the main DM does, a
    // thread reply only when also sent to the main DM.
    let length = text.encode_utf16().count();
    cx.log(&format!(
        "posted {length} characters {}",
        match (thread, opts.broadcast) {
            (None, _) => "in the main DM (notifies)",
            (Some(_), true) => "in a thread and the main DM (notifies)",
            (Some(_), false) => "in a thread",
        }
    ));
    Ok(ts)
}

/// Reacts to a message with `emoji` (a name, such as "eyes"). Reacting
/// twice is not an error worth reporting.
pub async fn react(cx: &Relay, ts: Option<&str>, emoji: &str) -> Result<()> {
    let Some(ts) = ts.filter(|t| !t.is_empty()) else {
        return Ok(());
    };
    match cx
        .slack
        .api(
            "reactions.add",
            json!({ "channel": channel(cx), "timestamp": ts, "name": emoji }),
        )
        .await
    {
        Err(e) if !e.to_string().contains("already_reacted") => Err(e),
        _ => Ok(()),
    }
}

/// Takes the relay's reaction off a message; one that isn't there is fine.
pub async fn unreact(cx: &Relay, ts: &str, emoji: &str) -> Result<()> {
    match cx
        .slack
        .api(
            "reactions.remove",
            json!({ "channel": channel(cx), "timestamp": ts, "name": emoji }),
        )
        .await
    {
        Err(e) if !e.to_string().contains("no_reaction") => Err(e),
        _ => Ok(()),
    }
}

/// Deletes a message the relay posted.
pub async fn remove(cx: &Relay, ts: &str) -> Result<Value> {
    cx.slack
        .api("chat.delete", json!({ "channel": channel(cx), "ts": ts }))
        .await
}

/// Edits a message the relay posted, in place.
pub async fn update(cx: &Relay, ts: &str, body: &str, buttons: &[Button]) -> Result<Value> {
    let md = as_markdown(body, buttons);
    cx.slack
        .api(
            "chat.update",
            json!({ "channel": channel(cx), "ts": ts, "text": md["text"], "blocks": md["blocks"] }),
        )
        .await
}

/// A file to upload.
#[derive(Clone, Debug)]
pub struct Upload {
    pub filename: String,
    pub title: String,
    pub content: Vec<u8>,
}

/// As many files as Slack shows in one message.
pub const FILES_PER_MESSAGE: usize = 10;

/// Uploads files into the DM, in `thread` if given: each file's bytes go
/// to an address Slack gives for it, then the files are shared together,
/// `FILES_PER_MESSAGE` to a message. A file that fails to upload is left
/// out and handed to `failed`; the rest still go. Gives the names shared.
pub async fn upload_files(
    cx: &Relay,
    files: Vec<Upload>,
    thread: Option<&str>,
    failed: &dyn Fn(&str, &anyhow::Error),
) -> Result<Vec<String>> {
    let mut sent: Vec<(String, String, String)> = Vec::new();
    for Upload {
        filename,
        title,
        content,
    } in files
    {
        let uploaded: Result<String> = async {
            let target = cx
                .slack
                .api(
                    "files.getUploadURLExternal",
                    json!({ "filename": filename, "length": content.len() }),
                )
                .await?;
            let url = target
                .get("upload_url")
                .and_then(Value::as_str)
                .unwrap_or_default();
            cx.slack.put(url, content).await?;
            Ok(target
                .get("file_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string())
        }
        .await;
        match uploaded {
            Ok(id) => sent.push((id, title, filename)),
            Err(e) => failed(&filename, &e),
        }
    }
    for batch in sent.chunks(FILES_PER_MESSAGE) {
        let files: Vec<Value> = batch
            .iter()
            .map(|(id, title, _)| json!({ "id": id, "title": title }))
            .collect();
        let mut params = json!({ "files": files, "channel_id": channel(cx) });
        if let Some(t) = thread {
            params["thread_ts"] = json!(t);
        }
        cx.slack.api("files.completeUploadExternal", params).await?;
    }
    Ok(sent.into_iter().map(|(_, _, name)| name).collect())
}

/// Every message showing a prompt's buttons for a tab: the alert in its
/// thread and the notice in the main DM (and `kind`, "wait" or "dialog",
/// says which sort of prompt). Answering from one clears the buttons from
/// the others, and a prompt that has gone, answered at the computer or
/// replaced, clears them all, so no button is left that would only be
/// refused.
pub fn offer_choice(cx: &Relay, tab: &str, kind: &str, ts: &str, text: &str) {
    let mut state = cx.state.borrow_mut();
    let offered = state
        .choices
        .entry(tab.to_string())
        .or_insert_with(|| Offered {
            kind: kind.to_string(),
            messages: Vec::new(),
        });
    if offered.kind != kind {
        offered.kind = kind.to_string();
        offered.messages.clear();
    }
    offered.messages.push(OfferedMessage {
        ts: ts.to_string(),
        text: text.to_string(),
    });
}

pub async fn settle_choice(cx: &Relay, tab: &str, note: &str, except: Option<&str>) {
    let Some(offered) = cx.state.borrow_mut().choices.shift_remove(tab) else {
        return;
    };
    for message in offered
        .messages
        .iter()
        .filter(|m| Some(m.ts.as_str()) != except)
    {
        if let Err(e) = update(
            cx,
            &message.ts,
            &format!("{}\n\n_{note}_", message.text),
            &[],
        )
        .await
        {
            cx.log(&format!("couldn't clear a prompt's buttons: {e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_text_splits_at_line_breaks_and_keeps_code_blocks_whole() {
        assert_eq!(chunks("short"), ["short"]);
        let lines: Vec<String> = (0..400)
            .map(|i| format!("line {i} of the output"))
            .collect();
        let text = format!("intro\n```\n{}\n```\nend", lines.join("\n"));
        let parts = chunks(&text);
        assert!(parts.len() > 1);
        for part in &parts {
            assert!(part.chars().count() <= MAX_POST_CHARS + 4);
            assert_eq!(
                part.matches("```").count() % 2,
                0,
                "every part opens and closes its code block"
            );
        }
        assert_eq!(parts.join("\n").replace("\n```\n```\n", "\n"), text);
    }

    #[test]
    fn posts_and_edits_go_out_as_markdown_blocks_with_the_same_words() {
        let md = as_markdown(
            "**Hi**",
            &[Button::new("Go", json!({ "a": "cmd" })).style("primary")],
        );
        assert_eq!(md["text"], "**Hi**");
        assert_eq!(
            md["blocks"][0],
            json!({ "type": "markdown", "text": "**Hi**" })
        );
        let button = &md["blocks"][1]["elements"][0];
        assert_eq!(button["value"], r#"{"a":"cmd"}"#);
        assert_eq!(button["style"], "primary");
        let long = button_element(&Button::new("x".repeat(80), json!({})), "0");
        assert_eq!(long["text"]["text"].as_str().unwrap().chars().count(), 75);
    }
}
