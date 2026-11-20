//! The relay's chat as a Discord bot's DM (`Chat` for Discord), and what
//! it hears from the Gateway.
//!
//! A DM has no threads, so a tab's messages are replies (Discord's
//! `message_reference`) to its header message, which is the tab's thread:
//! each message the relay posts or hears is recorded with the thread it is
//! in, so a reply to any of them is a reply in that thread, and a message
//! replying to none is one in the main DM. Every message in a DM notifies,
//! so a reply that shouldn't (Slack's thread reply without a mention, the
//! relay's "Thread only") goes as a silent message.
//!
//! There is no Home tab: the tabs' list is a message pinned in the DM,
//! edited in place, whose menu opens a tab's screen. A screen is a message
//! with its buttons rather than a modal, since a Discord modal only takes
//! input; Message, Skill and Rename still open a modal for their field. A
//! form with more fields than a modal holds (Autopilot's) is a message of
//! menus, with its text fields in a modal behind a button; and Skill's
//! form, which is drawn again once a skill is picked, starts as a menu of
//! skills whose choice opens the modal for that skill.
//!
//! A button's `custom_id` holds 100 characters, so it names the value the
//! relay keeps for it, in `discord.json` beside the state, with each
//! message's thread and the pinned list's message. Discord's state is
//! kept apart from Slack's (`discord-state.json` beside `state.json`), so
//! switching between them keeps each one's threads.
//!
//! Each interaction is answered within Discord's three seconds: by the
//! modal it opens, or else acknowledged once the press is handled, and no
//! later than `ACK_AFTER`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Cx;
use super::chat::{Chat, Fut, InThread, Kind, Outgoing};
use super::discord::{self as rules, Discord, Field};
use super::post::{Button, Upload};
use super::slack::{FileRef, Form, Message, Press, Pressed, ViewRef};
use super::state::{Dm, number};
use crate::util;

/// An interaction not answered by then is acknowledged, inside Discord's
/// three seconds. The tests' presses are handled at once, so they wait less.
#[cfg(not(test))]
pub const ACK_AFTER: Duration = Duration::from_millis(2_200);
#[cfg(test)]
pub const ACK_AFTER: Duration = Duration::from_millis(100);

/// How many buttons' values are kept.
const KEPT: usize = 4_000;

/// How many messages' threads are kept at most. A thread's are forgotten
/// once cleaning up deletes it, so this only bounds a DM never cleaned up.
const THREADS_KEPT: usize = 50_000;

/// The most pages of 100 a catch-up reads back from the newest message.
const CATCH_UP_PAGES: usize = 50;

/// The pinned list's ID among views.
pub const HOME: &str = "home";

/// What is kept between runs.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Kept {
    /// Each message's thread: the header it replies to, and whether the
    /// relay posted it.
    #[serde(default)]
    threads: IndexMap<String, (String, bool)>,
    /// What each button's or option's key stands for.
    #[serde(default)]
    values: IndexMap<String, Value>,
    /// The pinned tabs' list.
    #[serde(default)]
    home: Option<String>,
}

/// A form shown as a message of menus: the form, what has been chosen, and
/// the screen it was opened over.
#[derive(Clone, Debug)]
struct FormMessage {
    view: Value,
    chosen: HashMap<String, String>,
    root: Option<String>,
}

/// A modal open: the form it sends, the values it already has, and the
/// form message it fills in, if any.
#[derive(Clone, Debug)]
struct Modal {
    view: Value,
    fixed: HashMap<String, String>,
    root: Option<String>,
    /// The form message it fills in; `sends` says whether sending the
    /// modal sends the form, or only fills its fields in.
    message: Option<String>,
    sends: bool,
}

/// An interaction to answer: its token, the message it was on, and
/// whether it has been answered.
#[derive(Clone, Debug)]
struct Pending {
    token: String,
    message: String,
    answered: bool,
}

pub struct Inner {
    pub discord: Discord,
    file: PathBuf,
    kept: RefCell<Kept>,
    forms: RefCell<HashMap<String, FormMessage>>,
    modals: RefCell<HashMap<String, Modal>>,
    screens: RefCell<Vec<String>>,
    pending: RefCell<HashMap<String, Pending>>,
    home_view: RefCell<Option<Value>>,
    home_page: Cell<usize>,
    bot: RefCell<Option<String>>,
    next: Cell<u64>,
}

/// The relay's chat as Discord, shared with the Gateway's task.
#[derive(Clone)]
pub struct DiscordChat(pub Rc<Inner>);

impl DiscordChat {
    /// Discord through `discord`, keeping what it must in `file`.
    pub fn new(discord: Discord, file: PathBuf) -> DiscordChat {
        let kept = util::read_text(&file)
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        DiscordChat(Rc::new(Inner {
            discord,
            file,
            kept: RefCell::new(kept),
            forms: RefCell::default(),
            modals: RefCell::default(),
            screens: RefCell::default(),
            pending: RefCell::default(),
            home_view: RefCell::default(),
            home_page: Cell::new(0),
            bot: RefCell::default(),
            next: Cell::new(0),
        }))
    }

    fn save(&self) {
        let text =
            util::json_compact(&serde_json::to_value(&*self.0.kept.borrow()).unwrap_or_default());
        let _ = util::write_private(&self.0.file, text, false);
    }

    fn channel(&self) -> String {
        self.0.discord.channel.borrow().clone().unwrap_or_default()
    }

    /// The thread a message is in: the header it replies to, else itself.
    pub fn thread_of(&self, message: &str) -> String {
        self.0
            .kept
            .borrow()
            .threads
            .get(message)
            .map(|(t, _)| t.clone())
            .unwrap_or_else(|| message.to_string())
    }

