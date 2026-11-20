//! The Slack app (lib/slack-relay/slack.js): the relay talks to you in the
//! app's DM, as its bot, through Slack's Web API, and hears you through
//! Socket Mode, a WebSocket the relay opens to Slack, so nothing on this
//! computer listens for connections. Two tokens, both stored as secrets and
//! never in a file: the bot token (xoxb-…) for the Web API, and the
//! app-level token (xapp-…, scope connections:write) for the socket, each
//! stored with `switchboard platform secret-set <service> -a "$USER"`,
//! which reads it from stdin rather than the command line.
//!
//! The rules that keep the relay from being anyone else's remote control
//! are all here, and every Slack call goes through them:
//!
//! - `Slack::api` refuses, before anything reaches Slack, any call that
//!   names a conversation other than the DM, publishes the Home tab to
//!   anyone but the owner, or opens a DM with anyone else.
//! - `message_from` acts on a message only if `from_owner` passes (the
//!   owner, in the DM, in the relay's workspace, not a bot) and
//!   `typed_by_you` does (Slack's own `client_msg_id`, no `app_id`).
//! - `press_from` takes a button only from the owner, on a message in the
//!   DM, or on the Home tab or a tab's screen, which take only their own
//!   buttons.
//! - `submission_from` takes a form only from the owner, from one of the
//!   relay's own screens.
//!
//! The owner comes from `slack-relay.json`, never from a message, so
//! nothing anyone sends can make them the owner. Reactions control nothing.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::LazyLock;

use anyhow::{Result, bail};
use futures_util::future::LocalBoxFuture;
use regex::Regex;
use serde_json::{Value, json};

use super::config::Paths;

pub const BOT_TOKEN_SERVICE: &str = "slack-relay-bot-token";
pub const APP_TOKEN_SERVICE: &str = "slack-relay-app-token";

/// Slack's Web API, or `SWITCHBOARD_SLACK_API`, which the tests point at a
/// server of their own.
pub fn api_base() -> String {
    let base = crate::net::base_url("SWITCHBOARD_SLACK_API", "https://slack.com/api");
    format!("{base}/")
}

/// A stored token, for this user's account, else stored for the service
/// alone (the Keychain's generic password the Node relay read with
/// `security find-generic-password -a $USER -s <service>`).
pub fn token(service: &str) -> Option<String> {
    let user = std::env::var("USER").unwrap_or_default();
    crate::platform::secret_for(service, &user).or_else(|| crate::platform::secret(service))
}

/// Whose relay this is: your Slack member ID, `{ "owner": "U…" }` in
/// `slack-relay.json`. Only what that member sends, in the app's DM with
/// them, is ever acted on.
pub fn load_owner(paths: &Paths) -> Option<String> {
    static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[UW][A-Z0-9]+$").expect("a regex"));
    let config = crate::util::read_json(paths.config_file())?;
    let owner = config.get("owner")?.as_str()?;
    ID.is_match(owner).then(|| owner.to_string())
}

/// How the relay reaches Slack: a Web API call, a file's bytes sent to the
/// address Slack gave for it, and a file Slack keeps fetched with the bot's
/// token. The relay's own is `http::Https`; the tests put stand-ins here,
/// so they never reach Slack.
pub trait Transport {
    fn call<'a>(&'a self, method: &'a str, params: &'a Value) -> LocalBoxFuture<'a, Result<Value>>;
    fn put<'a>(&'a self, url: &'a str, bytes: Vec<u8>) -> LocalBoxFuture<'a, Result<()>>;
    fn get<'a>(&'a self, url: &'a str) -> LocalBoxFuture<'a, Result<Vec<u8>>>;
}

/// The Web API as the relay may use it: the DM, once known, and the owner.
pub struct Slack {
    pub transport: Box<dyn Transport>,
    pub channel: RefCell<Option<String>>,
    pub owner: RefCell<Option<String>>,
}

impl Slack {
    pub fn new(transport: Box<dyn Transport>) -> Slack {
        Slack {
            transport,
            channel: RefCell::new(None),
            owner: RefCell::new(None),
        }
    }

    /// One Web API call as the bot. Everything the relay writes goes to the
    /// DM: a call naming any other conversation is refused here, before it
    /// reaches Slack, whatever asked for it. The Home tab is published only
    /// to the owner, and the only DM opened is the owner's.
    pub async fn api(&self, method: &str, params: Value) -> Result<Value> {
        refuse(
            method,
            &params,
            self.channel.borrow().as_deref(),
            self.owner.borrow().as_deref(),
        )?;
        self.transport.call(method, &params).await
    }

    pub async fn put(&self, url: &str, bytes: Vec<u8>) -> Result<()> {
        self.transport.put(url, bytes).await
    }

    pub async fn get(&self, url: &str) -> Result<Vec<u8>> {
        self.transport.get(url).await
    }
}

/// Why a call may not go out, or nothing.
pub fn refuse(method: &str, params: &Value, dm: Option<&str>, owner: Option<&str>) -> Result<()> {
    for key in ["channel", "channel_id"] {
        match params.get(key) {
            None | Some(Value::Null) => {}
            Some(Value::String(c)) if Some(c.as_str()) == dm => {}
            Some(other) => {
                let named = other
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or(other.to_string());
                bail!("{method}: slack-relay only uses its DM, not {named}");
            }
        }
    }
    match params.get("user_id") {
        None | Some(Value::Null) => {}
        Some(Value::String(u)) if Some(u.as_str()) == owner => {}
        Some(_) => bail!("{method}: slack-relay only shows its Home tab to its owner"),
    }
    match params.get("users") {
        None | Some(Value::Null) => {}
        Some(Value::String(u)) if Some(u.as_str()) == owner => {}
        Some(_) => bail!("{method}: slack-relay only opens its DM with its owner"),
    }
    Ok(())
}

