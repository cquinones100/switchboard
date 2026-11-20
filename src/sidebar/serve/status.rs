//! A tab's status set by hand (new with the Rust port, board 158): "Set
//! its status…" in a tab's menu and M-p's "Set this tab's status…" open a
//! menu of every group a tab can be in, by the sidebar's names and in its
//! order. What you choose is kept as the tab's `@tab_status` with the group
//! Switchboard read as you chose it, saved with the layout as every tab
//! option is, and files the tab there (`model::by_hand`) until
//! Switchboard's own reading moves, when `tab_status_settle` drops it, so a
//! status you set never hides the tab's next real move.

use super::super::model::{self, Rules, Stage};
use crate::mux::TabId;
use crate::mux::server::{ClientId, Server};
use crate::settings;
use crate::ui::menu::{Menu, MenuOption};

/// The tab option a status set by hand is kept in.
pub const OPTION: &str = "@tab_status";
/// How long after the server starts a status kept from before may wait
/// for Switchboard's reading to come back to the one it was set with.
const SETTLE_GRACE: std::time::Duration = std::time::Duration::from_secs(15 * 60);

impl Server {
    /// The menu of the groups a tab can be in: "Let Switchboard decide"
    /// first while a status you set holds, naming the group Switchboard
    /// would file it in, and the group it is in marked.
    pub fn tab_status_menu(&mut self, client: ClientId, tab: TabId) {
        if !settings::is_on("tab_status_by_hand") {
            self.say(
                Some(client),
                "Setting a tab's status by hand is off in Settings (tab_status_by_hand).",
            );
            return;
        }
        let Some(t) = self.sidebar_all_tabs().into_iter().find(|t| t.id == tab) else {
            return;
        };
        let r = Rules::now();
        // A pin or a reminder wins over any status set, so one set now
        // would never show.
        let reminded = model::reminder_due(&t, &r) || model::reminder_later(&t, &r);
        if t.pinned || reminded {
            let which = if t.pinned {
                "Unpin it (M-f)"
            } else {
                "Clear its reminder"
            };
            self.say(
                Some(client),
                &format!(
                    "{which} to set its status: a pin or a reminder keeps the tab where it is."
                ),
            );
            return;
        }
        let held = model::by_hand(&t, &r);
        let own = model::own_stage(&t, &r);
        let mut options = Vec::new();
        let mut stages: Vec<Option<Stage>> = Vec::new();
        if held.is_some() {
            options.push(MenuOption::new("Let Switchboard decide").aside(own.title()));
            stages.push(None);
        }
        for stage in model::SETTABLE {
            let mark = match held {
                Some(h) if h == stage => "✓ set by you",
                None if own == stage => "✓",
                _ => "",
            };
            options.push(MenuOption::new(stage.title()).aside(mark));
            stages.push(Some(stage));
        }
        let menu = Menu::new("Set its status", options);
        self.choose(client, menu, move |server, _, chose| {
            if let Some(stage) = chose.and_then(|i| stages.get(i).copied()) {
                server.set_tab_status(tab, stage);
            }
        });
    }

    /// Keeps `chosen` as the tab's status, with what Switchboard reads now;
    /// `None` ("Let Switchboard decide") or the group it is in already
    /// clears it.
    pub(crate) fn set_tab_status(&mut self, tab: TabId, chosen: Option<Stage>) {
        let Some(t) = self.sidebar_all_tabs().into_iter().find(|t| t.id == tab) else {
            return;
        };
        let kept = model::choose(&t, chosen, &Rules::now());
        if let Some(t) = self.tabs.get_mut(&tab) {
            match kept {
                Some((chosen, read)) => {
                    self.sidebar.status_seen.insert(tab);
                    t.opts
                        .insert(OPTION.into(), model::by_hand_value(chosen, read));
                }
                None => {
                    t.opts.remove(OPTION);
                }
            }
        }
        // A sidebar held still while you browse moves the tab you just
        // filed, since you moved it yourself.
        if let Some(session) = self.session_of_tab(tab).map(|s| s.name.clone())
            && self.sidebar.side(&session).frozen.is_some()
        {
            let now = self.sidebar_frozen_now(&session, &Rules::now());
            self.sidebar.side(&session).frozen = Some(now);
        }
        self.redraw_tab(tab);
        self.mission_redraw();
    }

    /// On each pass: drops every status set by hand whose tab Switchboard
    /// now reads otherwise than as it was set, and all of them with Settings'
    /// `tab_status_by_hand` off. A status not yet seen holding this run (one
    /// kept through a restart or an upgrade) is dropped only once seen to
    /// hold and then moved, or after `SETTLE_GRACE`, since until its
    /// Claude has resumed and its PR been looked up again Switchboard's
    /// reading is still loading, not changed.
    pub(super) fn tab_status_settle(&mut self) {
        let on = settings::is_on("tab_status_by_hand");
        let r = Rules::now();
        let since = *self
            .sidebar
            .status_since
            .get_or_insert_with(std::time::Instant::now);
        let grace_over = since.elapsed() >= SETTLE_GRACE;
        let mut gone = Vec::new();
        for t in self.sidebar_all_tabs() {
            let Some((_, read)) = t.by_hand else {
                continue;
            };
            if model::own_stage(&t, &r) == read {
                self.sidebar.status_seen.insert(t.id);
            } else if grace_over || self.sidebar.status_seen.contains(&t.id) {
                gone.push(t.id);
            }
        }
        // Off, or a value no build reads (a stage renamed since).
        gone.extend(
            self.tabs
                .iter()
                .filter(|(_, t)| {
                    t.opts
                        .get(OPTION)
                        .is_some_and(|v| !on || model::by_hand_of(v).is_none())
                })
                .map(|(id, _)| *id),
        );
        for tab in gone {
            self.sidebar.status_seen.remove(&tab);
            if let Some(t) = self.tabs.get_mut(&tab) {
                t.opts.remove(OPTION);
            }
            self.redraw_tab(tab);
        }
    }
}
