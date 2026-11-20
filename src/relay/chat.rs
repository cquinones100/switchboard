//! What the relay does in its chat, apart from which chat it is: post a
//! message with buttons (in a thread or not, notifying or not), edit,
//! delete and react to one, upload and fetch files, read what was sent
//! while it couldn't hear, show and update the tabs' list, open and update
//! a tab's screen, open a form, and schedule the offline warning.
//!
//! The relay talks to its owner through Slack (`slack::SlackChat`) or a
//! Discord bot (`discord::DiscordChat`), picked by Settings' "Relay
//! through" (`relay_chat`) as it starts. The parts build what they show as
//! Slack's Block Kit, which `SlackChat` sends as it is and `DiscordChat`
//! draws as Discord's messages, components and modals. A message is named
//! by its `ts` either way: Slack's timestamp, or a Discord message's ID
//! (`state::number` reads both as a time). Who may be heard, and where the
//! relay may write, stay each chat's own rules (`slack.rs`, `discord.rs`),
//! which every call here goes through.

use anyhow::Result;
use futures_util::future::LocalBoxFuture;
use serde_json::Value;

use super::post::{Button, Upload};
use super::slack::{FileRef, Message};
use super::state::Dm;

pub type Fut<'a, T> = LocalBoxFuture<'a, Result<T>>;

/// Which chat the relay talks through, from Settings' `relay_chat`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Slack,
    Discord,
}

/// Settings' key for which chat the relay talks through.
pub const SETTING: &str = "relay_chat";

impl Kind {
    /// The chat Settings name, Slack unless it says Discord.
    pub fn from_setting(value: Option<&Value>) -> Kind {
        match value.and_then(Value::as_str) {
            Some("discord") => Kind::Discord,
            _ => Kind::Slack,
        }
    }

    /// The chat this computer's Settings name.
    pub fn current() -> Kind {
        Kind::from_setting(crate::settings::setting(SETTING).as_ref())
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Slack => "Slack",
            Kind::Discord => "Discord",
        }
    }
}

/// A message to post: its Markdown, the one plain line a notification
/// shows, its buttons, the thread it replies in, and whether it notifies
/// you there (`mention`; a message outside a thread always does).
#[derive(Clone, Copy, Debug)]
pub struct Outgoing<'a> {
    pub body: &'a str,
    pub notice: &'a str,
    pub buttons: &'a [Button],
    pub thread: Option<&'a str>,
    pub mention: bool,
}

/// A message in a thread, as cleaning up a closed tab's thread reads it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InThread {
    pub ts: String,
    /// Posted by the relay, which may delete it; yours stay.
    pub from_app: bool,
    /// The files it uploaded with it, deleted with it.
    pub files: Vec<String>,
}

pub trait Chat {
    /// The chat's name, as its words to you say it.
    fn kind(&self) -> Kind;

    /// The DM and the owner every call is held to, once known.
    fn set_dm(&self, dm: &Dm);

    /// Opens the DM with the owner, and gives it.
    fn open_dm<'a>(&'a self, owner: &'a str) -> Fut<'a, Dm>;

    /// The address links to a message start with, when the DM doesn't
    /// have it yet.
    fn workspace_url(&self) -> Fut<'_, Option<String>>;

    /// The most one message holds, in UTF-16 units.
    fn max_post_chars(&self) -> usize;

    /// Posts one message, and gives its ts.
    fn post<'a>(&'a self, message: Outgoing<'a>) -> Fut<'a, String>;

    /// Edits a message the relay posted.
    fn update<'a>(
        &'a self,
        ts: &'a str,
        body: &'a str,
        notice: &'a str,
        buttons: &'a [Button],
    ) -> Fut<'a, ()>;

    /// Deletes a message the relay posted.
    fn delete<'a>(&'a self, ts: &'a str) -> Fut<'a, ()>;

    /// Reacts to a message with `emoji`, by Slack's name for it ("eyes").
    /// Reacting twice is no error.
    fn react<'a>(&'a self, ts: &'a str, emoji: &'a str) -> Fut<'a, ()>;

    /// Takes the relay's reaction off; one that isn't there is no error.
    fn unreact<'a>(&'a self, ts: &'a str, emoji: &'a str) -> Fut<'a, ()>;

    /// Uploads files into the DM, in `thread` if given. A file that fails
    /// is handed to `failed` and left out; gives the names shared.
    fn upload<'a>(
        &'a self,
        files: Vec<Upload>,
        thread: Option<&'a str>,
        failed: &'a dyn Fn(&str, &anyhow::Error),
    ) -> Fut<'a, Vec<String>>;

    /// What the chat knows of an attached file, when a message named it
    /// only by its ID.
    fn file_info<'a>(&'a self, id: &'a str) -> Fut<'a, FileRef>;

    /// The hosts an attached file may be fetched from.
    fn file_hosts(&self) -> &'static [&'static str];

    /// An attached file's bytes.
    fn fetch<'a>(&'a self, url: &'a str) -> Fut<'a, Vec<u8>>;

    /// Deletes a file the relay uploaded.
    fn delete_file<'a>(&'a self, id: &'a str) -> Fut<'a, ()>;

    /// Every message in a thread, its first message first.
    fn thread_messages<'a>(&'a self, ts: &'a str) -> Fut<'a, Vec<InThread>>;

    /// What you sent while the relay couldn't hear: the DM after `cursor`,
    /// and each watched thread after the reply it last handled, by the
    /// chat's own rules for what is yours.
    fn catch_up<'a>(
        &'a self,
        dm: &'a Dm,
        cursor: Option<&'a str>,
        threads: &'a [(String, String)],
        log: &'a dyn Fn(&str),
    ) -> Fut<'a, Vec<Message>>;

    /// Shows the tabs' list (the Home tab, Block Kit's `home` view).
    fn publish_home<'a>(&'a self, view: &'a Value) -> Fut<'a, ()>;

    /// Opens a screen or a form (Block Kit's `modal`) on a press's
    /// `trigger`, and gives the ID that updates it.
    fn open_view<'a>(&'a self, trigger: &'a str, view: &'a Value) -> Fut<'a, Option<String>>;

    /// Opens a form over a screen, on a press made on it.
    fn push_view<'a>(&'a self, trigger: &'a str, view: &'a Value) -> Fut<'a, ()>;

    /// Draws an open screen or form again. An error naming `not_found`
    /// means it has been closed.
    fn update_view<'a>(
        &'a self,
        id: &'a str,
        view: &'a Value,
        hash: Option<&'a str>,
    ) -> Fut<'a, ()>;

    /// Leaves `text` with the chat to post at `post_at` (seconds), and
    /// gives its ID; empty where the chat can't hold a message for later.
    fn schedule<'a>(&'a self, text: &'a str, post_at: i64) -> Fut<'a, String>;

    /// Every message left with the chat for later.
    fn scheduled(&self) -> Fut<'_, Vec<String>>;

    /// Takes back a message left for later. An error naming
    /// `invalid_scheduled_message_id` means it was already sent.
    fn unschedule<'a>(&'a self, id: &'a str) -> Fut<'a, ()>;

    /// A thread has been deleted: forget what the chat kept of it.
    fn forget_thread(&self, _ts: &str) {}

    /// A link to the message `at` in the thread `thread`.
    fn message_url(&self, dm: &Dm, thread: &str, at: &str) -> Option<String>;

    /// A link to the tabs' list, once there is one.
    fn home_url(&self, dm: &Dm) -> Option<String>;
}