/// The app's DM with you: conversations.open returns the existing one.
pub async fn open_dm(slack: &Slack, owner: &str) -> Result<super::state::Dm> {
    let opened = slack
        .api("conversations.open", json!({ "users": owner }))
        .await?;
    let auth = slack.api("auth.test", json!({})).await?;
    let text = |v: &Value, p: &str| v.pointer(p).and_then(Value::as_str).map(str::to_string);
    Ok(super::state::Dm {
        channel: text(&opened, "/channel/id").unwrap_or_default(),
        user: owner.to_string(),
        team: text(&auth, "/team_id"),
        bot: text(&auth, "/user_id"),
        url: text(&auth, "/url"),
        app: true,
        app_id: None,
    })
}

// ── The relay's chat ────────────────────────────────────────────────────────

/// The relay's chat as Slack: each of `Chat`'s calls as the Web API method
/// the relay has always made, through `Slack::api` and its rules.
pub struct SlackChat {
    pub slack: Slack,
    /// The app's bot user, whose posts cleaning up a thread may delete.
    bot: RefCell<Option<String>>,
}

impl SlackChat {
    pub fn new(transport: Box<dyn Transport>) -> SlackChat {
        SlackChat {
            slack: Slack::new(transport),
            bot: RefCell::new(None),
        }
    }

    fn channel(&self) -> String {
        self.slack.channel.borrow().clone().unwrap_or_default()
    }
}

