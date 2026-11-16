//! Replying from the Slack view (UX 2.0 boards 107 to 110), with Settings'
//! `slack_reply` on: a message's menu offers the replies that apply, each
//! naming where it goes (a message in a channel: its thread, or the
//! channel; a direct message: the DM), the reply box takes what you type,
//! and Send asks first, naming who can read it. Nothing is sent without
//! that yes, and nothing is written for you: the text goes as typed.
//!
//! It is sent through Claude's Slack connector's `slack_send_message` alone
//! (`crate::connector`, a headless Haiku allowed that one tool, the answer
//! the tool's own result), to the conversation the item names and, for a
//! thread, its first message's ts; a reply never names any other
//! conversation. The request audit counts it as the connector counts every
//! call. Once sent, what came before it in that conversation and thread is
//! read (`slack::mark_replied`), as a reply the look finds is.
//! New with UX 2.0, with no tmux-version file behind it.

use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde_json::{Value, json};

use super::Connector;
use crate::items::Item;

/// The connector's tool that sends.
pub const TOOL: &str = "mcp__claude_ai_Slack__slack_send_message";

/// Where a reply goes, and the message it answers.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    pub channel: String,
    /// The thread's first message's ts, for a reply in the thread; `None`
    /// for one in the conversation itself.
    pub thread: Option<String>,
    /// The conversation as the view names it: `#web-app`, or the DM's
    /// people.
    pub place: String,
    pub dm: bool,
    /// Who started the thread.
    pub starter: String,
    /// The message it answers (the thread's first, once read), whose
    /// author is `starter`.
    pub text: String,
    /// The thread's replies, once read.
    pub replies: Option<u64>,
    /// The thread the item is in (`<channel>:<ts>`), read through the reply
    /// in the pane once it is sent, wherever the reply goes.
    pub shown: Option<String>,
}

impl Reply {
    /// Its option in the message's menu, naming where it goes.
    pub fn label(&self) -> String {
        match (&self.thread, self.dm) {
            (_, true) => format!("Reply to {}", self.place),
            (Some(_), false) => format!("Reply in {}'s thread", self.starter),
            (None, false) => format!("Reply in {}", self.place),
        }
    }

    /// The reply box's title.
    pub fn title(&self) -> String {
        match (&self.thread, self.dm) {
            (Some(_), false) => format!("Reply in {}'s thread · {}", self.starter, self.place),
            _ => self.label(),
        }
    }

    /// The box's first line: where it goes, word for word.
    pub fn to_line(&self) -> String {
        match (&self.thread, self.dm) {
            (_, true) => format!("To {}, in your direct messages", self.place),
            (Some(_), false) => {
                let mut line = format!("To {}, in the thread {} started", self.place, self.starter);
                if let Some(n) = self.replies {
                    line.push_str(&format!(" · {n} repl{}", if n == 1 { "y" } else { "ies" }));
                }
                line
            }
            (None, false) => format!("To {}, in the channel itself", self.place),
        }
    }

    /// The question Send asks.
    pub fn question(&self) -> String {
        match (&self.thread, self.dm) {
            (Some(_), false) => format!(
                "Send this reply to {}, in {}'s thread?",
                self.place, self.starter
            ),
            _ => format!("Send this reply to {}?", self.place),
        }
    }

    /// Who can read it, under the question.
    pub fn readers(&self) -> String {
        if self.dm {
            format!("{} can read it; it can't be unsent from here.", self.place)
        } else {
            format!(
                "Everyone in {} can read it; it can't be unsent from here.",
                self.place
            )
        }
    }

    /// The connector's input: the item's conversation and thread, and the
    /// text as typed.
    pub fn input(&self, text: &str) -> Value {
        let mut input = json!({ "channel_id": self.channel, "message": text });
        if let Some(t) = &self.thread {
            input["thread_ts"] = json!(t);
        }
        input
    }
}

