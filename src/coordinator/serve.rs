//! The coordinator on the server (src/coordinator/mod.rs): starting its
//! pane as Settings' `coordinator` says and keeping it running, the events
//! that wake it, its hooks, M-C showing it beside the tab in front and the
//! keys it takes then, the two commands its permissions allow
//! (`coordinator-close`, `coordinator-open`), and its line in `list`.

use std::time::Instant;

use ratatui::layout::Rect;
use serde_json::{Value, json};

use super::{Coordinator, LEAST_COLUMNS, Shown, TAB, Wake};
use crate::agent::{Hook, State};
use crate::facts::{Facts, PrState};
use crate::mission::answer;
use crate::mux::compose::{self, Frame};
use crate::mux::input::{Key, KeyCode, Mouse, MouseButton, MouseKind};
use crate::mux::pane::PaneView;
use crate::mux::server::{ClientId, Opened, Reply, Server};
use crate::mux::{PaneId, TabId};
use crate::{settings, util};

/// The size its pane starts at, before a client shows it.
const START_SIZE: (u16, u16) = (100, 40);

impl Server {
    /// Whether the coordinator is on in Settings.
    fn coordinator_on() -> bool {
        settings::is_on(super::SETTING)
    }

    /// Whether `pane` is the coordinator's.
    pub fn is_coordinator(&self, pane: PaneId) -> bool {
        self.coordinator.pane == Some(pane)
    }

    /// Its share of the tick: started while it is on and not running (no
    /// sooner than `RESTART_AFTER` after it last started), ended while it is
    /// off, its note looked at, and what waits typed once it sleeps.
    pub fn coordinator_tick(&mut self) {
        if !Self::coordinator_on() {
            self.coordinator_stop();
            return;
        }
        if let Some(pane) = self.coordinator.pane
            && !self.panes.contains_key(&pane)
        {
            self.coordinator_ended();
        }
        // Another note chosen: started again on it, resumed, once it
        // sleeps, so its permissions name the new one.
        if self.coordinator.pane.is_some()
            && self.coordinator.note != Self::coordinator_note_title()
            && !matches!(
                self.coordinator.pane.and_then(|p| self.agents.state(p)),
                Some(State::Working | State::Waiting)
            )
        {
            self.coordinator_restart();
        }
        let due = self
            .coordinator
            .started
            .is_none_or(|at| at.elapsed() >= super::RESTART_AFTER);
        if self.coordinator.pane.is_none() && due {
            self.coordinator_start();
        }
        // A client gone shows it no more.
        let clients = &self.clients;
        self.coordinator
            .shown
            .retain(|c, _| clients.contains_key(c));
        self.coordinator_note_look();
        self.coordinator_flush();
    }

    /// Starts its pane: Claude on Settings' model, resuming the session
    /// kept, or fresh with its skill.
    fn coordinator_start(&mut self) {
        if self.coordinator.started.is_none() {
            self.coordinator.load();
        }
        let dir = super::dir();
        let notes = crate::notes::store::notes_dir();
        self.coordinator.note = Self::coordinator_note_title();
        let note = super::note_file(&notes, &self.coordinator.note);
        let written = std::fs::create_dir_all(super::posts_dir()).and_then(|()| {
            let text =
                serde_json::to_string_pretty(&super::permissions(&note, &super::posts_dir()))
                    .unwrap_or_default();
            util::write_atomic(super::settings_file(), text.as_bytes())
        });
        if let Err(e) = written {
            eprintln!("coordinator: couldn't write its settings: {e}");
        }
        let model = settings::setting(super::MODEL)
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        let resume = self.coordinator.saved.session.clone();
        let command = super::command(
            &crate::worktree::work_shell(),
            &super::model_of(&model, "sonnet"),
            &super::settings_file(),
            resume.as_deref(),
        );
        self.coordinator.started = Some(Instant::now());
        self.coordinator.resumed = resume.is_some();
        self.coordinator.ready = false;
        let (cols, rows) = START_SIZE;
        match self.spawn_pane(TAB, Some(dir), Some(command), cols, rows) {
            Ok(pane) => {
                // A fresh session is given its skill, a model call.
                if resume.is_none() {
                    crate::requests::count("Anthropic", "claude", "coordinator: started");
                }
                self.coordinator.pane = Some(pane);
                self.coordinator.note_seen = self.coordinator_note_modified();
            }
            Err(e) => eprintln!("coordinator: couldn't start: {e:#}"),
        }
        self.coordinator_redraw();
    }

