//! The relay through a Discord bot: the relay talks to you in the bot's DM
//! with you, through Discord's REST API, and hears you through the
//! Gateway, a WebSocket the relay opens to Discord, so nothing on this
//! computer listens for connections. One token, the bot's, stored as a
//! secret and never in a file (`switchboard platform secret-set
//! discord-relay-bot-token -a "$USER"`, which reads it from stdin).
//!
//! The rules that keep the relay from being anyone else's remote control
//! are all here, as `slack.rs` holds Slack's, and every Discord call goes
//! through them:
//!
//! - `Discord::api` refuses, before anything reaches Discord, any call
//!   that names a channel other than the DM, opens a DM with anyone but the
//!   owner, answers an interaction Discord didn't send the relay, or is
//!   any route the relay has no use for.
//! - `message_from` acts on a message only from the owner (`author.id`),
//!   in the DM (channel type 1, the DM's ID, no `guild_id`), typed by a
//!   person: no bot, no webhook, no application.
//! - `press_from` takes a button or menu only from the owner, in the DM,
//!   on one of the relay's own messages.
//! - `submission_from` takes a form only from the owner, in the DM.
//!
//! The owner comes from `discord-relay.json`, never from a message, so
//! nothing anyone sends can make them the owner. Reactions control nothing.
//!
//! The rest is how the relay's Block Kit looks in Discord (`DiscordChat`,
//! in `discord_chat.rs`, uses it): Slack's mrkdwn as Discord's Markdown, a
//! post's buttons as action rows, a tab's screen as a message with its
//! buttons, a form as a modal (or, when it is more than a modal holds, a
//! message of menus), and the Home tab as a pinned message whose menu
//! opens a tab's screen.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::LazyLock;

use anyhow::{Result, bail};
use futures_util::future::LocalBoxFuture;
use regex::Regex;
use serde_json::{Value, json};

use super::config::Paths;
use super::state::Dm;

pub const BOT_TOKEN_SERVICE: &str = "discord-relay-bot-token";

/// Whose relay this is, outside this repository, since it names a person.
pub const CONFIG_NAME: &str = ".config/switchboard/discord-relay.json";

/// Discord's REST API, or `SWITCHBOARD_DISCORD_API`, which the tests point
/// at a server of their own.
pub fn api_base() -> String {
    crate::net::base_url("SWITCHBOARD_DISCORD_API", "https://discord.com/api/v10")
}

/// The Gateway's address, when `SWITCHBOARD_DISCORD_GATEWAY` names one
/// rather than Discord's `/gateway/bot` giving it.
pub fn gateway_override() -> Option<String> {
    std::env::var("SWITCHBOARD_DISCORD_GATEWAY")
        .ok()
        .filter(|g| !g.trim().is_empty())
}

/// Where a link to a message in the DM starts.
pub const DM_URL: &str = "https://discord.com/channels/@me/";

/// The hosts an attachment you send is fetched from.
pub const FILE_HOSTS: &[&str] = &["cdn.discordapp.com", "media.discordapp.net"];

/// The most one message holds, kept under Discord's 2,000 so a code block
/// closed at a split still fits.
pub const MAX_POST_CHARS: usize = 1_900;

/// The most a bot uploads in one message.
pub const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;

/// Every intent the relay asks for: messages in DMs (the bot reads their
/// words without the Message Content intent, which DMs don't need).
pub const INTENTS: u64 = 1 << 12;

/// A message flag: no link previews, as Slack's `unfurl_links: false`.
pub const SUPPRESS_EMBEDS: u64 = 1 << 2;
/// A message flag: posted without a notification.
pub const SUPPRESS_NOTIFICATIONS: u64 = 1 << 12;
/// A message flag: only you see it, for a press's answer.
pub const EPHEMERAL: u64 = 1 << 6;

/// Your Discord user ID, `{ "owner": "<id>" }` in `discord-relay.json`.
pub fn load_owner(paths: &Paths) -> Option<String> {
    let config = crate::util::read_json(paths.home.join(CONFIG_NAME))?;
    let owner = config.get("owner")?.as_str()?;
    is_snowflake(owner).then(|| owner.to_string())
}

/// A Discord ID: a number of 17 to 20 digits.
pub fn is_snowflake(id: &str) -> bool {
    (17..=20).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit())
}

/// When Discord's IDs start: 2015's first millisecond.
pub const EPOCH_MS: u64 = 1_420_070_400_000;

/// The time an ID was made, in ms.
pub fn snowflake_ms(id: &str) -> Option<u64> {
    let n: u64 = id.parse().ok()?;
    Some((n >> 22) + EPOCH_MS)
}

/// The smallest ID made at `ms`, for reading the DM after a time.
pub fn snowflake_at(ms: u64) -> String {
    (ms.saturating_sub(EPOCH_MS) << 22).to_string()
}

// ── The REST API and its rules ──────────────────────────────────────────────