/// The ts in a Web API answer.
fn ts_of(reply: &Value) -> String {
    reply
        .get("ts")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

impl super::chat::Chat for SlackChat {
    fn kind(&self) -> super::chat::Kind {
        super::chat::Kind::Slack
    }

    fn set_dm(&self, dm: &super::state::Dm) {
        *self.slack.channel.borrow_mut() = Some(dm.channel.clone());
        *self.slack.owner.borrow_mut() = Some(dm.user.clone());
        *self.bot.borrow_mut() = dm.bot.clone();
    }

    fn open_dm<'a>(&'a self, owner: &'a str) -> super::chat::Fut<'a, super::state::Dm> {
        Box::pin(async move {
            *self.slack.owner.borrow_mut() = Some(owner.to_string());
            open_dm(&self.slack, owner).await
        })
    }

    fn workspace_url(&self) -> super::chat::Fut<'_, Option<String>> {
        Box::pin(async move {
            Ok(self
                .slack
                .api("auth.test", json!({}))
                .await
                .ok()
                .and_then(|a| a.get("url").and_then(Value::as_str).map(str::to_string)))
        })
    }

    fn max_post_chars(&self) -> usize {
        super::config::MAX_POST_CHARS
    }

    /// Links aren't expanded into previews, which would bury a short reply
    /// under a PR's card. A thread reply that notifies starts with a
    /// mention of the owner, which is what makes Slack notify you of a
    /// reply in a thread you don't follow (a thread the relay started),
    /// without also sending it to the main DM.
    fn post<'a>(&'a self, m: super::chat::Outgoing<'a>) -> super::chat::Fut<'a, String> {
        Box::pin(async move {
            let mut params = json!({ "channel": self.channel() });
            let mut md = super::post::as_markdown(m.body, m.notice, m.buttons);
            if let Some(t) = m.thread {
                params["thread_ts"] = json!(t);
                let owner = self.slack.owner.borrow().clone().unwrap_or_default();
                if m.mention && !owner.is_empty() {
                    md = super::post::with_mention(md, &owner);
                }
            }
            params["text"] = md["text"].clone();
            params["blocks"] = md["blocks"].clone();
            params["unfurl_links"] = json!(false);
            params["unfurl_media"] = json!(false);
            let sent = self.slack.api("chat.postMessage", params).await?;
            Ok(ts_of(&sent))
        })
    }

    fn update<'a>(
        &'a self,
        ts: &'a str,
        body: &'a str,
        notice: &'a str,
        buttons: &'a [super::post::Button],
    ) -> super::chat::Fut<'a, ()> {
        Box::pin(async move {
            let md = super::post::as_markdown(body, notice, buttons);
            self.slack
                .api(
                    "chat.update",
                    json!({ "channel": self.channel(), "ts": ts, "text": md["text"], "blocks": md["blocks"] }),
                )
                .await
                .map(|_| ())
        })
    }

    fn delete<'a>(&'a self, ts: &'a str) -> super::chat::Fut<'a, ()> {
        Box::pin(async move {
            self.slack
                .api(
                    "chat.delete",
                    json!({ "channel": self.channel(), "ts": ts }),
                )
                .await
                .map(|_| ())
        })
    }

    fn react<'a>(&'a self, ts: &'a str, emoji: &'a str) -> super::chat::Fut<'a, ()> {
        Box::pin(async move {
            match self
                .slack
                .api(
                    "reactions.add",
                    json!({ "channel": self.channel(), "timestamp": ts, "name": emoji }),
                )
                .await
            {
                Err(e) if !e.to_string().contains("already_reacted") => Err(e),
                _ => Ok(()),
            }
        })
    }

    fn unreact<'a>(&'a self, ts: &'a str, emoji: &'a str) -> super::chat::Fut<'a, ()> {
        Box::pin(async move {
            match self
                .slack
                .api(
                    "reactions.remove",
                    json!({ "channel": self.channel(), "timestamp": ts, "name": emoji }),
                )
                .await
            {
                Err(e) if !e.to_string().contains("no_reaction") => Err(e),
                _ => Ok(()),
            }
        })
    }

    /// Each file's bytes go to an address Slack gives for it, then the
    /// files are shared together, `FILES_PER_MESSAGE` to a message.
    fn upload<'a>(
        &'a self,
        files: Vec<super::post::Upload>,
        thread: Option<&'a str>,
        failed: &'a dyn Fn(&str, &anyhow::Error),
    ) -> super::chat::Fut<'a, Vec<String>> {
        Box::pin(async move {
            let mut sent: Vec<(String, String, String)> = Vec::new();
            for super::post::Upload {
                filename,
                title,
                content,
            } in files
            {
                let uploaded: Result<String> = async {
                    let target = self
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
                    self.slack.put(url, content).await?;
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
            for batch in sent.chunks(super::post::FILES_PER_MESSAGE) {
                let files: Vec<Value> = batch
                    .iter()
                    .map(|(id, title, _)| json!({ "id": id, "title": title }))
                    .collect();
                let mut params = json!({ "files": files, "channel_id": self.channel() });
                if let Some(t) = thread {
                    params["thread_ts"] = json!(t);
                }
                self.slack
                    .api("files.completeUploadExternal", params)
                    .await?;
            }
            Ok(sent.into_iter().map(|(_, _, name)| name).collect())
        })
    }

    fn file_info<'a>(&'a self, id: &'a str) -> super::chat::Fut<'a, FileRef> {
        Box::pin(async move {
            let got = self.slack.api("files.info", json!({ "file": id })).await?;
            Ok(FileRef::from_json(got.get("file").unwrap_or(&Value::Null)))
        })
    }

    fn file_hosts(&self) -> &'static [&'static str] {
        &[super::files::FILE_HOST]
    }

    fn fetch<'a>(&'a self, url: &'a str) -> super::chat::Fut<'a, Vec<u8>> {
        Box::pin(self.slack.get(url))
    }

    fn delete_file<'a>(&'a self, id: &'a str) -> super::chat::Fut<'a, ()> {
        Box::pin(async move {
            self.slack
                .api("files.delete", json!({ "file": id }))
                .await
                .map(|_| ())
        })
    }

    /// Following Slack's pages.
    fn thread_messages<'a>(
        &'a self,
        ts: &'a str,
    ) -> super::chat::Fut<'a, Vec<super::chat::InThread>> {
        Box::pin(async move {
            let mut messages = Vec::new();
            let mut cursor: Option<String> = None;
            let bot = self.bot.borrow().clone();
            loop {
                let mut params = json!({ "channel": self.channel(), "ts": ts, "limit": 200 });
                if let Some(c) = &cursor {
                    params["cursor"] = json!(c);
                }
                let page = self.slack.api("conversations.replies", params).await?;
                for m in page
                    .get("messages")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    messages.push(super::chat::InThread {
                        ts: ts_of(m),
                        from_app: m.get("bot_id").is_some_and(crate::util::truthy)
                            || m.get("user").and_then(Value::as_str) == bot.as_deref(),
                        files: m
                            .get("files")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .map(|f| f.get("id").map(crate::util::js_string).unwrap_or_default())
                            .collect(),
                    });
                }
                cursor = page
                    .pointer("/response_metadata/next_cursor")
                    .and_then(Value::as_str)
                    .filter(|c| !c.is_empty())
                    .map(str::to_string);
                if cursor.is_none() {
                    return Ok(messages);
                }
            }
        })
    }

    /// The DM from the cursor, and each watched thread from its last
    /// handled reply; only your messages come back, by the same rules as
    /// events (`message_from`).
    fn catch_up<'a>(
        &'a self,
        dm: &'a super::state::Dm,
        cursor: Option<&'a str>,
        threads: &'a [(String, String)],
        log: &'a dyn Fn(&str),
    ) -> super::chat::Fut<'a, Vec<Message>> {
        use super::state::number;
        Box::pin(async move {
            let mine = |messages: &Value, thread: Option<&str>| -> Vec<Message> {
                messages
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|m| {
                        let mut event = m.clone();
                        event["type"] = json!("message");
                        event["channel"] = json!(dm.channel);
                        if event.get("thread_ts").is_none_or(Value::is_null)
                            && let Some(t) = thread
                        {
                            event["thread_ts"] = json!(t);
                        }
                        message_from(dm, &event, None, log)
                    })
                    .collect()
            };
            let mut params = json!({ "channel": dm.channel, "limit": super::read::PAGE });
            if let Some(c) = cursor {
                params["oldest"] = json!(c);
            }
            let top = self.slack.api("conversations.history", params).await?;
            let from = number(cursor.unwrap_or("0"));
            let mut messages: Vec<Message> =
                mine(top.get("messages").unwrap_or(&Value::Null), None)
                    .into_iter()
                    .filter(|m| m.thread_ts.is_none() && number(&m.ts) > from)
                    .collect();
            for (thread, handled_to) in threads {
                let replies = self
                    .slack
                    .api(
                        "conversations.replies",
                        json!({ "channel": dm.channel, "ts": thread, "oldest": handled_to, "limit": super::read::PAGE }),
                    )
                    .await
                    .unwrap_or_else(|_| json!({ "messages": [] }));
                messages.extend(
                    mine(
                        replies.get("messages").unwrap_or(&Value::Null),
                        Some(thread),
                    )
                    .into_iter()
                    .filter(|m| {
                        m.thread_ts.as_deref() == Some(thread.as_str())
                            && number(&m.ts) > number(handled_to)
                    }),
                );
            }
            Ok(messages)
        })
    }

    fn publish_home<'a>(&'a self, view: &'a Value) -> super::chat::Fut<'a, ()> {
        Box::pin(async move {
            let user = self.slack.owner.borrow().clone().unwrap_or_default();
            self.slack
                .api("views.publish", json!({ "user_id": user, "view": view }))
                .await
                .map(|_| ())
        })
    }

    fn open_view<'a>(
        &'a self,
        trigger: &'a str,
        view: &'a Value,
    ) -> super::chat::Fut<'a, Option<String>> {
        Box::pin(async move {
            let opened = self
                .slack
                .api("views.open", json!({ "trigger_id": trigger, "view": view }))
                .await?;
            Ok(opened
                .pointer("/view/id")
                .and_then(Value::as_str)
                .map(str::to_string))
        })
    }

    fn push_view<'a>(&'a self, trigger: &'a str, view: &'a Value) -> super::chat::Fut<'a, ()> {
        Box::pin(async move {
            self.slack
                .api("views.push", json!({ "trigger_id": trigger, "view": view }))
                .await
                .map(|_| ())
        })
    }

    /// The hash makes Slack refuse it if the view has changed since.
    fn update_view<'a>(
        &'a self,
        id: &'a str,
        view: &'a Value,
        hash: Option<&'a str>,
    ) -> super::chat::Fut<'a, ()> {
        Box::pin(async move {
            let mut params = json!({ "view_id": id, "view": view });
            if let Some(hash) = hash {
                params["hash"] = json!(hash);
            }
            self.slack.api("views.update", params).await.map(|_| ())
        })
    }

    fn schedule<'a>(&'a self, text: &'a str, post_at: i64) -> super::chat::Fut<'a, String> {
        Box::pin(async move {
            let md = super::post::as_markdown(text, &super::post::notice_line(text), &[]);
            let scheduled = self
                .slack
                .api(
                    "chat.scheduleMessage",
                    json!({
                        "channel": self.channel(),
                        "post_at": post_at,
                        "text": md["text"],
                        "blocks": md["blocks"],
                    }),
                )
                .await?;
            Ok(scheduled
                .get("scheduled_message_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string())
        })
    }

    fn scheduled(&self) -> super::chat::Fut<'_, Vec<String>> {
        Box::pin(async move {
            let reply = self
                .slack
                .api(
                    "chat.scheduledMessages.list",
                    json!({ "channel": self.channel() }),
                )
                .await?;
            Ok(reply
                .get("scheduled_messages")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_string))
                .collect())
        })
    }

    fn unschedule<'a>(&'a self, id: &'a str) -> super::chat::Fut<'a, ()> {
        Box::pin(async move {
            self.slack
                .api(
                    "chat.deleteScheduledMessage",
                    json!({ "channel": self.channel(), "scheduled_message_id": id }),
                )
                .await
                .map(|_| ())
        })
    }

    /// The way Slack builds a reply's link: the workspace's address, the
    /// DM, the message's ts without its dot, and the thread it is in.
    fn message_url(&self, dm: &super::state::Dm, thread: &str, at: &str) -> Option<String> {
        let url = dm.url.as_ref()?;
        Some(format!(
            "{url}archives/{}/p{}?thread_ts={thread}&cid={}",
            dm.channel,
            at.replace('.', ""),
            dm.channel
        ))
    }

    /// The app's Home tab, which needs the app's ID, which Slack only
    /// gives with an event.
    fn home_url(&self, dm: &super::state::Dm) -> Option<String> {
        dm.app_id.is_some().then(|| super::buttons::home_link(dm))
    }
}

