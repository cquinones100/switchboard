//! Mission control's Slack view: what is new for you on Slack in the last
//! day, newest first, as the server's own look (`slack_look`, the
//! slack_background setting) or the /slack-updates skill in a Slack tab last
//! saved it (`slack.json` in the state directory): direct messages and
//! mentions marked as needing you, then replies in your threads and the
//! channels you chose. Ported from lib/sidebar/slack.js, and the saving from
//! the skill's scripts/save.js, which `switchboard slack-updates-save` runs
//! and the look calls directly. The relay's own messages are never in it.
//!
//! Slack's connector can't say what you have read, so the view remembers
//! what you have seen in it: leaving the view marks everything the look on
//! screen had brought as seen (`slack-seen`, the look's time). Seen messages
//! go below the new ones, faded, so what is new since you last looked is on
//! top. It can say what you wrote, though: a message before your own later
//! one in the same DM, or in the same thread, has been read (`replied`).
//! One message acted on from its menu is seen on its own
//! (`slack-seen-items.json`), and one dismissed is gone until something
//! newer comes in its thread (`slack-dismissed.json`), both kept two days.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::items::{Item, State, Target, ViewData};
use crate::util;

pub const STALE_MS: i64 = 15 * 60_000;
const DISMISS_KEEP_MS: i64 = 2 * 24 * 60 * 60_000;
/// Messages over a day old are let go.
pub const KEEP_MS: i64 = 24 * 60 * 60_000;
// The first look goes back only this far: a day's worth is mostly read.
const FIRST_MS: i64 = 60 * 60_000;
// Each look starts this long before the last one was saved: a message sent
// while that look was searching, or one Slack's search had yet to index, is
// found the next time. Messages already saved aren't saved twice.
const OVERLAP_MS: i64 = 10 * 60_000;
const MAX_ITEMS: usize = 300;
const KINDS: [&str; 4] = ["mention", "dm", "thread", "channel"];

pub fn file(state: &Path) -> PathBuf {
    state.join("slack.json")
}

fn seen_file(state: &Path) -> PathBuf {
    state.join("slack-seen")
}

fn seen_items_file(state: &Path) -> PathBuf {
    state.join("slack-seen-items.json")
}

fn dismissed_file(state: &Path) -> PathBuf {
    state.join("slack-dismissed.json")
}

/// One message, as saved.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// "dm", "mention", "thread" or "channel".
    pub kind: String,
    pub channel: String,
    #[serde(default)]
    pub r#where: String,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub text: String,
    pub ts: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

impl Message {
    fn ts(&self) -> f64 {
        self.ts.parse().unwrap_or(0.0)
    }

    // Its thread: its channel and the thread's ts, or its own ts when it
    // starts or is in none, so a reply to it lands in the same one.
    fn thread_key(&self) -> String {
        format!(
            "{}:{}",
            self.channel,
            self.thread
                .as_deref()
                .filter(|t| !t.is_empty())
                .unwrap_or(&self.ts)
        )
    }
}

/// What a look saved.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Saved {
    /// When, in milliseconds.
    pub checked: i64,
    pub items: Vec<Message>,
    /// The newest ts you wrote in each conversation and thread.
    pub replied: Map<String, Value>,
    has_replied: bool,
}

/// `slack.json`, or `None` before anything is saved.
pub fn read(state: &Path) -> Option<Saved> {
    let v = util::read_json(file(state))?;
    let items = v
        .get("items")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|i| serde_json::from_value(i.clone()).ok())
                .collect()
        })
        .unwrap_or_default();
    Some(Saved {
        checked: v.get("checked").and_then(Value::as_f64).unwrap_or(0.0) as i64,
        items,
        has_replied: v.get("replied").is_some_and(Value::is_object),
        replied: v
            .get("replied")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
    })
}

fn seen_through(state: &Path) -> i64 {
    util::read_text(seen_file(state))
        .and_then(|t| t.trim().parse::<f64>().ok())
        .unwrap_or(0.0) as i64
}

/// Leaving the view: what its look brought is seen.
pub fn mark_seen(state: &Path) {
    let Some(data) = read(state) else {
        return;
    };
    if data.checked == 0 || data.checked <= seen_through(state) {
        return;
    }
    let _ = util::write_atomic(seen_file(state), data.checked.to_string());
}

