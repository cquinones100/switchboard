//! Mission control's views on the phone, on the server: the views the
//! phone's stream carries (`phone_views`), built from mission control's
//! own (`mission_views`, `item_choices`, `view_acts::of`) with no picker,
//! neither view kept and a `NoticeLook` of the phone's own, and the two
//! commands its taps send, `phone-item <view> <id> <label>` and
//! `phone-view <view> <label>`, which build the menu again, find the
//! option by its label and run it with no client (`mission_run`,
//! `whole_run`), answering with what it says, which the phone shows as a
//! toast. One the phone leaves out (`views::kind_of`), or no longer
//! offered, is refused. Settings' `phone_views` off sends no views and
//! refuses both. `phone::views` sorts what they give; this only reaches
//! the server for it. New with the phone's panel.

use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::oneshot;

use crate::items::Item;
use crate::mission::choices::{self, Do};
use crate::mission::view_acts;
use crate::mission::views::{View, ViewId};
use crate::mission::{Finish, Notice, NoticeLook, Ran, with_check};
use crate::mux::input::{Key, KeyCode, Mods};
use crate::mux::server::{Answer, Event, Opened, Reply, Server};
use crate::settings;

use super::serve::PhoneEvent;
use super::views::{self, Kind};

/// The longest the views the stream carries are kept before they are
/// worked out again, so a burst of changes builds them once.
pub(crate) const VIEWS_EVERY: Duration = Duration::from_secs(1);

/// What the server says when a tap names something that has gone.
const GONE: &str = "That has changed since the menu opened.";

/// The server's own `Finish` for an option run with no client, handed back
/// to it with the phone's reply once its work off the task is done.
pub struct Finished {
    pub finish: Finish,
    pub reply: oneshot::Sender<Answer>,
}

impl std::fmt::Debug for Finished {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Finished")
    }
}

/// An option of a menu the phone offers, found by its label.
struct Offered {
    label: String,
    kind: Kind,
}

impl Server {
    /// The views mission control shows that the phone lists, in the
    /// phone's order: `None` while Settings' `phone_views` is off.
    fn phone_shown_views(&mut self) -> Option<Vec<View>> {
        if !settings::is_on("phone_views") {
            return None;
        }
        let look = NoticeLook::every(views::SCREENFUL);
        let all = self.mission_views(false, false, false, &look);
        Some(
            views::SHOWN
                .iter()
                .filter_map(|id| all.iter().find(|v| v.id == *id).cloned())
                .collect(),
        )
    }

    /// The tab's Claude pane, for going to its channel.
    fn phone_pane_of(&self) -> impl Fn(crate::mux::TabId) -> Option<String> + use<> {
        let panes: Vec<(crate::mux::TabId, String)> = self
            .sidebar_all_tabs()
            .into_iter()
            .filter_map(|t| Some((t.id, t.claude_pane?.to_string())))
            .collect();
        move |tab| {
            panes
                .iter()
                .find(|(t, _)| *t == tab)
                .map(|(_, p)| p.clone())
        }
    }

    /// The item a menu is opened for: CI's on its first failed check, as
    /// mission control's opens on the one chosen.
    fn phone_menu_item(view: &View, item: &Item) -> Item {
        if view.id == ViewId::Ci {
            with_check(item.clone(), 0)
        } else {
            item.clone()
        }
    }

    /// What an item's menu offers on the phone, in mission control's order.
    fn phone_item_options(&self, view: &View, item: &Item) -> Vec<(Offered, Do)> {
        let pane_of = self.phone_pane_of();
        self.phone_menu(view, item)
            .0
            .into_iter()
            .filter_map(|c| {
                let kind = views::kind_of(&c.act, &c.label, &pane_of)?;
                Some((
                    Offered {
                        label: c.label,
                        kind,
                    },
                    c.act,
                ))
            })
            .collect()
    }

    /// An item's mission control menu, and the notification a pick from it
    /// puts away: a notification's as mission control opens its row
    /// (`notice_row_menu`), none for a row it opens no menu for.
    fn phone_menu(&self, view: &View, item: &Item) -> (Vec<choices::Choice>, Notice) {
        if view.id == ViewId::Notifications {
            return self.notice_row_menu(item).unwrap_or_default();
        }
        let menu = Self::phone_menu_item(view, item);
        (self.item_choices(&view.id, &menu, &view.data.items), None)
    }

