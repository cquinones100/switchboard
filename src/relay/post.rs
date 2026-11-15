//! Posting in the DM (lib/slack-relay/post.js): messages split to fit,
//! written as Markdown, with buttons; reactions, edits, deletions and
//! uploads; and the buttons a prompt is offered as, cleared once it is
//! answered or gone.
//!
//! Everything the relay posts is standard Markdown, the way Claude writes,
//! sent as a markdown block, which Slack renders: headings, **bold**,
//! [links](url), lists, tables and code. Slack's older mrkdwn would show
//! most of that as raw symbols. `text`, what a phone's notification and
//! the DM's preview show, is one plain line instead: the tab's name and
//! what happened where the message is about a tab, else the body's first
//! paragraph with its marks dropped.

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

/// The most a button's text can hold in Slack; a longer one is cut.
pub const BUTTON_TEXT: usize = 75;

/// A button as Block Kit wants it, its action named by its place.
pub fn button_element(b: &Button, id: &str) -> Value {
    let text: String = if b.text.chars().count() > BUTTON_TEXT {
        format!(
            "{}…",
            b.text.chars().take(BUTTON_TEXT - 1).collect::<String>()
        )
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

/// The most a notification's line holds.
pub const NOTICE_CHARS: usize = 100;

/// Markdown as the one plain line a notification shows: the words of its
/// first paragraph or heading (a code block passed over, unless code is
/// all there is), with no marks, quotes or line breaks, cut to
/// `NOTICE_CHARS`.
pub fn notice_line(md: &str) -> String {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};
    let mut words = String::new();
    let mut code = String::new();
    let mut in_code = false;
    for event in Parser::new(md) {
        match event {
            Event::Start(Tag::CodeBlock(_)) => in_code = true,
            Event::End(TagEnd::CodeBlock) => in_code = false,
            Event::Text(t) if in_code => code.push_str(&t),
            Event::Text(t) | Event::Code(t) => words.push_str(&t),
            Event::SoftBreak | Event::HardBreak => words.push(' '),
            Event::End(TagEnd::Paragraph | TagEnd::Heading(_)) if !words.trim().is_empty() => {
                break;
            }
            _ => {}
        }
    }
    let words = if words.trim().is_empty() { code } else { words };
    cut_to(
        &words.split_whitespace().collect::<Vec<_>>().join(" "),
        NOTICE_CHARS,
    )
}

/// `line` in at most `most` characters, an ellipsis ending one cut short.
fn cut_to(line: &str, most: usize) -> String {
    if line.chars().count() > most {
        let cut: String = line.chars().take(most.saturating_sub(1)).collect();
        format!("{}…", cut.trim_end())
    } else {
        line.to_string()
    }
}

/// A notification's line for a message about a tab: its name and what
/// happened ("Relay notices: waiting on you"), a long name cut so what
/// happened still fits.
pub fn tab_notice(name: &str, what: &str) -> String {
    let what = notice_line(what);
    let room = NOTICE_CHARS
        .saturating_sub(what.chars().count() + 2)
        .max(NOTICE_CHARS / 3);
    notice_line(&format!("{}: {what}", cut_to(&notice_line(name), room)))
}

/// A message's `text` and `blocks`: the plain `notice` line, and the
/// Markdown with its buttons in a row under it.
pub fn as_markdown(body: &str, notice: &str, buttons: &[Button]) -> Value {
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
    json!({ "text": notice, "blocks": blocks })
}

/// A message from `as_markdown` that mentions `owner` first: a rich text
/// block, the one Slack's own clients send a mention in, and the same
/// mention at the front of `text`.
pub fn with_mention(md: Value, owner: &str) -> Value {
    let mention = json!({
        "type": "rich_text",
        "elements": [{
            "type": "rich_text_section",
            "elements": [{ "type": "user", "user_id": owner }],
        }],
    });
    let mut blocks = vec![mention];
    blocks.extend(md["blocks"].as_array().cloned().unwrap_or_default());
    json!({
        "text": format!("<@{owner}> {}", md["text"].as_str().unwrap_or_default()),
        "blocks": blocks,
    })
}

/// Text longer than Slack shows in one message, as several, split at
/// paragraph breaks (or lines, or anywhere as a last resort), each under
/// `MAX_POST_CHARS`. A code block split across two is closed in the first
/// and opened again in the second. Lengths are counted in UTF-16 units, as
/// Slack counts them and post.js did, so text full of emoji splits as soon
/// as plain text would.
pub fn chunks(text: &str) -> Vec<String> {
    let units = |cs: &[char]| cs.iter().map(|c| c.len_utf16()).sum::<usize>();
    let mut parts = Vec::new();
    let mut rest: Vec<char> = text.chars().collect();
    while units(&rest) > MAX_POST_CHARS {
        // The most characters that fit, never half of one.
        let mut fit = 0;
        let mut used = 0;
        for c in &rest {
            if used + c.len_utf16() > MAX_POST_CHARS {
                break;
            }
            used += c.len_utf16();
            fit += 1;
        }
        let window = &rest[..fit];
        let last = |pattern: &[char]| {
            (0..=window.len().saturating_sub(pattern.len()))
                .rev()
                .find(|&i| window[i..].starts_with(pattern))
        };
        let cut = [last(&['\n', '\n']), last(&['\n'])]
            .into_iter()
            .flatten()
            .find(|&i| units(&window[..i]) > MAX_POST_CHARS / 2)
            .unwrap_or(fit);
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
/// quickreplies.rs); and the notification's line of each recent part, by
/// its ts, so an edit keeps the tab's name and what happened.
#[derive(Default)]
pub struct Memory {
    pub last_parts: IndexMap<String, (String, String)>,
    pub notices: IndexMap<String, String>,
}

const LAST_PARTS_KEPT: usize = 100;

fn channel(cx: &Relay) -> String {
    cx.state.borrow().dm().channel
}

/// Posts one message and gives its ts. Links aren't expanded into
/// previews, which would bury a short reply under a PR's card.
/// `mention` starts a thread reply with a mention of the owner, which is
/// what makes Slack notify you of a reply in a thread you don't follow (a
/// thread the relay started), without also sending it to the main DM.
pub async fn post_one(
    cx: &Relay,
    body: &str,
    thread: Option<&str>,
    mention: bool,
    notice: &str,
    buttons: &[Button],
) -> Result<String> {
    let mut params = json!({ "channel": channel(cx) });
    let mut md = as_markdown(body, notice, buttons);
    if let Some(t) = thread {
        params["thread_ts"] = json!(t);
        let owner = cx.state.borrow().dm().user;
        if mention && !owner.is_empty() {
            md = with_mention(md, &owner);
        }
    }
    params["text"] = md["text"].clone();
    params["blocks"] = md["blocks"].clone();
    params["unfurl_links"] = json!(false);
    params["unfurl_media"] = json!(false);
    let sent = cx.slack.api("chat.postMessage", params).await?;
    Ok(sent
        .get("ts")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string())
}

/// Options for `post`. `notice` is the notification's line (`tab_notice`);
/// without one, each part's `notice_line` is.
#[derive(Clone, Debug, Default)]
pub struct PostOpts {
    pub mention: bool,
    pub buttons: Vec<Button>,
    pub notice: Option<String>,
}

impl PostOpts {
    pub fn buttons(buttons: Vec<Button>) -> PostOpts {
        PostOpts {
            buttons,
            ..PostOpts::default()
        }
    }

    pub fn mention(mention: bool) -> PostOpts {
        PostOpts {
            mention,
            ..PostOpts::default()
        }
    }

    pub fn notice(mut self, notice: String) -> PostOpts {
        self.notice = Some(notice);
        self
    }
}

/// Posts `text`, in parts if it is long, and gives the first part's ts. A
/// message at the top level is a thread you may reply under, so it is
/// watched. Buttons go under the last part, where you finish reading.
pub async fn post(cx: &Relay, text: &str, thread: Option<&str>, opts: PostOpts) -> Result<String> {
    let parts = chunks(text);
    let only = parts.len() == 1;
    let notice_of = |part: &str| opts.notice.clone().unwrap_or_else(|| notice_line(part));
    let ts = post_one(
        cx,
        &parts[0],
        thread,
        opts.mention,
        &notice_of(&parts[0]),
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
        last = post_one(cx, part, thread, false, &notice_of(part), buttons).await?;
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
        // A line of its own outlives an edit, which would otherwise take
        // one from the edited body; the first part and the one with the
        // buttons are the ones edited.
        if let Some(notice) = &opts.notice {
            let notices = &mut mem.post.notices;
            notices.insert(ts.clone(), notice.clone());
            notices.insert(last.clone(), notice.clone());
            while notices.len() > LAST_PARTS_KEPT {
                notices.shift_remove_index(0);
            }
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
    // thread reply only when it mentions you.
    let length = text.encode_utf16().count();
    cx.log(&format!(
        "posted {length} characters {}",
        match (thread, opts.mention) {
            (None, _) => "in the main DM (notifies)",
            (Some(_), true) => "in a thread, mentioning you (notifies)",
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

/// Edits a message the relay posted, in place, keeping the notification's
/// line it was posted with while that is remembered.
pub async fn update(cx: &Relay, ts: &str, body: &str, buttons: &[Button]) -> Result<Value> {
    let kept = cx.mem.borrow().post.notices.get(ts).cloned();
    let md = as_markdown(body, &kept.unwrap_or_else(|| notice_line(body)), buttons);
    cx.slack
        .api(
            "chat.update",
            json!({ "channel": channel(cx), "ts": ts, "text": md["text"], "blocks": md["blocks"] }),
        )
        .await
}

/// Edits a message the relay posted, its notification's line now `notice`
/// (a header whose tab changed state), which later edits keep.
pub async fn update_noticed(
    cx: &Relay,
    ts: &str,
    body: &str,
    notice: &str,
    buttons: &[Button],
) -> Result<Value> {
    {
        let mut mem = cx.mem.borrow_mut();
        let notices = &mut mem.post.notices;
        notices.insert(ts.to_string(), notice.to_string());
        while notices.len() > LAST_PARTS_KEPT {
            notices.shift_remove_index(0);
        }
    }
    update(cx, ts, body, buttons).await
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
    // A long message went in parts, its buttons under the last, so that
    // part is the one an answer edits.
    let (ts, text) = match cx.mem.borrow().post.last_parts.get(ts) {
        Some((last, part)) if last != ts => (last.clone(), part.clone()),
        _ => (ts.to_string(), text.to_string()),
    };
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
    offered.messages.push(OfferedMessage { ts, text });
}

/// Takes the buttons off every message offering a tab's choice but
/// `except`, noting what was answered, and gives whether any of them now
/// says so, so an answer noted there needn't be posted as well.
pub async fn settle_choice(cx: &Relay, tab: &str, note: &str, except: Option<&str>) -> bool {
    let Some(offered) = cx.state.borrow_mut().choices.shift_remove(tab) else {
        return false;
    };
    let mut noted = false;
    for message in offered
        .messages
        .iter()
        .filter(|m| Some(m.ts.as_str()) != except)
    {
        match update(
            cx,
            &message.ts,
            &format!("{}\n\n_{note}_", message.text),
            &[],
        )
        .await
        {
            Ok(_) => noted = true,
            Err(e) => cx.log(&format!("couldn't clear a prompt's buttons: {e}")),
        }
    }
    noted
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
    fn long_text_is_measured_as_slack_measures_it() {
        // Each of these is two UTF-16 units, so 2,000 of them are 4,000
        // units, over what one message takes, though only 2,000 characters.
        let text = "🙂".repeat(2_000);
        let parts = chunks(&text);
        assert_eq!(parts.len(), 2);
        for part in &parts {
            assert!(part.encode_utf16().count() <= MAX_POST_CHARS);
        }
        assert_eq!(parts.concat(), text);
    }

    #[test]
    fn a_mention_comes_first_as_a_user_and_keeps_the_message_after_it() {
        let md = with_mention(
            as_markdown("**Waiting**", "Waiting", &[Button::new("Go", json!({}))]),
            "U1",
        );
        assert_eq!(md["text"], "<@U1> Waiting");
        assert_eq!(md["blocks"][0]["type"], "rich_text");
        assert_eq!(
            md["blocks"][0]["elements"][0]["elements"][0]["user_id"],
            "U1"
        );
        assert_eq!(md["blocks"][1]["type"], "markdown");
        assert_eq!(md["blocks"][2]["type"], "actions");
    }

    #[test]
    fn posts_and_edits_go_out_as_markdown_blocks_with_a_plain_notice() {
        let md = as_markdown(
            "**Hi**",
            "Hi",
            &[Button::new("Go", json!({ "a": "cmd" })).style("primary")],
        );
        assert_eq!(md["text"], "Hi");
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

    #[test]
    fn a_notice_is_the_first_paragraph_as_one_plain_line() {
        assert_eq!(
            notice_line(
                "🤖 **Tab 2, Notes** is **waiting on you**.\n\n> Should I go on?\n\n```\nls\n```"
            ),
            "🤖 Tab 2, Notes is waiting on you."
        );
        assert_eq!(
            notice_line("```\ncode\n```\n\n> _Quoted_ `x`\nmore"),
            "Quoted x more"
        );
        assert_eq!(notice_line("# Title\n\nbody"), "Title");
        assert_eq!(
            notice_line("```\nline 1\nline 2\n```"),
            "line 1 line 2",
            "a part that is only code still says something"
        );
        let long = notice_line(&"word ".repeat(60));
        assert_eq!(long.chars().count(), NOTICE_CHARS);
        assert!(long.ends_with('…'));
        assert_eq!(
            tab_notice("snake_case **tab**", "waiting on you"),
            "snake_case tab: waiting on you"
        );
        let long = tab_notice(&"name ".repeat(30), "may be stuck, nothing new for 11m");
        assert!(
            long.ends_with("…: may be stuck, nothing new for 11m"),
            "{long}"
        );
        assert!(long.chars().count() <= NOTICE_CHARS);
    }
}
