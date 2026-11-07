//! The status line on the server: its rows at the bottom of each client's
//! frame, the tick that checks the health light and lays the tabs out again
//! when the second status line comes or goes, a click on the ⌘ Commands
//! button (tmux.conf's `MouseDown1Status` pressing M-p on the `palette`
//! range), each terminal asked for its background as it attaches, and M-p's
//! "Toggle light and dark theme" (settings.sh's `toggle-theme`).

use std::sync::atomic::Ordering;

use ratatui::layout::Rect;
use serde_json::Value;
use tokio::sync::mpsc;

use super::second::{self, Second};
use super::{Hits, LINES, Left, StatusEvent, health, lines, theme};
use crate::mux::compose::{self, Frame, TabStatus};
use crate::mux::proto::ServerMsg;
use crate::mux::server::{ClientId, Server, Session};
use crate::{paths, settings, util};

impl Server {
    /// On the server's tick: the status line's rows from the
    /// `second_status` setting, every tab laid out again when they change;
    /// the health light checked again; and with the second status line on,
    /// every client drawn again for the times it shows ("waiting on you for
    /// 4m").
    pub fn status_tick(&mut self) {
        let want = if settings::is_on("second_status") {
            2
        } else {
            1
        };
        if LINES.swap(want, Ordering::Relaxed) != want {
            let tabs: Vec<_> = self.tabs.keys().copied().collect();
            for tab in tabs {
                self.relayout(tab);
            }
            self.dirty_all();
        }
        let health = if settings::is_on("health_light") {
            let asked = if self.panes.is_empty() {
                None
            } else {
                crate::mux::persist::last_asked(self)
            };
            health::check(&paths::state_dir(), &health::relay_dir(), asked)
        } else {
            Vec::new()
        };
        if health != self.status.health {
            self.status.health = health;
            self.dirty_all();
        }
        if want == 2 {
            self.push_look();
            self.dirty_all();
        }
    }

    /// The commits each session's current tab has to push, read again from
    /// git's files for a tab with a PR, as tab-status.sh counted them only
    /// then; a pair of commits not counted yet is counted off the task.
    fn push_look(&mut self) {
        let dirs: Vec<std::path::PathBuf> = self
            .sessions
            .iter()
            .map(|s| s.current)
            .filter(|t| self.tab_pr(*t).is_some())
            .filter_map(|t| self.tab_path(t).map(|p| p.to_path_buf()))
            .collect();
        for dir in dirs {
            let Some(pair) = self.status.push.look(&dir) else {
                continue;
            };
            if tokio::runtime::Handle::try_current().is_err() {
                return;
            }
            let tx = self.tx.clone();
            tokio::task::spawn_blocking(move || {
                let count = super::push::count(&dir, &pair);
                let _ = tx.send(crate::mux::server::Event::Status(StatusEvent::Counted {
                    pair,
                    count,
                }));
            });
        }
    }

    /// What the status line's work off the task answered.
    pub fn status_event(&mut self, event: StatusEvent) {
        match event {
            StatusEvent::Counted { pair, count } => {
                self.status.push.counted(pair, count);
                self.dirty_all();
            }
        }
    }

    /// Every client draws again.
    fn dirty_all(&mut self) {
        for c in self.clients.values_mut() {
            c.dirty = true;
        }
    }

    /// What `status-left` shows after the counts, for a session.
    pub fn status_left(&self, s: &Session) -> Left {
        Left {
            color: self.tab_color(s.current).map(Into::into),
            button: settings::is_on("palette_button"),
            health: self.status.health.clone(),
        }
    }

