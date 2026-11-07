//! The Notifications window on the server (lib/sidebar/daemon.js's
//! `noticesWindow`, `showNotices`, `noticesKey` and the notices client's
//! drawing): a session's tab of its own, marked `@notices_window`, made the
//! first time it is wanted and kept last among the session's tabs, its one
//! pane drawn over with the list (`notices::list_lines`) and its keys taken
//! here rather than given to the program waiting in it.
//!
//! Its cursor and the sidebar's go together: the sidebar's cursor moving
//! onto a notification brings the window to the front on that one, and
//! moving in the window moves the sidebar's cursor with it. j and k choose,
//! Enter (or l) opens the chosen one's menu over it (mission control's
//! `notice_choices`, as the Notifications view offers them), and Esc, q or
//! h go back to the tab it was brought in front of. With Tab moving focus,
//! Tab and Shift-Tab go round "‹ tabs" and the notifications, Enter pressing
//! the one focused. The wheel chooses, and a click chooses one, opens the
//! one chosen, or goes back from "‹ tabs".

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::super::notices::{self, Look, MARK, Spot};
use super::super::render::Owner;
use crate::items::Item;
use crate::mux::input::{Key, KeyCode, Mods, Mouse, MouseButton, MouseKind};
use crate::mux::server::{ClientId, Server};
use crate::mux::{PaneId, TabId};
use crate::{paths, settings};

/// The pane option that marks the pane the list is drawn over.
const PANE: &str = "@notices_pane";

impl Server {
    /// Whether a tab is a session's Notifications window.
    pub fn is_notices_tab(&self, tab: TabId) -> bool {
        self.tabs
            .get(&tab)
            .is_some_and(|t| t.opts.get(MARK).is_some_and(|v| v == "1"))
    }

    /// A session's Notifications window, when it has one.
    pub fn notices_tab(&self, session: &str) -> Option<TabId> {
        self.session(session)?
            .tabs
            .iter()
            .copied()
            .find(|t| self.is_notices_tab(*t))
    }

    /// Whether a pane is the one a Notifications window was made with,
    /// which the server draws over (a shell split off beside it is a pane
    /// like any other).
    pub fn is_notices_pane(&self, pane: PaneId) -> bool {
        self.panes.get(&pane).is_some_and(|s| {
            s.opts.get(PANE).is_some_and(|v| v == "1") && self.is_notices_tab(s.tab)
        })
    }

    /// Keeps a session's Notifications window last among its tabs, so it
    /// never takes a number from a tab opened after it.
    pub fn notices_last(&mut self, session: &str) {
        let Some(tab) = self.notices_tab(session) else {
            return;
        };
        if let Some(s) = self.session_mut(session)
            && s.tabs.last() != Some(&tab)
        {
            s.tabs.retain(|t| *t != tab);
            s.tabs.push(tab);
        }
    }

    /// The notifications as the window lists them, each as the sidebar
    /// names it and its item.
    fn notice_entries(&self) -> Vec<(Owner, Item)> {
        if !settings::is_on("sidebar_notifications") {
            return Vec::new();
        }
        self.notices
            .iter()
            .flat_map(|(source, items)| {
                items.iter().map(move |item| {
                    (
                        Owner::Notice {
                            source: source.clone(),
                            id: item.id.clone(),
                        },
                        item.clone(),
                    )
                })
            })
            .collect()
    }

    /// The chosen notification's place in a session's window.
    fn notice_cursor(&self, session: &str, entries: &[(Owner, Item)]) -> usize {
        self.sidebar
            .windows
            .at
            .get(session)
            .and_then(|o| entries.iter().position(|(e, _)| e == o))
            .unwrap_or(0)
    }