/// One message acted on (anything picked from its menu): seen, though the
/// rest of the look isn't.
pub fn mark_item_seen(state: &Path, channel: &str, ts: &str, now: i64) {
    if channel.is_empty() || ts.is_empty() {
        return;
    }
    let mut kept: Map<String, Value> = util::read_json_object(seen_items_file(state))
        .into_iter()
        .filter(|(_, at)| now - at.as_f64().unwrap_or(0.0) as i64 <= DISMISS_KEEP_MS)
        .collect();
    kept.insert(format!("{channel}:{ts}"), json!(now));
    let _ = util::write_atomic(
        seen_items_file(state),
        util::json_compact(&Value::Object(kept)),
    );
}

/// A message to dismiss, from an item's `data.message`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ref {
    pub channel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    pub ts: String,
}

/// Dismisses messages until their threads get something newer.
pub fn dismiss(state: &Path, messages: &[Ref], now: i64) {
    let mut all = util::read_json_object(dismissed_file(state));
    for m in messages {
        let key = Message {
            channel: m.channel.clone(),
            thread: m.thread.clone(),
            ts: m.ts.clone(),
            ..Message::default()
        }
        .thread_key();
        let ts: f64 = m.ts.parse().unwrap_or(0.0);
        let was = all.get(&key).and_then(Value::as_f64).unwrap_or(0.0);
        all.insert(key, util::js_number(was.max(ts)));
    }
    let cutoff = (now - DISMISS_KEEP_MS) as f64 / 1000.0;
    all.retain(|_, ts| ts.as_f64().unwrap_or(0.0) >= cutoff);
    let _ = util::write_atomic(
        dismissed_file(state),
        util::json_compact(&Value::Object(all)),
    );
}

const fn mark(kind: &str) -> &'static str {
    match kind.as_bytes() {
        b"dm" => "DM",
        b"mention" => "@",
        b"thread" => "↳",
        b"channel" => "#",
        _ => "",
    }
}

/// text.js's `ago`: `<1m`, `4m`, `1h`, `2d`.
pub fn ago(seconds: i64, now_ms: i64) -> String {
    if seconds == 0 {
        return String::new();
    }
    let s = (now_ms.div_euclid(1000) - seconds).max(0);
    if s < 60 {
        "<1m".into()
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86_400)
    }
}

/// What the view shows with nothing to list: when it last looked, so an
/// empty view reads as current rather than broken.
pub fn empty_words(checked: i64, now_ms: i64) -> String {
    let secs = checked.div_euclid(1000);
    if now_ms / 1000 - secs < 60 {
        "Nothing new on Slack. Last checked just now.".into()
    } else {
        format!(
            "Nothing new on Slack. Last checked {} ago.",
            ago(secs, now_ms)
        )
    }
}

/// What the view knows besides the saved look.
pub struct Look<'a> {
    pub state: &'a Path,
    /// The slack_background setting: the server looks itself.
    pub background: bool,
    /// Whether a tab already keeps it (the Slack tab with Claude, or a tab
    /// looping /slack-updates).
    pub looping: bool,
    /// Why the connector is down, when it is.
    pub down: Option<String>,
    pub now: i64,
}

