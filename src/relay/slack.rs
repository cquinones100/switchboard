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
            text: text("/message/text").unwrap_or_default().to_string(),
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
    // A text field's value, or a menu's chosen option.
    let values = fields
        .iter()
        .map(|field| {
            let input = payload.pointer(&format!("/view/state/values/{field}/{field}"));
            let value = input
                .and_then(|i| i.get("value").filter(|v| !v.is_null()))
                .or_else(|| input.and_then(|i| i.pointer("/selected_option/value")))
                .map(crate::util::js_string)
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
        assert_eq!(got.message.unwrap().ts, "5.5");
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
