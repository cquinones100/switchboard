//! Replying from the Slack view (UX 2.0 boards 107 to 110), with Settings'
//! `slack_reply` on: a message's menu offers the replies that apply, each
//! naming where it goes (a message in a channel: its thread, or the
//! channel; a direct message: the DM), and the reply box takes what you
//! type. Nothing is written for you: the text goes as typed.
//!
//! How it leaves is Settings' `slack_send_as`, for every message
//! Switchboard sends as you, through `deliver` alone. As a draft, the
//! default: Send makes a draft in that conversation with the connector's
//! `slack_send_message_draft`, which you send from Slack, since a message
//! the connector sends carries a "Sent using Claude" line Slack adds and a
//! draft you send carries none; nothing is asked first, as nothing is
//! sent. Slack keeps one draft a conversation, so one already there fails
//! (`attached_draft_exists`). Sent at once: Send asks first, naming who
//! can read it, and only a yes sends it, with `slack_send_message`.
//!
//! Either goes through Claude's Slack connector (`crate::connector`, a
//! headless Haiku allowed that one tool, the answer the tool's own
//! result), to the conversation the item names and, for a thread, its
//! first message's ts; a reply never names any other conversation. The
//! request audit counts it as the connector counts every call. Once sent
//! at once, what came before it in that conversation and thread is read
//! (`slack::mark_replied`), as a reply the look finds is; a draft marks
//! nothing until the look finds it sent.
//! New, with no tmux-version file behind it.

use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde_json::{Value, json};

use super::Connector;
use crate::items::Item;

/// The connector's tool that sends.
pub const TOOL: &str = "mcp__claude_ai_Slack__slack_send_message";

/// The connector's tool that makes a draft for you to send from Slack.
pub const DRAFT_TOOL: &str = "mcp__claude_ai_Slack__slack_send_message_draft";

/// How a message Switchboard sends as you leaves (Settings'
/// `slack_send_as`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    /// A draft in Slack, which you send there.
    Draft,
    /// Sent at once, after a yes.
    Now,
}

impl Way {
    /// The setting's value as a way: a draft unless it says at once.
    pub fn of(value: Option<&Value>) -> Way {
        match value.and_then(Value::as_str) {
            Some("now") => Way::Now,
            _ => Way::Draft,
        }
    }

    /// The way Settings has now.
    pub fn now() -> Way {
        Way::of(crate::settings::cached("slack_send_as").as_ref())
    }
}

/// What `deliver` did.
#[derive(Clone, Debug, PartialEq)]
pub enum Delivered {
    /// Sent, with the ts Slack gave it when its result said.
    Sent(Option<String>),
    /// A draft in Slack, with the link to where it is when the result
    /// gave one.
    Drafted(Option<String>),
}