// ── Hearing you ─────────────────────────────────────────────────────────────

/// Whether an event is yours to act on: sent by the owner, in the app's DM
/// with them, in the relay's workspace, and not by a bot (the relay's own
/// posts come back as events too). Anyone in the workspace can open a DM
/// with the app; whatever they send fails this and is only logged, with no
/// reply, so the app doesn't even say it is listening.
pub fn from_owner(dm: &super::state::Dm, event: &Value, team_id: Option<&str>) -> bool {
    let text = |k: &str| event.get(k).and_then(Value::as_str);
    !dm.user.is_empty()
        && text("user") == Some(dm.user.as_str())
        && !dm.channel.is_empty()
        && text("channel") == Some(dm.channel.as_str())
        && !present(event.get("bot_id"))
        && team_id.is_none_or(|t| Some(t) == dm.team.as_deref())
}

/// Whether you typed a message yourself, in a Slack app. Other apps can
/// post as you too (anything holding your user token, the Slack connector
/// your Claude sessions use included), and their posts carry your member ID
/// like your own, so passing for you is not enough: a message you typed
/// carries the `client_msg_id` Slack's own apps give it, which the Web API
/// doesn't let an app set, and no `app_id`. Without this, a Claude session
/// that read something malicious could post here as you, as a reply in a
/// tab's thread, and drive that tab.
pub fn typed_by_you(event: &Value) -> bool {
    present(event.get("client_msg_id"))
        && !present(event.get("app_id"))
        && !present(event.get("bot_profile"))
}

/// Whether a field is there and truthy, as JavaScript's `Boolean(x)`.
fn present(v: Option<&Value>) -> bool {
    v.is_some_and(crate::util::truthy)
}

/// A file you attached, as a message names it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileRef {
    pub id: String,
    pub name: Option<String>,
    pub mimetype: Option<String>,
    pub size: Option<u64>,
    pub url: Option<String>,
}