/// How the relay reaches Discord: a REST call, a message posted with files,
/// and an attachment fetched. The relay's own is `http::DiscordHttps`; the
/// tests put stand-ins here, so they never reach Discord.
pub trait Rest {
    fn call<'a>(
        &'a self,
        method: &'a str,
        path: &'a str,
        body: Option<&'a Value>,
    ) -> LocalBoxFuture<'a, Result<Value>>;
    fn upload<'a>(
        &'a self,
        path: &'a str,
        payload: &'a Value,
        files: Vec<(String, Vec<u8>)>,
    ) -> LocalBoxFuture<'a, Result<Value>>;
    fn get<'a>(&'a self, url: &'a str) -> LocalBoxFuture<'a, Result<Vec<u8>>>;
}

/// The REST API as the relay may use it: the DM, once known, the owner, and
/// the interactions Discord sent it.
pub struct Discord {
    pub rest: Box<dyn Rest>,
    pub channel: RefCell<Option<String>>,
    pub owner: RefCell<Option<String>>,
    /// The interactions Discord sent, the only ones the relay may answer.
    pub interactions: RefCell<HashSet<String>>,
}

impl Discord {
    pub fn new(rest: Box<dyn Rest>) -> Discord {
        Discord {
            rest,
            channel: RefCell::new(None),
            owner: RefCell::new(None),
            interactions: RefCell::new(HashSet::new()),
        }
    }

    /// One REST call as the bot, refused here, before it reaches Discord,
    /// unless it is one the relay may make.
    pub async fn api(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        self.check(method, path, body.as_ref())?;
        self.rest.call(method, path, body.as_ref()).await
    }

    /// A message with files, posted in the DM alone.
    pub async fn upload(
        &self,
        path: &str,
        payload: Value,
        files: Vec<(String, Vec<u8>)>,
    ) -> Result<Value> {
        self.check("POST", path, Some(&payload))?;
        self.rest.upload(path, &payload, files).await
    }

    pub async fn get(&self, url: &str) -> Result<Vec<u8>> {
        self.rest.get(url).await
    }

    fn check(&self, method: &str, path: &str, body: Option<&Value>) -> Result<()> {
        refuse(
            method,
            path,
            body,
            self.channel.borrow().as_deref(),
            self.owner.borrow().as_deref(),
            &self.interactions.borrow(),
        )
    }
}

/// Why a call may not go out, or nothing. Only these go: the bot's own user
/// and the Gateway's address; opening the DM with the owner; anything in
/// the DM's channel; and answering an interaction Discord sent the relay.
pub fn refuse(
    method: &str,
    path: &str,
    body: Option<&Value>,
    dm: Option<&str>,
    owner: Option<&str>,
    interactions: &HashSet<String>,
) -> Result<()> {
    let route = path.split('?').next().unwrap_or(path);
    let parts: Vec<&str> = route.trim_start_matches('/').split('/').collect();
    match parts.as_slice() {
        ["users", "@me"] | ["gateway", "bot"] if method == "GET" => Ok(()),
        ["users", "@me", "channels"] if method == "POST" => {
            let to = body
                .and_then(|b| b.get("recipient_id"))
                .and_then(Value::as_str);
            if to.is_some() && to == owner {
                Ok(())
            } else {
                bail!("{method} {route}: the relay only opens its DM with its owner")
            }
        }
        ["channels", channel, ..] => {
            if Some(*channel) == dm {
                Ok(())
            } else {
                bail!("{method} {route}: the relay only uses its DM, not {channel}")
            }
        }
        ["interactions", id, _token, "callback"] if method == "POST" => {
            if interactions.contains(*id) {
                Ok(())
            } else {
                bail!("{method} /interactions/{id}: not an interaction Discord sent the relay")
            }
        }
        _ => bail!("{method} {route}: not a call the relay makes"),
    }
}

/// The bot's DM with the owner, which Discord gives again if it is open.
pub async fn open_dm(discord: &Discord, owner: &str) -> Result<Dm> {
    *discord.owner.borrow_mut() = Some(owner.to_string());
    let opened = discord
        .api(
            "POST",
            "/users/@me/channels",
            Some(json!({ "recipient_id": owner })),
        )
        .await?;
    let me = discord.api("GET", "/users/@me", None).await?;
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    if opened.get("type").and_then(Value::as_u64) != Some(1) {
        bail!("Discord didn't open a DM with the owner");
    }
    Ok(Dm {
        channel: text(&opened, "id").unwrap_or_default(),
        user: owner.to_string(),
        team: None,
        bot: text(&me, "id"),
        url: Some(DM_URL.to_string()),
        app: true,
        app_id: None,
    })
}

// ── Hearing you ─────────────────────────────────────────────────────────────

fn text_at<'a>(v: &'a Value, pointer: &str) -> Option<&'a str> {
    v.pointer(pointer).and_then(Value::as_str)
}

fn present(v: Option<&Value>) -> bool {
    v.is_some_and(crate::util::truthy)
}

