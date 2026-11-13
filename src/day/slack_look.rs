//! The Slack view's look, run by the server in the background in place of a
//! Slack routine running /slack-updates: every `SLACK_LOOK_MS` while you're at
//! the computer, and when you go to the view. Ported from
//! lib/sidebar/slack-look.js. It reaches Slack through Claude's connector
//! (`crate::connector`), a headless Haiku allowed one tool at a time with no
//! session kept, so a look costs a few cheap calls and nothing grows.
//!
//! One search does it all: every message since the last look, in the
//! conversations you are in, a page (`SEARCH_PAGE`) to a call, at most
//! `MAX_PAGES`. Each result is sorted into what the view shows, as
//! /slack-updates sorts them: a DM or group DM ("dm"), one that mentions you
//! ("mention"), a reply by someone else in a thread you started ("thread"),
//! or anything in the channels listed in
//! `~/.config/switchboard/slack-updates.json` ("channel"); and what you wrote
//! yourself ("replied", which tells the view that what came before it in
//! that DM or thread was read). The rest is dropped. What it found is saved
//! as the skill's save does (`slack::save`), which also leaves out the
//! relay's own DM.
//!
//! Your member ID and the threads you started in the last day are kept in
//! `slack-look.json` in the state directory. The connector answers in text,
//! which `results_from` parses, so a change to its layout belongs there.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use super::{Connector, Dirs, slack};
use crate::util;

pub const SLACK_LOOK_MS: i64 = 60_000;
const SEARCH_PAGE: u32 = 20;
const MAX_PAGES: usize = 10;
const DAY_MS: i64 = 24 * 60 * 60_000;
const SLACK: &str = "mcp__claude_ai_Slack__";

/// A search result, as parsed from the connector's text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Found {
    pub channel: String,
    pub r#where: String,
    pub user: String,
    pub from: String,
    pub bot: bool,
    /// Each participant's name and member ID, for a DM.
    pub participants: Vec<(String, String)>,
    pub ts: String,
    pub thread: Option<String>,
    pub link: Option<String>,
    pub text: String,
}

static HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^### Result \d+ of \d+[ \t]*\r?$").unwrap());
static NAMED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.*) \(ID: (\w+)\)$").unwrap());
static FROM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.*?) \(ID: (\w+)\)").unwrap());
static PERSON: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([^,]+?) \(ID: (\w+)\)").unwrap());
static EMAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r" *<[^>]*>").unwrap());
static LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[link\]\((https://[^)\s]+)\)").unwrap());
static TS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+(\.\d+)?$").unwrap());

/// The search's detailed results, one block each:
///
/// ```text
/// ### Result 1 of 20
/// Channel: DM (ID: D0…)            or  Channel: #general (ID: C0…)
/// Participants: Ada (ID: U1), Bo (ID: U2)
/// From: Ada (ID: U1)  [BOT]
/// Message_ts: 1793282136.936439
/// Permalink: [link](https://….slack.com/archives/D0…/p1793…?thread_ts=1793…&cid=D0…)
/// Text:
/// …
/// ---
/// ```
pub fn results_from(text: &str) -> Vec<Found> {
    let starts: Vec<usize> = HEADER.find_iter(text).map(|m| m.end()).collect();
    let ends: Vec<usize> = HEADER
        .find_iter(text)
        .map(|m| m.start())
        .skip(1)
        .chain(std::iter::once(text.len()))
        .collect();
    starts
        .into_iter()
        .zip(ends)
        .map(|(from, to)| &text[from..to])
        .map(|block| {
            let field = |name: &str| {
                block
                    .lines()
                    .find_map(|l| l.strip_prefix(&format!("{name}: ")))
                    .map(|v| v.trim().to_string())
                    .unwrap_or_default()
            };
            let channel = field("Channel");
            let channel = NAMED.captures(&channel);
            let from_field = field("From");
            let from = FROM.captures(&from_field);
            let link = LINK.captures(&field("Permalink")).map(|c| c[1].to_string());
            let participants = PERSON
                .captures_iter(&field("Participants"))
                .map(|m| {
                    (
                        EMAIL.replace_all(&m[1], "").trim().to_string(),
                        m[2].to_string(),
                    )
                })
                .collect();
            let body = match block.find("\nText:") {
                Some(at) => {
                    let rest = &block[at + 1..];
                    let rest = rest.strip_prefix("Text:").unwrap_or(rest);
                    let rest = rest.strip_prefix(' ').unwrap_or(rest);
                    let rest = rest.strip_prefix('\n').unwrap_or(rest);
                    let rest = rest.trim_end();
                    rest.strip_suffix("---").unwrap_or(rest).to_string()
                }
                None => String::new(),
            };
            let thread = link.as_deref().and_then(|l| {
                let query = l.split_once('?')?.1;
                query.split('&').find_map(|kv| {
                    let (k, v) = kv.split_once('=')?;
                    (k == "thread_ts").then(|| v.to_string())
                })
            });
            Found {
                channel: channel
                    .as_ref()
                    .map(|c| c[2].to_string())
                    .unwrap_or_default(),
                r#where: channel
                    .as_ref()
                    .map(|c| c[1].to_string())
                    .unwrap_or_default(),
                user: from.as_ref().map(|c| c[2].to_string()).unwrap_or_default(),
                from: from
                    .as_ref()
                    .map(|c| EMAIL.replace_all(&c[1], "").into_owned())
                    .unwrap_or_default(),
                bot: from_field.contains("[BOT]"),
                participants,
                ts: field("Message_ts"),
                thread,
                link,
                text: body.trim().to_string(),
            }
        })
        .filter(|m| !m.channel.is_empty() && TS.is_match(&m.ts))
        .collect()
}

/// The connector's answer: JSON with the text under `results` (or
/// `messages`) and the next page's cursor in `pagination_info`, or the text
/// itself.
pub fn page_of(answer: &str) -> (String, Option<String>) {
    static CURSOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"cursor[^`]*`([^`]+)`").unwrap());
    let Ok(parsed) = serde_json::from_str::<Value>(answer) else {
        return (answer.to_string(), None);
    };
    let text = parsed
        .get("results")
        .or_else(|| parsed.get("messages"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let cursor = parsed
        .get("pagination_info")
        .and_then(Value::as_str)
        .and_then(|p| CURSOR.captures(p))
        .map(|c| c[1].to_string());
    (text, cursor)
}

/// A message's text as the view shows it: `<@U1|ada>` as `@ada`, a link
/// `<https://…|words>` as its words.
pub fn plain(text: &str) -> String {
    static MENTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<@\w+\|([^>]+)>").unwrap());
    static LINKED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"<(https?:[^|>]+)\|([^>]+)>").unwrap());
    LINKED
        .replace_all(&MENTION.replace_all(text, "@$1"), "$2")
        .into_owned()
}