impl FileRef {
    pub fn from_json(f: &Value) -> FileRef {
        let text = |k: &str| f.get(k).and_then(Value::as_str).map(str::to_string);
        FileRef {
            id: text("id").unwrap_or_default(),
            name: text("name"),
            mimetype: text("mimetype"),
            size: f.get("size").and_then(Value::as_u64),
            url: text("url_private_download").or_else(|| text("url_private")),
        }
    }
}

/// A message as the dispatcher takes it: yours to act on, or the relay's
/// own (`by_claude`), seen so the cursor moves past it. A reply has its
/// thread's ts; a message at the top level has none. `fresh` marks one made
/// by a button or a form, never sent while the relay was away; `request` is
/// the message whose reaction says where it is, when it isn't this one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Message {
    pub ts: String,
    pub text: String,
    pub by_claude: bool,
    pub thread_ts: Option<String>,
    pub files: Vec<FileRef>,
    pub fresh: bool,
    pub request: Option<String>,
}

/// Message events that are something you wrote. Edits, deletions and joins
/// are other subtypes and are not acted on; a message with a file attached
/// is.
fn subtype_is_yours(event: &Value) -> bool {
    match event.get("subtype") {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s == "file_share" || s == "thread_broadcast",
        Some(_) => false,
    }
}

/// Slack escapes &, < and > in message text, and wraps links and mentions
/// in <…>; the text you typed is what goes into a tab.
pub fn plain_text(text: &str) -> String {
    static NAMED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"<((?:https?|mailto):[^|>]+)\|([^>]+)>").expect("a regex"));
    static BARE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"<((?:https?|mailto):[^>]+)>").expect("a regex"));
    static SCHEME: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(?:https?://|mailto:)").expect("a regex"));
    let named = NAMED.replace_all(text, |c: &regex::Captures| {
        let (url, shown) = (&c[1], &c[2]);
        if shown == SCHEME.replace(url, "") {
            shown.to_string()
        } else {
            url.to_string()
        }
    });
    BARE.replace_all(&named, "$1")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// A message event as the dispatcher takes it, or `None` when it isn't one
/// to act on.
pub fn message_from(
    dm: &super::state::Dm,
    event: &Value,
    team_id: Option<&str>,
    log: impl FnOnce(&str),
) -> Option<Message> {
    if event.get("type").and_then(Value::as_str) != Some("message") || !subtype_is_yours(event) {
        return None;
    }
    if !from_owner(dm, event, team_id) || !typed_by_you(event) {
        if !present(event.get("bot_id")) {
            let text = |k: &str| event.get(k).and_then(Value::as_str);
            log(&format!(
                "ignored a message from {} in {}{}: not typed by the relay's owner in its DM",
                text("user").unwrap_or("someone"),
                text("channel").unwrap_or("undefined"),
                text("app_id")
                    .map(|a| format!(" posted by app {a}"))
                    .unwrap_or_default()
            ));
        }
        return None;
    }
    let ts = event.get("ts").and_then(Value::as_str).unwrap_or_default();
    let thread = event
        .get("thread_ts")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty() && *t != ts)
        .map(str::to_string);
    let files = event
        .get("files")
        .and_then(Value::as_array)
        .map(|fs| fs.iter().map(FileRef::from_json).collect())
        .unwrap_or_default();
    Some(Message {
        ts: ts.to_string(),
        text: plain_text(event.get("text").and_then(Value::as_str).unwrap_or("")),
        by_claude: false,
        thread_ts: thread,
        files,
        fresh: false,
        request: None,
    })
}

/// The message a button was on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pressed {
    pub ts: String,
    pub thread_ts: Option<String>,
    pub text: String,
}

/// A screen a button was on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewRef {
    pub id: String,
    pub hash: Option<String>,
}

/// A button you pressed: what it means to the relay, the message it was on
/// (none on the Home tab or a screen), the screen, and the trigger Slack
/// gives for opening a screen in answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Press {
    pub value: Value,
    pub message: Option<Pressed>,
    pub view: Option<ViewRef>,
    pub trigger_id: Option<String>,
}

/// What the Home tab's buttons may do, and a tab screen's.
const HOME_ACTIONS: &[&str] = &[
    "open",
    "refresh",
    "new",
    "autopilot",
    "autopilot-pause",
    "autopilot-resume",
    "cleanup",
    "home-opt",
    "home-dialog",
];
const SCREEN_ACTIONS: &[&str] = &[
    "screen-message",
    "screen-skill",
    "screen-opt",
    "screen-dialog",
    "screen-progress",
    "screen-rename",
    "screen-stop",
    "screen-close",
];