/// Whether a message is yours to act on: typed by the owner, in the bot's
/// DM with them, by a person. Anyone who shares a server with the bot can
/// DM it; whatever they send fails this and is only logged, with no reply.
/// A bot, a webhook or an application posting as anyone is never acted on,
/// and the relay's own messages come back as events too.
pub fn from_owner(dm: &Dm, message: &Value) -> bool {
    !dm.user.is_empty()
        && text_at(message, "/author/id") == Some(dm.user.as_str())
        && !dm.channel.is_empty()
        && text_at(message, "/channel_id") == Some(dm.channel.as_str())
        && message
            .get("channel_type")
            .and_then(Value::as_u64)
            .is_none_or(|t| t == 1)
        && !present(message.get("guild_id"))
        && !present(message.get("webhook_id"))
        && !present(message.get("application_id"))
        && !present(message.get("interaction_metadata"))
        && !present(message.pointer("/author/bot"))
        && matches!(message.get("type").and_then(Value::as_u64), Some(0 | 19))
}

/// A message you sent, as the dispatcher takes it: its ID, its words, the
/// message it replies to, and its attachments. `None`, logged unless it is
/// a bot's, when it isn't yours.
pub fn message_from(
    dm: &Dm,
    message: &Value,
    log: impl FnOnce(&str),
) -> Option<(super::slack::Message, Option<String>)> {
    if !from_owner(dm, message) {
        if !present(message.pointer("/author/bot")) {
            log(&format!(
                "ignored a Discord message from {} in {}: not typed by the relay's owner in its DM",
                text_at(message, "/author/id").unwrap_or("someone"),
                text_at(message, "/channel_id").unwrap_or("undefined"),
            ));
        }
        return None;
    }
    let files = message
        .get("attachments")
        .and_then(Value::as_array)
        .map(|fs| {
            fs.iter()
                .map(|f| {
                    let text = |k: &str| f.get(k).and_then(Value::as_str).map(str::to_string);
                    super::slack::FileRef {
                        id: text("id").unwrap_or_default(),
                        name: text("filename"),
                        mimetype: text("content_type")
                            .map(|c| c.split(';').next().unwrap_or("").trim().to_string()),
                        size: f.get("size").and_then(Value::as_u64),
                        url: text("url"),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let replied = text_at(message, "/message_reference/message_id").map(str::to_string);
    Some((
        super::slack::Message {
            ts: text_at(message, "/id").unwrap_or_default().to_string(),
            text: text_at(message, "/content").unwrap_or_default().to_string(),
            files,
            ..Default::default()
        },
        replied,
    ))
}

/// A press of a button or a choice in a menu, on one of the relay's own
/// messages: what it carries, and the message it was on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Component {
    pub interaction: String,
    pub custom_id: String,
    pub values: Vec<String>,
    pub message: String,
    pub content: String,
    pub replied: Option<String>,
}

/// Whether an interaction is the owner's, in the DM, and why not.
fn refused_interaction(dm: &Dm, interaction: &Value) -> Option<String> {
    let user = text_at(interaction, "/user/id");
    let channel =
        text_at(interaction, "/channel_id").or_else(|| text_at(interaction, "/channel/id"));
    if dm.user.is_empty() || user != Some(dm.user.as_str()) {
        Some("not the relay's owner".into())
    } else if present(interaction.get("guild_id")) || present(interaction.get("member")) {
        Some("in a server, not the DM".into())
    } else if dm.channel.is_empty() || channel != Some(dm.channel.as_str()) {
        Some(format!(
            "in {}, not the DM",
            channel.unwrap_or("no channel")
        ))
    } else {
        None
    }
}

/// A button or menu you pressed, or `None` when it isn't one to act on:
/// the owner's, in the DM, on a message the relay's bot posted.
pub fn press_from(dm: &Dm, interaction: &Value, log: impl FnOnce(&str)) -> Option<Component> {
    if interaction.get("type").and_then(Value::as_u64) != Some(3) {
        return None;
    }
    let refused = refused_interaction(dm, interaction).or_else(|| {
        let author = text_at(interaction, "/message/author/id");
        (dm.bot.is_none() || author != dm.bot.as_deref())
            .then(|| "on a message the relay didn't post".to_string())
    });
    if let Some(why) = refused {
        log(&format!(
            "ignored a Discord press from {}: {why}",
            text_at(interaction, "/user/id")
                .or_else(|| text_at(interaction, "/member/user/id"))
                .unwrap_or("someone")
        ));
        return None;
    }
    Some(Component {
        interaction: text_at(interaction, "/id")?.to_string(),
        custom_id: text_at(interaction, "/data/custom_id")?.to_string(),
        values: interaction
            .pointer("/data/values")
            .and_then(Value::as_array)
            .map(|v| {
                v.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        message: text_at(interaction, "/message/id")
            .unwrap_or_default()
            .to_string(),
        content: text_at(interaction, "/message/content")
            .unwrap_or_default()
            .to_string(),
        replied: text_at(interaction, "/message/message_reference/message_id").map(str::to_string),
    })
}

/// A form you sent: its interaction, the form it was, and each field's
/// value (a menu's choices joined by commas).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Submission {
    pub interaction: String,
    pub custom_id: String,
    pub values: Vec<(String, String)>,
}

/// A form, from the owner in the DM only, or `None`.
pub fn submission_from(dm: &Dm, interaction: &Value, log: impl FnOnce(&str)) -> Option<Submission> {
    if interaction.get("type").and_then(Value::as_u64) != Some(5) {
        return None;
    }
    if let Some(why) = refused_interaction(dm, interaction) {
        log(&format!(
            "ignored a Discord form from {}: {why}",
            text_at(interaction, "/user/id").unwrap_or("someone")
        ));
        return None;
    }
    let mut values = Vec::new();
    fn walk(v: &Value, out: &mut Vec<(String, String)>) {
        match v {
            Value::Array(items) => items.iter().for_each(|i| walk(i, out)),
            Value::Object(map) => {
                if let Some(id) = map.get("custom_id").and_then(Value::as_str) {
                    if let Some(value) = map.get("value").and_then(Value::as_str) {
                        out.push((id.to_string(), value.to_string()));
                        return;
                    }
                    if let Some(chosen) = map.get("values").and_then(Value::as_array) {
                        let joined: Vec<&str> = chosen.iter().filter_map(Value::as_str).collect();
                        out.push((id.to_string(), joined.join(",")));
                        return;
                    }
                }
                for key in ["components", "component"] {
                    if let Some(inner) = map.get(key) {
                        walk(inner, out);
                    }
                }
            }
            _ => {}
        }
    }
    walk(
        interaction
            .pointer("/data/components")
            .unwrap_or(&Value::Null),
        &mut values,
    );
    Some(Submission {
        interaction: text_at(interaction, "/id")?.to_string(),
        custom_id: text_at(interaction, "/data/custom_id")?.to_string(),
        values,
    })
}

// ── How the relay's messages look in Discord ────────────────────────────────

/// Slack's names for the reactions the relay uses, as the emoji Discord
/// takes.
pub fn emoji(name: &str) -> &str {
    match name {
        "eyes" => "👀",
        "hourglass_flowing_sand" => "⏳",
        "white_check_mark" => "✅",
        "x" => "❌",
        "warning" => "⚠️",
        other => other,
    }
}

/// Slack's older mrkdwn (sections and context) as Discord's Markdown:
/// `*bold*`, `<url|label>` links, `<!date^…>` times, and its escapes.
pub fn markdown_of(mrkdwn: &str) -> String {
    static DATE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"<!date\^(\d+)\^[^|>]*\|[^>]*>").expect("a regex"));
    static NAMED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"<((?:https?|mailto|slack):[^|>]+)\|([^>]+)>").expect("a regex")
    });
    static BARE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"<((?:https?|mailto):[^>]+)>").expect("a regex"));
    static BOLD: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(^|[\s(\[_~>•])\*([^*\n]+)\*").expect("a regex"));
    let text = DATE.replace_all(mrkdwn, "<t:$1:R>");
    let text = NAMED.replace_all(&text, "[$2]($1)");
    let text = BARE.replace_all(&text, "$1");
    // Code is left as it is; everything outside it gets Discord's bold.
    let mut out = String::new();
    for (i, part) in text.split('`').enumerate() {
        if i > 0 {
            out.push('`');
        }
        if i % 2 == 0 {
            out.push_str(&BOLD.replace_all(part, "$1**$2**"));
        } else {
            out.push_str(part);
        }
    }
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Plain words of a Block Kit text object, whatever its type.
fn words(text: &Value) -> String {
    let t = text.get("text").and_then(Value::as_str).unwrap_or("");
    match text.get("type").and_then(Value::as_str) {
        Some("mrkdwn") => markdown_of(t),
        _ => t.to_string(),
    }
}

/// `text` in at most `most` characters, an ellipsis ending one cut short.
pub fn clip(text: &str, most: usize) -> String {
    if text.chars().count() > most {
        let cut: String = text.chars().take(most.saturating_sub(1)).collect();
        format!("{}…", cut.trim_end())
    } else {
        text.to_string()
    }
}

/// A body cut to what one message holds, at a line where it can be.
pub fn fit(text: &str, most: usize) -> String {
    if text.chars().count() <= most {
        return text.to_string();
    }
    let room: String = text.chars().take(most.saturating_sub(2)).collect();
    let cut = room
        .rfind('\n')
        .filter(|&i| i > most / 2)
        .unwrap_or(room.len());
    format!("{}\n…", room[..cut].trim_end())
}

/// A button as Discord draws it, with the `custom_id` that names what it
/// does (a link has none).
pub fn button(b: &super::post::Button, custom_id: &str) -> Value {
    let label = clip(&b.text, 80);
    match &b.url {
        Some(url) => json!({ "type": 2, "style": 5, "label": label, "url": url }),
        None => json!({
            "type": 2,
            "style": match b.style { Some("primary") => 1, Some("danger") => 4, _ => 2 },
            "label": label,
            "custom_id": custom_id,
        }),
    }
}

/// Buttons in Discord's rows: five to a row, five rows at most.
pub fn rows(buttons: Vec<Value>) -> Vec<Value> {
    buttons
        .chunks(5)
        .take(5)
        .map(|row| json!({ "type": 1, "components": row }))
        .collect()
}

/// A Block Kit button element as the relay's `Button`.
pub fn button_of(element: &Value) -> Option<super::post::Button> {
    if element.get("type").and_then(Value::as_str) != Some("button") {
        return None;
    }
    let text = element.pointer("/text/text").and_then(Value::as_str)?;
    Some(super::post::Button {
        text: text.to_string(),
        url: element
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_string),
        value: element
            .get("value")
            .and_then(Value::as_str)
            .and_then(|v| serde_json::from_str(v).ok()),
        style: match element.get("style").and_then(Value::as_str) {
            Some("primary") => Some("primary"),
            Some("danger") => Some("danger"),
            _ => None,
        },
        confirm: element.get("confirm").cloned(),
    })
}

/// A view's blocks as Markdown and buttons, in the order they come: a
/// header as a heading, a section's text (and a button beside it), context
/// in small type, an action row's buttons, a divider as a blank line.
pub fn blocks_as_message(blocks: &[Value]) -> (String, Vec<super::post::Button>) {
    let mut text: Vec<String> = Vec::new();
    let mut buttons = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("header") => text.push(format!("## {}", words(&block["text"]))),
            Some("section") => {
                if block.get("text").is_some() {
                    text.push(words(&block["text"]));
                }
                if let Some(b) = block.get("accessory").and_then(button_of)
                    && b.url.is_none()
                {
                    buttons.push(b);
                }
            }
            Some("markdown") => text.push(
                block
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ),
            Some("context") => {
                let line: Vec<String> = block
                    .get("elements")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(words)
                    .collect();
                for l in line.join(" ").lines() {
                    text.push(format!("-# {l}"));
                }
            }
            Some("actions") => buttons.extend(
                block
                    .get("elements")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(button_of),
            ),
            Some("divider") => text.push(String::new()),
            _ => {}
        }
    }
    let joined = text.join("\n");
    (joined.trim().to_string(), buttons)
}