/// The Slack view: always there, saying, before anything is saved, that a
/// look is coming. Each item's `lines` are the message as written (for
/// drawing as Slack does), its `detail` the same on one line, its `data`
/// `{ kind, old, message: { channel, thread, ts } }`; seen ones below the
/// new, faded (`State::Idle`), with "· seen" after the title. With nothing to
/// list, `at` says when it last looked (`empty_words`).
pub fn slack_view(look: &Look) -> ViewData {
    let Some(data) = read(look.state) else {
        let note = if look.background {
            look.down
                .clone()
                .unwrap_or_else(|| "Nothing saved yet: mission control is looking.".into())
        } else if look.looping {
            "Nothing saved yet: the Slack tab is looking.".into()
        } else {
            "Nothing saved yet: going to this view opens a Slack tab, which looks every 5 minutes (M-p, Open a Slack tab).".into()
        };
        return ViewData {
            error: Some(note),
            ..ViewData::default()
        };
    };
    let seen = seen_through(look.state);
    let acted = util::read_json_object(seen_items_file(look.state));
    let gone = util::read_json_object(dismissed_file(look.state));
    let replied_after = |k: &str, ts: f64| {
        data.replied
            .get(k)
            .map(|v| util::js_string(v).parse::<f64>().unwrap_or(0.0) > ts)
            .unwrap_or(false)
    };
    let mut items: Vec<(bool, i64, Item)> = data
        .items
        .iter()
        .filter(|m| m.ts() > gone.get(&m.thread_key()).and_then(Value::as_f64).unwrap_or(0.0))
        .map(|m| {
            let answered = if m.kind == "dm" {
                replied_after(&m.channel, m.ts())
            } else {
                replied_after(
                    &format!("{}:{}", m.channel, m.thread.as_deref().unwrap_or(&m.ts)),
                    m.ts(),
                )
            };
            let time = (m.ts() * 1000.0) as i64;
            let old = time <= seen || answered || acted.contains_key(&format!("{}:{}", m.channel, m.ts));
            let from = if m.from.is_empty() { "Someone" } else { &m.from };
            let place = if m.r#where.is_empty() { "Slack" } else { &m.r#where };
            let mut item = Item::new(
                format!("{}:{}", m.channel, m.ts),
                format!(
                    "{} {from} · {place}{}",
                    mark(&m.kind),
                    if old { " · seen" } else { "" }
                ),
            )
            .detail(m.text.split_whitespace().collect::<Vec<_>>().join(" "))
            .state(if old {
                State::Idle
            } else if m.kind == "dm" || m.kind == "mention" {
                State::Attention
            } else {
                State::Active
            })
            .time(time)
            .data(json!({
                "kind": m.kind,
                "old": old,
                "message": Ref { channel: m.channel.clone(), thread: m.thread.clone(), ts: m.ts.clone() },
            }));
            item.lines = m.text.split('\n').map(str::to_string).collect();
            if let Some(link) = &m.link {
                item = item.target(Target::Url(link.clone()));
            }
            (old, time, item)
        })
        .collect();
    items.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    let catching = if look.background {
        "mission control looks every minute while you're here"
    } else if look.looping {
        "the Slack tab looks every 5 minutes"
    } else {
        "opening a Slack tab to catch up"
    };
    // Its connector down says so first: nothing else will fix it.
    let error = look.down.clone().filter(|_| look.background).or_else(|| {
        stale(Some(&data), look.now).then(|| {
            format!(
                "Out of date, last looked {} ago: {catching}.",
                ago(data.checked.div_euclid(1000), look.now)
            )
        })
    });
    ViewData {
        items: items.into_iter().map(|(_, _, i)| i).collect(),
        error,
        at: Some(data.checked),
        ..ViewData::default()
    }
}

/// Saved more than `STALE_MS` ago.
pub fn stale(data: Option<&Saved>, now: i64) -> bool {
    data.is_some_and(|d| now - d.checked > STALE_MS)
}

/// Whether a tab keeps the Slack view: the Slack tab with Claude in it, or
/// one looping /slack-updates.
pub fn keeps_slack(name: &str, runs_claude: bool, loop_prompt: Option<&str>) -> bool {
    (name == "Slack" && runs_claude)
        || loop_prompt.is_some_and(|p| p == "/slack-updates" || p.starts_with("/slack-updates "))
}

// ── Saving a look (the skill's scripts/save.js) ─────────────────────────────

/// Where to look back to: `OVERLAP_MS` before the last look, or an hour ago
/// the first time; and where to look for your own messages from: the same,
/// or a day back while none have been kept yet. Both in milliseconds.
pub fn since(state: &Path, now: i64) -> (i64, i64) {
    let saved = read(state);
    let since = match saved.as_ref().map(|s| s.checked).filter(|c| *c != 0) {
        Some(c) => c - OVERLAP_MS,
        None => now - FIRST_MS,
    };
    let mine = if saved.is_some_and(|s| s.has_replied) {
        since
    } else {
        now - KEEP_MS
    };
    (since, mine)
}