/// A button you pressed, or `None` when it isn't one to act on. Only
/// Slack's own apps can press a button, on a person's tap: no app can press
/// one as you, so a press carries no doubt about who made it. It still has
/// to be yours, on a message in the relay's DM, in your workspace.
pub fn press_from(dm: &super::state::Dm, payload: &Value, log: impl FnOnce(&str)) -> Option<Press> {
    let text = |p: &str| payload.pointer(p).and_then(Value::as_str);
    if text("/type") != Some("block_actions") {
        return None;
    }
    let action = payload.pointer("/actions/0");
    let action_text = |k: &str| action.and_then(|a| a.get(k)).and_then(Value::as_str);
    // A link button (the Home tab's "Thread") only opens its link; Slack
    // tells the app it was pressed, and there is nothing to do.
    if action
        .and_then(|a| a.get("url"))
        .is_some_and(crate::util::truthy)
    {
        return None;
    }
    let channel = text("/container/channel_id").or_else(|| text("/channel/id"));
    let team = text("/team/id").or_else(|| text("/user/team_id"));
    let view_type = text("/view/type");
    // The Home tab is the owner's alone (it is only published to them), and
    // so is a tab's screen, which only opens from it.
    let from_view =
        text("/container/type") == Some("view") && matches!(view_type, Some("home" | "modal"));
    // Picking a skill in the Skill form's menu, which has no value of its
    // own: the form is drawn again to say what that skill takes.
    let chosen = from_view
        && view_type == Some("modal")
        && text("/view/callback_id") == Some("skill")
        && action_text("action_id") == Some("skill")
        && action_text("type") == Some("static_select");
    let user = text("/user/id");
    let value_text = action_text("value").filter(|v| !v.is_empty());
    // The first check that fails, named in the log, since a refused press of
    // your own is otherwise a mystery.
    let refused = if dm.user.is_empty() || user != Some(dm.user.as_str()) {
        Some("not the relay's owner".to_string())
    } else if !from_view && (dm.channel.is_empty() || channel != Some(dm.channel.as_str())) {
        Some(format!(
            "on {} in {}, not the DM",
            text("/container/type").unwrap_or("something"),
            channel.unwrap_or("no conversation")
        ))
    } else if team.is_some_and(|t| Some(t) != dm.team.as_deref()) {
        Some(format!("from workspace {}", team.unwrap_or("")))
    } else if value_text.is_none() && !chosen {
        Some(format!(
            "a {} with no value",
            action_text("type").unwrap_or("control")
        ))
    } else {
        None
    };
    if let Some(why) = refused {
        log(&format!(
            "ignored a button press from {}: {why}",
            user.unwrap_or("someone")
        ));
        return None;
    }
    let trigger_id = text("/trigger_id").map(str::to_string);
    if chosen {
        let meta: Value = serde_json::from_str(text("/view/private_metadata")?).ok()?;
        let value = json!({
            "a": "skill-chosen",
            "w": meta.get("w"),
            "p": meta.get("p"),
            "from": meta.get("from"),
            "skill": action
                .and_then(|a| a.pointer("/selected_option/value"))
                .map(crate::util::js_string)
                .unwrap_or_default(),
            "args": payload
                .pointer("/view/state/values/args/args/value")
                .map(crate::util::js_string)
                .unwrap_or_default(),
        });
        return Some(Press {
            value,
            message: None,
            view: Some(ViewRef {
                id: text("/view/id").unwrap_or_default().to_string(),
                hash: text("/view/hash").map(str::to_string),
            }),
            trigger_id,
        });
    }
    let value: Value = serde_json::from_str(value_text?).ok()?;
    let a = value.get("a").and_then(Value::as_str).unwrap_or_default();
    if from_view {
        let allowed = if view_type == Some("home") {
            HOME_ACTIONS
        } else {
            SCREEN_ACTIONS
        };
        if !allowed.contains(&a) {
            return None;
        }
        return Some(Press {
            value,
            message: None,
            view: Some(ViewRef {
                id: text("/view/id").unwrap_or_default().to_string(),
                hash: None,
            }),
            trigger_id,
        });
    }
    Some(Press {
        value,
        trigger_id,
        view: None,
        message: Some(Pressed {
            ts: text("/message/ts")
                .or_else(|| text("/container/message_ts"))
                .unwrap_or_default()
                .to_string(),
            thread_ts: text("/message/thread_ts").map(str::to_string),
            // The message's Markdown, which an answer's edit writes back
            // with a line added; its `text` is only the notification's.
            text: payload
                .pointer("/message/blocks")
                .and_then(Value::as_array)
                .and_then(|blocks| blocks.iter().find(|b| b["type"] == "markdown"))
                .and_then(|b| b["text"].as_str())
                .or_else(|| text("/message/text"))
                .unwrap_or_default()
                .to_string(),
        }),
    })
}

/// A form sent from one of the relay's screens: Rename's Save, Message's
/// Send, Skill's Run, or New tab's Start, with the tab it was opened for
/// (`meta`), what was typed in each field, and the tab's screen under it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Form {
    pub kind: String,
    pub meta: Value,
    pub values: HashMap<String, String>,
    pub root_view: Option<String>,
}

impl Form {
    pub fn value(&self, field: &str) -> &str {
        self.values.get(field).map(String::as_str).unwrap_or("")
    }
}

/// Each form's fields.
fn form_fields(kind: &str) -> Option<&'static [&'static str]> {
    Some(match kind {
        "rename" => &["name"],
        "message" => &["text"],
        "skill" => &["skill", "typed", "args"],
        "new" => &["place", "typed", "prompt"],
        "autopilot" => &[
            "note",
            "note_typed",
            "target",
            "target_typed",
            "slots",
            "limit",
            "switches",
        ],
        _ => return None,
    })
}