/// Why `deliver` delivered nothing, in words for you.
#[derive(Clone, Debug, PartialEq)]
pub struct Undelivered {
    pub why: String,
    /// Slack already has an unsent draft in the conversation.
    pub draft_there: bool,
}

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
    /// The message's own link in Slack, which leads to its conversation.
    pub link: Option<String>,
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

    /// Where a draft of it is, said once it is made.
    pub fn drafted(&self) -> String {
        let place = match (&self.thread, self.dm) {
            (_, true) => format!("your DM with {}", self.place),
            (Some(_), false) => format!("{}, in {}'s thread", self.place, self.starter),
            (None, false) => self.place.clone(),
        };
        format!("Your reply is a draft in {place}. Send it from Slack.")
    }

    /// The link to where its draft is: the conversation's (`channel_link`,
    /// from the draft's result) for a DM or a channel, and for a thread the
    /// thread's first message there, opened as the thread.
    pub fn draft_link(&self, channel_link: &str) -> String {
        static ARCHIVE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"^(https://[^/?#]+/archives/)([A-Z0-9]+)/?$").expect("a valid pattern")
        });
        match (&self.thread, ARCHIVE.captures(channel_link)) {
            (Some(ts), Some(c)) => format!(
                "{}{}/p{}?thread_ts={ts}&cid={}",
                &c[1],
                &c[2],
                ts.replace('.', ""),
                &c[2]
            ),
            _ => channel_link.to_string(),
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
        link: match &item.target {
            crate::items::Target::Url(url) => Some(url.clone()),
            _ => None,
        },
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

/// A field of a draft's result, read the one way whichever form the
/// result takes: a JSON object's member, or `name: value` (or `name=value`)
/// in words or in an object inside it. `None` when the result has no such field or it holds nothing
/// (null, an empty string).
fn field(result: &str, name: &str) -> Option<String> {
    let member = match serde_json::from_str::<Value>(result) {
        Ok(Value::Object(members)) => members.get(name).cloned(),
        _ => None,
    };
    let value = match member {
        Some(Value::Null) => return None,
        Some(Value::String(text)) => text,
        Some(other) => other.to_string(),
        // In words, or inside what the object holds.
        None => {
            let named = Regex::new(&format!(
                r#"(?i)(?:^|[^A-Za-z0-9_]){}["']?\s*[:=]\s*["']?([^\s"',}}\]]*)"#,
                regex::escape(name)
            ))
            .ok()?;
            // Without its backslashes, so JSON kept as a string inside the
            // result (its quotes escaped) reads as the JSON it is.
            named.captures(&result.replace('\\', ""))?[1].to_string()
        }
    };
    let nothing = ["", "null", "none", "undefined"];
    (!nothing.contains(&value.to_ascii_lowercase().as_str())).then_some(value)
}

/// The conversation's link in a draft's result (`channel_link`), when it
/// is one of Slack's: the result is the connector's words, so no other
/// address in it is ever opened.
pub fn channel_link(result: &str) -> Option<String> {
    let link = field(result, "channel_link")?;
    // A browser reads `\` as `/` and what is before an `@` as a sign-in, so
    // a link with either could name another host than the one checked.
    if link.contains(['\\', '@']) || link.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let host = crate::links::host_of(&link)?;
    (link.starts_with("https://") && (host == "slack.com" || host.ends_with(".slack.com")))
        .then_some(link)
}

/// Why Slack made no draft, from what the tool said, in plain words.
fn no_draft(said: &str, reply: &Reply) -> Option<Undelivered> {
    let why = if said.contains("attached_draft_exists") {
        "Slack already has an unsent draft in this conversation: send or delete it there, then try again.".to_string()
    } else if said.contains("not_in_channel") {
        format!(
            "You aren't in {}, so Slack can't keep a draft there.",
            reply.place
        )
    } else if said.contains("channel_not_found") {
        format!("Slack couldn't find {}.", reply.place)
    } else if said.contains("failed_to_create_draft") {
        "Slack couldn't make the draft. Try again.".to_string()
    } else {
        return None;
    };
    Some(Undelivered {
        why,
        draft_there: said.contains("attached_draft_exists"),
    })
}

/// Whether a draft's result says Slack refused: one of the errors with
/// words of their own, or a result whose `ok` is false or whose `error`
/// names anything, as JSON or in words, whatever else it carries.
fn refused(result: &str) -> bool {
    const KNOWN: [&str; 4] = [
        "attached_draft_exists",
        "not_in_channel",
        "channel_not_found",
        "failed_to_create_draft",
    ];
    KNOWN.iter().any(|code| result.contains(code))
        || field(result, "ok").is_some_and(|ok| ok.eq_ignore_ascii_case("false"))
        || field(result, "error").is_some_and(|e| !e.eq_ignore_ascii_case("false"))
}

/// Whether a draft's result names the draft made: a `draft_id` that holds
/// an ID, as JSON or in words.
fn has_draft_id(result: &str) -> bool {
    field(result, "draft_id").is_some_and(|id| !id.eq_ignore_ascii_case("false"))
}

/// Makes a draft of `text` where `reply` says, for you to send from
/// Slack; gives the link to where it is, when the result has one.
async fn draft(
    connector: &impl Connector,
    reply: &Reply,
    text: &str,
) -> Result<Option<String>, Undelivered> {
    let plain = |why: String| Undelivered {
        why,
        draft_there: false,
    };
    if text.trim().is_empty() {
        return Err(plain(
            "Couldn't make the draft: there is nothing to send.".into(),
        ));
    }
    match connector
        .call(
            DRAFT_TOOL,
            reply.input(text),
            Some(Duration::from_secs(120)),
        )
        .await
    {
        // Made only when the result says so, with the draft's ID or the
        // conversation's link: a result that names an error, one of the
        // four known or any other, made none, and the text is kept.
        Ok(result) => {
            let link = channel_link(&result);
            if !refused(&result) && (has_draft_id(&result) || link.is_some()) {
                return Ok(link.map(|l| reply.draft_link(&l)));
            }
            Err(no_draft(&result, reply).unwrap_or_else(|| {
                let said = result.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                plain(format!(
                    "Slack didn't say the draft was made ({}): look in Slack before trying again.",
                    crate::ui::draw::cut(said.trim(), 120)
                ))
            }))
        }
        Err(e) if e.not_connected => Err(plain(format!("Couldn't make the draft: {}", e.message))),
        Err(e) => Err(no_draft(&e.message, reply).unwrap_or_else(|| {
            plain(format!(
                "The draft may not have been made ({}): look in Slack before trying again.",
                e.message
            ))
        })),
    }
}

/// The one way a message Switchboard sends as you leaves: as a draft in
/// Slack or sent at once, as `way` (Settings' `slack_send_as`) says.
pub async fn deliver(
    connector: &impl Connector,
    reply: &Reply,
    text: &str,
    way: Way,
) -> Result<Delivered, Undelivered> {
    match way {
        Way::Draft => draft(connector, reply, text).await.map(Delivered::Drafted),
        Way::Now => send(connector, reply, text)
            .await
            .map(Delivered::Sent)
            .map_err(|why| Undelivered {
                why,
                draft_there: false,
            }),
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

    /// A connector answering `0`, and writing down the tool it was asked.
    struct Answering(
        Result<String, String>,
        std::sync::Mutex<Vec<(String, Value)>>,
    );

    impl Answering {
        fn new(answer: Result<&str, &str>) -> Answering {
            Answering(
                answer.map(str::to_string).map_err(str::to_string),
                Default::default(),
            )
        }
    }

    impl Connector for Answering {
        fn call(
            &self,
            tool: &str,
            input: Value,
            _timeout: Option<Duration>,
        ) -> impl std::future::Future<Output = Result<String, crate::connector::ConnectorError>> + Send
        {
            self.1.lock().unwrap().push((tool.to_string(), input));
            let answer = self
                .0
                .clone()
                .map_err(|message| crate::connector::ConnectorError {
                    message,
                    not_connected: false,
                });
            async move { answer }
        }
    }

    #[test]
    fn a_draft_is_made_with_the_draft_tool_alone_and_its_link_is_the_threads() {
        let replies = replies_of(&item("mention", Some("1700000000.000100"), true));
        let slack = Answering::new(Ok(
            r#"{"channel_link":"https://acme.slack.com/archives/C1","draft_id":"Dr1"}"#,
        ));
        let made = crate::util::block_on(deliver(&slack, &replies[0], "On it", Way::Draft));
        assert_eq!(
            made,
            Ok(Delivered::Drafted(Some(
                "https://acme.slack.com/archives/C1/p1700000000000100?thread_ts=1700000000.000100&cid=C1"
                    .into()
            )))
        );
        let asked = slack.1.lock().unwrap().clone();
        assert_eq!(asked.len(), 1, "nothing is sent");
        assert_eq!(asked[0].0, DRAFT_TOOL);
        assert_eq!(
            asked[0].1,
            json!({ "channel_id": "C1", "message": "On it", "thread_ts": "1700000000.000100" })
        );
        assert_eq!(
            replies[0].drafted(),
            "Your reply is a draft in #web-app, in Ana's thread. Send it from Slack."
        );
        // The channel itself, and a DM: the conversation's own link.
        assert_eq!(
            replies[1].draft_link("https://acme.slack.com/archives/C1"),
            "https://acme.slack.com/archives/C1"
        );
        let dm = replies_of(&item("dm", None, false)).remove(0);
        assert_eq!(
            dm.drafted(),
            "Your reply is a draft in your DM with Ben. Send it from Slack."
        );
        // Sent at once goes through the send tool, as before.
        let slack = Answering::new(Ok("ts 1700000300.000400"));
        let sent = crate::util::block_on(deliver(&slack, &dm, "hi", Way::Now));
        assert_eq!(sent, Ok(Delivered::Sent(Some("1700000300.000400".into()))));
        assert_eq!(slack.1.lock().unwrap()[0].0, TOOL);
    }

    #[test]
    fn only_a_link_of_slacks_is_taken_from_a_drafts_result() {
        assert_eq!(
            channel_link("channel_link: https://acme.slack.com/archives/D1\ndraft_id: Dr1")
                .as_deref(),
            Some("https://acme.slack.com/archives/D1")
        );
        assert_eq!(
            channel_link(r#"{"channel_link":"https://slack.com.example.org/archives/C1"}"#),
            None
        );
        assert_eq!(
            channel_link(r#"{"channel_link":"http://acme.slack.com/archives/C1"}"#),
            None
        );
        assert_eq!(channel_link("draft made"), None);
        // A host a browser would read otherwise than the check does.
        assert_eq!(
            channel_link(r#"{"channel_link":"https://evil.example\\@acme.slack.com/archives/C1"}"#),
            None
        );
        // The same with an at sign alone, put together here so the test
        // holds nothing that reads as an address.
        let signed_in = format!(
            r#"{{"channel_link":"https://evil.example{}acme.slack.com/archives/C1"}}"#,
            '@'
        );
        assert_eq!(channel_link(&signed_in), None);
    }

    #[test]
    fn a_draft_slack_refused_says_why_in_plain_words() {
        let r = replies_of(&item("mention", None, false)).remove(0);
        let why = |said: &str| {
            crate::util::block_on(deliver(&Answering::new(Err(said)), &r, "hi", Way::Draft))
                .unwrap_err()
        };
        let there = why("Error: attached_draft_exists");
        assert!(there.draft_there);
        assert_eq!(
            there.why,
            "Slack already has an unsent draft in this conversation: send or delete it there, then try again."
        );
        assert_eq!(
            why("not_in_channel").why,
            "You aren't in #web-app, so Slack can't keep a draft there."
        );
        assert_eq!(
            why("channel_not_found").why,
            "Slack couldn't find #web-app."
        );
        assert_eq!(
            why("failed_to_create_draft").why,
            "Slack couldn't make the draft. Try again."
        );
        let unsure = why("timed out");
        assert!(!unsure.draft_there);
        assert!(unsure.why.contains("look in Slack before trying again"));
        // A result that names the error, with no draft made.
        let said = Answering::new(Ok(r#"{"ok":false,"error":"attached_draft_exists"}"#));
        assert!(
            crate::util::block_on(deliver(&said, &r, "hi", Way::Draft))
                .unwrap_err()
                .draft_there
        );
        // A refusal it has no words for, or a result that says nothing of a
        // draft, is no draft either, so the text is kept.
        let other = Answering::new(Ok(r#"{"ok":false,"error":"missing_scope"}"#));
        let none = crate::util::block_on(deliver(&other, &r, "hi", Way::Draft)).unwrap_err();
        assert!(none.why.contains("missing_scope"), "{}", none.why);
        assert!(none.why.starts_with("Slack didn't say the draft was made"));
        // A refusal that carries a draft's field or the conversation's link
        // all the same is still a refusal.
        for said in [
            r#"{"ok":false,"error":"failed_to_create_draft","draft_id":null}"#,
            r#"{"ok":false,"error":"missing_scope","draft_id":"Dr1"}"#,
            r#"{"error":"attached_draft_exists","channel_link":"https://acme.slack.com/archives/C1"}"#,
            r#"{"draft_id":null}"#,
            r#"{"draft_id":""}"#,
            "draft_id: null",
            "ok: false\nerror: missing_scope\nchannel_link: https://acme.slack.com/archives/C1",
            "Error=missing_scope draft_id=Dr1",
            r#"{"result":{"ok":false,"error":"missing_scope","draft_id":"Dr1"}}"#,
            r#"{"result":"{\"ok\":false,\"error\":\"missing_scope\"}","channel_link":"https://acme.slack.com/archives/C1"}"#,
        ] {
            let slack = Answering::new(Ok(said));
            assert!(
                crate::util::block_on(deliver(&slack, &r, "hi", Way::Draft)).is_err(),
                "{said}"
            );
        }
        // A draft's ID alone, as JSON or in words, is a draft made.
        for said in [
            r#"{"draft_id":"Dr1"}"#,
            "Draft created. draft_id: Dr1",
            r#"{"result":{"ok":true,"draft_id":"Dr1"}}"#,
        ] {
            let slack = Answering::new(Ok(said));
            assert_eq!(
                crate::util::block_on(deliver(&slack, &r, "hi", Way::Draft)),
                Ok(Delivered::Drafted(None)),
                "{said}"
            );
        }
        assert_eq!(Way::of(Some(&json!("now"))), Way::Now);
        assert_eq!(Way::of(Some(&json!("draft"))), Way::Draft);
        assert_eq!(Way::of(None), Way::Draft);
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
