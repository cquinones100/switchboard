//! The sidebar's right click (new with the Rust port, after mission
//! control's in src/mission/serve.rs): a menu by the click for what is
//! under it. On a tab, the tab's menu as mission control's Tabs view opens
//! it on its card, with peeking at it first and chosen, since a right click
//! peeked before it opened a menu; on a notification, its own menu, as the
//! Notifications window opens it; anywhere else, the sidebar's own menu:
//! a new tab, the repository filter, the notifications, with folding or
//! unfolding first on a group's heading or the notifications'. It follows Settings' "Click in menus", off
//! leaving a right click on a tab the peek it was.

use super::super::model::Stage;
use super::super::render::Owner;
use crate::extras::Command as Extra;
use crate::mux::TabId;
use crate::mux::keys::Action;
use crate::mux::server::{ClientId, Server};
use crate::settings;

/// What the sidebar's own menu does.
enum Act {
    Fold(Stage, bool),
    FoldNotices(bool),
    Run(Action),
    DismissAll,
}

impl Server {
    /// A right press in the sidebar, on `owner` (none on a blank row or the
    /// line under the tabs).
    pub(super) fn sidebar_right_press(
        &mut self,
        client: ClientId,
        session: &str,
        owner: Option<Owner>,
    ) {
        if !settings::is_on("menu_mouse") {
            if let Some(Owner::Tab(t)) = owner {
                self.sidebar_peek(client, t);
            }
            return;
        }
        match owner {
            Some(Owner::Tab(t)) => self.sidebar_tab_menu(client, t),
            Some(notice @ Owner::Notice { .. }) => self.sidebar_notice_menu(client, &notice),
            other => self.sidebar_own_menu(client, session, other),
        }
    }

    fn sidebar_tab_menu(&mut self, client: ClientId, tab: TabId) {
        let Some(model) = self.sidebar_all_tabs().into_iter().find(|t| t.id == tab) else {
            return;
        };
        self.tab_menu(client, &model, None, true);
    }

    /// A notification's menu, the one the Notifications window and mission
    /// control's view open for it; a row standing for several of a kind
    /// (the landings) opens the newest's, which its line shows
    /// (`notices::summary`), as `sidebar_open_notice` leads to it.
    fn sidebar_notice_menu(&mut self, client: ClientId, owner: &Owner) {
        let Owner::Notice { source, .. } = owner else {
            return;
        };
        let shows_the_newest = matches!(source.as_str(), "autolanded" | "landed");
        let entries = self.notice_entries();
        let found = if shows_the_newest {
            entries.iter().rfind(|(o, _)| o.same_row(owner))
        } else {
            entries.iter().find(|(o, _)| o == owner)
        };
        let Some((Owner::Notice { source, .. }, item)) = found else {
            return;
        };
        let mut item = item.clone();
        item.id = format!("{source}\t{}", item.id);
        self.notice_menu(client, item);
    }

    /// The sidebar's own menu, each option what its palette command does;
    /// on a group's heading or the notifications', folding or unfolding it
    /// first, as a click on it does.
    fn sidebar_own_menu(&mut self, client: ClientId, session: &str, on: Option<Owner>) {
        let mut list: Vec<(String, Act)> = Vec::new();
        match on {
            Some(group @ Owner::Group(stage)) => {
                let folded = self.sidebar_stops(session).contains(&group);
                let verb = if folded { "Unfold" } else { "Fold" };
                list.push((
                    format!("{verb} {}", stage.title()),
                    Act::Fold(stage, !folded),
                ));
            }
            Some(Owner::Notices) => {
                let folded = self
                    .sidebar
                    .sides
                    .get(session)
                    .is_some_and(|s| s.notices_folded);
                list.push(if folded {
                    ("Unfold the notifications".into(), Act::FoldNotices(false))
                } else {
                    ("Fold the notifications".into(), Act::FoldNotices(true))
                });
            }
            _ => {}
        }
        list.push(("New tab".into(), Act::Run(Action::NewTab)));
        if settings::is_on("sidebar_repo_filter") {
            list.push((
                "Filter the sidebar to a repository…".into(),
                Act::Run(Action::Extra(Extra::SidebarRepo)),
            ));
            if self.sidebar_repo(session).is_some() {
                list.push((
                    "Show every repository".into(),
                    Act::Run(Action::Extra(Extra::SidebarAll)),
                ));
            }
        }
        if !self.sidebar_notices().is_empty() {
            list.push((
                "Go to the notifications".into(),
                Act::Run(Action::Extra(Extra::Notifications)),
            ));
            list.push(("Dismiss every notification shown".into(), Act::DismissAll));
        }
        let labels: Vec<&str> = list.iter().map(|(l, _)| l.as_str()).collect();
        let menu = crate::ui::Menu::of("Sidebar", &labels);
        let session = session.to_string();
        self.choose(client, menu, move |server, client, chose| {
            let Some((_, act)) = chose.and_then(|i| list.into_iter().nth(i)) else {
                return;
            };
            match act {
                Act::Fold(stage, fold) => {
                    if !fold {
                        server.sidebar_enter(&session, Some(Owner::Group(stage)));
                    }
                    server.sidebar_fold(&session, stage, fold);
                }
                Act::FoldNotices(fold) => server.sidebar_fold_notices(&session, fold),
                Act::Run(action) => server.act(client, action),
                Act::DismissAll => {
                    match crate::mission::choices::dismiss_all(&server.sidebar_notice_items()) {
                        Some(all) => server.mission_do(client, all, ""),
                        None => server.say(Some(client), "Nothing in the sidebar to dismiss."),
                    }
                }
            }
        });
    }
}