/// A form, from the owner only, or `None`.
pub fn submission_from(
    dm: &super::state::Dm,
    payload: &Value,
    log: impl FnOnce(&str),
) -> Option<Form> {
    let text = |p: &str| payload.pointer(p).and_then(Value::as_str);
    let kind = text("/view/callback_id")?;
    let fields = form_fields(kind)?;
    if text("/type") != Some("view_submission") {
        return None;
    }
    let team = text("/team/id").or_else(|| text("/user/team_id"));
    let user = text("/user/id");
    if dm.user.is_empty()
        || user != Some(dm.user.as_str())
        || team.is_some_and(|t| Some(t) != dm.team.as_deref())
    {
        log(&format!(
            "ignored a form from {}: not the relay's owner",
            user.unwrap_or("someone")
        ));
        return None;
    }
    let meta = if kind == "new" {
        json!({})
    } else {
        serde_json::from_str(text("/view/private_metadata")?).ok()?
    };
    // A text field's value, a menu's chosen option, or checkboxes' ticked
    // options, joined by commas.
    let values = fields
        .iter()
        .map(|field| {
            let input = payload.pointer(&format!("/view/state/values/{field}/{field}"));
            let value = input
                .and_then(|i| i.get("value").filter(|v| !v.is_null()))
                .or_else(|| input.and_then(|i| i.pointer("/selected_option/value")))
                .map(crate::util::js_string)
                .or_else(|| {
                    let ticked = input?.get("selected_options")?.as_array()?;
                    Some(
                        ticked
                            .iter()
                            .filter_map(|o| o.get("value").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join(","),
                    )
                })
                .unwrap_or_default();
            (field.to_string(), value)
        })
        .collect();
    Some(Form {
        kind: kind.to_string(),
        meta,
        values,
        root_view: text("/view/root_view_id")
            .or_else(|| text("/view/previous_view_id"))
            .map(str::to_string),
    })
}

// ── Socket Mode ─────────────────────────────────────────────────────────────

/// Waits after each failed connection in a row.
pub const RECONNECT_DELAYS_MS: [u64; 5] = [1_000, 2_000, 5_000, 10_000, 30_000];

/// What an envelope from Socket Mode asks of the relay. Each one carrying
/// an `envelope_id` is acknowledged before anything else, or Slack sends it
/// again after three seconds.
#[derive(Clone, Debug, PartialEq)]
pub enum Envelope {
    /// Connected: catch up on what was sent while the socket was down.
    Hello,
    /// Slack asks the app to reconnect.
    Disconnect,
    /// An event, with its workspace and app.
    Event {
        event: Value,
        team: Option<String>,
        app: Option<String>,
    },
    /// A button pressed or a form sent.
    Interactive(Value),
    Other,
}

/// An envelope's acknowledgement, if it needs one, and what it asks.
pub fn envelope(text: &str) -> Option<(Option<String>, Envelope)> {
    let v: Value = serde_json::from_str(text).ok()?;
    let ack = v
        .get("envelope_id")
        .and_then(Value::as_str)
        .map(|id| crate::util::json_compact(&json!({ "envelope_id": id })));
    let kind = v.get("type").and_then(Value::as_str).unwrap_or_default();
    let what = match kind {
        "hello" => Envelope::Hello,
        "disconnect" => Envelope::Disconnect,
        "events_api" => match v.pointer("/payload/event") {
            Some(event) if !event.is_null() => Envelope::Event {
                event: event.clone(),
                team: v
                    .pointer("/payload/team_id")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                app: v
                    .pointer("/payload/api_app_id")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            },
            _ => Envelope::Other,
        },
        "interactive" => Envelope::Interactive(v.get("payload").cloned().unwrap_or(Value::Null)),
        _ => Envelope::Other,
    };
    Some((ack, what))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::state::Dm;

    fn dm() -> Dm {
        Dm {
            channel: "D1".into(),
            user: "U1".into(),
            team: Some("T1".into()),
            ..Dm::default()
        }
    }

    #[test]
    fn only_the_owners_messages_in_the_apps_dm_and_workspace_are_acted_on() {
        let typed = json!({ "type": "message", "user": "U1", "channel": "D1", "ts": "1.1", "text": "hi", "client_msg_id": "c1" });
        let mut logged = Vec::new();
        let mut take = |e: &Value, team: Option<&str>| {
            message_from(&dm(), e, team, |l| logged.push(l.to_string()))
        };
        assert_eq!(take(&typed, Some("T1")).unwrap().text, "hi");
        assert!(
            take(&typed, None).is_some(),
            "an event with no workspace given"
        );
        let with = |k: &str, v: Value| {
            let mut e = typed.clone();
            e[k] = v;
            e
        };
        assert!(
            take(&with("user", json!("U2")), Some("T1")).is_none(),
            "someone else"
        );
        assert!(
            take(&with("channel", json!("C9")), Some("T1")).is_none(),
            "another conversation"
        );
        assert!(take(&typed, Some("T2")).is_none(), "another workspace");
        assert!(
            take(&with("bot_id", json!("B1")), Some("T1")).is_none(),
            "a bot"
        );
        // Posted as you by another app (the Slack connector, say): no
        // client_msg_id, or an app_id.
        let mut by_app = typed.clone();
        by_app.as_object_mut().unwrap().remove("client_msg_id");
        assert!(take(&by_app, Some("T1")).is_none());
        assert!(take(&with("app_id", json!("A9")), Some("T1")).is_none());
        assert!(take(&with("bot_profile", json!({"id": "x"})), Some("T1")).is_none());
        assert!(
            take(&with("subtype", json!("message_changed")), Some("T1")).is_none(),
            "an edit"
        );
        assert!(take(&with("subtype", json!("file_share")), Some("T1")).is_some());
        let reply = take(&with("thread_ts", json!("0.5")), Some("T1")).unwrap();
        assert_eq!(reply.thread_ts.as_deref(), Some("0.5"));
        assert!(logged.iter().any(|l| l.contains("posted by app A9")));
        // An owner never comes from a message: a DM with no owner known
        // takes nothing.
        assert!(message_from(&Dm::default(), &typed, None, |_| {}).is_none());
    }

    #[test]
    fn the_text_you_typed_is_what_reaches_a_tab() {
        assert_eq!(
            plain_text(
                "see <https://example.com|example.com> and <https://a.b/c|the docs>, &lt;b&gt; &amp; <mailto:x@y.z>"
            ),
            "see example.com and https://a.b/c, <b> & mailto:x@y.z"
        );
    }

    #[test]
    fn calls_naming_another_conversation_or_person_are_refused() {
        let ok = |m: &str, p: Value| refuse(m, &p, Some("D1"), Some("U1")).is_ok();
        assert!(ok(
            "chat.postMessage",
            json!({ "channel": "D1", "text": "x" })
        ));
        assert!(ok("auth.test", json!({})));
        assert!(!ok("chat.postMessage", json!({ "channel": "C2" })));
        assert!(!ok(
            "files.completeUploadExternal",
            json!({ "channel_id": "C2" })
        ));
        assert!(!ok("views.publish", json!({ "user_id": "U2" })));
        assert!(ok("views.publish", json!({ "user_id": "U1" })));
        assert!(!ok("conversations.open", json!({ "users": "U2" })));
        // Before the DM is known, nothing names a conversation.
        assert!(refuse("chat.postMessage", &json!({ "channel": "D1" }), None, None).is_err());
    }

    #[test]
    fn only_your_presses_on_the_relays_dm_count_and_a_link_button_is_left_to_slack() {
        let press = |user: &str, channel: &str, value: &str| {
            json!({
                "type": "block_actions", "user": { "id": user, "team_id": "T1" },
                "container": { "type": "message", "channel_id": channel, "message_ts": "5.5" },
                "message": { "ts": "5.5", "text": "hi" },
                "actions": [{ "type": "button", "value": value }],
                "trigger_id": "tr"
            })
        };
        let v = r#"{"a":"cmd","w":"@1","c":"progress"}"#;
        let got = press_from(&dm(), &press("U1", "D1", v), |_| {}).unwrap();
        assert_eq!(got.value["c"], "progress");
        let pressed = got.message.unwrap();
        assert_eq!(pressed.ts, "5.5");
        assert_eq!(pressed.text, "hi", "with no Markdown block, its text");
        // Its body is the Markdown block's, not the notification's line.
        let mut posted = press("U1", "D1", v);
        posted["message"]["text"] = json!("work: waiting on you");
        posted["message"]["blocks"] = json!([
            { "type": "markdown", "text": "🤖 **Tab 1, work** is waiting.\n\n> go on?" },
            { "type": "actions", "elements": [] }
        ]);
        let got = press_from(&dm(), &posted, |_| {}).unwrap();
        assert_eq!(
            got.message.unwrap().text,
            "🤖 **Tab 1, work** is waiting.\n\n> go on?"
        );
        let mut why = String::new();
        assert!(press_from(&dm(), &press("U2", "D1", v), |l| why = l.to_string()).is_none());
        assert!(why.contains("not the relay's owner"));
        assert!(press_from(&dm(), &press("U1", "C9", v), |l| why = l.to_string()).is_none());
        assert!(why.contains("not the DM"));
        let mut link = press("U1", "D1", v);
        link["actions"][0]["url"] = json!("https://x");
        assert!(press_from(&dm(), &link, |_| {}).is_none());
        let mut elsewhere = press("U1", "D1", v);
        elsewhere["team"] = json!({ "id": "T9" });
        assert!(press_from(&dm(), &elsewhere, |_| {}).is_none());
        // A view's buttons are only its own.
        let home = |a: &str| {
            json!({
                "type": "block_actions", "user": { "id": "U1" },
                "container": { "type": "view" }, "view": { "id": "V1", "type": "home" },
                "actions": [{ "type": "button", "value": format!(r#"{{"a":"{a}"}}"#) }]
            })
        };
        assert!(press_from(&dm(), &home("refresh"), |_| {}).is_some());
        assert!(press_from(&dm(), &home("close"), |_| {}).is_none());
    }

    #[test]
    fn forms_come_only_from_the_owner_and_the_relays_screens() {
        let form = |user: &str, callback: &str| {
            json!({
                "type": "view_submission", "user": { "id": user },
                "view": {
                    "callback_id": callback, "root_view_id": "V0",
                    "private_metadata": r#"{"w":"@1","p":"10"}"#,
                    "state": { "values": { "name": { "name": { "value": "fresh" } } } }
                }
            })
        };
        let got = submission_from(&dm(), &form("U1", "rename"), |_| {}).unwrap();
        assert_eq!(got.value("name"), "fresh");
        assert_eq!(got.meta["w"], "@1");
        assert_eq!(got.root_view.as_deref(), Some("V0"));
        assert!(submission_from(&dm(), &form("U2", "rename"), |_| {}).is_none());
        assert!(submission_from(&dm(), &form("U1", "something"), |_| {}).is_none());
    }

    #[test]
    fn envelopes_are_acknowledged_and_read() {
        let (ack, what) = envelope(
            r#"{"envelope_id":"e1","type":"events_api","payload":{"team_id":"T1","api_app_id":"A1","event":{"type":"message"}}}"#,
        )
        .unwrap();
        assert_eq!(ack.as_deref(), Some(r#"{"envelope_id":"e1"}"#));
        assert!(matches!(what, Envelope::Event { team: Some(t), .. } if t == "T1"));
        assert_eq!(
            envelope(r#"{"type":"hello"}"#).unwrap(),
            (None, Envelope::Hello)
        );
        assert_eq!(
            envelope(r#"{"type":"disconnect"}"#).unwrap().1,
            Envelope::Disconnect
        );
        assert!(envelope("not json").is_none());
    }
}