    /// Its pane, handed over by the build before an upgrade, running on:
    /// its session started long since, and its session's ID kept.
    pub fn coordinator_adopted(&mut self, pane: Option<PaneId>, handed: super::Handed) {
        self.coordinator = Coordinator {
            pane,
            started: pane.map(|_| Instant::now()),
            ready: pane.is_some() && handed.ready,
            note: Self::coordinator_note_title(),
            pending: handed.pending,
            opened: handed.opened.map(|(tab, dir)| (tab, dir, Instant::now())),
            ..Coordinator::default()
        };
        self.coordinator.load();
        self.coordinator.note_seen = self.coordinator_note_modified();
    }

    /// Its pane ended: a resumed session that ended at once couldn't be
    /// resumed, so the next start is fresh.
    fn coordinator_ended(&mut self) {
        self.coordinator.pane = None;
        self.coordinator.ready = false;
        let at_once = self
            .coordinator
            .started
            .is_some_and(|at| at.elapsed() < super::FAILED_WITHIN);
        if self.coordinator.resumed && at_once {
            self.coordinator.saved.session = None;
            self.coordinator_save();
        }
        self.coordinator.shown.clear();
        self.coordinator_redraw();
    }

    /// Ends its pane and starts it again at once, resumed, keeping what
    /// waits.
    fn coordinator_restart(&mut self) {
        if let Some(pane) = self.coordinator.pane.take()
            && let Some(mut slot) = self.panes.remove(&pane)
        {
            slot.pane.kill();
            self.agents.retain(|p| p != pane);
        }
        self.coordinator.ready = false;
        self.coordinator_start();
    }

    /// Ends it, with Settings' `coordinator` off; its session stays kept,
    /// to resume when it is turned on again.
    fn coordinator_stop(&mut self) {
        let Some(pane) = self.coordinator.pane.take() else {
            return;
        };
        if let Some(mut slot) = self.panes.remove(&pane) {
            slot.pane.kill();
        }
        self.agents.retain(|p| p != pane);
        let saved = std::mem::take(&mut self.coordinator.saved);
        self.coordinator = Coordinator {
            saved,
            ..Coordinator::default()
        };
        self.coordinator_redraw();
    }

    fn coordinator_save(&self) {
        let mut saved = self.coordinator.saved.clone();
        saved.pending = self.coordinator.pending.clone();
        let text = serde_json::to_string(&saved).unwrap_or_default();
        let written = std::fs::create_dir_all(super::dir())
            .and_then(|()| util::write_atomic(super::state_file(), text.as_bytes()));
        if let Err(e) = written {
            eprintln!("coordinator: couldn't keep its session: {e}");
        }
    }