    /// A session's Notifications window, made the first time it is wanted:
    /// a tab named Notifications, last among the session's tabs, whose pane
    /// runs `switchboard notifications`.
    fn notices_window(&mut self, session: &str) -> Option<TabId> {
        if let Some(tab) = self.notices_tab(session) {
            return Some(tab);
        }
        let exe = crate::util::exe()
            .ok()
            .filter(|p| p.exists())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "switchboard".into());
        let command = vec![exe, "notifications".into()];
        let (tab, pane) =
            match self.new_tab(session, Some(paths::home()), Some(command), None, true) {
                Ok(made) => made,
                Err(e) => {
                    let client = self.latest_client_of(session);
                    self.say(client, &format!("Couldn't open the notifications: {e}"));
                    return None;
                }
            };
        if let Some(t) = self.tabs.get_mut(&tab) {
            t.opts.insert(MARK.into(), "1".into());
            t.opts.insert("@tab_name".into(), "Notifications".into());
        }
        if let Some(slot) = self.panes.get_mut(&pane) {
            slot.opts.insert(PANE.into(), "1".into());
        }
        self.notices_last(session);
        self.name_tab(tab);
        Some(tab)
    }

    /// Brings a session's Notifications window to the front on `owner`, as
    /// moving onto a tab brings it (showNotices): the tab it was brought in
    /// front of is kept for Esc. Gives whether there is a window.
    pub fn show_notices(&mut self, session: &str, owner: &Owner) -> bool {
        let Some(here) = self.session(session).map(|s| s.current) else {
            return false;
        };
        let w = &mut self.sidebar.windows;
        w.at.insert(session.to_string(), owner.clone());
        w.back.remove(session);
        if !self.is_notices_tab(here) {
            self.sidebar.windows.from.insert(session.to_string(), here);
        }
        let Some(tab) = self.notices_window(session) else {
            return false;
        };
        if self.session(session).is_some_and(|s| s.current != tab) {
            self.select_tab(session, tab);
        }
        self.dirty_session(session);
        true
    }

    /// Into the window's pane, its keys taking over from the sidebar's, as
    /// Enter on a notification in the sidebar goes into it.
    pub fn notices_enter(&mut self, session: &str, owner: &Owner) {
        if !self.show_notices(session, owner) {
            return;
        }
        let Some(pane) = self
            .notices_tab(session)
            .and_then(|t| self.tabs.get(&t))
            .and_then(|t| {
                t.layout
                    .panes()
                    .into_iter()
                    .find(|p| self.is_notices_pane(*p))
            })
        else {
            return;
        };
        self.focus_pane(pane);
    }

    /// Back to the tab the window was brought in front of (else the first),
    /// the sidebar's cursor on it.
    fn notices_back(&mut self, session: &str) {
        let first = self
            .session(session)
            .and_then(|s| s.tabs.iter().copied().find(|t| !self.is_notices_tab(*t)));
        let from = self.sidebar.windows.from.remove(session);
        let Some(to) = from.filter(|t| self.tabs.contains_key(t)).or(first) else {
            return;
        };
        self.sidebar.windows.back.remove(session);
        if let Some(side) = self.sidebar.sides.get_mut(session)
            && side.focused
        {
            side.cursor = Some(Owner::Tab(to));
        }
        self.select_tab(session, to);
    }

    /// Chooses the `to`th notification in a session's window, the sidebar's
    /// cursor going with it.
    fn notices_choose(&mut self, session: &str, entries: &[(Owner, Item)], to: isize) {
        if entries.is_empty() {
            return;
        }
        let to = to.clamp(0, entries.len() as isize - 1) as usize;
        let owner = entries[to].0.clone();
        self.sidebar
            .windows
            .at
            .insert(session.to_string(), owner.clone());
        self.sidebar.windows.back.remove(session);
        self.sidebar.side(session).cursor = Some(owner);
        self.dirty_session(session);
    }

    /// The chosen notification's menu over the window, as its row's menu in
    /// mission control's Notifications view.
    fn notices_open(&mut self, client: ClientId, entries: &[(Owner, Item)], at: usize) {
        let Some((Owner::Notice { source, .. }, item)) = entries.get(at) else {
            return;
        };
        let mut item = item.clone();
        item.id = format!("{source}\t{}", item.id);
        self.notice_menu(client, item);
    }

    /// A key with the window's pane in front and focused; gives whether the
    /// window took it.
    pub fn notices_key(&mut self, client: ClientId, pane: PaneId, key: Key) -> bool {
        if !self.is_notices_pane(pane) {
            return false;
        }
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return true;
        };
        let entries = self.notice_entries();
        let at = self.notice_cursor(&session, &entries) as isize;
        let back = self.sidebar.windows.back.contains(&session);
        let k = key.normalized();
        let none = k.mods == Mods::NONE;
        let ctrl = k.mods == Mods::CTRL;
        match k.code {
            KeyCode::Char('j') | KeyCode::Down if none => {
                self.notices_choose(&session, &entries, at + 1)
            }
            KeyCode::Char('n') if ctrl => self.notices_choose(&session, &entries, at + 1),
            KeyCode::Char('k') | KeyCode::Up if none => {
                self.notices_choose(&session, &entries, at - 1)
            }
            KeyCode::Char('p') if ctrl => self.notices_choose(&session, &entries, at - 1),
            KeyCode::Char('g') | KeyCode::Home if none => {
                self.notices_choose(&session, &entries, 0)
            }
            KeyCode::Char('G') | KeyCode::End if none => {
                self.notices_choose(&session, &entries, entries.len() as isize - 1)
            }
            // Tab and Shift-Tab go round "‹ tabs", then each notification,
            // as on a webpage.
            KeyCode::Tab | KeyCode::BackTab if settings::is_on("tab_focus") => {
                let forward = k.code == KeyCode::Tab;
                let ends = if forward {
                    at + 1 >= entries.len() as isize
                } else {
                    at <= 0
                };
                if !back && (ends || entries.is_empty()) {
                    self.sidebar.windows.back.insert(session.clone());
                    self.dirty_session(&session);
                } else if back {
                    let to = if forward {
                        0
                    } else {
                        entries.len() as isize - 1
                    };
                    self.notices_choose(&session, &entries, to);
                } else {
                    self.notices_choose(&session, &entries, at + if forward { 1 } else { -1 });
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') if back => self.notices_back(&session),
            KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Right | KeyCode::Char('l')
                if none || k.code == KeyCode::Enter =>
            {
                self.notices_open(client, &entries, at.max(0) as usize)
            }
            KeyCode::Esc | KeyCode::Left if none => self.notices_back(&session),
            KeyCode::Char('q' | 'h') if none => self.notices_back(&session),
            KeyCode::Char('c') if ctrl => self.notices_back(&session),
            _ => {}
        }
        true
    }

    /// The mouse over the window's pane (`row` in it); gives whether the
    /// window took it.
    pub fn notices_mouse(&mut self, client: ClientId, pane: PaneId, row: u16, m: Mouse) -> bool {
        if !self.is_notices_pane(pane) {
            return false;
        }
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return true;
        };
        let entries = self.notice_entries();
        let at = self.notice_cursor(&session, &entries);
        let (cols, rows) = self
            .panes
            .get(&pane)
            .map(|s| s.pane.size())
            .unwrap_or((1, 1));
        // Where each line goes, as it was drawn.
        let look = Look {
            cursor: at,
            back: false,
            hover: None,
            theme: &super::theme(),
        };
        let items: Vec<Item> = entries.iter().map(|(_, i)| i.clone()).collect();
        let (_, spots) = notices::list_lines(&items, &look, cols as usize, rows as usize);
        let spot = spots.get(row as usize).copied().flatten();
        match (m.kind, m.button) {
            (MouseKind::Move, _) => {
                let lit = spot.filter(|_| settings::is_on("hover_highlight"));
                let had = self.sidebar.windows.hover.get(&client).copied();
                if had != lit {
                    match lit {
                        Some(s) => self.sidebar.windows.hover.insert(client, s),
                        None => self.sidebar.windows.hover.remove(&client),
                    };
                    self.dirty_session(&session);
                }
            }
            (MouseKind::Press, MouseButton::WheelUp) => {
                self.notices_choose(&session, &entries, at as isize - 1)
            }
            (MouseKind::Press, MouseButton::WheelDown) => {
                self.notices_choose(&session, &entries, at as isize + 1)
            }
            (MouseKind::Press, MouseButton::Left) => {
                if self.current_pane(client) != Some(pane) {
                    self.focus_pane(pane);
                }
                match spot {
                    Some(Spot::Back) => self.notices_back(&session),
                    Some(Spot::Item(i)) if i == at => self.notices_open(client, &entries, i),
                    Some(Spot::Item(i)) => self.notices_choose(&session, &entries, i as isize),
                    None => {}
                }
            }
            _ => {}
        }
        true
    }

    /// Draws a session's window over its pane's place.
    pub fn notices_draw(&self, buf: &mut Buffer, area: Rect, session: &str, client: ClientId) {
        let entries = self.notice_entries();
        let look = Look {
            cursor: self.notice_cursor(session, &entries),
            back: self.sidebar.windows.back.contains(session),
            hover: self.sidebar.windows.hover.get(&client).copied(),
            theme: &super::theme(),
        };
        let items: Vec<Item> = entries.into_iter().map(|(_, i)| i).collect();
        let (lines, _) =
            notices::list_lines(&items, &look, area.width as usize, area.height as usize);
        super::super::render::draw(buf, area, &lines);
    }
}