    fn record(&self, message: &str, thread: &str, ours: bool) {
        let mut kept = self.0.kept.borrow_mut();
        kept.threads
            .insert(message.to_string(), (thread.to_string(), ours));
        while kept.threads.len() > THREADS_KEPT {
            kept.threads.shift_remove_index(0);
        }
        drop(kept);
        self.save();
    }

    /// A key for `value`, kept (and saved) so a press can give it back.
    fn register(&self, value: &Value) -> String {
        let text = util::json_compact(value);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        let key = format!("{:016x}", hasher.finish());
        let mut kept = self.0.kept.borrow_mut();
        let new = kept.values.shift_remove(&key).is_none();
        kept.values.insert(key.clone(), value.clone());
        while kept.values.len() > KEPT {
            kept.values.shift_remove_index(0);
        }
        drop(kept);
        // Saved at once, so a button outlives a restart whatever posted it.
        if new {
            self.save();
        }
        key
    }

    fn value(&self, key: &str) -> Option<Value> {
        self.0.kept.borrow().values.get(key).cloned()
    }

    /// An option's value as a menu holds it: as it is when it fits, else
    /// by its key.
    fn option_value(&self, value: &str) -> String {
        if value.len() <= rules::LONGEST_VALUE && !value.starts_with("k:") {
            value.to_string()
        } else {
            format!("k:{}", self.register(&json!(value)))
        }
    }