fn text(value: Option<&Value>, max: usize) -> String {
    let s = match value {
        Some(Value::Null) | None => String::new(),
        Some(v) => util::js_string(v),
    };
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}

// A message keeps its lines, for the views to draw as Slack does (bullets,
// quotes, code blocks), with no more than one blank line together.
fn body(value: Option<&Value>, max: usize) -> String {
    let s = match value {
        Some(Value::Null) | None => String::new(),
        Some(v) => util::js_string(v),
    };
    let cleaned: Vec<String> = s
        .split('\n')
        .map(|line| {
            line.trim_end_matches([' ', '\t'])
                .chars()
                .filter(|c| !matches!(*c as u32, 0..=8 | 0x0b..=0x1f | 0x7f))
                .collect()
        })
        .collect();
    static BLANKS: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\n{3,}").unwrap());
    BLANKS
        .replace_all(&cleaned.join("\n"), "\n\n")
        .trim()
        .chars()
        .take(max)
        .collect()
}

fn is_ts(text: &str) -> bool {
    let mut parts = text.splitn(2, '.');
    let whole = parts.next().unwrap_or("");
    !whole.is_empty()
        && whole.bytes().all(|b| b.is_ascii_digit())
        && parts
            .next()
            .is_none_or(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()))
}

fn https(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|u| u.starts_with("https://") && u.len() > 8 && !u.chars().any(char::is_whitespace))
        .map(str::to_string)
}

/// The relay's DM and bot, from its state file, whose messages are left out.
pub fn relay_dm(relay: &Path) -> (Option<String>, Option<String>) {
    let dm = util::read_json(relay.join("state.json"))
        .and_then(|v| v.get("dm").cloned())
        .unwrap_or(Value::Null);
    let get = |k: &str| dm.get(k).and_then(Value::as_str).map(str::to_string);
    (get("channel"), get("bot"))
}