    /// Its note's title, from Settings' `coordinator_note`.
    fn coordinator_note_title() -> String {
        settings::setting(super::NOTE)
            .and_then(|v| v.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Coordination".into())
    }

    fn coordinator_note_path(&self) -> std::path::PathBuf {
        super::note_file(
            &crate::notes::store::notes_dir(),
            &Self::coordinator_note_title(),
        )
    }

    fn coordinator_note_modified(&self) -> Option<std::time::SystemTime> {
        std::fs::metadata(self.coordinator_note_path())
            .and_then(|m| m.modified())
            .ok()
    }

    /// Its note changed since last seen, not by its own edits (which its
    /// hooks mark seen): it is woken.
    fn coordinator_note_look(&mut self) {
        if self.coordinator.editing_note {
            return;
        }
        let now = self.coordinator_note_modified();
        if now == self.coordinator.note_seen {
            return;
        }
        // A note made or removed wakes nothing: it made it, or there is
        // nothing to read.
        let changed = self.coordinator.note_seen.is_some() && now.is_some();
        self.coordinator.note_seen = now;
        if changed {
            self.coordinator.push(&Wake::NoteChanged {
                note: Self::coordinator_note_title(),
            });
        }
    }

    /// Something that wakes it: kept until it sleeps, then typed.
    pub fn coordinator_wake(&mut self, wake: Wake) {
        if !Self::coordinator_on() {
            return;
        }
        if self.coordinator.started.is_none() {
            self.coordinator.load();
            self.coordinator.started = Instant::now().checked_sub(super::RESTART_AFTER);
        }
        self.coordinator.push(&wake);
        self.coordinator_save();
        self.coordinator_flush();
    }

    /// Types what waits, once it may (`Coordinator::take_due`).
    fn coordinator_flush(&mut self) {
        let Some(pane) = self.coordinator.pane else {
            return;
        };
        // A message you left half written in its prompt waits too, rather
        // than going in with the events after it.
        let drafted = self.panes.get(&pane).is_some_and(|slot| {
            crate::relay::tabs::draft_in(&slot.pane.text(0), &slot.pane.typed_text(0))
                .is_some_and(|d| !d.trim().is_empty())
        });
        let asleep = !drafted
            && !matches!(
                self.agents.state(pane),
                Some(State::Working | State::Waiting)
            );
        if let Some(text) = self.coordinator.take_due(asleep, Instant::now()) {
            self.type_into_claude(pane, answer::message_presses(&text));
            crate::requests::count("Anthropic", "claude", "coordinator: woken");
            self.coordinator_save();
        }
    }

    /// A tab's Claude got to a state: finished or waiting on you wakes it,
    /// but for a tab of autopilot's, which it leaves alone.
    pub fn coordinator_tab_state(&mut self, tab: TabId, state: Option<State>) {
        if !Self::coordinator_on() || self.autopilot_tab(tab) {
            return;
        }
        let (number, name) = (self.tab_number(tab), self.tab_label_without_pr_title(tab));
        match state {
            Some(State::Done) => self.coordinator_wake(Wake::Finished { tab: number, name }),
            Some(State::Waiting) => self.coordinator_wake(Wake::Waiting { tab: number, name }),
            _ => {}
        }
    }

    fn autopilot_tab(&self, tab: TabId) -> bool {
        self.tabs
            .get(&tab)
            .is_some_and(|t| t.opts.contains_key(crate::autopilot::OPENED))
    }

    /// The tabs' facts before `new` takes their place: a PR that was open
    /// and now has merged wakes it.
    pub fn coordinator_facts(&mut self, new: &Facts) {
        if !Self::coordinator_on() {
            return;
        }
        let mut merged = Vec::new();
        for (tab, facts) in new {
            let Some(pr) = &facts.pr else {
                continue;
            };
            // A merge it was never told of, kept by the PR's address in
            // state.json, so one that merged while the server restarted is
            // news, and one told before (a tab kept open, or reopened) never
            // is again.
            if self.coordinator.started.is_none() {
                // Not started yet: what is kept is read first, so saving
                // keeps its session.
                self.coordinator.load();
                self.coordinator.started = Instant::now().checked_sub(super::RESTART_AFTER);
            }
            let told = self.coordinator.saved.told.contains(&pr.url);
            if !told && pr.state == PrState::Merged && !self.autopilot_tab(*tab) {
                merged.push(Wake::Merged {
                    tab: self.tab_number(*tab),
                    name: self.tab_label_without_pr_title(*tab),
                    number: pr.number,
                    url: pr.url.clone(),
                });
            }
        }
        if merged.is_empty() {
            return;
        }
        for wake in &merged {
            if let Wake::Merged { url, .. } = wake {
                self.coordinator.saved.tell(url);
            }
        }
        self.coordinator_save();
        for wake in merged {
            self.coordinator_wake(wake);
        }
    }

    /// A hook from its session: its state, its session's ID kept for a
    /// restart, its note marked seen after its own edits, and what waits
    /// typed once it sleeps. Answers nothing, as every hook.
    pub fn coordinator_hook(&mut self, pane: PaneId, event: &str, payload: &Value) -> Reply {
        let hook = Hook::read(event, payload);
        let now = util::now_secs();
        // A prompt that reached you (in auto mode, only its Notification
        // says so) is a wait, as for a tab's Claude.
        let changed = if hook.event == "Notification" {
            crate::agent::QUESTION_KINDS.contains(&hook.kind.as_str()) && {
                let shown = self.agents.turn_over(pane) && self.prompt_shown(pane);
                self.agents.wait(pane, now, false, shown)
            }
        } else {
            self.agents.apply(pane, &hook, now, false)
        };
        if hook.agent.is_empty() {
            self.agents.heard(pane, &hook, util::now_ms());
        }
        if let Some(session) = payload.get("session_id").and_then(Value::as_str)
            && hook.agent.is_empty()
            && self.coordinator.saved.session.as_deref() != Some(session)
        {
            self.coordinator.saved.session = Some(session.to_string());
            self.coordinator_save();
        }
        // Its own edit of its note, from the call's start to its end, is
        // no change from outside: the tick looks past it meanwhile, and
        // takes the note as seen once it is done. Any other call leaves
        // the note's look as it was, so your edit meanwhile still wakes it.
        let edits_note = matches!(hook.tool.as_str(), "Edit" | "Write" | "MultiEdit")
            && payload
                .pointer("/tool_input/file_path")
                .and_then(Value::as_str)
                .is_some_and(|f| std::path::Path::new(f) == self.coordinator_note_path());
        match hook.event.as_str() {
            "SessionStart" => self.coordinator.ready = true,
            "PreToolUse" if edits_note => self.coordinator.editing_note = true,
            "PostToolUse" | "PostToolUseFailure" if edits_note => {
                self.coordinator.editing_note = false;
                self.coordinator.note_seen = self.coordinator_note_modified();
            }
            "Stop" | "SessionEnd" => self.coordinator.editing_note = false,
            _ => {}
        }
        if changed {
            self.coordinator_redraw();
        }
        self.coordinator_flush();
        Reply::ok("")
    }

    /// Whether it sleeps, works or waits on you, for the sidebar's row;
    /// `None` while it isn't running.
    pub fn coordinator_state(&self) -> Option<Option<State>> {
        let pane = self.coordinator.pane?;
        Some(self.agents.state(pane))
    }

    /// Its row in the sidebar's footer while it runs: its mark, and
    /// whether it works, waits on you or sleeps.
    pub fn coordinator_row(&self) -> Option<(crate::ui::kit::Mark, &'static str)> {
        use crate::ui::kit::Mark;
        Some(match self.coordinator_state()? {
            Some(State::Working) => (Mark::Working, "working"),
            Some(State::Waiting) => (Mark::Needs, "waits on you"),
            _ => (Mark::Idle, "asleep"),
        })
    }

    /// Every client drawn again: the sidebar's row and the pane shown.
    fn coordinator_redraw(&mut self) {
        for c in self.clients.values_mut() {
            c.dirty = true;
        }
    }

    /// Its pane printed something: the clients showing it are drawn again.
    pub fn coordinator_output(&mut self) {
        let showing: Vec<ClientId> = self.coordinator.shown.keys().copied().collect();
        for id in showing {
            if let Some(c) = self.clients.get_mut(&id) {
                c.dirty = true;
            }
        }
    }

    // ── M-C ─────────────────────────────────────────────────────────────────

    /// M-C: shows it beside the tab in front with the keys, or hides it.
    pub fn coordinator_toggle(&mut self, client: ClientId) {
        if !Self::coordinator_on() {
            self.say(
                Some(client),
                "The coordinator is off in Settings (Coordinator).",
            );
            return;
        }
        if self.coordinator.shown.remove(&client).is_some() {
            self.coordinator_redraw();
            return;
        }
        if self.coordinator.pane.is_none() {
            self.coordinator_start();
        }
        if self.coordinator.pane.is_none() {
            self.say(Some(client), "The coordinator couldn't start.");
            return;
        }
        if let Some(session) = self.clients.get(&client).map(|c| c.session.clone())
            && self.sidebar.focused(&session)
        {
            self.sidebar_leave(&session);
        }
        self.coordinator.shown.insert(
            client,
            Shown {
                focused: true,
                at: None,
            },
        );
        self.coordinator_redraw();
    }

    /// Whether a client shows it with the keys.
    pub(crate) fn coordinator_focused(&self, client: ClientId) -> bool {
        self.coordinator
            .shown
            .get(&client)
            .is_some_and(|s| s.focused)
    }

    /// Where it goes for a client: the side panel's spare columns beside
    /// the tab in front when they are wide enough, else the tab's right half.
    fn coordinator_place(&self, client: ClientId) -> Option<Rect> {
        let c = self.clients.get(&client)?;
        let s = self.session(&c.session)?;
        let tab = self.tabs.get(&s.current)?;
        let (spread, area) = (s.spread_for(tab), s.area_for(tab));
        let spare = crate::mux::beside::spare_of(spread, area).filter(|r| r.width >= LEAST_COLUMNS);
        Some(spare.unwrap_or_else(|| {
            let half = area.width / 2;
            Rect::new(area.x + half, area.y, area.width - half, area.height)
        }))
    }

    /// Before a client is drawn: its pane sized to the box it goes in.
    pub fn coordinator_before_draw(&mut self, client: ClientId) {
        if !self.coordinator.shown.contains_key(&client) {
            return;
        }
        let inner = self.coordinator_place(client).map(|r| {
            Rect::new(
                r.x + 1,
                r.y + 1,
                r.width.saturating_sub(2),
                r.height.saturating_sub(2),
            )
        });
        if let Some(shown) = self.coordinator.shown.get_mut(&client) {
            shown.at = inner;
        }
        if let (Some(inner), Some(slot)) = (
            inner,
            self.coordinator.pane.and_then(|p| self.panes.get_mut(&p)),
        ) && inner.width > 0
            && inner.height > 0
        {
            slot.pane.resize(inner.width, inner.height);
        }
    }

    /// Draws it for a client that shows it, over the tab, greying the
    /// tab's panes while it has the keys.
    pub fn coordinator_draw(&self, frame: &mut Frame, client: ClientId) {
        let Some(shown) = self.coordinator.shown.get(&client) else {
            return;
        };
        let (Some(outer), Some(slot)) = (
            self.coordinator_place(client),
            self.coordinator.pane.and_then(|p| self.panes.get(&p)),
        ) else {
            return;
        };
        let palette = crate::status::theme::palette();
        if shown.focused
            && let Some(area) = self
                .clients
                .get(&client)
                .and_then(|c| self.session(&c.session))
                .and_then(|s| Some(s.area_for(self.tabs.get(&s.current)?)))
        {
            compose::tint_inactive(&mut frame.buf, area, palette.inactive_bg);
        }
        let outer = outer.intersection(frame.area());
        frame.links.clear_area(outer);
        let title = match self.coordinator.pane.and_then(|p| self.agents.state(p)) {
            Some(State::Working) => "Coordinator · working",
            Some(State::Waiting) => "Coordinator · waits on you",
            _ => "Coordinator · asleep",
        };
        let inner = compose::draw_box(&mut frame.buf, outer, title);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let at = slot.pane.render(
            &mut frame.buf,
            inner,
            &PaneView {
                focused: shown.focused,
                dim: false,
            },
        );
        slot.pane.put_links(inner, frame.area(), &mut frame.links);
        if shown.focused {
            frame.cursor = at;
            frame.cursor_style = Some(slot.pane.cursor_style());
        } else {
            compose::tint_inactive(&mut frame.buf, inner, palette.inactive_bg);
        }
    }

    /// A workspace key pressed while it has the keys: the keys go back to
    /// the tab, where the key acts (M-c, a move between panes, a tab),
    /// unless it is M-C itself.
    pub fn coordinator_action(&mut self, client: ClientId, action: &crate::mux::keys::Action) {
        if *action != crate::mux::keys::Action::Coordinator
            && let Some(shown) = self.coordinator.shown.get_mut(&client)
            && shown.focused
        {
            shown.focused = false;
            self.coordinator_redraw();
        }
    }

    /// Takes the keys from it, as a picture chosen beside the pane does.
    pub(crate) fn coordinator_unfocus(&mut self, client: ClientId) {
        if let Some(shown) = self.coordinator.shown.get_mut(&client)
            && shown.focused
        {
            shown.focused = false;
            self.coordinator_redraw();
        }
    }

    /// A key while it has the keys: Escape gives them back to the tab,
    /// anything else is its. False when it hasn't the keys.
    pub fn coordinator_key(&mut self, client: ClientId, key: Key, raw: &[u8]) -> bool {
        if !self.coordinator_focused(client) {
            return false;
        }
        // The sidebar taken into (M-s) has the keys from then on.
        if let Some(session) = self.clients.get(&client).map(|c| c.session.clone())
            && self.sidebar.focused(&session)
        {
            if let Some(shown) = self.coordinator.shown.get_mut(&client) {
                shown.focused = false;
            }
            self.coordinator_redraw();
            return false;
        }
        if key.code == KeyCode::Esc && key.mods.is_empty() {
            if let Some(shown) = self.coordinator.shown.get_mut(&client) {
                shown.focused = false;
            }
            self.coordinator_redraw();
            return true;
        }
        let Some(slot) = self.coordinator.pane.and_then(|p| self.panes.get_mut(&p)) else {
            return false;
        };
        if slot.pane.scrolled() > 0 {
            slot.pane.scroll_to_bottom();
        }
        let bytes = crate::mux::input::encode_key(&key, &slot.pane.modes());
        let bytes = if bytes.is_empty() {
            raw.to_vec()
        } else {
            bytes
        };
        let _ = slot.pane.write(&bytes);
        self.coordinator.typed = Some(Instant::now());
        true
    }

    /// Text pasted while it has the keys goes to it.
    pub fn coordinator_paste(&mut self, client: ClientId, text: &str) -> bool {
        if !self.coordinator_focused(client) {
            return false;
        }
        let Some(slot) = self.coordinator.pane.and_then(|p| self.panes.get_mut(&p)) else {
            return false;
        };
        let bytes = crate::mux::input::encode_paste(text, &slot.pane.modes());
        let _ = slot.pane.write(&bytes);
        self.coordinator.typed = Some(Instant::now());
        true
    }

    /// The mouse over it: a press takes the keys there, the wheel scrolls
    /// its history; a press elsewhere gives them back to the tab and goes
    /// on to what is there. True when it was its.
    pub fn coordinator_mouse(&mut self, client: ClientId, m: Mouse) -> bool {
        let Some(shown) = self.coordinator.shown.get(&client).copied() else {
            return false;
        };
        let inside = shown.at.is_some_and(|r| {
            let outer = Rect::new(
                r.x.saturating_sub(1),
                r.y.saturating_sub(1),
                r.width + 2,
                r.height + 2,
            );
            outer.contains(ratatui::layout::Position::new(m.col, m.row))
        });
        if !inside {
            if m.kind == MouseKind::Press && shown.focused {
                if let Some(s) = self.coordinator.shown.get_mut(&client) {
                    s.focused = false;
                }
                self.coordinator_redraw();
            }
            return false;
        }
        match (m.kind, m.button) {
            (MouseKind::Press, MouseButton::WheelUp | MouseButton::WheelDown) => {
                let lines = if m.button == MouseButton::WheelUp {
                    3
                } else {
                    -3
                };
                if let Some(slot) = self.coordinator.pane.and_then(|p| self.panes.get_mut(&p)) {
                    slot.pane.scroll(lines);
                }
            }
            (MouseKind::Press, _) => {
                if let Some(s) = self.coordinator.shown.get_mut(&client) {
                    s.focused = true;
                }
            }
            // A move or a release over it changes nothing.
            _ => return true,
        }
        self.coordinator_redraw();
        true
    }

    // ── Its commands ────────────────────────────────────────────────────────

    /// The tab a command names: `@<id>`, or the number `switchboard tabs`
    /// shows.
    fn coordinator_tab_named(&self, named: &str) -> Result<TabId, String> {
        let tab = match named.strip_prefix('@') {
            Some(id) => id.parse().ok().map(TabId),
            None => named.parse().ok().and_then(|n| self.tab_numbered(n)),
        };
        tab.filter(|t| self.tabs.contains_key(t))
            .ok_or_else(|| format!("no tab {named}"))
    }

    /// `coordinator-close <tab>`: closes a tab whose PR has merged, as
    /// GitHub last answered; refuses any other, and a tab of autopilot's.
    pub fn cmd_coordinator_close(&mut self, rest: &[String]) -> Result<Reply, String> {
        let [named] = rest else {
            return Err("usage: switchboard coordinator-close <tab>".into());
        };
        let tab = self.coordinator_tab_named(named)?;
        if self.autopilot_tab(tab) {
            return Err(format!(
                "tab {named} is autopilot's; it closes its own tabs"
            ));
        }
        let merged = self
            .facts
            .get(&tab)
            .and_then(|f| f.pr.as_ref())
            .is_some_and(|pr| pr.state == PrState::Merged);
        if !merged {
            return Err(format!(
                "tab {named}'s PR hasn't merged, as GitHub last answered; ask before closing it"
            ));
        }
        self.kill_tab(tab);
        Ok(Reply::ok(""))
    }

    /// `coordinator-open-tab -c <dir> <owner/repo#number>`, from `switchboard
    /// coordinator-open` once GitHub says the issue may be taken: a tab in the
    /// background running `wt --issue` on that issue in `dir`, once the tab
    /// it opened last has made its worktree (or is closed); prints the tab.
    pub fn cmd_coordinator_open(&mut self, rest: &[String]) -> Result<Reply, String> {
        const USAGE: &str = "usage: switchboard coordinator-open-tab -c <dir> <owner/repo#number>";
        let (dir, reference) = match rest {
            [c, dir, reference] if c == "-c" => (dir, reference),
            _ => return Err(USAGE.into()),
        };
        let (repo, number) =
            crate::autopilot::issues::parse_reference(reference).ok_or(USAGE.to_string())?;
        let dir = std::path::PathBuf::from(dir);
        if !dir.is_absolute() || !dir.is_dir() {
            return Err(format!("{} isn't a folder", dir.display()));
        }
        if let Some((last, from, at)) = self.coordinator.opened.clone()
            && self.tabs.contains_key(&last)
            && at.elapsed() < super::OPEN_WAIT
            && !self.coordinator_branch_made(last, &from)
        {
            return Err(format!(
                "tab {} hasn't made its worktree yet; open the next once it has",
                self.tab_number(last)
            ));
        }
        let session = self
            .latest_client()
            .and_then(|c| self.clients.get(&c))
            .map(|c| c.session.clone())
            .or_else(|| self.sessions.first().map(|s| s.name.clone()))
            .ok_or("no session to open a tab in")?;
        let args = vec!["--issue".to_string(), format!("{repo}#{number}")];
        let command = crate::worktree::tab_running("worktree-from-ticket", "wt", &args);
        let (tab, _) = self
            .new_tab(
                &session,
                Some(dir.clone()),
                Some(command),
                None,
                Opened::Background,
            )
            .map_err(|e| format!("couldn't open a tab: {e}"))?;
        self.tab_to_end(tab);
        self.coordinator.opened = Some((tab, dir, Instant::now()));
        Ok(Reply::ok(format!("{}\n", self.tab_number(tab))))
    }

    /// Whether a tab opened in `from` has moved into a worktree on a
    /// branch of its own, as `wt` does once it has recorded its branch.
    fn coordinator_branch_made(&self, tab: TabId, from: &std::path::Path) -> bool {
        self.tab_path(tab)
            .is_some_and(|dir| dir != from && self.tab_names.names.branch(dir).is_some())
    }

    /// Its line in `list`: its pane as a tab's are listed, with the Claude
    /// session in front for its address; nothing while it isn't running.
    pub fn coordinator_json(&self) -> Value {
        let Some(pane) = self.coordinator.pane else {
            return Value::Null;
        };
        let Some(slot) = self.panes.get(&pane) else {
            return Value::Null;
        };
        json!({
            "id": pane.to_string(),
            "cwd": self.pane_own_dir(pane).map(|d| d.to_string_lossy().into_owned()),
            "command": slot.pane.foreground_command(),
            "agent": self.agent_json(pane),
            "claude": self.sidebar.panes.get(&pane).and_then(|r| {
                let s = r.session.as_ref()?;
                Some(json!({
                    "session": s.session_id,
                    "peer": {
                        "session": s.peer_session,
                        "pid": s.pid,
                        "name": s.peer_name,
                        "socket": s.socket,
                    },
                }))
            }),
        })
    }
}