    /// What the second status line says for a session: about its current
    /// tab and the pane focused in it, which stays the tab's own while the
    /// sidebar has the focus, as tab-status.sh looked past the sidebar.
    pub fn second_status(&self, s: &Session) -> Second {
        let tab_id = s.current;
        let Some(tab) = self.tabs.get(&tab_id) else {
            return Second::default();
        };
        let pane = tab.active;
        let pr = self.tab_pr(tab_id);
        let mut label = self.tab_label(tab_id);
        if let Some(rest) = pr.and_then(|n| label.strip_prefix(&format!("#{n} "))) {
            label = rest.to_string();
        }
        let read = self.sidebar.panes.get(&pane);
        let claude = read.and_then(|r| r.session.as_ref());
        let agent = claude.and_then(|_| self.agents.get(pane));
        Second {
            label,
            own_name: self.tab_own_name(tab_id).is_some(),
            pr,
            ahead: pr
                .and(self.tab_path(tab_id))
                .and_then(|d| self.status.push.ahead(d)),
            command: if claude.is_some() {
                Some("claude".into())
            } else {
                read.and_then(|r| r.command.clone())
            },
            color: self.tab_color(tab_id).map(Into::into),
            state: agent.and_then(|a| a.state.map(|st| (st, a.since))),
            session_name: claude.and_then(|c| c.name.clone()),
            prompt: claude
                .and(read.and_then(|r| r.turn.as_ref()))
                .map(|t| t.prompt.clone()),
        }
    }

    /// Draws a session's status line at the bottom of a frame: the tabs'
    /// row, and the second status line under it when it is on. Gives where
    /// its clickable parts went.
    pub fn draw_status_rows(&self, frame: &mut Frame, s: &Session) -> Hits {
        let area = frame.area();
        let p = theme::palette();
        let n = lines().min(area.height);
        let row = Rect::new(0, area.height.saturating_sub(n), area.width, 1);
        // The Notifications window is shown only while it is in front, in
        // the notifications' yellow, as daemon.js's window formats had it.
        let tabs: Vec<TabStatus> = s
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| **t == s.current || !self.is_notices_tab(**t))
            .filter_map(|(i, t)| {
                self.tabs.get(t).map(|_| TabStatus {
                    index: i + 1,
                    name: self.tab_label(*t),
                    current: *t == s.current,
                    agent: self.tab_agent(*t),
                    color: if self.is_notices_tab(*t) {
                        Some(theme::YELLOW)
                    } else {
                        self.tab_color(*t).map(Into::into)
                    },
                })
            })
            .collect();
        let hits =
            compose::draw_status(&mut frame.buf, row, &s.name, &self.status_left(s), &tabs, p);
        if n > 1 {
            let under = Rect::new(0, row.y + 1, area.width, 1);
            let pieces = second::pieces(&self.second_status(s), p, util::now_secs());
            second::draw(&mut frame.buf, under, &pieces, p);
        }
        hits
    }

    /// A click on the ⌘ Commands button opens the palette, as
    /// `MouseDown1Status` pressed M-p on its range; gives whether the click
    /// was the button's.
    pub fn status_button(&mut self, client: ClientId, hit: Option<super::Hit>) -> bool {
        if hit != Some(super::Hit::Button) {
            return false;
        }
        self.open_palette(client);
        true
    }

    /// Asks a terminal that has just attached for its colours: its
    /// background's for the `system` theme (`theme::QUERY`), and the rest
    /// for the panes' programs (src/mux/terminal.rs).
    pub fn ask_background(tx: &mpsc::UnboundedSender<ServerMsg>) {
        let _ = tx.send(ServerMsg::Output(crate::mux::terminal::queries()));
    }

    /// A terminal's answer about its background, among its keys: kept, and
    /// every client drawn again when it changes what `system` shows. Gives
    /// whether the bytes were that answer, which no pane is given.
    pub fn status_reply(&mut self, raw: &[u8]) -> bool {
        let Some(light) = theme::read_reply(raw) else {
            return false;
        };
        if theme::set_system(light)
            && settings::cached("theme") == Some(Value::String("system".into()))
        {
            self.dirty_all();
        }
        true
    }

    /// M-p's "Toggle light and dark theme": the theme setting set to the
    /// other of what is drawn now.
    pub fn toggle_theme(&mut self, client: ClientId) {
        match settings::write_setting("theme", Value::from(theme::toggled())) {
            Ok(()) => {
                settings::reload_settings();
                self.dirty_all();
            }
            Err(e) => self.say(Some(client), &format!("Couldn't change the theme: {e}")),
        }
    }
}
