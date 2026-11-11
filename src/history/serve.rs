//! A tab's history on the server: what the hooks and the panes' output
//! keep (`history_hook`, `history_output`), the snapshots taken on the tick
//! (`history_tick`), M-h opening the view (`open_history`), the view kept
//! current as it is drawn (`history_before_draw`), and the transcript read
//! off the server's task (`HistoryEvent`).

use std::path::PathBuf;

use serde_json::Value;

use super::timeline::{self, Model};
use super::view::HistoryView;
use crate::agent::Hook;
use crate::mux::keys::Action;
use crate::mux::server::ClientId;
use crate::mux::server::{Event, Server};
use crate::mux::{PaneId, TabId};
use crate::{paths, settings, util};

/// What a read of a transcript off the server's task found.
#[derive(Debug)]
pub enum HistoryEvent {
    Read {
        pane: PaneId,
        session: String,
        /// What the transcript says, taken in as far as it has grown, with
        /// where the next read starts and the context window; `None` when
        /// it couldn't be read.
        read: Option<(super::transcript::Lanes, u64, u64)>,
    },
}

fn on() -> bool {
    settings::is_on("tab_history")
}

impl Server {
    /// A hook from a pane's Claude: its session, and its state once the
    /// hook has been applied.
    pub fn history_hook(&mut self, pane: PaneId, hook: &Hook, payload: &Value) {
        if !on() {
            return;
        }
        let now = util::now_secs();
        let session = payload
            .get("session_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let transcript = payload
            .get("transcript_path")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map(PathBuf::from);
        let fresh = hook.event == "SessionStart" && hook.source != "compact";
        self.history.session(pane, session, transcript, fresh, now);
        let state = self.agents.state(pane);
        self.history.state(pane, state, now);
        // A commit shows on the screen as its call ends, which is no change
        // of state: the next tick takes it, so moving to the commit shows it.
        let command = payload
            .pointer("/tool_input/command")
            .and_then(Value::as_str)
            .unwrap_or("");
        if matches!(hook.event.as_str(), "PostToolUse" | "PostToolUseFailure")
            && super::transcript::is_commit(command)
        {
            self.history.marked(pane);
        }
    }

    /// A pane drew something.
    pub fn history_output(&mut self, pane: PaneId) {
        self.history.touched(pane);
    }

    /// On the tick: a snapshot of each Claude pane that changed and is due
    /// one, and the transcripts of the histories open read on.
    pub fn history_tick(&mut self) {
        if !on() {
            if !self.history.records.is_empty() {
                self.history.clear();
            }
            return;
        }
        let now = util::now_secs();
        // A state changed without a hook (a permission prompt denied or
        // escaped, a finished tab seen, a pane gone back to its shell) is
        // taken in here, at the time the agent says it began, or now.
        // A pane gone however it went (its tab closed whole, say) takes its
        // history with it.
        let alive = &self.panes;
        self.history.records.retain(|p, _| alive.contains_key(p));
        let panes: Vec<PaneId> = self.history.records.keys().copied().collect();
        for pane in panes {
            let agent = self.agents.get(pane);
            let state = agent.and_then(|a| a.state);
            let since = agent.map(|a| a.since).filter(|s| *s > 0 && state.is_some());
            self.history
                .state(pane, state, since.unwrap_or(now).min(now));
        }
        for pane in self.history.due(now) {
            match self.panes.get(&pane) {
                Some(slot) => {
                    let lines = slot.pane.styled_lines();
                    self.history.shoot(pane, now, lines);
                }
                None => self.history.forget(pane),
            }
        }
        for pane in self.history_open_panes() {
            self.history_read(pane);
        }
    }

    /// The panes a client shows the history of.
    fn history_open_panes(&self) -> Vec<PaneId> {
        let mut panes: Vec<PaneId> = self
            .clients
            .values()
            .filter_map(|c| c.overlays.find::<HistoryView>().map(|v| v.pane))
            .collect();
        panes.sort();
        panes.dedup();
        panes
    }

    /// Reads on in a pane's transcript, off the server's task, unless a
    /// read is under way.
    fn history_read(&mut self, pane: PaneId) {
        let Some(record) = self.history.get_mut(pane) else {
            return;
        };
        let Some(file) = record.transcript.clone() else {
            return;
        };
        if record.reading {
            return;
        }
        record.reading = true;
        let (session, offset) = (record.session.clone(), record.offset);
        let mut lanes = record.lanes.clone();
        let tx = self.tx.clone();
        let state = paths::state_dir();
        // Read and parsed here, off the server's task: a transcript read
        // from its start can be tens of megabytes.
        tokio::task::spawn_blocking(move || {
            let read = super::transcript::read_from(&file, offset).map(|(text, next)| {
                if next < offset {
                    lanes = Default::default();
                }
                lanes.take(&text);
                let (used, _) = crate::usage::sessions::context_of(&state, &session);
                let window = lanes.window(used);
                (lanes, next, window)
            });
            let _ = tx.send(Event::History(HistoryEvent::Read {
                pane,
                session,
                read,
            }));
        });
    }

    pub fn history_event(&mut self, event: HistoryEvent) {
        match event {
            HistoryEvent::Read {
                pane,
                session,
                read,
            } => {
                let Some(record) = self.history.get_mut(pane).filter(|r| r.session == session)
                else {
                    return;
                };
                record.reading = false;
                if let Some((lanes, next, window)) = read {
                    record.lanes = lanes;
                    record.offset = next;
                    record.window = window;
                }
                self.history_dirty(pane);
            }
        }
    }

    /// Draws again the clients showing a pane's history.
    fn history_dirty(&mut self, pane: PaneId) {
        for c in self.clients.values_mut() {
            if c.overlays
                .find::<HistoryView>()
                .is_some_and(|v| v.pane == pane)
            {
                c.dirty = true;
            }
        }
    }

    /// M-h: the history of the tab in front, its Claude's.
    pub fn open_history(&mut self, client: ClientId) {
        if !on() {
            self.say(
                Some(client),
                "A tab's history is off: turn on \"Tab history\" in Settings",
            );
            return;
        }
        let Some(tab) = self
            .clients
            .get(&client)
            .and_then(|c| self.session(&c.session))
            .map(|s| s.current)
        else {
            return;
        };
        let Some(pane) = self.history_pane(tab).or_else(|| self.history_seed(tab)) else {
            self.say(
                Some(client),
                "This tab has no Claude session, so it has no history yet",
            );
            return;
        };
        let toggles = self
            .bindings
            .root
            .iter()
            .filter(|(_, a)| **a == Action::History)
            .map(|(k, _)| *k)
            .collect();
        // A Claude parked behind a tool in its place (the diff, a PR or
        // ticket view) comes back beside it first, as M-p's "side" brings
        // it, so typing that closes the history reaches it.
        let parked = self
            .tabs
            .get(&tab)
            .is_some_and(|t| !t.layout.panes().contains(&pane));
        if parked {
            self.unpark_beside(pane);
        }
        // The pane it shows has the focus (the sidebar's given up too), so
        // typing that closes it goes to the Claude whose screen was up.
        let in_layout = self
            .tabs
            .get(&tab)
            .is_some_and(|t| t.layout.panes().contains(&pane));
        if !in_layout {
            // Still behind its tool: typing would reach another pane.
            self.say(
                Some(client),
                "This tab's Claude is behind a tool that couldn't make room for it, so its history isn't shown",
            );
            return;
        }
        self.focus_pane(pane);
        // Out of copy mode too, which would take the typing.
        self.leave_copy(pane);
        self.open(client, Box::new(HistoryView::new(tab, pane, toggles)), None);
        self.history_read(pane);
    }

    /// The tab's Claude pane, of those with a history.
    fn history_pane(&self, tab: TabId) -> Option<PaneId> {
        let panes = self.tab_panes(tab);
        let active = self.tabs.get(&tab).map(|t| t.active);
        panes
            .iter()
            .copied()
            .filter(|p| self.history.get(*p).is_some())
            .max_by_key(|p| Some(*p) == active)
    }

    /// A history for the tab's Claude pane from the session the sidebar's
    /// read knows for it, when its Claude has run no hook since the server
    /// started (one restarted on a new build): its lanes come from the
    /// transcript, its screen is kept from now on.
    fn history_seed(&mut self, tab: TabId) -> Option<PaneId> {
        let active = self.tabs.get(&tab).map(|t| t.active);
        let (pane, session, transcript) = self
            .tab_panes(tab)
            .into_iter()
            .filter_map(|p| {
                let read = self.sidebar.panes.get(&p)?;
                let session = read.session.as_ref()?.session_id.clone();
                Some((p, session, read.transcript.clone()))
            })
            .max_by_key(|(p, _, _)| Some(*p) == active)?;
        let now = util::now_secs();
        self.history.session(pane, &session, transcript, false, now);
        let agent = self.agents.get(pane);
        let state = agent.and_then(|a| a.state);
        let since = agent.map(|a| a.since).filter(|s| *s > 0 && state.is_some());
        self.history
            .state(pane, state, since.unwrap_or(now).min(now));
        self.history.marked(pane);
        self.history.get(pane).map(|_| pane)
    }

    /// Keeps a client's history view on its tab's place, with its model and
    /// the screen at the moment shown; closes it once its tab isn't in
    /// front or its pane has gone.
    pub fn history_before_draw(&mut self, id: ClientId) {
        let Some(c) = self.clients.get(&id) else {
            return;
        };
        let Some(view) = c.overlays.find::<HistoryView>() else {
            return;
        };
        let (tab, pane, shown) = (view.tab, view.pane, view.shown);
        let session = self.session(&c.session);
        let place = session
            .filter(|s| s.current == tab)
            .and_then(|s| self.tabs.get(&tab).map(|t| s.area_for(t.zoomed)));
        let (Some(area), Some(record), Some(slot)) =
            (place, self.history.get(pane), self.panes.get(&pane))
        else {
            self.close_overlays(id, "history");
            return;
        };
        let now = util::now_secs();
        let model = Model {
            start: record.start(),
            now,
            states: record.states.clone(),
            lanes: record.lanes.clone(),
            window: record.window,
        };
        let screen = match shown {
            None => Some(slot.pane.styled_lines()),
            Some(at) => record.shot_settled(at).map(|s| s.lines.clone()),
        };
        let header = crate::status::top() > 0 && !self.tabs.get(&tab).is_some_and(|t| t.zoomed);
        let caps = crate::ui::kit::caps(&c.term);
        let c = self.clients.get_mut(&id).expect("checked above");
        if let Some(view) = c.overlays.find_mut::<HistoryView>() {
            view.area = Some(area);
            view.model = model;
            view.screen = screen;
            view.header = header;
            view.caps = caps;
        }
    }

    /// What the header says of the moment a client's history shows on
    /// `tab`, and whether it is open there.
    pub fn history_shown(&self, client: ClientId, tab: TabId) -> (bool, Option<String>) {
        let view = self
            .clients
            .get(&client)
            .and_then(|c| c.overlays.find::<HistoryView>())
            .filter(|v| v.tab == tab);
        match view {
            None => (false, None),
            Some(v) => (
                true,
                v.shown.map(|at| timeline::viewing(at, util::now_secs())),
            ),
        }
    }
}
