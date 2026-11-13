//! The sidebar's right click (new with the Rust port, after mission
//! control's in src/mission/serve.rs): a menu by the click for what is
//! under it. On a tab, the tab's menu as mission control's Tabs view opens
//! it on its card, with peeking at it first and chosen, since a right click
//! peeked before it opened a menu; on a notification, its own menu, as the
//! Notifications window opens it; anywhere else, the sidebar's own menu:
//! a new tab, the repository filter, the notifications, with folding or
//! unfolding first on a group's heading or the notifications'. It follows Settings' "Click in menus", off
//! leaving a right click on a tab the peek it was. `m` in the sidebar and
//! the context-menu key (S-F10) open the same menu for the cursor's row,
//! beside it, whatever that setting says, since a key is no click. On ←
//! or →, the whole trail of tabs visited (board 58), any step one choice
//! away; on ⊞, mission control's views by group (board 60).

use super::super::model::{self, Rules, Stage};
use super::super::render::Owner;
use crate::extras::Command as Extra;
use crate::mux::TabId;
use crate::mux::keys::Action;
use crate::mux::server::{ClientId, Server};
use crate::settings;
use crate::sidebar::text::{self, width};

use crate::ui::kit::Mark;

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
        self.sidebar_menu(client, session, owner);
    }

    /// `m` or the context-menu key with the focus in the sidebar: the menu
    /// a right click on the cursor's row would open, beside that row. A
    /// cursor on a tab the sidebar doesn't list (filtered out, or under a
    /// folded group) is on no row, so the sidebar's own menu opens.
    pub(super) fn sidebar_menu_key(&mut self, client: ClientId, session: &str) {
        let Some(here) = self.session(session).map(|s| s.current) else {
            return;
        };
        // In select mode, the selection's menu, as m opens there.
        let selecting = settings::is_on("sidebar_select")
            && self
                .sidebar
                .sides
                .get(session)
                .is_some_and(|s| s.select.is_some());
        if selecting {
            self.sidebar_select_menu(client, session);
            return;
        }
        let cursor = self
            .sidebar
            .sides
            .get(session)
            .and_then(|s| s.cursor.clone())
            .unwrap_or(Owner::Tab(here));
        let owner = match cursor {
            Owner::Tab(t) if !self.sidebar_order(session).contains(&t) => None,
            other => Some(other),
        };
        let row = self.sidebar.drawn.get(&client).and_then(|d| {
            let on = owner.as_ref()?;
            let i = d
                .owners
                .iter()
                .position(|o| o.as_ref().is_some_and(|o| o.same_row(on)))?;
            Some((d.area.x, d.area.y + u16::try_from(i).ok()?))
        });
        let pressed = self.clients.get_mut(&client).map(|c| &mut c.pressed_at);
        let Some(pressed) = pressed else {
            return;
        };
        let before = std::mem::replace(pressed, row);
        self.sidebar_menu(client, session, owner);
        if let Some(c) = self.clients.get_mut(&client) {
            c.pressed_at = before;
        }
    }

    fn sidebar_menu(&mut self, client: ClientId, session: &str, owner: Option<Owner>) {
        match owner {
            Some(Owner::Tab(t)) => self.sidebar_tab_menu(client, t),
            Some(notice @ Owner::Notice { .. }) => self.sidebar_notice_menu(client, &notice),
            Some(Owner::Back | Owner::Forward) => self.sidebar_trail_menu(client, session),
            Some(Owner::Mission) => self.sidebar_views_menu(client),
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

    /// The whole trail of tabs a session visited, as board 58 draws it:
    /// the forward ones furthest first, the tab you are on marked "you are
    /// here", then the back ones nearest first, each its mark, number and
    /// name with its steps away at the right; a choice goes straight there,
    /// as that many steps would.
    pub(crate) fn sidebar_trail_menu(&mut self, client: ClientId, session: &str) {
        let Some((current, (back, forward))) = self
            .session(session)
            .map(|s| (s.current, s.trail.steps(s.current)))
        else {
            return;
        };
        let Some(s) = self.session(session) else {
            return;
        };
        let r = Rules::now();
        // How wide a tab's mark, number and name may be, so the widest row,
        // with "you are here" after it, fits the menu on this screen.
        const HERE: &str = "you are here";
        let cols = self
            .clients
            .get(&client)
            .map_or(80, |c| usize::from(c.cols));
        let room = crate::ui::menu::label_room(cols).saturating_sub(3 + width(HERE));
        let name = |t: TabId| {
            self.trail_model(s, t).map(|m| {
                let mark = Mark::of_badge(model::badge_state(&m, &r)).look().0;
                // Cut so its steps away stay in the menu's width
                // whatever the tab's title.
                text::cut(&format!("{mark} {} {}", m.index, m.name), room)
            })
        };
        // Each row: its words, its steps away, and where a choice goes
        // (back or not, and how many steps).
        type Row = (String, String, Option<(bool, usize)>);
        let mut rows: Vec<Row> = Vec::new();
        for (t, n) in forward.iter().rev() {
            if let Some(words) = name(*t) {
                rows.push((words, format!("→ {n}"), Some((false, *n))));
            }
        }
        let here = rows.len();
        rows.push((name(current).unwrap_or_default(), HERE.into(), None));
        for (t, n) in &back {
            if let Some(words) = name(*t) {
                rows.push((words, format!("← {n}"), Some((true, *n))));
            }
        }
        let left = rows.iter().map(|(w, _, _)| width(w)).max().unwrap_or(0);
        let labels: Vec<String> = rows
            .iter()
            .map(|(w, steps, _)| format!("{w}{}   {steps}", " ".repeat(left - width(w))))
            .collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        // The step back is chosen as it opens, else the tab you are on.
        let at = if back.is_empty() { here } else { here + 1 };
        let menu = crate::ui::Menu::of("Back and forward", &labels).at(at);
        let session = session.to_string();
        self.choose(client, menu, move |server, _, chose| {
            if let Some((back, n)) = chose.and_then(|i| rows.get(i)).and_then(|(_, _, go)| *go) {
                // As M-{ and M-} do: out of the sidebar, onto the tab.
                server.sidebar_leave(&session);
                server.trail_step(&session, back, n);
            }
        });
    }

    /// Mission control's views by group, as board 60 draws them: each
    /// view that is on under Work, Code, Day and System, the Tabs view
    /// with the tabs that need you; a choice opens mission control on it.
    pub(crate) fn sidebar_views_menu(&mut self, client: ClientId) {
        let needs = self
            .clients
            .get(&client)
            .map(|c| c.session.clone())
            .map_or(0, |s| {
                model::needs_you(&self.sidebar_tabs(&s), &Rules::now()).len()
            });
        let views = self.mission_view_list();
        let mut rows: Vec<(String, String, Option<String>)> = Vec::new();
        let mut group = None;
        for (g, title, slug, count) in views {
            use crate::mission::views::Group;
            let head = if group == Some(g) { "" } else { g.name() };
            group = Some(g);
            // The tabs that need you, and how many the views of your code
            // and your day hold.
            let fact = match slug.as_deref() {
                None if needs > 0 => format!("{needs} need you"),
                Some(_) if count > 0 && matches!(g, Group::Code | Group::Day) => count.to_string(),
                _ => String::new(),
            };
            rows.push((format!("{head:<7} {title}"), fact, slug));
        }
        let left = rows.iter().map(|(w, _, _)| width(w)).max().unwrap_or(0);
        let labels: Vec<String> = rows
            .iter()
            .map(|(w, fact, _)| {
                if fact.is_empty() {
                    w.clone()
                } else {
                    format!("{w}{}   {fact}", " ".repeat(left - width(w)))
                }
            })
            .collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        let menu = crate::ui::Menu::of("Mission control", &labels);
        self.choose(client, menu, move |server, client, chose| {
            let Some((_, _, slug)) = chose.and_then(|i| rows.into_iter().nth(i)) else {
                return;
            };
            let how = match slug {
                None => crate::mission::Opening::Tabs,
                Some(s) if s == "settings" => crate::mission::Opening::Settings,
                Some(s) => crate::mission::Opening::View(s),
            };
            if let Some(session) = server.clients.get(&client).map(|c| c.session.clone()) {
                server.sidebar_leave(&session);
            }
            server.open_mission(client, how);
        });
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