/// Sorts what the search found, as the module's comment says: the items to
/// save and what you wrote. Threads you start are added to `mine`.
pub fn sort_look(
    messages: &[Found],
    me: &str,
    mine: &mut Vec<String>,
    channels: &[String],
) -> (Vec<Value>, Vec<Value>) {
    static DM_NAME: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^(DM|Group DM|Multi-party DM)\b").unwrap());
    let mut items = Vec::new();
    let mut replied = Vec::new();
    for m in messages {
        let reply = m.thread.as_deref().filter(|t| *t != m.ts);
        if m.user == me {
            let mut r = json!({ "channel": m.channel, "ts": m.ts });
            if let Some(t) = &m.thread {
                r["thread"] = json!(t);
            }
            replied.push(r);
            if reply.is_none() && !mine.contains(&m.ts) {
                mine.push(m.ts.clone());
            }
            continue;
        }
        let dm = DM_NAME.is_match(&m.r#where) || m.channel.starts_with(['D', 'G']);
        let kind = if dm {
            "dm"
        } else if m.text.contains(&format!("<@{me}")) {
            "mention"
        } else if reply.is_some_and(|t| mine.iter().any(|x| x == t)) {
            "thread"
        } else if channels.contains(&m.channel) {
            "channel"
        } else {
            continue;
        };
        let others: Vec<&str> = m
            .participants
            .iter()
            .filter(|(_, id)| id != me)
            .map(|(name, _)| name.as_str())
            .collect();
        let place = if dm {
            if others.is_empty() {
                m.from.clone()
            } else {
                others.join(", ")
            }
        } else {
            m.r#where.clone()
        };
        let text = plain(&m.text);
        let mut item = json!({
            "kind": kind, "channel": m.channel, "where": place, "user": m.user,
            "from": m.from, "text": text, "ts": m.ts,
        });
        if let Some(t) = reply {
            item["thread"] = json!(t);
        }
        if let Some(l) = &m.link {
            item["link"] = json!(l);
        }
        items.push(item);
    }
    (items, replied)
}

/// Looks at Slack once and saves what it found; gives a line for the log.
pub async fn look_slack(dirs: &Dirs, call: &impl Connector, now: i64) -> anyhow::Result<String> {
    let state_file = dirs.state.join("slack-look.json");
    let mut state = util::read_json_object(&state_file);
    let mut me = state.get("me").and_then(Value::as_str).map(str::to_string);
    if me.is_none() {
        static USER: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"User ID: (U[A-Z0-9]{6,})").unwrap());
        let profile = call
            .call(
                &format!("{SLACK}slack_read_user_profile"),
                json!({ "response_format": "detailed" }),
                None,
            )
            .await?;
        me = USER.captures(&profile).map(|c| c[1].to_string());
        let Some(id) = &me else {
            anyhow::bail!("couldn't tell your Slack member ID");
        };
        state.insert("me".into(), json!(id));
    }
    let me = me.expect("set above");
    // Where to look from: the last look less an overlap, and a day back for
    // your own messages until it keeps some.
    let (since, mine_since) = slack::since(&dirs.state, now);
    let (since, mine_since) = (since.div_euclid(1000), mine_since.div_euclid(1000));
    let from = since.min(if mine_since != 0 { mine_since } else { since });
    let channels: Vec<String> = util::read_json(dirs.config.join("slack-updates.json"))
        .and_then(|v| v.get("channels").cloned())
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    let mut mine: Vec<String> = state
        .get("mine")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(util::js_string)
                .filter(|ts| now - (ts.parse::<f64>().unwrap_or(0.0) * 1000.0) as i64 <= DAY_MS)
                .collect()
        })
        .unwrap_or_default();
    let day = jiff::Timestamp::from_second(from - 86_400)
        .map(|t| t.strftime("%Y-%m-%d").to_string())
        .unwrap_or_default();
    let mut messages = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut input = json!({
            "filters": format!("after:{day}"),
            "after": from.to_string(),
            "sort": "timestamp",
            "limit": SEARCH_PAGE,
            "include_context": false,
            "include_bots": true,
            "only_my_channels": true,
            "response_format": "detailed",
        });
        if let Some(c) = &cursor {
            input["cursor"] = json!(c);
        }
        let answer = call
            .call(
                &format!("{SLACK}slack_search_public_and_private"),
                input,
                None,
            )
            .await?;
        let (text, next) = page_of(&answer);
        messages.extend(results_from(&text));
        match next {
            Some(n) => cursor = Some(n),
            None => break,
        }
    }
    // Messages from before the last look were only searched for your own.
    let wanted: Vec<Found> = messages
        .into_iter()
        .filter(|m| m.user == me || m.ts.parse::<f64>().unwrap_or(0.0) >= since as f64)
        .collect();
    let (items, replied) = sort_look(&wanted, &me, &mut mine, &channels);
    let (n_items, n_replied) = (items.len(), replied.len());
    slack::save(
        &dirs.state,
        &dirs.relay,
        &json!({ "items": items, "replied": replied }),
        now,
    )?;
    let skip = mine.len().saturating_sub(500);
    state.insert("mine".into(), json!(mine[skip..]));
    util::write_atomic(&state_file, util::json_compact(&Value::Object(state)))?;
    Ok(format!(
        "slack: looked, {n_items} new, {n_replied} of yours"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_searchs_results_are_read_block_by_block() {
        let text = "# Search Results\n\n## Messages (2 results)\n### Result 1 of 2\nChannel: DM (ID: DDEE)\nParticipants: Me (ID: U0ME), Dee <dee@example.com> (ID: UDEE)\nFrom: Dee <dee@example.com> (ID: UDEE)\nTime: 2026-10-29 09:55:36 EDT\nMessage_ts: 1793282136.000100\nPermalink: [link](https://x.slack.com/archives/DDEE/p1793282136000100?thread_ts=1793282000.000100&cid=DDEE)\nText: \ndid you see this?\nsecond line\n\n---\n\n### Result 2 of 2\nChannel: #general (ID: CGEN)\nFrom: Bot (ID: UB)  [BOT]\nMessage_ts: not a ts\nText: \nskipped\n";
        let found = results_from(text);
        assert_eq!(found.len(), 1, "a result with no ts is left out");
        let m = &found[0];
        assert_eq!((m.channel.as_str(), m.r#where.as_str()), ("DDEE", "DM"));
        assert_eq!((m.user.as_str(), m.from.as_str()), ("UDEE", "Dee"));
        assert_eq!(m.participants[1], ("Dee".to_string(), "UDEE".to_string()));
        assert_eq!(m.thread.as_deref(), Some("1793282000.000100"));
        assert_eq!(m.text, "did you see this?\nsecond line");
    }

    #[test]
    fn a_page_gives_its_text_and_the_next_cursor() {
        let (text, cursor) =
            page_of(r#"{"results":"hits","pagination_info":"Use cursor `abc=` for more"}"#);
        assert_eq!((text.as_str(), cursor.as_deref()), ("hits", Some("abc=")));
        assert_eq!(page_of("plain").0, "plain");
    }
}