/// The title of a view.
pub fn title_of(view: &Value) -> String {
    view.pointer("/title/text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// A form's fields: each input block's ID, label, hint and element.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub id: String,
    pub label: String,
    pub hint: String,
    pub optional: bool,
    /// Picking in it redraws the form (Slack's `dispatch_action`).
    pub dispatch: bool,
    pub element: Value,
}

impl Field {
    pub fn kind(&self) -> &str {
        self.element
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
    }

    pub fn is_text(&self) -> bool {
        self.kind() == "plain_text_input"
    }

    /// A menu's options, its groups flattened in their order, as (label,
    /// value, description).
    pub fn options(&self) -> Vec<(String, String, String)> {
        let one = |o: &Value| {
            (
                o.pointer("/text/text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                o.get("value")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                o.pointer("/description/text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            )
        };
        let mut out: Vec<_> = self
            .element
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(one)
            .collect();
        for group in self
            .element
            .get("option_groups")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            out.extend(
                group
                    .get("options")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(one),
            );
        }
        out
    }

    /// What the field starts with: a text field's words, a menu's chosen
    /// value, checkboxes' ticked values joined by commas.
    pub fn initial(&self) -> String {
        if let Some(v) = self.element.get("initial_value").and_then(Value::as_str) {
            return v.to_string();
        }
        if let Some(v) = self
            .element
            .pointer("/initial_option/value")
            .and_then(Value::as_str)
        {
            return v.to_string();
        }
        self.element
            .get("initial_options")
            .and_then(Value::as_array)
            .map(|os| {
                os.iter()
                    .filter_map(|o| o.get("value").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default()
    }
}

pub fn fields_of(view: &Value) -> Vec<Field> {
    view.get("blocks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("input"))
        .map(|b| Field {
            id: b
                .get("block_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            label: b
                .pointer("/label/text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            hint: b
                .pointer("/hint/text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            optional: b.get("optional").and_then(Value::as_bool).unwrap_or(false),
            dispatch: b
                .get("dispatch_action")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            element: b.get("element").cloned().unwrap_or(Value::Null),
        })
        .collect()
}

/// Whether a view is a form, one with a button that sends it, rather than
/// a screen to look at.
pub fn is_form(view: &Value) -> bool {
    view.get("submit").is_some_and(|s| !s.is_null())
}

/// The most fields a Discord modal holds, and options a menu.
pub const MODAL_FIELDS: usize = 5;
pub const MENU_OPTIONS: usize = 25;

/// Whether a form fits a modal: few enough fields, and none whose choice
/// redraws the form, which a modal can't do once open.
pub fn fits_modal(fields: &[Field]) -> bool {
    !fields.is_empty() && fields.len() <= MODAL_FIELDS && !fields.iter().any(|f| f.dispatch)
}

/// A menu's option values hold 100 characters; a longer one is named by a
/// key the relay keeps (`register`).
pub const LONGEST_VALUE: usize = 100;

/// A field as a modal's component: a label around a text input or a menu,
/// its `custom_id` the field's block ID. `option` gives an option's value
/// as Discord holds it.
pub fn modal_field(f: &Field, option: &mut dyn FnMut(&str) -> String) -> Value {
    let label = clip(&f.label, 45);
    let mut description = if f.hint.is_empty() && f.label.chars().count() > 45 {
        f.label.clone()
    } else {
        f.hint.clone()
    };
    description = clip(&description, 100);
    let component = if f.is_text() {
        let mut input = json!({
            "type": 4,
            "custom_id": f.id,
            "style": if f.element.get("multiline").and_then(Value::as_bool).unwrap_or(false) { 2 } else { 1 },
            "required": !f.optional,
        });
        if let Some(max) = f.element.get("max_length").and_then(Value::as_u64) {
            input["max_length"] = json!(max.min(4000));
        }
        if let Some(p) = f
            .element
            .pointer("/placeholder/text")
            .and_then(Value::as_str)
        {
            input["placeholder"] = json!(clip(p, 100));
        }
        let initial = f.initial();
        if !initial.is_empty() {
            input["value"] = json!(initial);
        }
        input
    } else {
        menu(f, option)
    };
    let mut label_component = json!({ "type": 18, "label": label, "component": component });
    if !description.is_empty() {
        label_component["description"] = json!(description);
    }
    label_component
}

/// A field's menu (or checkboxes, as a menu taking several), at most
/// `MENU_OPTIONS` of its options, the chosen ones marked.
pub fn menu(f: &Field, option: &mut dyn FnMut(&str) -> String) -> Value {
    let chosen: Vec<String> = f.initial().split(',').map(str::to_string).collect();
    let options: Vec<Value> = f
        .options()
        .into_iter()
        .take(MENU_OPTIONS)
        .map(|(label, value, description)| {
            let mut o = json!({
                "label": clip(if label.is_empty() { &value } else { &label }, 100),
                "value": option(&value),
                "default": chosen.contains(&value),
            });
            if !description.is_empty() {
                o["description"] = json!(clip(&description, 100));
            }
            o
        })
        .collect();
    let several = f.kind() == "checkboxes";
    let mut select = json!({
        "type": 3,
        "custom_id": f.id,
        "options": options,
        "min_values": if f.optional || several { 0 } else { 1 },
        "max_values": if several { options.len().max(1) } else { 1 },
        "required": !f.optional,
    });
    if let Some(p) = f
        .element
        .pointer("/placeholder/text")
        .and_then(Value::as_str)
    {
        select["placeholder"] = json!(clip(p, 150));
    }
    select
}

/// A form as a Discord modal, its `custom_id` naming the form the relay
/// keeps for it.
pub fn modal(
    view: &Value,
    fields: &[Field],
    custom_id: &str,
    option: &mut dyn FnMut(&str) -> String,
) -> Value {
    let title = title_of(view);
    json!({
        "custom_id": custom_id,
        "title": clip(if title.is_empty() { "Switchboard" } else { &title }, 45),
        "components": fields.iter().take(MODAL_FIELDS).map(|f| modal_field(f, option)).collect::<Vec<_>>(),
    })
}

// ── The Gateway ─────────────────────────────────────────────────────────────

/// What the relay keeps of its Gateway session, to resume it: the session,
/// the last event's number, and where to resume.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    pub id: Option<String>,
    pub seq: Option<u64>,
    pub resume_url: Option<String>,
    /// The last heartbeat was acknowledged; one that wasn't by the next
    /// means the connection has died without closing (a zombie).
    pub acked: bool,
}

/// What a Gateway message asks of the relay.
#[derive(Clone, Debug, PartialEq)]
pub enum Gateway {
    /// HELLO: heartbeat every this many ms, then identify or resume.
    Hello(u64),
    /// Send a heartbeat now (op 1 from Discord).
    Beat,
    /// A heartbeat was acknowledged.
    Ack,
    /// An event: its name and data.
    Dispatch(String, Value),
    /// Reconnect and resume (op 7).
    Reconnect,
    /// The session is gone (op 9): resume if it says so, else identify
    /// afresh.
    Invalid {
        resumable: bool,
    },
    Other,
}

/// Reads one Gateway message, keeping its sequence number and, from READY,
/// the session to resume.
pub fn read_gateway(session: &mut Session, text: &str) -> Gateway {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return Gateway::Other;
    };
    if let Some(s) = v.get("s").and_then(Value::as_u64) {
        session.seq = Some(s);
    }
    match v.get("op").and_then(Value::as_u64) {
        Some(0) => {
            let name = v.get("t").and_then(Value::as_str).unwrap_or("").to_string();
            let d = v.get("d").cloned().unwrap_or(Value::Null);
            if name == "READY" {
                session.id = d
                    .get("session_id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                session.resume_url = d
                    .get("resume_gateway_url")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            Gateway::Dispatch(name, d)
        }
        Some(1) => Gateway::Beat,
        Some(7) => Gateway::Reconnect,
        Some(9) => Gateway::Invalid {
            resumable: v.get("d").and_then(Value::as_bool).unwrap_or(false),
        },
        Some(10) => Gateway::Hello(
            v.pointer("/d/heartbeat_interval")
                .and_then(Value::as_u64)
                .unwrap_or(41_250),
        ),
        Some(11) => {
            session.acked = true;
            Gateway::Ack
        }
        _ => Gateway::Other,
    }
}

/// A heartbeat, carrying the last event's number.
pub fn heartbeat(session: &Session) -> String {
    json!({ "op": 1, "d": session.seq }).to_string()
}

/// IDENTIFY, asking for DMs alone.
pub fn identify(token: &str) -> String {
    json!({
        "op": 2,
        "d": {
            "token": token,
            "intents": INTENTS,
            "properties": { "os": std::env::consts::OS, "browser": "switchboard", "device": "switchboard" },
        },
    })
    .to_string()
}

/// RESUME, from the last event the relay saw.
pub fn resume(token: &str, session: &Session) -> Option<String> {
    Some(
        json!({
            "op": 6,
            "d": { "token": token, "session_id": session.id.as_ref()?, "seq": session.seq },
        })
        .to_string(),
    )
}

/// The close codes after which connecting again can't help (a bad token,
/// intents not allowed), so the relay stops and says why.
pub fn fatal_close(code: u16) -> Option<&'static str> {
    match code {
        4004 => Some("Discord refused the bot's token"),
        4010..=4012 => Some("Discord refused the Gateway's settings"),
        4013 | 4014 => Some("Discord refused the bot's intents"),
        _ => None,
    }
}

/// Close codes after which the session can't be resumed, so the relay
/// identifies afresh.
pub fn fresh_after(code: u16) -> bool {
    matches!(code, 4007 | 4009)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dm() -> Dm {
        Dm {
            channel: "1000000000000000001".into(),
            user: "2000000000000000002".into(),
            bot: Some("3000000000000000003".into()),
            ..Dm::default()
        }
    }

    fn owners_message() -> Value {
        json!({
            "id": "1300000000000000000",
            "type": 0,
            "channel_id": "1000000000000000001",
            "author": { "id": "2000000000000000002" },
            "content": "hi there",
        })
    }

    #[test]
    fn only_the_owners_own_messages_in_the_dm_are_acted_on() {
        let logged = RefCell::new(Vec::new());
        let take = |m: &Value| message_from(&dm(), m, |l| logged.borrow_mut().push(l.to_string()));
        let (m, replied) = take(&owners_message()).unwrap();
        assert_eq!((m.text.as_str(), replied), ("hi there", None));
        let with = |key: &str, v: Value| {
            let mut m = owners_message();
            m[key] = v;
            m
        };
        let mut other = owners_message();
        other["author"]["id"] = json!("9");
        assert!(take(&other).is_none(), "another user");
        assert!(
            take(&with("channel_id", json!("4"))).is_none(),
            "another channel"
        );
        assert!(
            take(&with("guild_id", json!("5"))).is_none(),
            "a server channel"
        );
        assert!(take(&with("webhook_id", json!("6"))).is_none(), "a webhook");
        assert!(
            take(&with("application_id", json!("7"))).is_none(),
            "an application"
        );
        assert!(take(&with("channel_type", json!(0))).is_none(), "not a DM");
        assert!(
            take(&with("type", json!(7))).is_none(),
            "a member joining, not a message"
        );
        let mut bot = owners_message();
        bot["author"]["bot"] = json!(true);
        assert!(take(&bot).is_none(), "a bot");
        assert_eq!(
            logged.borrow().len(),
            7,
            "each logged but the bot's: {logged:?}"
        );
        let mut reply = owners_message();
        reply["type"] = json!(19);
        reply["message_reference"] = json!({ "message_id": "1200000000000000000" });
        assert_eq!(
            take(&reply).unwrap().1.as_deref(),
            Some("1200000000000000000")
        );
    }

    fn press() -> Value {
        json!({
            "id": "I1",
            "type": 3,
            "channel_id": "1000000000000000001",
            "user": { "id": "2000000000000000002" },
            "data": { "custom_id": "v:abc", "component_type": 2 },
            "message": { "id": "M1", "content": "Waiting", "author": { "id": "3000000000000000003" } },
        })
    }

    #[test]
    fn a_press_counts_only_from_the_owner_in_the_dm_on_the_relays_message() {
        let logged = RefCell::new(Vec::new());
        let take = |p: &Value| press_from(&dm(), p, |l| logged.borrow_mut().push(l.to_string()));
        let c = take(&press()).unwrap();
        assert_eq!((c.custom_id.as_str(), c.message.as_str()), ("v:abc", "M1"));
        let mut other = press();
        other["user"]["id"] = json!("9");
        assert!(take(&other).is_none(), "another user");
        let mut server = press();
        server["guild_id"] = json!("5");
        assert!(take(&server).is_none(), "in a server");
        let mut channel = press();
        channel["channel_id"] = json!("4");
        assert!(take(&channel).is_none(), "another channel");
        let mut theirs = press();
        theirs["message"]["author"]["id"] = json!("8");
        assert!(take(&theirs).is_none(), "on someone else's message");
        assert_eq!(logged.borrow().len(), 4);
    }

    #[test]
    fn a_form_counts_only_from_the_owner_in_the_dm_and_reads_every_field() {
        let form = json!({
            "id": "I2",
            "type": 5,
            "channel_id": "1000000000000000001",
            "user": { "id": "2000000000000000002" },
            "data": { "custom_id": "m:k", "components": [
                { "type": 18, "component": { "type": 4, "custom_id": "name", "value": "Fix it" } },
                { "type": 18, "component": { "type": 3, "custom_id": "switches", "values": ["a", "b"] } },
                { "type": 1, "components": [{ "type": 4, "custom_id": "old", "value": "row" }] },
            ] },
        });
        let s = submission_from(&dm(), &form, |_| {}).unwrap();
        assert_eq!(s.custom_id, "m:k");
        assert_eq!(
            s.values,
            [
                ("name".to_string(), "Fix it".to_string()),
                ("switches".to_string(), "a,b".to_string()),
                ("old".to_string(), "row".to_string()),
            ]
        );
        let mut other = form.clone();
        other["user"]["id"] = json!("9");
        assert!(submission_from(&dm(), &other, |_| {}).is_none());
    }

    #[test]
    fn every_call_but_the_dms_and_its_owners_is_refused() {
        let known: HashSet<String> = ["I1".to_string()].into();
        let ok = |method: &str, path: &str, body: Value| {
            refuse(method, path, Some(&body), Some("D1"), Some("U1"), &known).is_ok()
        };
        assert!(ok("POST", "/channels/D1/messages", json!({})));
        assert!(ok("PATCH", "/channels/D1/messages/9", json!({})));
        assert!(!ok("POST", "/channels/C2/messages", json!({})));
        assert!(ok(
            "POST",
            "/users/@me/channels",
            json!({ "recipient_id": "U1" })
        ));
        assert!(!ok(
            "POST",
            "/users/@me/channels",
            json!({ "recipient_id": "U2" })
        ));
        assert!(ok("POST", "/interactions/I1/tok/callback", json!({})));
        assert!(!ok("POST", "/interactions/I9/tok/callback", json!({})));
        assert!(ok("GET", "/users/@me", json!(null)));
        assert!(!ok("POST", "/guilds/1/channels", json!({})));
        assert!(!ok("DELETE", "/users/@me/guilds/1", json!({})));
        assert!(
            refuse("POST", "/channels/D1/messages", None, None, None, &known).is_err(),
            "nothing goes before the DM is known"
        );
    }

    #[test]
    fn mrkdwn_reads_as_discords_markdown() {
        assert_eq!(
            markdown_of("*Tab 2* is <https://x.test/a|its thread> &amp; `*code*`"),
            "**Tab 2** is [its thread](https://x.test/a) & `*code*`"
        );
        assert_eq!(
            markdown_of("🟢 checked in <!date^1700000000^{ago}|at 10:00>."),
            "🟢 checked in <t:1700000000:R>."
        );
    }

    #[test]
    fn ids_read_as_times_and_times_as_ids() {
        let id = snowflake_at(1_700_000_000_000);
        assert_eq!(snowflake_ms(&id), Some(1_700_000_000_000));
        assert!(is_snowflake("1000000000000000001"));
        assert!(!is_snowflake("U123"));
    }

    #[test]
    fn the_gateway_keeps_its_session_and_says_what_each_message_asks() {
        let mut s = Session::default();
        assert_eq!(
            read_gateway(&mut s, r#"{"op":10,"d":{"heartbeat_interval":45000}}"#),
            Gateway::Hello(45000)
        );
        let ready =
            r#"{"op":0,"s":1,"t":"READY","d":{"session_id":"S","resume_gateway_url":"wss://r"}}"#;
        assert!(matches!(read_gateway(&mut s, ready), Gateway::Dispatch(n, _) if n == "READY"));
        assert_eq!((s.id.as_deref(), s.seq), (Some("S"), Some(1)));
        assert_eq!(s.resume_url.as_deref(), Some("wss://r"));
        assert_eq!(read_gateway(&mut s, r#"{"op":11}"#), Gateway::Ack);
        assert!(s.acked);
        assert_eq!(
            read_gateway(&mut s, r#"{"op":7,"d":null}"#),
            Gateway::Reconnect
        );
        assert_eq!(
            read_gateway(&mut s, r#"{"op":9,"d":false}"#),
            Gateway::Invalid { resumable: false }
        );
        let beat: Value = serde_json::from_str(&heartbeat(&s)).unwrap();
        assert_eq!(beat, json!({ "op": 1, "d": 1 }));
        let resumed: Value = serde_json::from_str(&resume("T", &s).unwrap()).unwrap();
        assert_eq!(resumed["d"]["session_id"], "S");
        let identified: Value = serde_json::from_str(&identify("T")).unwrap();
        assert_eq!(identified["d"]["intents"], json!(INTENTS));
        assert_eq!(fatal_close(4004), Some("Discord refused the bot's token"));
        assert!(fresh_after(4009) && !fresh_after(4000));
    }

    #[test]
    fn a_form_too_big_or_redrawn_as_you_pick_isnt_a_modal() {
        let input = |id: &str, dispatch: bool| {
            json!({ "type": "input", "block_id": id, "dispatch_action": dispatch,
                "label": { "type": "plain_text", "text": id },
                "element": { "type": "plain_text_input", "action_id": id } })
        };
        let view = |blocks: Vec<Value>| json!({ "type": "modal", "submit": {}, "blocks": blocks });
        let fields = |v: &Value| fields_of(v);
        assert!(fits_modal(&fields(&view(vec![input("a", false)]))));
        assert!(!fits_modal(&fields(&view(vec![input("a", true)]))));
        let many: Vec<Value> = (0..6).map(|i| input(&i.to_string(), false)).collect();
        assert!(!fits_modal(&fields(&view(many))));
    }
}