    /// What a view's own menu offers on the phone.
    fn phone_view_options(&self, view: &View) -> Vec<(Offered, view_acts::Whole)> {
        let facts = view_acts::Facts {
            github: settings::is_on("github"),
            linear: settings::is_on("linear"),
            ..view_acts::Facts::default()
        };
        let items = &view.data.items;
        view_acts::of(Some(&view.id), items, items, &facts)
            .into_iter()
            .filter_map(|(label, act)| {
                let kind = views::whole_kind(&act, &label)?;
                Some((Offered { label, kind }, act))
            })
            .collect()
    }

    /// The views the phone's stream carries, worked out again at most
    /// every `VIEWS_EVERY`: each with its line, what needs you counted and
    /// marked as its first, its groups of items with their options, its
    /// own options, and its words for having none. `None` while Settings'
    /// `phone_views` is off.
    pub(crate) fn phone_views(&mut self) -> Option<Value> {
        if let Some((at, kept)) = &self.phone.views
            && at.elapsed() < VIEWS_EVERY
            && settings::is_on("phone_views")
        {
            return Some(kept.clone());
        }
        let shown = self.phone_shown_views()?;
        let out: Vec<Value> = shown
            .iter()
            .map(|v| {
                let options = |item: &Item| {
                    self.phone_item_options(v, item)
                        .iter()
                        .map(|(o, _)| views::option_json(&o.label, &o.kind))
                        .collect()
                };
                let acts = self
                    .phone_view_options(v)
                    .iter()
                    .map(|(o, _)| views::option_json(&o.label, &o.kind))
                    .collect();
                views::view_json(v, &options, acts)
            })
            .collect();
        let value = Value::Array(out);
        self.phone.views = Some((Instant::now(), value.clone()));
        Some(value)
    }

    /// `phone-item <view> <id> <label>` and `phone-view <view> <label>`:
    /// a tap on an option of an item's menu or of a view's own, from the
    /// phone, run on the computer and answered with what it says once it
    /// is done.
    pub(crate) fn phone_option_asked(
        &mut self,
        args: &[String],
        by_hand: bool,
        reply: oneshot::Sender<Answer>,
    ) {
        let answer = |reply: oneshot::Sender<Answer>, r: Reply| {
            let _ = reply.send(Answer::Reply(r));
        };
        let (view_key, id, label) = match args {
            [c, v, id, label] if c == "phone-item" => (v, Some(id), label),
            [c, v, label] if c == "phone-view" => (v, None, label),
            _ => {
                return answer(
                    reply,
                    Reply::fail("phone-item takes <view> <id> <label>; phone-view <view> <label>"),
                );
            }
        };
        // Only the phone app sends them, which asks first where mission
        // control does: typed in a pane, they would be a way round that
        // question, as `answer` would be round a prompt.
        if by_hand {
            return answer(
                reply,
                Reply::fail("phone-item and phone-view come from the phone app alone"),
            );
        }
        if !settings::is_on("phone") {
            return answer(reply, Reply::fail("The phone app is off."));
        }
        let Some(shown) = self.phone_shown_views() else {
            return answer(reply, Reply::fail("Mission control on the phone is off."));
        };
        let Some(view) = shown.into_iter().find(|v| v.key() == *view_key) else {
            return answer(reply, Reply::fail(GONE));
        };
        let ran = match id {
            Some(id) => {
                let Some(item) = view
                    .data
                    .items
                    .iter()
                    .find(|i| i.id == *id && !crate::mission::views::is_header(i))
                    .cloned()
                else {
                    return answer(reply, Reply::fail(GONE));
                };
                let options = self.phone_item_options(&view, &item);
                let Some((offered, act)) = options.into_iter().find(|(o, _)| o.label == *label)
                else {
                    return answer(
                        reply,
                        Reply::fail(phone_refusal(&self.phone_menu(&view, &item).0, label)),
                    );
                };
                // A notification is put away after what is picked in its
                // menu, as mission control's does, and a Slack message
                // seen: for what the phone does itself too (going to a
                // tab, opening or copying a link), which it tells the
                // server of, so only that is done here.
                let notice = self.phone_menu(&view, &item).1;
                let seen = choices::is_slack_message(&view.id, &item).then(|| item.clone());
                let after = self.pick_before(&act, label, seen.as_ref(), &notice);
                let ran = if matches!(offered.kind, Kind::Run(_)) {
                    self.phone_run(act, label)
                } else {
                    Ok(Ran::Said(String::new()))
                };
                self.pick_after(notice, after);
                ran
            }
            None => {
                let options = self.phone_view_options(&view);
                let Some((offered, act)) = options.into_iter().find(|(o, _)| o.label == *label)
                else {
                    return answer(reply, Reply::fail(GONE));
                };
                if !matches!(offered.kind, Kind::Run(_)) {
                    return answer(reply, Reply::fail("The phone does that one itself."));
                }
                self.whole_run(act)
                    .map_err(|_| "The phone can't do that one.".to_string())
            }
        };
        eprintln!("switchboard server: from your phone: {view_key}: {label}");
        let ran = match ran {
            Ok(ran) => ran,
            Err(why) => return answer(reply, Reply::fail(why)),
        };
        self.phone_answer_ran(ran, reply);
    }