/// The replies a Slack item's menu offers: a direct message's DM; a
/// message in a channel's thread (its own, or the one it is in), then the
/// channel. None for an item that names no conversation, or for a DM
/// with more than one person.
pub fn replies_of(item: &Item) -> Vec<Reply> {
    let d = &item.data;
    let get = |p: &str| {
        d.pointer(p)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    // Only a conversation ID and a ts as Slack writes them, whatever a
    // look saved, so nothing else reaches the connector's input.
    let (Some(channel), Some(ts)) = (
        get("/message/channel").filter(|c| super::slack::is_channel(c)),
        get("/message/ts").filter(|t| super::slack::is_ts(t)),
    ) else {
        return Vec::new();
    };
    let dm = get("/kind").as_deref() == Some("dm");
    // A direct message only to one person (a `D` conversation): the look
    // counts a group DM, and a private channel's `G` ID, as a DM too, and
    // saves no name that says who reads one, so none of those is offered.
    if dm && !channel.starts_with('D') {
        return Vec::new();
    }
    let from = get("/from").unwrap_or_else(|| "Someone".into());
    let place = get("/where").unwrap_or_else(|| {
        if dm {
            from.clone()
        } else {
            "the channel".into()
        }
    });
    // The thread's first message, once read, else the message itself.
    let (starter, text) = match (get("/root/from"), get("/root/text")) {
        (Some(f), Some(t)) => (f, t),
        _ => (from.clone(), item.lines.join("\n")),
    };
    let reply = |thread: Option<String>| Reply {
        channel: channel.clone(),
        thread,
        place: place.clone(),
        dm,
        starter: starter.clone(),
        text: text.clone(),
        replies: d.get("replies").and_then(Value::as_u64),
        shown: get("/thread"),
    };
    if dm {
        return vec![reply(None)];
    }
    let root = get("/message/thread")
        .filter(|t| super::slack::is_ts(t))
        .unwrap_or(ts);
    vec![reply(Some(root)), reply(None)]
}

/// The ts Slack gave the message sent, from the tool's result: the
/// `p<digits>` of its link, else a ts that isn't named as its thread's
/// (`thread_ts`, which a reply's link and result carry too).
pub fn sent_ts(result: &str) -> Option<String> {
    static TS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b(\d{9,11}\.\d{6})\b").expect("a valid pattern"));
    static LINK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"/p(\d{9,11})(\d{6})\b").expect("a valid pattern"));
    if let Some(c) = LINK.captures(result) {
        return Some(format!("{}.{}", &c[1], &c[2]));
    }
    TS.find_iter(result)
        .find(|m| {
            !result[..m.start()]
                .trim_end_matches(['"', '\'', ':', '=', ' '])
                .ends_with("thread_ts")
        })
        .map(|m| m.as_str().to_string())
}