    /// A chosen option's value as the relay gave it.
    fn chosen_value(&self, value: &str) -> String {
        value
            .strip_prefix("k:")
            .and_then(|k| self.value(k))
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| value.to_string())
    }

    fn chosen_values(&self, joined: &str) -> String {
        joined
            .split(',')
            .filter(|v| !v.is_empty())
            .map(|v| self.chosen_value(v))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// The relay's buttons as Discord's rows, each naming the value it
    /// carries; one Slack would confirm first asks in a message of its own.
    fn components(&self, buttons: &[Button]) -> Vec<Value> {
        rules::rows(
            buttons
                .iter()
                .map(|b| {
                    let id = match (&b.value, &b.confirm) {
                        (Some(v), Some(c)) => {
                            format!("c:{}", self.register(&json!({ "value": v, "confirm": c })))
                        }
                        (Some(v), None) => format!("v:{}", self.register(v)),
                        _ => String::new(),
                    };
                    rules::button(b, &id)
                })
                .collect(),
        )
    }

    fn key(&self) -> String {
        let n = self.0.next.get() + 1;
        self.0.next.set(n);
        format!("{}{n}", util::now_ms())
    }

    // ── Answering interactions ─────────────────────────────────────────

    /// Takes an interaction Discord sent, so the relay may answer it.
    fn take(&self, id: &str, token: &str, message: &str) {
        self.0
            .discord
            .interactions
            .borrow_mut()
            .insert(id.to_string());
        self.0.pending.borrow_mut().insert(
            id.to_string(),
            Pending {
                token: token.to_string(),
                message: message.to_string(),
                answered: false,
            },
        );
    }

    /// Answers an interaction, once.
    async fn answer(&self, id: &str, body: Value) -> Result<()> {
        let token = {
            let mut pending = self.0.pending.borrow_mut();
            let p = pending
                .get_mut(id)
                .ok_or_else(|| anyhow!("the press has been answered already"))?;
            if p.answered {
                bail!("the press has been answered already");
            }
            p.answered = true;
            p.token.clone()
        };
        self.0
            .discord
            .api(
                "POST",
                &format!("/interactions/{id}/{token}/callback"),
                Some(body),
            )
            .await
            .map(|_| ())
    }

    /// Acknowledges an interaction not answered otherwise.
    pub async fn ack(&self, id: &str) {
        let open = self.0.pending.borrow().get(id).is_some_and(|p| !p.answered);
        if open {
            let _ = self.answer(id, json!({ "type": 6 })).await;
        }
        self.0.pending.borrow_mut().remove(id);
        self.0.discord.interactions.borrow_mut().remove(id);
    }

    fn pending_on(&self, message: &str) -> Option<String> {
        self.0
            .pending
            .borrow()
            .iter()
            .find(|(_, p)| p.message == message && !p.answered)
            .map(|(id, _)| id.clone())
    }

    fn pending_message(&self, id: &str) -> Option<String> {
        self.0.pending.borrow().get(id).map(|p| p.message.clone())
    }

    // ── Posting ─────────────────────────────────────────────────────────

    async fn send(
        &self,
        content: &str,
        components: Vec<Value>,
        thread: Option<&str>,
        silent: bool,
    ) -> Result<String> {
        let mut body = json!({
            "content": rules::fit(content, 2_000),
            "components": components,
            "flags": rules::SUPPRESS_EMBEDS | if silent { rules::SUPPRESS_NOTIFICATIONS } else { 0 },
            "allowed_mentions": { "parse": [] },
        });
        if let Some(t) = thread {
            body["message_reference"] = json!({ "message_id": t, "channel_id": self.channel(), "fail_if_not_exists": false });
        }
        let sent = self
            .0
            .discord
            .api(
                "POST",
                &format!("/channels/{}/messages", self.channel()),
                Some(body),
            )
            .await?;
        let id = sent
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if let Some(t) = thread {
            self.record(&id, &self.thread_of(t), true);
        }
        Ok(id)
    }

    async fn edit(&self, ts: &str, content: &str, components: Vec<Value>) -> Result<()> {
        self.0
            .discord
            .api(
                "PATCH",
                &format!("/channels/{}/messages/{ts}", self.channel()),
                Some(json!({ "content": rules::fit(content, 2_000), "components": components })),
            )
            .await
            .map(|_| ())
    }

    // ── Screens and forms ───────────────────────────────────────────────

    fn screen_message(&self, view: &Value) -> (String, Vec<Value>) {
        let (text, buttons) =
            rules::blocks_as_message(view["blocks"].as_array().map_or(&[], |b| b));
        let title = rules::title_of(view);
        let content = if title.is_empty() {
            text
        } else {
            format!("### {title}\n{text}")
        };
        (content, self.components(&buttons))
    }

    /// A form as a message: its fields and what each holds, a menu for each
    /// one that has options, and its text fields behind Fill in.
    fn form_message(&self, form: &FormMessage) -> (String, Vec<Value>) {
        let view = &form.view;
        let fields = rules::fields_of(view);
        let mut lines = vec![format!("### {}", rules::title_of(view))];
        let mut rows = Vec::new();
        for f in &fields {
            let shown = form.chosen.get(&f.id).cloned().unwrap_or_default();
            if f.is_text() {
                lines.push(format!(
                    "**{}**: {}",
                    f.label,
                    if shown.is_empty() {
                        "—".to_string()
                    } else {
                        shown
                    }
                ));
            } else if rows.len() < 4 {
                let mut field = f.clone();
                field.element["initial_options"] = Value::Null;
                field.element["initial_option"] = Value::Null;
                let chosen: Vec<&str> = shown.split(',').collect();
                let options: Vec<Value> = f
                    .options()
                    .into_iter()
                    .filter(|(_, v, _)| chosen.contains(&v.as_str()))
                    .map(|(label, value, _)| json!({ "text": { "text": label }, "value": value }))
                    .collect();
                field.element["initial_options"] = json!(options);
                let mut menu = rules::menu(&field, &mut |v| self.option_value(v));
                menu["custom_id"] = json!(format!("fs:{}", f.id));
                menu["placeholder"] = json!(rules::clip(&f.label, 150));
                menu.as_object_mut().map(|m| m.remove("required"));
                rows.push(json!({ "type": 1, "components": [menu] }));
            }
            if !f.hint.is_empty() {
                lines.push(format!("-# {}", f.hint));
            }
        }
        for block in view["blocks"].as_array().into_iter().flatten() {
            if block["type"] == "context" || block["type"] == "section" {
                let (text, _) = rules::blocks_as_message(std::slice::from_ref(block));
                lines.push(text);
            }
        }
        let picker = fields.iter().any(|f| f.dispatch);
        let mut buttons = Vec::new();
        if fields.iter().any(Field::is_text) {
            buttons.push(json!({ "type": 2, "style": 2, "label": "Fill in…", "custom_id": "ft" }));
        }
        if !picker {
            let submit = view
                .pointer("/submit/text")
                .and_then(Value::as_str)
                .unwrap_or("Send");
            buttons.push(json!({ "type": 2, "style": 1, "label": submit, "custom_id": "fo" }));
        }
        buttons.push(json!({ "type": 2, "style": 2, "label": "Cancel", "custom_id": "fx" }));
        rows.push(json!({ "type": 1, "components": buttons }));
        (lines.join("\n"), rows)
    }

    /// Opens a modal for `fields` of `view` on an interaction.
    async fn open_modal(
        &self,
        trigger: &str,
        view: &Value,
        fields: &[Field],
        modal: Modal,
    ) -> Result<()> {
        let key = self.key();
        let body = rules::modal(view, fields, &format!("m:{key}"), &mut |v| {
            self.option_value(v)
        });
        self.0.modals.borrow_mut().insert(key, modal);
        self.answer(trigger, json!({ "type": 9, "data": body }))
            .await
    }

    fn form_values(view: &Value) -> HashMap<String, String> {
        rules::fields_of(view)
            .into_iter()
            .map(|f| {
                let initial = f.initial();
                (f.id, initial)
            })
            .collect()
    }

    fn form_of(&self, view: &Value, values: HashMap<String, String>, root: Option<String>) -> Form {
        let kind = view
            .get("callback_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let meta = view
            .get("private_metadata")
            .and_then(Value::as_str)
            .and_then(|m| serde_json::from_str(m).ok())
            .unwrap_or_else(|| json!({}));
        Form {
            kind,
            meta,
            values,
            root_view: root,
        }
    }

    // ── What the Gateway brings ─────────────────────────────────────────

    /// An event from the Gateway.
    pub fn on_dispatch(&self, cx: &Cx, name: &str, d: Value) {
        match name {
            "READY" | "RESUMED" => {
                if let Some(bot) = d.pointer("/user/id").and_then(Value::as_str) {
                    *self.0.bot.borrow_mut() = Some(bot.to_string());
                }
                cx.log(&format!("Discord: {}", name.to_lowercase()));
                super::run::on_connect(cx);
            }
            "MESSAGE_CREATE" => {
                if super::run::quiet(cx) {
                    return;
                }
                let dm = cx.state.borrow().dm();
                let Some((mut message, replied)) = rules::message_from(&dm, &d, |l| cx.log(l))
                else {
                    return;
                };
                if let Some(replied) = replied {
                    let thread = self.thread_of(&replied);
                    self.record(&message.ts, &thread, false);
                    message.thread_ts = Some(thread);
                }
                super::run::process_messages(cx, vec![message]);
            }
            "INTERACTION_CREATE" => {
                if super::run::quiet(cx) {
                    return;
                }
                let dm = cx.state.borrow().dm();
                let token = d.get("token").and_then(Value::as_str).unwrap_or_default();
                if let Some(c) = rules::press_from(&dm, &d, |l| cx.log(l)) {
                    self.take(&c.interaction, token, &c.message);
                    self.on_component(cx, c);
                } else if let Some(s) = rules::submission_from(&dm, &d, |l| cx.log(l)) {
                    let on = d
                        .pointer("/message/id")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    self.take(&s.interaction, token, on);
                    self.on_submission(cx, s);
                }
            }
            _ => {}
        }
    }

    /// Acknowledges `id` after `ACK_AFTER` if nothing answered it first.
    fn ack_later(&self, id: &str) {
        let (chat, id) = (self.clone(), id.to_string());
        super::run::spawn(async move {
            tokio::time::sleep(ACK_AFTER).await;
            chat.ack(&id).await;
        });
    }

    /// Hands a press to the relay as Slack's would be, and acknowledges it
    /// once it is handled.
    fn act(&self, cx: &Cx, id: &str, press: Option<Press>, form: Option<Form>) {
        self.ack_later(id);
        let (chat, cx, id) = (self.clone(), cx.clone(), id.to_string());
        super::run::spawn(async move {
            super::run::act(&cx, press, form).await;
            chat.ack(&id).await;
        });
    }

    fn on_component(&self, cx: &Cx, c: rules::Component) {
        let id = c.interaction.clone();
        let home = self.0.kept.borrow().home.clone();
        let on_home = home.as_deref() == Some(c.message.as_str());
        let custom_id = c.custom_id.clone();
        let (kind, rest) = custom_id.split_once(':').unwrap_or((&custom_id, ""));
        let value_of = |key: &str| self.value(key);
        match kind {
            "v" => {
                let Some(value) = value_of(rest) else {
                    cx.log("ignored a Discord press on a button the relay no longer knows");
                    return self.ack_later(&id);
                };
                self.press(cx, &id, &c, value, on_home);
            }
            "home" => {
                let chosen = c.values.first().cloned().unwrap_or_default();
                if let Some(page) = chosen.strip_prefix("page:") {
                    self.0.home_page.set(page.parse().unwrap_or(0));
                    let chat = self.clone();
                    super::run::spawn(async move {
                        let view = chat.0.home_view.borrow().clone();
                        if let Some(view) = view {
                            let (content, rows) = chat.home_message(&view);
                            let _ = chat
                                .answer(&id, json!({ "type": 7, "data": { "content": content, "components": rows } }))
                                .await;
                        }
                        chat.ack(&id).await;
                    });
                    return;
                }
                match chosen.strip_prefix("v:").and_then(value_of) {
                    Some(value) => self.press(cx, &id, &c, value, true),
                    None => self.ack_later(&id),
                }
            }
            "c" => {
                let Some(kept) = value_of(rest) else {
                    return self.ack_later(&id);
                };
                let confirm = &kept["confirm"];
                let text = |p: &str| confirm.pointer(p).and_then(Value::as_str).unwrap_or("");
                // Yes is pressed on the message that asks, so it carries
                // the message the button was on, for the press to act there.
                let yes = format!(
                    "y:{}",
                    self.register(
                        &json!({ "value": kept["value"], "on": c.message, "content": c.content })
                    )
                );
                let body = json!({
                    "type": 4,
                    "data": {
                        "flags": rules::EPHEMERAL,
                        "content": format!("**{}**\n{}", text("/title/text"), text("/text/text")),
                        "components": [{ "type": 1, "components": [
                            { "type": 2, "style": 4, "label": rules::clip(text("/confirm/text"), 80), "custom_id": yes },
                            { "type": 2, "style": 2, "label": rules::clip(text("/deny/text"), 80), "custom_id": "x" },
                        ] }],
                    },
                });
                let chat = self.clone();
                super::run::spawn(async move {
                    let _ = chat.answer(&id, body).await;
                    chat.ack(&id).await;
                });
            }
            "y" => {
                let Some(kept) = value_of(rest) else {
                    return self.ack_later(&id);
                };
                let on = kept["on"].as_str().unwrap_or_default().to_string();
                let asked = rules::Component {
                    message: on.clone(),
                    content: kept["content"].as_str().unwrap_or_default().to_string(),
                    ..c.clone()
                };
                let on_home = home.as_deref() == Some(on.as_str());
                let chat = self.clone();
                let done = id.clone();
                super::run::spawn(async move {
                    let _ = chat
                        .answer(
                            &done,
                            json!({ "type": 7, "data": { "content": "Done.", "components": [] } }),
                        )
                        .await;
                });
                self.press(cx, &id, &asked, kept["value"].clone(), on_home);
            }
            "x" => {
                let chat = self.clone();
                super::run::spawn(async move {
                    let _ = chat
                        .answer(&id, json!({ "type": 7, "data": { "content": "Left as it was.", "components": [] } }))
                        .await;
                    chat.ack(&id).await;
                });
            }
            "fs" | "ft" | "fo" | "fx" => self.on_form_message(cx, c, kind, rest),
            _ => self.ack_later(&id),
        }
    }

    /// A button's value, pressed on a message, the pinned list or a screen.
    fn press(&self, cx: &Cx, id: &str, c: &rules::Component, value: Value, on_home: bool) {
        let a = value.get("a").and_then(Value::as_str).unwrap_or("");
        let on_screen = self.0.screens.borrow().contains(&c.message);
        let press = if on_home || on_screen {
            let allowed = if on_home {
                super::slack::HOME_ACTIONS
            } else {
                super::slack::SCREEN_ACTIONS
            };
            if !allowed.contains(&a) {
                return self.ack_later(id);
            }
            Press {
                value,
                message: None,
                view: Some(ViewRef {
                    id: if on_home {
                        HOME.to_string()
                    } else {
                        c.message.clone()
                    },
                    hash: None,
                }),
                trigger_id: Some(id.to_string()),
            }
        } else {
            let thread = self.thread_of(&c.message);
            Press {
                value,
                trigger_id: Some(id.to_string()),
                view: None,
                message: Some(Pressed {
                    ts: c.message.clone(),
                    thread_ts: (thread != c.message).then_some(thread),
                    text: c.content.clone(),
                }),
            }
        };
        self.act(cx, id, Some(press), None);
    }

    fn on_form_message(&self, cx: &Cx, c: rules::Component, kind: &str, field: &str) {
        let id = c.interaction.clone();
        let Some(mut form) = self.0.forms.borrow().get(&c.message).cloned() else {
            cx.log("ignored a Discord press on a form the relay no longer has");
            return self.ack_later(&id);
        };
        let chat = self.clone();
        let cx = cx.clone();
        match kind {
            "fs" => {
                let chosen = self.chosen_values(&c.values.join(","));
                form.chosen.insert(field.to_string(), chosen.clone());
                self.0
                    .forms
                    .borrow_mut()
                    .insert(c.message.clone(), form.clone());
                let dispatch = rules::fields_of(&form.view)
                    .iter()
                    .any(|f| f.id == field && f.dispatch);
                if dispatch {
                    // Skill's menu: the relay draws the form again for the
                    // skill picked, which opens its modal (`update_view`).
                    let meta: Value = form
                        .view
                        .get("private_metadata")
                        .and_then(Value::as_str)
                        .and_then(|m| serde_json::from_str(m).ok())
                        .unwrap_or_default();
                    let mut value = json!({
                        "a": format!("{field}-chosen"),
                        "w": meta.get("w"),
                        "p": meta.get("p"),
                        "from": meta.get("from"),
                        "args": form.chosen.get("args").cloned().unwrap_or_default(),
                    });
                    value[field] = json!(chosen);
                    let press = Press {
                        value,
                        message: None,
                        view: Some(ViewRef {
                            id: c.message.clone(),
                            hash: None,
                        }),
                        trigger_id: Some(id.clone()),
                    };
                    return self.act(&cx, &id, Some(press), None);
                }
                super::run::spawn(async move {
                    let (content, rows) = chat.form_message(&form);
                    let _ = chat
                        .answer(&id, json!({ "type": 7, "data": { "content": content, "components": rows } }))
                        .await;
                    chat.ack(&id).await;
                });
            }
            "ft" => {
                let picker = rules::fields_of(&form.view).iter().any(|f| f.dispatch);
                let fields: Vec<Field> = rules::fields_of(&form.view)
                    .into_iter()
                    .filter(Field::is_text)
                    .map(|mut f| {
                        if let Some(v) = form.chosen.get(&f.id) {
                            f.element["initial_value"] = json!(v);
                        }
                        f
                    })
                    .collect();
                let modal = Modal {
                    view: form.view.clone(),
                    fixed: HashMap::new(),
                    root: form.root.clone(),
                    message: Some(c.message.clone()),
                    sends: picker,
                };
                let view = form.view.clone();
                super::run::spawn(async move {
                    if let Err(e) = chat.open_modal(&id, &view, &fields, modal).await {
                        cx.log(&format!("couldn't open the form's fields: {e}"));
                    }
                    chat.ack(&id).await;
                });
            }
            "fo" => {
                self.0.forms.borrow_mut().remove(&c.message);
                let sent = self.form_of(&form.view, form.chosen.clone(), form.root.clone());
                let done = format!("### {}\n-# Sent.", rules::title_of(&form.view));
                let chat2 = chat.clone();
                let id2 = id.clone();
                super::run::spawn(async move {
                    let _ = chat2
                        .answer(
                            &id2,
                            json!({ "type": 7, "data": { "content": done, "components": [] } }),
                        )
                        .await;
                });
                self.act(&cx, &id, None, Some(sent));
            }
            _ => {
                self.0.forms.borrow_mut().remove(&c.message);
                super::run::spawn(async move {
                    let _ = chat
                        .answer(&id, json!({ "type": 7, "data": { "content": "Cancelled.", "components": [] } }))
                        .await;
                    chat.ack(&id).await;
                });
            }
        }
    }

    fn on_submission(&self, cx: &Cx, s: rules::Submission) {
        let id = s.interaction.clone();
        let Some(modal) = s
            .custom_id
            .strip_prefix("m:")
            .and_then(|k| self.0.modals.borrow_mut().remove(k))
        else {
            cx.log("ignored a Discord form the relay no longer has");
            return self.ack_later(&id);
        };
        let fields = rules::fields_of(&modal.view);
        let menus: Vec<&str> = fields
            .iter()
            .filter(|f| !f.is_text())
            .map(|f| f.id.as_str())
            .collect();
        let mut values = modal.fixed.clone();
        for (field, value) in s.values {
            let value = if menus.contains(&field.as_str()) {
                self.chosen_values(&value)
            } else {
                value
            };
            values.insert(field, value);
        }
        if let Some(message) = &modal.message {
            let form = self.0.forms.borrow().get(message).cloned();
            if let Some(mut form) = form {
                form.chosen.extend(values.clone());
                if !modal.sends {
                    self.0
                        .forms
                        .borrow_mut()
                        .insert(message.clone(), form.clone());
                    let (chat, message) = (self.clone(), message.clone());
                    super::run::spawn(async move {
                        let (content, rows) = chat.form_message(&form);
                        let _ = chat.edit(&message, &content, rows).await;
                        chat.ack(&id).await;
                    });
                    return;
                }
                values = form.chosen;
            }
            self.0.forms.borrow_mut().remove(message);
            let (chat, message) = (self.clone(), message.clone());
            super::run::spawn(async move {
                let _ = chat.delete_message(&message).await;
            });
        }
        let form = self.form_of(&modal.view, values, modal.root.clone());
        self.act(cx, &id, None, Some(form));
    }

    async fn delete_message(&self, ts: &str) -> Result<()> {
        self.0
            .discord
            .api(
                "DELETE",
                &format!("/channels/{}/messages/{ts}", self.channel()),
                None,
            )
            .await
            .map(|_| ())
    }

    // ── The pinned list ─────────────────────────────────────────────────

    /// The Home tab as the pinned message: its words, a menu of the tabs
    /// that opens one's screen (a page of them at a time), and the Home
    /// tab's other buttons.
    fn home_message(&self, view: &Value) -> (String, Vec<Value>) {
        let blocks = view["blocks"].as_array().map_or(&[][..], |b| b);
        let (text, tabs, buttons) = home_parts(blocks);
        let mut rows = Vec::new();
        if !tabs.is_empty() {
            let per = if tabs.len() > rules::MENU_OPTIONS {
                rules::MENU_OPTIONS - 1
            } else {
                rules::MENU_OPTIONS
            };
            let pages = tabs.len().div_ceil(per);
            let page = self.0.home_page.get().min(pages - 1);
            let mut options: Vec<Value> = tabs
                .iter()
                .skip(page * per)
                .take(per)
                .map(|(label, b)| {
                    let value = b.value.clone().unwrap_or_default();
                    json!({
                        "label": rules::clip(label, 100),
                        "value": format!("v:{}", self.register(&value)),
                    })
                })
                .collect();
            if pages > 1 {
                let next = (page + 1) % pages;
                options.push(json!({
                    "label": if next == 0 { "Back to the first tabs".to_string() } else { format!("More tabs ({} of {pages})", next + 1) },
                    "value": format!("page:{next}"),
                }));
            }
            rows.push(json!({ "type": 1, "components": [{
                "type": 3,
                "custom_id": "home",
                "placeholder": "Open a tab's screen",
                "options": options,
            }] }));
        }
        let mut more = self.components(&buttons);
        more.truncate(5 - rows.len());
        rows.extend(more);
        (rules::fit(&text, 2_000), rows)
    }
}

/// The Home tab's blocks as the pinned message's parts: its words, each
/// tab's Screen button named by the tab's line, and the buttons that act
/// on the whole list. A tab's own choices are left to its screen.
pub fn home_parts(blocks: &[Value]) -> (String, Vec<(String, Button)>, Vec<Button>) {
    let mut text = Vec::new();
    let mut tabs = Vec::new();
    let mut buttons = Vec::new();
    let mut last = String::new();
    for block in blocks {
        let (words, found) = rules::blocks_as_message(std::slice::from_ref(block));
        if block["type"] == "section" {
            last = words
                .lines()
                .next()
                .unwrap_or("")
                .replace("**", "")
                .replace(['[', ']'], "");
            if let Some(at) = last.find("](") {
                last.truncate(at);
            }
            if let Some(at) = last.find('(') {
                last.truncate(at);
            }
        }
        if !words.is_empty() || block["type"] == "divider" {
            text.push(words);
        }
        for b in found {
            let a = b.action().to_string();
            if a == "open" && b.text == "Screen" {
                tabs.push((last.trim().to_string(), b));
            } else if !a.starts_with("home-") {
                buttons.push(b);
            }
        }
    }
    let joined = text.join("\n");
    (joined.trim().to_string(), tabs, buttons)
}

impl Chat for DiscordChat {
    fn kind(&self) -> Kind {
        Kind::Discord
    }

    fn set_dm(&self, dm: &Dm) {
        *self.0.discord.channel.borrow_mut() = Some(dm.channel.clone());
        *self.0.discord.owner.borrow_mut() = Some(dm.user.clone());
        *self.0.bot.borrow_mut() = dm.bot.clone();
    }

    fn open_dm<'a>(&'a self, owner: &'a str) -> Fut<'a, Dm> {
        Box::pin(rules::open_dm(&self.0.discord, owner))
    }

    fn workspace_url(&self) -> Fut<'_, Option<String>> {
        Box::pin(async { Ok(Some(rules::DM_URL.to_string())) })
    }

    fn max_post_chars(&self) -> usize {
        rules::MAX_POST_CHARS
    }

    /// A reply to the thread's header; silent unless it notifies.
    fn post<'a>(&'a self, m: Outgoing<'a>) -> Fut<'a, String> {
        Box::pin(async move {
            let silent = m.thread.is_some() && !m.mention;
            self.send(m.body, self.components(m.buttons), m.thread, silent)
                .await
        })
    }

    fn update<'a>(
        &'a self,
        ts: &'a str,
        body: &'a str,
        _notice: &'a str,
        buttons: &'a [Button],
    ) -> Fut<'a, ()> {
        Box::pin(async move { self.edit(ts, body, self.components(buttons)).await })
    }

    fn delete<'a>(&'a self, ts: &'a str) -> Fut<'a, ()> {
        Box::pin(self.delete_message(ts))
    }

    fn react<'a>(&'a self, ts: &'a str, emoji: &'a str) -> Fut<'a, ()> {
        Box::pin(async move {
            let path = format!(
                "/channels/{}/messages/{ts}/reactions/{}/@me",
                self.channel(),
                escape(rules::emoji(emoji))
            );
            self.0.discord.api("PUT", &path, None).await.map(|_| ())
        })
    }

    fn unreact<'a>(&'a self, ts: &'a str, emoji: &'a str) -> Fut<'a, ()> {
        Box::pin(async move {
            let path = format!(
                "/channels/{}/messages/{ts}/reactions/{}/@me",
                self.channel(),
                escape(rules::emoji(emoji))
            );
            match self.0.discord.api("DELETE", &path, None).await {
                Err(e) if !e.to_string().contains("not_found") => Err(e),
                _ => Ok(()),
            }
        })
    }

    /// Up to ten files a message, within what a bot may upload at once.
    fn upload<'a>(
        &'a self,
        files: Vec<Upload>,
        thread: Option<&'a str>,
        failed: &'a dyn Fn(&str, &anyhow::Error),
    ) -> Fut<'a, Vec<String>> {
        Box::pin(async move {
            let mut batches: Vec<Vec<Upload>> = Vec::new();
            let mut size = 0;
            for file in files {
                if file.content.len() > rules::MAX_UPLOAD_BYTES {
                    failed(
                        &file.filename,
                        &anyhow!(
                            "it is larger than the {} MB Discord takes from a bot",
                            rules::MAX_UPLOAD_BYTES / 1024 / 1024
                        ),
                    );
                    continue;
                }
                let room = batches.last().is_some_and(|b| {
                    b.len() < super::post::FILES_PER_MESSAGE
                        && size + file.content.len() <= rules::MAX_UPLOAD_BYTES
                });
                if !room {
                    batches.push(Vec::new());
                    size = 0;
                }
                size += file.content.len();
                batches.last_mut().expect("a batch").push(file);
            }
            let mut names = Vec::new();
            for batch in batches {
                let mut payload = json!({
                    "attachments": batch.iter().enumerate().map(|(i, f)| json!({ "id": i, "filename": f.filename, "description": rules::clip(&f.title, 1024) })).collect::<Vec<_>>(),
                    "flags": rules::SUPPRESS_EMBEDS | if thread.is_some() { rules::SUPPRESS_NOTIFICATIONS } else { 0 },
                    "allowed_mentions": { "parse": [] },
                });
                if let Some(t) = thread {
                    payload["message_reference"] = json!({ "message_id": t, "channel_id": self.channel(), "fail_if_not_exists": false });
                }
                let shared: Vec<String> = batch.iter().map(|f| f.filename.clone()).collect();
                let files = batch.into_iter().map(|f| (f.filename, f.content)).collect();
                let sent = self
                    .0
                    .discord
                    .upload(
                        &format!("/channels/{}/messages", self.channel()),
                        payload,
                        files,
                    )
                    .await?;
                if let (Some(t), Some(id)) = (thread, sent.get("id").and_then(Value::as_str)) {
                    self.record(id, &self.thread_of(t), true);
                }
                names.extend(shared);
            }
            Ok(names)
        })
    }

    fn file_info<'a>(&'a self, _id: &'a str) -> Fut<'a, FileRef> {
        Box::pin(async { Err(anyhow!("Discord gave no address for the file")) })
    }

    fn file_hosts(&self) -> &'static [&'static str] {
        rules::FILE_HOSTS
    }

    fn fetch<'a>(&'a self, url: &'a str) -> Fut<'a, Vec<u8>> {
        Box::pin(self.0.discord.get(url))
    }

    /// A file goes with its message.
    fn delete_file<'a>(&'a self, _id: &'a str) -> Fut<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    /// The thread's header and every message recorded as in it.
    fn thread_messages<'a>(&'a self, ts: &'a str) -> Fut<'a, Vec<InThread>> {
        Box::pin(async move {
            let mut out = vec![InThread {
                ts: ts.to_string(),
                from_app: true,
                files: Vec::new(),
            }];
            out.extend(
                self.0
                    .kept
                    .borrow()
                    .threads
                    .iter()
                    .filter(|(_, (thread, _))| thread == ts)
                    .map(|(id, (_, ours))| InThread {
                        ts: id.clone(),
                        from_app: *ours,
                        files: Vec::new(),
                    }),
            );
            Ok(out)
        })
    }

    /// The DM read after the earliest of the cursor and each watched
    /// thread's last handled reply, a page at a time, by the same rules as
    /// the Gateway's messages.
    fn catch_up<'a>(
        &'a self,
        dm: &'a Dm,
        cursor: Option<&'a str>,
        threads: &'a [(String, String)],
        log: &'a dyn Fn(&str),
    ) -> Fut<'a, Vec<Message>> {
        Box::pin(async move {
            let from = number(cursor.unwrap_or("0"));
            let earliest = threads
                .iter()
                .map(|(_, handled)| number(handled))
                .chain([from])
                .fold(f64::INFINITY, f64::min);
            let floor: u64 = rules::snowflake_at((earliest.max(0.0) * 1000.0) as u64)
                .parse()
                .unwrap_or(0);
            let id_of = |m: &Value| {
                m.get("id")
                    .and_then(Value::as_str)
                    .and_then(|i| i.parse::<u64>().ok())
                    .unwrap_or(0)
            };
            // Newest first, a page at a time back to the earliest mark, so
            // what you sent last is read however much came before it.
            let mut page: Vec<Value> = Vec::new();
            let mut before: Option<u64> = None;
            for _ in 0..CATCH_UP_PAGES {
                let query = match before {
                    Some(b) => format!("before={b}&limit=100"),
                    None => "limit=100".to_string(),
                };
                let got = self
                    .0
                    .discord
                    .api(
                        "GET",
                        &format!("/channels/{}/messages?{query}", dm.channel),
                        None,
                    )
                    .await?;
                let got: Vec<Value> = got.as_array().cloned().unwrap_or_default();
                let full = got.len() >= 100;
                let oldest = got.iter().map(id_of).min();
                page.extend(got.into_iter().filter(|m| id_of(m) > floor));
                match oldest {
                    Some(o) if full && o > floor => before = Some(o),
                    _ => break,
                }
            }
            page.sort_by_key(id_of);
            page.dedup_by_key(|m| id_of(m));
            let mut messages = Vec::new();
            {
                for m in &page {
                    let Some((mut message, replied)) = rules::message_from(dm, m, log) else {
                        continue;
                    };
                    match replied.map(|r| self.thread_of(&r)) {
                        Some(thread) => {
                            let handled = threads.iter().find(|(t, _)| *t == thread);
                            if handled.is_some_and(|(_, h)| newer(&message.ts, h)) {
                                self.record(&message.ts, &thread, false);
                                message.thread_ts = Some(thread);
                                messages.push(message);
                            }
                        }
                        None if cursor.is_none_or(|c| newer(&message.ts, c)) => {
                            messages.push(message)
                        }
                        None => {}
                    }
                }
            }
            Ok(messages)
        })
    }

    /// The pinned message, edited in place, posted and pinned again if it
    /// has gone.
    fn publish_home<'a>(&'a self, view: &'a Value) -> Fut<'a, ()> {
        Box::pin(async move {
            *self.0.home_view.borrow_mut() = Some(view.clone());
            let (content, rows) = self.home_message(view);
            let home = self.0.kept.borrow().home.clone();
            if let Some(home) = home {
                match self.edit(&home, &content, rows.clone()).await {
                    Ok(()) => return Ok(()),
                    Err(e) if !e.to_string().contains("not_found") => return Err(e),
                    Err(_) => {}
                }
            }
            let id = self.send(&content, rows, None, true).await?;
            self.0.kept.borrow_mut().home = Some(id.clone());
            self.save();
            self.0
                .discord
                .api(
                    "PUT",
                    &format!("/channels/{}/pins/{id}", self.channel()),
                    None,
                )
                .await
                .map(|_| ())
        })
    }

    /// A screen is posted as a message; a form opens as a modal on the
    /// press, or, when it is more than a modal holds or is redrawn as you
    /// pick, is posted as a message of menus.
    fn open_view<'a>(&'a self, trigger: &'a str, view: &'a Value) -> Fut<'a, Option<String>> {
        Box::pin(async move {
            let on = self.pending_message(trigger);
            let root = on.filter(|m| self.0.screens.borrow().contains(m));
            if !rules::is_form(view) {
                self.ack(trigger).await;
                let (content, rows) = self.screen_message(view);
                let id = self.send(&content, rows, None, true).await?;
                let mut screens = self.0.screens.borrow_mut();
                screens.push(id.clone());
                let over = screens.len().saturating_sub(200);
                screens.drain(..over);
                return Ok(Some(id));
            }
            let fields = rules::fields_of(view);
            if rules::fits_modal(&fields) {
                let modal = Modal {
                    view: view.clone(),
                    fixed: HashMap::new(),
                    root,
                    message: None,
                    sends: true,
                };
                self.open_modal(trigger, view, &fields, modal).await?;
                return Ok(None);
            }
            self.ack(trigger).await;
            let form = FormMessage {
                view: view.clone(),
                chosen: DiscordChat::form_values(view),
                root,
            };
            let (content, rows) = self.form_message(&form);
            let id = self.send(&content, rows, None, true).await?;
            self.0.forms.borrow_mut().insert(id.clone(), form);
            Ok(Some(id))
        })
    }

    fn push_view<'a>(&'a self, trigger: &'a str, view: &'a Value) -> Fut<'a, ()> {
        Box::pin(async move { self.open_view(trigger, view).await.map(|_| ()) })
    }

    /// A screen is edited in place. A form message whose menu was just
    /// picked (Skill's) opens the modal for the rest of its fields.
    fn update_view<'a>(
        &'a self,
        id: &'a str,
        view: &'a Value,
        _hash: Option<&'a str>,
    ) -> Fut<'a, ()> {
        Box::pin(async move {
            let form = self.0.forms.borrow().get(id).cloned();
            if let Some(form) = form {
                let fields = rules::fields_of(view);
                let (picked, rest): (Vec<Field>, Vec<Field>) =
                    fields.into_iter().partition(|f| f.dispatch);
                let trigger = self.pending_on(id);
                if let Some(trigger) = trigger
                    && !picked.is_empty()
                    && rules::fits_modal(&rest)
                {
                    let fixed = picked.iter().map(|f| (f.id.clone(), f.initial())).collect();
                    let modal = Modal {
                        view: view.clone(),
                        fixed,
                        root: form.root.clone(),
                        message: Some(id.to_string()),
                        sends: true,
                    };
                    return self.open_modal(&trigger, view, &rest, modal).await;
                }
                let form = FormMessage {
                    view: view.clone(),
                    chosen: DiscordChat::form_values(view),
                    root: form.root,
                };
                let (content, rows) = self.form_message(&form);
                self.0.forms.borrow_mut().insert(id.to_string(), form);
                return self.edit(id, &content, rows).await;
            }
            let (content, rows) = self.screen_message(view);
            self.edit(id, &content, rows).await
        })
    }

    /// Discord holds no message for later.
    fn schedule<'a>(&'a self, _text: &'a str, _post_at: i64) -> Fut<'a, String> {
        Box::pin(async { Ok(String::new()) })
    }

    fn scheduled(&self) -> Fut<'_, Vec<String>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn unschedule<'a>(&'a self, _id: &'a str) -> Fut<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn forget_thread(&self, ts: &str) {
        self.0
            .kept
            .borrow_mut()
            .threads
            .retain(|id, (thread, _)| id != ts && thread != ts);
        self.save();
    }

    fn message_url(&self, dm: &Dm, _thread: &str, at: &str) -> Option<String> {
        Some(format!("{}{}/{at}", dm.url.as_deref()?, dm.channel))
    }

    fn home_url(&self, dm: &Dm) -> Option<String> {
        let home = self.0.kept.borrow().home.clone()?;
        Some(format!("{}{}/{home}", dm.url.as_deref()?, dm.channel))
    }
}

/// Whether message `a` came after `b`: by their IDs when both are
/// Discord's, which order messages made within the same millisecond, else
/// by their times.
fn newer(a: &str, b: &str) -> bool {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(a), Ok(b))
            if rules::is_snowflake(&a.to_string()) && rules::is_snowflake(&b.to_string()) =>
        {
            a > b
        }
        _ => number(a) > number(b),
    }
}

/// An emoji as a path segment: its UTF-8 bytes escaped.
fn escape(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}