/// Adds what a look found (`{ items, replied }`, as /slack-updates writes
/// it) to `slack.json` and marks the look: the relay's DM and bot left out,
/// anything over a day old let go, at most `MAX_ITEMS` kept, newest first.
/// Gives what to tell the skill.
pub fn save(state: &Path, relay: &Path, found: &Value, now: i64) -> std::io::Result<String> {
    let saved = read(state).unwrap_or_default();
    let (relay_channel, relay_bot) = relay_dm(relay);
    let empty = Vec::new();
    let items: Vec<Message> = found
        .get("items")
        .and_then(Value::as_array)
        .unwrap_or(&empty)
        .iter()
        .filter(|i| {
            i.get("kind")
                .and_then(Value::as_str)
                .is_some_and(|k| KINDS.contains(&k))
                && is_ts(&i.get("ts").map(util::js_string).unwrap_or_default())
        })
        .filter(|i| {
            let channel = i.get("channel").and_then(Value::as_str);
            let user = i.get("user").and_then(Value::as_str);
            !(relay_channel.is_some() && channel == relay_channel.as_deref())
                && !(relay_bot.is_some() && user == relay_bot.as_deref())
        })
        .map(|i| Message {
            kind: i["kind"].as_str().unwrap_or("").to_string(),
            channel: text(i.get("channel"), 40),
            r#where: text(i.get("where"), 80),
            user: text(i.get("user"), 40),
            from: text(i.get("from"), 80),
            text: body(i.get("text"), 2000),
            ts: util::js_string(&i["ts"]),
            thread: i.get("thread").map(util::js_string).filter(|t| is_ts(t)),
            link: https(i.get("link")),
        })
        .collect();
    let new = items.len();
    let mut replied = saved.replied.clone();
    let channel_ok = |c: &str| {
        !c.is_empty()
            && c.bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    };
    for r in found
        .get("replied")
        .and_then(Value::as_array)
        .unwrap_or(&empty)
    {
        let channel = r.get("channel").map(util::js_string).unwrap_or_default();
        let ts = r.get("ts").map(util::js_string).unwrap_or_default();
        if !channel_ok(&channel) || !is_ts(&ts) {
            continue;
        }
        let thread = r
            .get("thread")
            .map(util::js_string)
            .filter(|t| is_ts(t))
            .unwrap_or_else(|| ts.clone());
        let at: f64 = ts.parse().unwrap_or(0.0);
        for k in [channel.clone(), format!("{channel}:{thread}")] {
            let was = replied
                .get(&k)
                .map(|v| util::js_string(v).parse::<f64>().unwrap_or(f64::NAN));
            if !was.is_some_and(|w| w >= at) {
                replied.insert(k, json!(ts));
            }
        }
    }
    replied.retain(|_, v| {
        let ts = util::js_string(v).parse::<f64>().unwrap_or(0.0);
        now - (ts * 1000.0) as i64 <= KEEP_MS
    });
    let mut merged: indexmap::IndexMap<String, Message> = indexmap::IndexMap::new();
    for m in saved.items.into_iter().chain(items) {
        merged.insert(format!("{}:{}", m.channel, m.ts), m);
    }
    let mut kept: Vec<Message> = merged
        .into_values()
        .filter(|m| now - (m.ts() * 1000.0) as i64 <= KEEP_MS)
        .collect();
    kept.sort_by(|a, b| b.ts().total_cmp(&a.ts()));
    kept.truncate(MAX_ITEMS);
    let total = kept.len();
    let out = json!({ "checked": now, "items": kept, "replied": replied });
    util::write_atomic(file(state), util::json_compact(&out))?;
    Ok(format!("Saved {new} new, {total} in the last day."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(name: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "sb-slack-{name}-{}-{}",
            std::process::id(),
            util::now_ms()
        ));
        let (state, relay) = (root.join("state"), root.join("relay"));
        std::fs::create_dir_all(&relay).unwrap();
        std::fs::write(
            relay.join("state.json"),
            r#"{"dm":{"channel":"DRELAY","bot":"UBOT"}}"#,
        )
        .unwrap();
        (state, relay)
    }

    fn titles(state: &Path, now: i64) -> Vec<String> {
        slack_view(&Look {
            state,
            background: true,
            looping: false,
            down: None,
            now,
        })
        .items
        .into_iter()
        .map(|i| i.title)
        .collect()
    }

    // test/tmux-sidebar.test.js's "the Slack view lists what /slack-updates
    // saved, newest first, without the relay's own messages, and says when
    // it is out of date".
    #[test]
    fn the_view_lists_what_was_saved_newest_first_and_says_when_it_is_out_of_date() {
        let (state, relay) = dirs("view");
        let now = util::now_ms();
        let secs = now as f64 / 1000.0;
        let empty = slack_view(&Look {
            state: &state,
            background: true,
            looping: false,
            down: None,
            now,
        });
        assert_eq!(
            empty.error.as_deref(),
            Some("Nothing saved yet: mission control is looking.")
        );
        let (first, _) = since(&state, now);
        assert_eq!(first, now - 3_600_000, "an hour back the first time");
        let ts = |ago: f64| format!("{:.6}", secs - ago);
        let items = json!([
            { "kind": "mention", "channel": "C1", "where": "#general", "user": "U1", "from": "Ada", "text": "can you look\nat this", "ts": ts(60.0), "link": "https://x.slack.com/archives/C1/p1" },
            { "kind": "dm", "channel": "DRELAY", "where": "relay", "user": "U2", "from": "Me", "text": "tests failed", "ts": ts(30.0) },
            { "kind": "channel", "channel": "C2", "where": "#ops", "user": "UBOT", "from": "relay bot", "text": "notice", "ts": ts(20.0) },
            { "kind": "thread", "channel": "C3", "where": "#dev", "user": "U3", "from": "Lin", "text": "replied", "ts": ts(10.0), "link": "javascript:alert(1)" },
            { "kind": "dm", "channel": "D4", "where": "Old", "user": "U4", "from": "Old", "text": "stale", "ts": ts(2.0 * 86400.0) },
        ]);
        save(&state, &relay, &json!({ "items": items }), now).unwrap();
        save(&state, &relay, &json!({ "items": [items[0]] }), now).unwrap();
        assert_eq!(
            since(&state, now).0,
            now - 600_000,
            "the next look starts ten minutes before this one"
        );
        let view = slack_view(&Look {
            state: &state,
            background: true,
            looping: false,
            down: None,
            now,
        });
        let got: Vec<(&str, Option<&Target>)> = view
            .items
            .iter()
            .map(|i| (i.title.as_str(), Some(&i.target)))
            .collect();
        assert_eq!(got[0].0, "↳ Lin · #dev");
        assert_eq!(got[1].0, "@ Ada · #general");
        assert_eq!(
            view.items.len(),
            2,
            "newest first, once each, no relay DM or bot, nothing over a day old"
        );
        assert_eq!(view.items[0].target, Target::None, "only https links");
        assert_eq!(
            view.items[1].target,
            Target::Url("https://x.slack.com/archives/C1/p1".into())
        );
        assert_eq!(view.items[1].detail, "can you look at this");
        assert_eq!(view.items[1].lines, ["can you look", "at this"]);
        assert_eq!(view.error, None);

        // Leaving the view marks what its look brought as seen: below
        // anything new, faded.
        mark_seen(&state);
        let later = now + 1000;
        save(&state, &relay, &json!({ "items": [{ "kind": "dm", "channel": "D9", "where": "Bo", "user": "U9", "from": "Bo", "text": "new one", "ts": format!("{:.6}", later as f64 / 1000.0) }] }), later).unwrap();
        assert_eq!(
            titles(&state, later),
            [
                "DM Bo · Bo",
                "↳ Lin · #dev · seen",
                "@ Ada · #general · seen"
            ]
        );

        // Dismissed until its thread gets something newer.
        dismiss(
            &state,
            &[
                Ref {
                    channel: "C1".into(),
                    thread: None,
                    ts: items[0]["ts"].as_str().unwrap().into(),
                },
                Ref {
                    channel: "C3".into(),
                    thread: None,
                    ts: items[3]["ts"].as_str().unwrap().into(),
                },
            ],
            later,
        );
        assert_eq!(titles(&state, later), ["DM Bo · Bo"]);
        let reply_at = later + 1000;
        save(&state, &relay, &json!({ "items": [{ "kind": "mention", "channel": "C1", "where": "#general", "user": "U5", "from": "Cy", "text": "a reply", "ts": format!("{:.6}", reply_at as f64 / 1000.0), "thread": items[0]["ts"] }] }), reply_at).unwrap();
        assert_eq!(
            titles(&state, reply_at),
            ["@ Cy · #general", "DM Bo · Bo"],
            "a reply in its thread comes back, the dismissed message itself doesn't"
        );

        // Something you wrote later in the same DM, or thread, means you
        // read it in Slack.
        let wrote = format!("{:.6}", (reply_at + 1000) as f64 / 1000.0);
        save(&state, &relay, &json!({ "items": [], "replied": [{ "channel": "D9", "ts": wrote }, { "channel": "C1", "ts": wrote, "thread": items[0]["ts"] }] }), reply_at).unwrap();
        let mut read = titles(&state, reply_at);
        read.sort();
        assert_eq!(
            read,
            ["@ Cy · #general · seen", "DM Bo · Bo · seen"],
            "both read in Slack"
        );

        let old = slack_view(&Look {
            state: &state,
            background: true,
            looping: false,
            down: None,
            now: reply_at + 3_600_000,
        });
        assert_eq!(
            old.error.as_deref(),
            Some(
                "Out of date, last looked 1h ago: mission control looks every minute while you're here."
            )
        );
        let _ = std::fs::remove_dir_all(state.parent().unwrap());
    }

    #[test]
    fn an_empty_view_says_when_slack_was_last_checked() {
        let now = 1_800_000_000_000;
        assert_eq!(
            empty_words(now - 10_000, now),
            "Nothing new on Slack. Last checked just now."
        );
        assert_eq!(
            empty_words(now - 4 * 60_000, now),
            "Nothing new on Slack. Last checked 4m ago."
        );
    }

    #[test]
    fn a_message_keeps_its_lines_but_no_more_than_one_blank_together() {
        assert_eq!(body(Some(&json!("a  \n\n\n\nb\u{7}\n")), 100), "a\n\nb");
    }
}