/// Sends `text` as `reply` says; gives the ts Slack gave it, when its
/// result says (`None` when it doesn't: a time of the clock's would be
/// taken for one of Slack's, which the look's reading of threads goes
/// by), or what to tell you: that it wasn't sent, when the
/// connector wasn't connected, else that it may have been (a run that
/// failed or timed out after Slack had posted it), so look before sending
/// it again.
pub async fn send(
    connector: &impl Connector,
    reply: &Reply,
    text: &str,
) -> Result<Option<String>, String> {
    if text.trim().is_empty() {
        return Err("Couldn't send it: there is nothing to send.".into());
    }
    match connector
        .call(TOOL, reply.input(text), Some(Duration::from_secs(120)))
        .await
    {
        Ok(result) => Ok(sent_ts(&result)),
        Err(e) if e.not_connected => Err(format!("Couldn't send it: {}", e.message)),
        Err(e) => Err(format!(
            "It may not have been sent ({}): look in Slack before sending it again.",
            e.message
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: &str, thread: Option<&str>, root: bool) -> Item {
        let mut data = json!({
            "kind": kind,
            "message": { "channel": if kind == "dm" { "D1" } else { "C1" }, "ts": "1700000100.000200" },
            "from": "Ben",
            "where": if kind == "dm" { "Ben" } else { "#web-app" },
            "replies": 3,
        });
        if let Some(t) = thread {
            data["message"]["thread"] = json!(t);
        }
        if root {
            data["root"] =
                json!({ "from": "Ana", "text": "can you look?", "ts": "1700000000.000100" });
        }
        let mut item = Item::new("C1:1700000100.000200", "x").data(data);
        item.lines = vec!["I see it too".into()];
        item
    }

    #[test]
    fn a_channel_message_offers_its_thread_then_the_channel_and_a_dm_its_dm() {
        let replies = replies_of(&item("mention", Some("1700000000.000100"), true));
        let labels: Vec<String> = replies.iter().map(Reply::label).collect();
        assert_eq!(labels, ["Reply in Ana's thread", "Reply in #web-app"]);
        assert_eq!(replies[0].thread.as_deref(), Some("1700000000.000100"));
        assert_eq!(replies[0].title(), "Reply in Ana's thread · #web-app");
        assert_eq!(
            replies[0].to_line(),
            "To #web-app, in the thread Ana started · 3 replies"
        );
        assert_eq!(
            replies[0].question(),
            "Send this reply to #web-app, in Ana's thread?"
        );
        assert_eq!(replies[0].text, "can you look?");
        assert_eq!(replies[1].thread, None);
        assert_eq!(
            replies[1].input("hi"),
            json!({ "channel_id": "C1", "message": "hi" })
        );

        // A message in no thread starts its own.
        let own = replies_of(&item("channel", None, false));
        assert_eq!(own[0].thread.as_deref(), Some("1700000100.000200"));
        assert_eq!(own[0].label(), "Reply in Ben's thread");
        assert_eq!(own[0].text, "I see it too");

        let dm = replies_of(&item("dm", None, false));
        assert_eq!(dm.len(), 1);
        assert_eq!(dm[0].label(), "Reply to Ben");
        assert_eq!(dm[0].question(), "Send this reply to Ben?");
        assert!(dm[0].readers().starts_with("Ben can read it"));
        assert_eq!(dm[0].thread, None);

        // A group DM, or a private channel the look took for one: nobody
        // named reads it alone, so no reply is offered.
        let mut group = item("dm", None, false);
        group.data["message"]["channel"] = json!("G1");
        assert!(replies_of(&group).is_empty());
    }

    #[test]
    fn the_sent_ts_is_read_from_a_ts_or_a_link() {
        assert_eq!(
            sent_ts("Message sent: https://x.slack.com/archives/C1/p1700000300000400").as_deref(),
            Some("1700000300.000400")
        );
        assert_eq!(
            sent_ts(r#"{"ok":true,"ts":"1700000300.000400"}"#).as_deref(),
            Some("1700000300.000400")
        );
        // A reply's link and result name its thread too.
        assert_eq!(
            sent_ts(
                "https://x.slack.com/archives/C1/p1700000300000400?thread_ts=1700000000.000100"
            )
            .as_deref(),
            Some("1700000300.000400")
        );
        assert_eq!(
            sent_ts(r#"{"thread_ts":"1700000000.000100","ts":"1700000300.000400"}"#).as_deref(),
            Some("1700000300.000400")
        );
        assert_eq!(sent_ts("sent"), None);
    }

    /// A connector failing as `not_connected` says.
    struct Failing(bool);

    impl Connector for Failing {
        fn call(
            &self,
            _tool: &str,
            _input: Value,
            _timeout: Option<Duration>,
        ) -> impl std::future::Future<Output = Result<String, crate::connector::ConnectorError>> + Send
        {
            let err = crate::connector::ConnectorError {
                message: "it failed".into(),
                not_connected: self.0,
            };
            async move { Err(err) }
        }
    }

    #[test]
    fn a_failure_says_whether_it_may_have_gone_all_the_same() {
        let r = replies_of(&item("dm", None, false)).remove(0);
        let down = crate::util::block_on(send(&Failing(true), &r, "hi")).unwrap_err();
        assert!(down.starts_with("Couldn't send it"), "{down}");
        let unsure = crate::util::block_on(send(&Failing(false), &r, "hi")).unwrap_err();
        assert!(
            unsure.contains("look in Slack before sending it again"),
            "{unsure}"
        );
    }
}