    /// An item's option run from the phone: work started in a tab is
    /// typed into its Claude, or opened in a tab of its own, without
    /// bringing either to the front; anything else as mission control
    /// runs it with no client.
    fn phone_run(&mut self, act: Do, label: &str) -> Result<Ran, String> {
        match act {
            Do::TypeInto { tab, pane, text } => {
                if !self.panes.contains_key(&pane) {
                    return Err(GONE.into());
                }
                self.type_into_claude(
                    pane,
                    vec![
                        crate::mission::answer::Press::Text(text.clone()),
                        crate::mission::answer::Press::Key(Key::new(KeyCode::Enter, Mods::NONE)),
                    ],
                );
                Ok(Ran::Said(format!(
                    "Typed {text} into tab {}.",
                    self.tab_number(tab)
                )))
            }
            Do::StartIn { dir, work } => {
                let session = self
                    .latest_client()
                    .and_then(|c| self.clients.get(&c))
                    .map(|c| c.session.clone())
                    .or_else(|| self.sessions.first().map(|s| s.name.clone()))
                    .ok_or("No session to open a tab in.")?;
                let dir = if dir.is_dir() {
                    dir
                } else {
                    crate::paths::home()
                };
                match self.new_tab(
                    &session,
                    Some(dir),
                    Some(work.command()),
                    None,
                    Opened::Background,
                ) {
                    Ok((tab, _)) => Ok(Ran::Said(format!(
                        "Opened tab {} for it.",
                        self.tab_number(tab)
                    ))),
                    Err(e) => Err(format!("Couldn't open a tab: {e}")),
                }
            }
            act => self
                .mission_run(act)
                .map_err(|_| format!("The phone can't do \"{label}\".")),
        }
    }

    /// What an option run from the phone gave, answered: at once, once its
    /// work is done, or once the server has finished it.
    fn phone_answer_ran(&mut self, ran: Ran, reply: oneshot::Sender<Answer>) {
        let said = |text: Option<String>| {
            let text = text
                .filter(|t| !t.trim().is_empty())
                .unwrap_or("Done.".into());
            Answer::Reply(Reply::ok(text))
        };
        match ran {
            Ran::Said(text) => {
                let _ = reply.send(said(Some(text)));
            }
            Ran::Later(work) => {
                if tokio::runtime::Handle::try_current().is_err() {
                    let _ = reply.send(said(None));
                    return;
                }
                tokio::spawn(async move {
                    let _ = reply.send(said(work.await));
                });
            }
            Ran::OffTask(work) => {
                if tokio::runtime::Handle::try_current().is_err() {
                    let _ = reply.send(said(None));
                    return;
                }
                let tx = self.tx.clone();
                tokio::task::spawn_blocking(move || {
                    let finish = work();
                    let _ = tx.send(Event::Phone(PhoneEvent::Finished(Finished {
                        finish,
                        reply,
                    })));
                });
            }
        }
    }

    /// The server's part of an option run from the phone, once its work
    /// off the task is done, answered with what it says.
    pub(crate) fn phone_finished(&mut self, done: Finished) {
        let text = (done.finish)(self);
        let text = if text.trim().is_empty() {
            "Done.".into()
        } else {
            text
        };
        let _ = done.reply.send(Answer::Reply(Reply::ok(text)));
    }
}

/// Why an option the phone sent isn't run: one mission control offers that
/// the phone leaves out, or one no longer offered at all.
fn phone_refusal(offered: &[choices::Choice], label: &str) -> String {
    if offered.iter().any(|c| c.label == label) {
        "The phone leaves that one to the computer's screen.".into()
    } else {
        GONE.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_left_out_option_is_refused_by_name() {
        let offered = vec![choices::Choice {
            key: 'v',
            label: "Read the PR here, in a pane beside".into(),
            act: Do::ReadHere("https://x".into()),
        }];
        assert_eq!(
            phone_refusal(&offered, "Read the PR here, in a pane beside"),
            "The phone leaves that one to the computer's screen."
        );
        assert_eq!(phone_refusal(&offered, "Something else"), GONE);
    }
}
