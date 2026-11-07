//! The sidebar on the server: what bin/tmux-sidebar's daemon did, done in
//! the server task on its own model. Each tab's model is put together when
//! a client is drawn, from what the server holds (labels, colors, the agent
//! state, the facts the sources keep, the tab's options) and from what was
//! last read of Claude Code's files, which is read again every tick and
//! after a hook (`sidebar_read`), never while drawing. The daemon read tmux
//! in one `list-panes` per refresh for the same reason.
//!
//! Focus is the session's, as a sidebar pane's was its window's: M-s moves
//! it into the sidebar and back out (tmux.conf's binding), and while it is
//! there the keys the daemon's `sidebarKey` took move a cursor through the
//! tabs, each tab coming to the front as the cursor reaches it, with the
//! layout held still (`frozen`) so tabs don't jump about as you look at
//! them. Leaving the sidebar keeps the tab you are on.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use serde_json::Value;

use super::model::{self, MergeState, Rules, TabLoop, TabModel, TabState};
use super::peek::Peek;
use super::render::{self, Filter, Notice, Owner, View};
use super::walk::{Next, Place, Stop, Why};
use super::{Drawn, PaneRead, SidebarEvent, claude, config, text};
use crate::agent::State;
use crate::facts::{Merge, PrState};
use crate::items::Target;
use crate::mux::compose::colors;
use crate::mux::input::{Key, KeyCode, Mods, Mouse, MouseButton, MouseKind};
use crate::mux::keys::Action;
use crate::mux::server::{ClientId, Event, Server, Session};
use crate::mux::{PaneId, TabId};
use crate::ui::{Outcome, Theme};
use crate::{paths, settings, util};

/// How often the spinner turns.
const SPIN: Duration = Duration::from_millis(text::SPINNER_MS);

/// The sidebar's keys, for `?` (render.js's `HELP.sidebar`, less what comes
/// with mission control).
const KEYS: &[(&str, &str)] = &[
    ("j k", "show a tab"),
    ("g G", "first, last"),
    ("^d ^u", "half a screen down, up"),
    ("↵ l", "go to the tab"),
    ("h esc q ^c", "back to your pane"),
    ("1-9", "go to that tab"),
    ("/", "find a tab by title (↵ go, esc clear)"),
    ("tab ⇧tab", "next, last tab"),
    ("space", "peek at it"),
    ("n", "to the notifications, when there are any"),
    ("t", "new tab there (M-t)"),
    ("M-x C-x", "run that key there"),
    ("M-s", "back to your pane"),
    ("?", "these keys"),
];

/// The shades the sidebar is drawn in: the light ones when the theme is
/// light (src/status/theme.rs), as the boxes are.
fn theme() -> Theme {
    if crate::status::theme::light() {
        Theme::LIGHT
    } else {
        Theme::DARK
    }
}

/// A process's group, one call to the kernel; `None` for one gone.
fn process_group(pid: u32) -> Option<u32> {
    let pid = libc::pid_t::try_from(pid).ok().filter(|p| *p > 0)?;
    // SAFETY: getpgid only reads the kernel's process table; a pid that
    // isn't there is an error, not undefined.
    let group = unsafe { libc::getpgid(pid) };
    u32::try_from(group).ok().filter(|g| *g > 0)
}

/// A label's PR in front (`#12 name`) and the rest.
fn split_pr(label: &str) -> (Option<u64>, String) {
    if let Some(rest) = label.strip_prefix('#')
        && let Some((num, name)) = rest.split_once(' ')
        && let Ok(n) = num.parse::<u64>()
    {
        return (Some(n), name.to_string());
    }
    (None, label.to_string())
}

impl Server {
    // ── Its place on the screen ─────────────────────────────────────────────

    /// Reads sidebar.json, at the start and whenever it changes.
    pub fn sidebar_load(&mut self) {
        let seen = std::fs::metadata(config::config_file())
            .and_then(|m| m.modified())
            .ok();
        if self.sidebar.config_seen != Some(seen) {
            self.sidebar.config_seen = Some(seen);
            self.sidebar.config = config::Config::read();
        }
    }

    /// The columns the sidebar takes in a session `cols` wide, its line
    /// beside the panes included: none with the sidebar setting off or a
    /// window narrower than sidebar.json's `minWindowWidth`.
    pub fn sidebar_columns(&self, cols: u16) -> u16 {
        if settings::is_on("sidebar") {
            self.sidebar.config.columns_in(cols)
        } else {
            0
        }
    }

    /// Whether a session shows its sidebar now: it has room for one and its
    /// tab in front isn't zoomed (a zoomed pane covers it, as a zoomed tmux
    /// pane covered the sidebar's).
    pub fn sidebar_shown(&self, session: &str) -> bool {
        self.session(session)
            .is_some_and(|s| s.sidebar > 0 && self.tabs.get(&s.current).is_none_or(|t| !t.zoomed))
    }

    /// Gives each session the sidebar its size and the settings allow, laying
    /// its tabs out again where that changed.
    pub fn fit_sidebars(&mut self) {
        let wanted: Vec<(String, u16)> = self
            .sessions
            .iter()
            .map(|s| (s.name.clone(), self.sidebar_columns(s.cols)))
            .collect();
        for (name, columns) in wanted {
            let Some(s) = self.session_mut(&name) else {
                continue;
            };
            if s.sidebar == columns {
                continue;
            }
            s.sidebar = columns;
            let tabs = s.tabs.clone();
            for t in tabs {
                self.relayout(t);
            }
            if columns == 0 {
                self.sidebar_leave(&name);
            }
            self.dirty_session(&name);
        }
    }

    // ── Reading ─────────────────────────────────────────────────────────────

    /// Reads, for every pane, the Claude conversation in front of it and what
    /// it runs in the background, from Claude Code's files: the session files
    /// (each read again only when it changed), each process's group (one
    /// call to the kernel), and each transcript as far as it has grown.
    pub fn sidebar_read(&mut self) {
        let now = util::now_ms();
        let claude_dir = paths::claude_dir();
        let reader = &mut self.sidebar.reader;
        let sessions = reader.sessions(&claude_dir.join("sessions"));
        let groups: Vec<Option<u32>> = sessions
            .iter()
            .map(|s| s.pid.and_then(process_group))
            .collect();
        let mut reads = std::collections::HashMap::new();
        let mut files = std::collections::HashSet::new();
        for (id, slot) in &self.panes {
            let front = slot.pane.foreground_pgid();
            let in_front: Vec<&claude::SessionInfo> = sessions
                .iter()
                .zip(&groups)
                .filter(|(_, g)| g.is_some() && **g == front)
                .map(|(s, _)| s)
                .collect();
            let mut read = PaneRead {
                session: claude::session_of(&sessions, &in_front),
                command: slot.pane.foreground_command(),
                ..PaneRead::default()
            };
            if let Some(session) = &read.session
                && let Some(file) = reader.transcript(&claude_dir, session)
            {
                files.insert(file.clone());
                // A transcript that can't be read leaves the tab shown
                // without what it would have added, never the others.
                if let Ok(work) =
                    reader.background_work(&file, session.started_at.unwrap_or(0), now)
                {
                    read.work = work;
                }
                read.turn = reader.last_turn(&file);
                read.said_at = std::fs::metadata(&file)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64);
                read.transcript = Some(file);
            }
            reads.insert(*id, read);
        }
        reader.retain(&files);
        let changed = reads != self.sidebar.panes;
        self.sidebar.panes = reads;
        self.sidebar.stale = false;
        if changed {
            self.dirty_sidebars();
        }
    }

    /// Every client showing a sidebar draws again.
    fn dirty_sidebars(&mut self) {
        let ids: Vec<ClientId> = self.sidebar.drawn.keys().copied().collect();
        for id in ids {
            if let Some(c) = self.clients.get_mut(&id) {
                c.dirty = true;
            }
        }
    }

    /// Every tick: sidebar.json read again when it changed, each session's
    /// sidebar fitted to its size and the settings, the panes read again,
    /// and the tips only the sidebar can tell are due; what was kept for a
    /// client, session or pane that has gone is let go. The sidebars draw
    /// again, for the times they show ("waiting on you 4m").
    pub fn sidebar_tick(&mut self) {
        self.sidebar_load();
        self.fit_sidebars();
        self.sidebar_read();
        self.sidebar_tips();
        let clients = &self.clients;
        self.sidebar.drawn.retain(|id, _| clients.contains_key(id));
        self.sidebar.hover.retain(|id, _| clients.contains_key(id));
        let sessions = &self.sessions;
        self.sidebar
            .sides
            .retain(|name, _| sessions.iter().any(|s| &s.name == name));
        let panes = &self.panes;
        self.sidebar.output_at.retain(|p, _| panes.contains_key(p));
        self.dirty_sidebars();
    }

    /// Before drawing: read again when a hook has come since the last read.
    pub fn sidebar_before_draw(&mut self) {
        if self.sidebar.stale && self.clients.values().any(|c| c.dirty) {
            self.sidebar_read();
        }
    }

    /// The tips only the sidebar can tell are due, asked once a run: the
    /// first merged tab and the first tab looping in the background (the
    /// sidebar's own comes when it is first focused).
    fn sidebar_tips(&mut self) {
        let tabs = self.sidebar_all_tabs();
        let due = [
            (
                "merged",
                tabs.iter().any(|t| t.merge == Some(MergeState::Merged)),
            ),
            (
                "loop",
                tabs.iter()
                    .any(|t| t.state == TabState::Background && t.tab_loop.is_some()),
            ),
        ];
        for (name, now) in due {
            if now && self.sidebar.tips_asked.insert(name) {
                self.tip(name, None);
            }
        }
    }

    // ── The model ───────────────────────────────────────────────────────────

    /// A session's tabs as the sidebar and mission control see them, in the
    /// session's order.
    pub fn sidebar_tabs(&self, session: &str) -> Vec<TabModel> {
        let Some(s) = self.session(session) else {
            return Vec::new();
        };
        let attached = self.clients.values().any(|c| c.session == s.name);
        s.tabs
            .iter()
            .enumerate()
            .filter_map(|(i, id)| self.tab_model(s, attached, *id, i + 1))
            .collect()
    }

    /// Every session's tabs, one list.
    pub fn sidebar_all_tabs(&self) -> Vec<TabModel> {
        self.sessions
            .iter()
            .flat_map(|s| self.sidebar_tabs(&s.name))
            .collect()
    }

    /// One tab (model.js's `tabsFrom` for one window).
    fn tab_model(&self, s: &Session, attached: bool, id: TabId, index: usize) -> Option<TabModel> {
        let tab = self.tabs.get(&id)?;
        let (pr, name) = split_pr(&self.tab_label(id));
        let panes = tab.layout.panes();
        // The pane whose state the tab shows, and since when: the longest
        // there of those in that state; else the first running Claude.
        let leader = self.agents.leader(&panes);
        let read_of = |p: PaneId| self.sidebar.panes.get(&p);
        let claude_pane = leader.map(|(p, _, _)| p).or_else(|| {
            panes
                .iter()
                .copied()
                .find(|p| read_of(*p).is_some_and(|r| r.session.is_some()))
        });
        let read = claude_pane.and_then(read_of);
        let session = read.and_then(|r| r.session.clone());
        let base = match leader.map(|(_, state, _)| state) {
            Some(State::Waiting) => TabState::Waiting,
            Some(State::Done) => TabState::Done,
            Some(State::Working) => TabState::Working,
            None => TabState::Idle,
        };
        // Read for a working tab too, since a turn an API error cut short
        // leaves its pane marked working (no Stop hook runs); its background
        // work counts only for a tab between turns.
        let mut work = match read {
            Some(r) if session.is_some() && base != TabState::Waiting => r.work.clone(),
            _ => claude::Work::default(),
        };
        if base == TabState::Working {
            work = claude::Work {
                api_error: work.api_error,
                ..claude::Work::default()
            };
        }
        let stopped = work.api_error.clone();
        let state = if stopped.is_some() {
            TabState::Waiting
        } else if work.count > 0 || work.scheduled.is_some() {
            TabState::Background
        } else {
            base
        };
        let agent = claude_pane.and_then(|p| self.agents.get(p));
        let opt = |k: &str| tab.opts.get(k).filter(|v| !v.is_empty()).cloned();
        let since = if let Some((_, at)) = &stopped {
            Some(at.div_euclid(1000)).filter(|s| *s > 0)
        } else if base == TabState::Idle {
            // When it last did anything: a Claude tab's last word in its
            // conversation, else the last output to its pane.
            read.and_then(|r| r.said_at)
                .or_else(|| self.sidebar.output_at.get(&tab.active).copied())
        } else {
            leader.map(|(_, _, since)| since).filter(|s| *s > 0)
        };
        let mut t = TabModel::new(id, index, &name);
        t.session = s.name.clone();
        t.attached = attached;
        t.current = s.current == id;
        t.zoomed = tab.zoomed;
        t.own_name = self.tab_own_name(id).map(str::to_string);
        t.color = self.tab_color(id);
        t.state = state;
        t.pinned = opt("@pinned").is_some_and(|v| v != "0");
        t.remind_at = opt("@remind_at")
            .and_then(|v| v.trim().parse::<i64>().ok())
            .filter(|n| *n != 0);
        t.remind_note = opt("@remind_note");
        t.autopilot = opt("@autopilot")
            .and_then(|v| v.trim().parse::<f64>().ok())
            .is_some_and(|n| n != 0.0);
        t.background = work.count;
        t.monitors = work.monitors.clone();
        t.tab_loop = work
            .loop_run
            .clone()
            .map(TabLoop::Run)
            .or_else(|| work.scheduled.clone().map(TabLoop::Scheduled));
        let some = |v: &Option<String>| v.clone().filter(|v| !v.is_empty());
        t.activity = agent.and_then(|a| some(&a.activity));
        t.wait = stopped
            .map(|(text, _)| text)
            .or_else(|| agent.and_then(|a| some(&a.wait)));
        t.loop_skill = agent.and_then(|a| some(&a.loop_name));
        t.phase = agent.and_then(|a| some(&a.phase));
        t.since = since;
        t.path = self.tab_path(id).map(std::path::Path::to_path_buf);
        t.command = read_of(tab.active).and_then(|r| r.command.clone());
        t.panes = panes;
        t.active = Some(tab.active);
        t.claude_pane = claude_pane;
        t.claude = session;
        t.turn = read.and_then(|r| r.turn.clone());
        // The PR its label shows, else the one GitHub says its branch has.
        t.pr = pr;
        // What GitHub and the landing queue say of it, as prs.js's mergeOf
        // and landing.js's landingOf gave it; none of GitHub's with the
        // github setting off.
        let facts = self.facts.get(&id);
        if let Some(p) = facts
            .and_then(|f| f.pr.as_ref())
            .filter(|_| settings::is_on("github"))
        {
            t.pr = t.pr.or(Some(p.number));
            t.pr_state = Some(p.state);
            t.merge = match p.merge {
                Some(Merge::Merged) => Some(MergeState::Merged),
                Some(Merge::Queued { position }) => {
                    t.position = Some(position);
                    Some(MergeState::Queued)
                }
                None if p.state == PrState::Merged => Some(MergeState::Merged),
                None => None,
            };
            t.checks = p.checks.filter(|_| p.state != PrState::Merged);
            t.review = p.review_decision.clone();
            t.size = Some((p.additions, p.deletions));
        }
        t.landing = facts.and_then(|f| f.landing);
        Some(t)
    }

    /// The notifications the sources have given, in the sidebar's shape:
    /// none with the sidebar_notifications setting off.
    fn sidebar_notices(&self) -> Vec<Notice> {
        if !settings::is_on("sidebar_notifications") {
            return Vec::new();
        }
        self.notices
            .iter()
            .flat_map(|(source, items)| {
                items.iter().map(move |item| Notice {
                    owner: Owner::Notice {
                        source: source.clone(),
                        id: item.id.clone(),
                    },
                    title: item.title.clone(),
                    detail: item.detail.clone(),
                })
            })
            .collect()
    }

    /// A session's tabs as its sidebar lists them: all, or those whose name
    /// has its / filter's text.
    fn sidebar_listed(&self, session: &str) -> Vec<TabModel> {
        let text = self
            .sidebar
            .sides
            .get(session)
            .filter(|s| s.focused)
            .and_then(|s| s.filter.as_ref())
            .map(|f| f.text.trim().to_lowercase())
            .unwrap_or_default();
        let tabs = self.sidebar_tabs(session);
        if text.is_empty() {
            return tabs;
        }
        tabs.into_iter()
            .filter(|t| t.name.to_lowercase().contains(&text))
            .collect()
    }

    /// The tabs in the order a session's sidebar lists them.
    fn sidebar_order(&self, session: &str) -> Vec<TabId> {
        let r = Rules::now();
        let tabs = self.sidebar_listed(session);
        let frozen = self
            .sidebar
            .sides
            .get(session)
            .and_then(|s| s.frozen.as_ref());
        model::sidebar_order(&tabs, frozen, &r)
            .iter()
            .map(|t| t.id)
            .collect()
    }

    // ── Drawing ─────────────────────────────────────────────────────────────

    /// Draws a session's sidebar into a client's frame, on the left of the
    /// panes, with the line between them in the accent color while it has
    /// the focus; gives where it went, or `None` when it isn't shown.
    pub fn sidebar_draw(&self, buf: &mut Buffer, s: &Session, client: ClientId) -> Option<Drawn> {
        if !self.sidebar_shown(&s.name) {
            return None;
        }
        let status = crate::status::lines();
        let rows = s
            .rows
            .saturating_sub(status)
            .min(buf.area.height.saturating_sub(status));
        let width = (s.sidebar - 1).min(buf.area.width);
        let area = Rect::new(0, 0, width, rows).intersection(buf.area);
        if area.is_empty() {
            return None;
        }
        let r = Rules::now();
        let side = self.sidebar.sides.get(&s.name);
        let focused = side.is_some_and(|x| x.focused);
        let tabs = self.sidebar_listed(&s.name);
        let notices = self.sidebar_notices();
        let view = View {
            focused,
            cursor: side.and_then(|x| x.cursor.as_ref()),
            frame: self.sidebar.frame,
            here: s.current,
            filter: side.and_then(|x| x.filter.as_ref()).filter(|_| focused),
            top: side.filter(|x| x.focused).and_then(|x| x.top),
            hover: self.sidebar.hover.get(&client),
        };
        let out = render::sidebar_lines(
            &tabs,
            side.and_then(|x| x.frozen.as_ref()),
            &notices,
            &s.name,
            area.width as usize,
            area.height as usize,
            &view,
            &r,
            &theme(),
        );
        render::draw(buf, area, &out.lines);
        if width < buf.area.width {
            let line = Style::new().fg(if focused {
                colors::ACCENT
            } else {
                crate::status::theme::palette().border
            });
            for y in 0..rows {
                buf[(width, y)].set_symbol("│").set_style(line);
            }
        }
        Some(Drawn {
            area,
            owners: out.owners,
            scroll: out.scroll,
            spins: out.spins,
        })
    }

    /// After a client is drawn: where its sidebar went, kept for the mouse
    /// and C-d; the list's place, kept while it has the focus; and the
    /// spinner's next turn, timed only while something spins.
    pub fn sidebar_drawn(&mut self, client: ClientId, session: &str, drawn: Option<Drawn>) {
        let Some(d) = drawn else {
            self.sidebar.drawn.remove(&client);
            return;
        };
        if let Some(side) = self.sidebar.sides.get_mut(session)
            && side.focused
        {
            side.top = Some(d.scroll.top);
        }
        let spins = d.spins;
        self.sidebar.drawn.insert(client, d);
        if spins && !self.sidebar.spin_pending {
            self.sidebar.spin_pending = true;
            self.send_later(SPIN, Event::Sidebar(SidebarEvent::Spin));
        }
    }

    /// What the sidebar's timers say.
    pub fn sidebar_event(&mut self, event: SidebarEvent) {
        match event {
            SidebarEvent::Spin => {
                self.sidebar.spin_pending = false;
                self.sidebar.frame = self.sidebar.frame.wrapping_add(1);
                let spinning: Vec<ClientId> = self
                    .sidebar
                    .drawn
                    .iter()
                    .filter(|(_, d)| d.spins)
                    .map(|(id, _)| *id)
                    .collect();
                for id in spinning {
                    if let Some(c) = self.clients.get_mut(&id) {
                        c.dirty = true;
                    }
                }
            }
        }
    }

    /// Whether a client's terminal should report every mouse movement for
    /// the sidebar: while it shows one and hover_highlight is on.
    pub fn sidebar_wants_motion(&self, client: ClientId) -> bool {
        self.sidebar.drawn.contains_key(&client) && settings::is_on("hover_highlight")
    }

    // ── Focus ───────────────────────────────────────────────────────────────

    /// Focus into a session's sidebar, its cursor on `cursor` or the tab you
    /// are in, the tabs held where they are while you browse them.
    pub fn sidebar_enter(&mut self, session: &str, cursor: Option<Owner>) {
        if !self.sidebar_shown(session) {
            return;
        }
        let Some(here) = self.session(session).map(|s| s.current) else {
            return;
        };
        let r = Rules::now();
        let frozen = model::frozen_from(&self.sidebar_tabs(session), &r);
        let side = self.sidebar.side(session);
        if !side.focused {
            side.frozen = Some(frozen);
            side.top = None;
        }
        side.focused = true;
        side.cursor = Some(cursor.unwrap_or(Owner::Tab(here)));
        self.dirty_session(session);
        if self.sidebar.tips_asked.insert("sidebar") {
            let client = self.latest_client_of(session);
            self.tip("sidebar", client);
        }
    }

    /// Focus back out of a session's sidebar, to the tab in front's pane.
    pub fn sidebar_leave(&mut self, session: &str) {
        let Some(side) = self.sidebar.sides.get_mut(session) else {
            return;
        };
        if !side.focused {
            return;
        }
        side.focused = false;
        side.frozen = None;
        side.filter = None;
        side.top = None;
        self.dirty_session(session);
    }

    /// A pane of this tab took the focus: its session's sidebar gives it up.
    pub fn sidebar_leave_tab(&mut self, tab: TabId) {
        if let Some(name) = self.session_of_tab(tab).map(|s| s.name.clone()) {
            self.sidebar_leave(&name);
        }
    }

    /// M-s: into the sidebar, or back out of it. A zoomed pane is unzoomed
    /// first, as moving to the sidebar pane unzoomed a tmux window; with no
    /// sidebar to go to, it says why.
    fn sidebar_toggle_focus(&mut self, client: ClientId) {
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        if self.sidebar.focused(&session) {
            self.sidebar_leave(&session);
            return;
        }
        let Some(s) = self.session(&session) else {
            return;
        };
        if s.sidebar == 0 {
            let why = if settings::is_on("sidebar") {
                "This window is too narrow for the sidebar."
            } else {
                "The sidebar is off. (Settings: Sidebar, or \"Turn the sidebar on or off\" in M-p)"
            };
            self.say(Some(client), why);
            return;
        }
        let current = s.current;
        if let Some(t) = self.tabs.get_mut(&current)
            && t.zoomed
        {
            t.zoomed = false;
            self.relayout(current);
        }
        self.sidebar_enter(&session, None);
    }

    /// "Turn the sidebar on or off" (the palette's): the sidebar setting.
    fn sidebar_toggle(&mut self, client: ClientId) {
        let on = settings::is_on("sidebar");
        if let Err(e) = settings::write_setting("sidebar", Value::Bool(!on)) {
            self.say(Some(client), &format!("Couldn't change the setting: {e}"));
            return;
        }
        settings::reload_settings();
        self.fit_sidebars();
    }

    /// Before a key's action runs: with the sidebar focused, an action on
    /// "the pane" acts on your pane, so the sidebar gives up the focus first
    /// (tmux.conf's `leave-sidebar`); M-a, M-1 to M-9 and the boxes keep it.
    /// Moving between panes from the sidebar comes back out to the right, and
    /// goes nowhere else. Gives whether the action is done with, and whether
    /// the sidebar had the focus.
    pub fn sidebar_before_action(&mut self, client: ClientId, action: &Action) -> (bool, bool) {
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return (false, false);
        };
        match action {
            Action::FocusSidebar => {
                self.sidebar_toggle_focus(client);
                return (true, false);
            }
            Action::ToggleSidebar => {
                self.sidebar_toggle(client);
                return (true, false);
            }
            _ => {}
        }
        if !self.sidebar.focused(&session) {
            return (false, false);
        }
        match action {
            Action::SelectPane(dir) => {
                if *dir == crate::mux::Direction::Right {
                    self.sidebar_leave(&session);
                }
                (true, true)
            }
            Action::NextNeedsYou
            | Action::SelectTab(_)
            | Action::Palette
            | Action::ListKeys
            | Action::Detach
            | Action::ResetTips
            | Action::TogglePin => (false, true),
            _ => {
                self.sidebar_leave(&session);
                (false, true)
            }
        }
    }

    /// After a key's action: M-1 to M-9 pressed from the sidebar leave its
    /// cursor on the tab they went to (sidebar.sh refocus).
    pub fn sidebar_after_action(&mut self, client: ClientId, was_focused: bool, action: &Action) {
        if !was_focused || !matches!(action, Action::SelectTab(_)) {
            return;
        }
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        let Some(here) = self.session(&session).map(|s| s.current) else {
            return;
        };
        if let Some(side) = self.sidebar.sides.get_mut(&session)
            && side.focused
        {
            side.cursor = Some(Owner::Tab(here));
            self.dirty_session(&session);
        }
    }

    /// M-Left from the leftmost pane: into the sidebar, which was the pane
    /// on the left in tmux.
    pub fn sidebar_from_left(&mut self, client: ClientId) {
        if let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) {
            self.sidebar_enter(&session, None);
        }
    }

    // ── Keys ────────────────────────────────────────────────────────────────

    /// A key while the sidebar has the focus (after the bindings); gives
    /// whether the sidebar took it.
    pub fn sidebar_key(&mut self, client: ClientId, key: Key, raw: &[u8]) -> bool {
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return false;
        };
        if !self.sidebar.focused(&session) {
            return false;
        }
        if !self.sidebar_shown(&session) {
            self.sidebar_leave(&session);
            return false;
        }
        self.sidebar_key_in(client, &session, key, raw);
        true
    }

    /// Text pasted while the sidebar has the focus goes into a filter being
    /// typed, or nowhere. Gives whether the sidebar took it.
    pub fn sidebar_paste(&mut self, client: ClientId, pasted: &str) -> bool {
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return false;
        };
        if !self.sidebar.focused(&session) {
            return false;
        }
        let side = self.sidebar.side(&session);
        if let Some(f) = side.filter.as_mut().filter(|f| f.typing) {
            f.text.push_str(&pasted.replace(['\n', '\r'], " "));
            self.sidebar_refilter(&session);
        }
        true
    }

    /// The cursor to the first tab a changed filter leaves.
    fn sidebar_refilter(&mut self, session: &str) {
        let first = self.sidebar_order(session).first().copied();
        let here = self.session(session).map(|s| s.current);
        if let Some(t) = first.or(here) {
            self.sidebar.side(session).cursor = Some(Owner::Tab(t));
        }
        self.dirty_session(session);
    }

    /// Moves the cursor to the `to`th stop: a tab, which comes to the front
    /// with the focus still in the sidebar, or past the last tab, one of the
    /// notifications.
    fn sidebar_move(&mut self, session: &str, to: isize) {
        let order = self.sidebar_order(session);
        let notices: Vec<Owner> = self
            .sidebar_notices()
            .into_iter()
            .map(|n| n.owner)
            .collect();
        let typing = self
            .sidebar
            .sides
            .get(session)
            .and_then(|s| s.filter.as_ref())
            .is_some_and(|f| f.typing);
        if to >= order.len() as isize && !notices.is_empty() && !typing {
            let i = ((to - order.len() as isize) as usize).min(notices.len() - 1);
            self.sidebar.side(session).cursor = Some(notices[i].clone());
            self.dirty_session(session);
            return;
        }
        if order.is_empty() {
            return;
        }
        let tab = order[to.clamp(0, order.len() as isize - 1) as usize];
        self.sidebar.side(session).cursor = Some(Owner::Tab(tab));
        if self.session(session).is_some_and(|s| s.current != tab) {
            self.select_tab(session, tab);
        }
        self.dirty_session(session);
    }

    /// Goes to a tab from the sidebar, the focus into its pane.
    fn sidebar_go(&mut self, session: &str, tab: TabId) {
        self.sidebar_leave(session);
        if let Some(s) = self.session_of_tab(tab).map(|s| s.name.clone()) {
            self.select_tab(&s, tab);
        }
    }

    fn sidebar_key_in(&mut self, client: ClientId, session: &str, key: Key, raw: &[u8]) {
        let order = self.sidebar_order(session);
        let notices: Vec<Owner> = self
            .sidebar_notices()
            .into_iter()
            .map(|n| n.owner)
            .collect();
        let Some(here) = self.session(session).map(|s| s.current) else {
            return;
        };
        let side = self.sidebar.side(session).clone();
        let cursor = side.cursor.clone().unwrap_or(Owner::Tab(here));
        let on_notice = notices.iter().position(|o| *o == cursor);
        let at = match on_notice {
            Some(i) => order.len() + i,
            None => order
                .iter()
                .position(|t| Owner::Tab(*t) == cursor)
                .unwrap_or(0),
        } as isize;
        let k = key.normalized();
        let none = k.mods.is_empty();
        let ctrl = k.mods == Mods::CTRL;
        let tab_focus = settings::is_on("tab_focus");
        let chosen = order.get(at.max(0) as usize).copied();

        // Typing a / filter: letters go into it and the cursor to the first
        // tab it leaves; the arrows move, Enter goes to the chosen tab, and
        // Esc clears it.
        if side.filter.as_ref().is_some_and(|f| f.typing) {
            match k.code {
                KeyCode::Up if none => self.sidebar_move(session, at - 1),
                KeyCode::Down if none => self.sidebar_move(session, at + 1),
                KeyCode::Char('p') if ctrl => self.sidebar_move(session, at - 1),
                KeyCode::Char('n') if ctrl => self.sidebar_move(session, at + 1),
                KeyCode::Tab | KeyCode::BackTab if tab_focus && !order.is_empty() => {
                    let n = order.len() as isize;
                    let by = if k.code == KeyCode::Tab { 1 } else { -1 };
                    self.sidebar_move(session, (at + by).rem_euclid(n));
                }
                KeyCode::Enter => {
                    let go = order
                        .iter()
                        .find(|t| Owner::Tab(**t) == cursor)
                        .or(order.first())
                        .copied();
                    if let Some(t) = go {
                        self.sidebar_go(session, t);
                    } else {
                        self.sidebar.side(session).filter = None;
                        self.dirty_session(session);
                    }
                }
                KeyCode::Esc => {
                    self.sidebar.side(session).filter = None;
                    self.sidebar_refilter(session);
                }
                KeyCode::Char('c') if ctrl => {
                    self.sidebar.side(session).filter = None;
                    self.sidebar_refilter(session);
                }
                KeyCode::Backspace => {
                    if let Some(f) = self.sidebar.side(session).filter.as_mut() {
                        f.text.pop();
                    }
                    self.sidebar_refilter(session);
                }
                KeyCode::Char(c) if none => {
                    if let Some(f) = self.sidebar.side(session).filter.as_mut() {
                        f.text.push(c);
                    }
                    self.sidebar_refilter(session);
                }
                _ => {}
            }
            return;
        }

        match k.code {
            KeyCode::Char('j') | KeyCode::Down if none => self.sidebar_move(session, at + 1),
            KeyCode::Char('n') if ctrl => self.sidebar_move(session, at + 1),
            KeyCode::Char('k') | KeyCode::Up if none => self.sidebar_move(session, at - 1),
            KeyCode::Char('p') if ctrl => self.sidebar_move(session, at - 1),
            KeyCode::Char('g') | KeyCode::Home if none => self.sidebar_move(session, 0),
            KeyCode::Char('G') | KeyCode::End if none => {
                self.sidebar_move(session, order.len() as isize - 1)
            }
            KeyCode::Char('d' | 'u') if ctrl => {
                self.sidebar_half_page(client, session, k, raw, &order, on_notice.is_some())
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l')
                if none || k.code == KeyCode::Enter =>
            {
                if on_notice.is_some() {
                    self.sidebar_open_notice(client, &cursor);
                } else if let Some(t) = chosen {
                    self.sidebar_go(session, t);
                }
            }
            KeyCode::Char('/') if none => {
                let text = side.filter.map(|f| f.text).unwrap_or_default();
                self.sidebar.side(session).filter = Some(Filter { text, typing: true });
                self.sidebar_refilter(session);
            }
            KeyCode::Esc if none && side.filter.is_some() => {
                // Esc clears a kept filter before it backs out.
                self.sidebar.side(session).filter = None;
                self.sidebar_refilter(session);
            }
            KeyCode::Esc | KeyCode::Left if none => self.sidebar_leave(session),
            KeyCode::Char('h' | 'q') if none => self.sidebar_leave(session),
            KeyCode::Char('c') if ctrl => self.sidebar_leave(session),
            // Tab and Shift-Tab go round its tabs and notifications, as on a
            // webpage, the last wrapping to the first.
            KeyCode::Tab if tab_focus => {
                let last = order.len() as isize + notices.len() as isize - 1;
                self.sidebar_move(session, if at >= last { 0 } else { at + 1 });
            }
            KeyCode::BackTab if tab_focus => {
                let last = order.len() as isize + notices.len() as isize - 1;
                self.sidebar_move(session, if at <= 0 { last } else { at - 1 });
            }
            KeyCode::Char(' ') if none => {
                if let Some(t) = chosen.filter(|_| on_notice.is_none()) {
                    self.sidebar_peek(client, t);
                }
            }
            KeyCode::Char('n') if none => {
                if notices.is_empty() {
                    self.say(Some(client), "No notifications");
                } else {
                    self.sidebar_move(session, order.len() as isize);
                }
            }
            KeyCode::Char('t') if none => {
                if let Some(t) = chosen {
                    self.sidebar_go(session, t);
                    self.act(client, Action::NewTab);
                }
            }
            KeyCode::Char('?') if none => {
                let rows = KEYS
                    .iter()
                    .map(|(k, what)| (k.to_string(), what.to_string()))
                    .collect();
                self.open(
                    client,
                    Box::new(crate::ui::keylist::KeyList::new(rows)),
                    None,
                );
            }
            // A number puts the cursor on that tab, in the sidebar, as j and
            // k do; one the sidebar doesn't list (filtered out) is gone to.
            KeyCode::Char(c @ '1'..='9') if none => {
                let n = c.to_digit(10).unwrap_or(0) as usize;
                let tabs = self.sidebar_tabs(session);
                let target = tabs.iter().find(|t| t.index == n).map(|t| t.id);
                match target.and_then(|id| order.iter().position(|t| *t == id)) {
                    Some(i) => self.sidebar_move(session, i as isize),
                    None => {
                        if let Some(t) = target {
                            self.sidebar_go(session, t);
                        }
                    }
                }
            }
            // Any other Alt or Ctrl key runs in the chosen tab, as if pressed
            // there.
            _ if k.mods.alt || k.mods.ctrl => {
                if let Some(t) = chosen {
                    self.sidebar_go(session, t);
                    self.press_key(client, key, raw);
                }
            }
            _ => {}
        }
    }

    /// C-d and C-u: the list half a screen on, and the cursor onto the tab
    /// there, as in Vim, stopping at the first and last tab rather than going
    /// on into the notifications; from a notification, C-u goes back to the
    /// last tab. With sidebar_half_page off, the key runs in the chosen tab.
    fn sidebar_half_page(
        &mut self,
        client: ClientId,
        session: &str,
        key: Key,
        raw: &[u8],
        order: &[TabId],
        on_notice: bool,
    ) {
        let down = key.code == KeyCode::Char('d');
        let cursor = self.sidebar.side(session).cursor.clone();
        if !settings::is_on("sidebar_half_page") {
            let at = cursor
                .as_ref()
                .and_then(Owner::tab)
                .or(order.first().copied());
            if let Some(t) = at {
                self.sidebar_go(session, t);
                self.press_key(client, key, raw);
            }
            return;
        }
        if order.is_empty() {
            return;
        }
        if on_notice {
            if !down {
                self.sidebar_move(session, order.len() as isize - 1);
            }
            return;
        }
        let Some(scroll) = self.sidebar.drawn.get(&client).map(|d| d.scroll.clone()) else {
            return;
        };
        let by = (scroll.room / 2).max(1) as isize * if down { 1 } else { -1 };
        let most = scroll.length.saturating_sub(scroll.room) as isize;
        self.sidebar.side(session).top = Some((scroll.top as isize + by).clamp(0, most) as usize);
        // The tab on the line half a screen on, else the nearest one past
        // it, never the tab the cursor is on, which a tab taller than half
        // the list would otherwise hold it on press after press.
        let here = cursor.as_ref().and_then(Owner::tab);
        let line = scroll.cursor_at as isize + by;
        let found = if down {
            scroll
                .owners
                .iter()
                .skip(line.max(0) as usize)
                .flatten()
                .find(|t| Some(**t) != here)
                .copied()
        } else {
            scroll
                .owners
                .iter()
                .take((line + 1).max(0) as usize)
                .rev()
                .flatten()
                .find(|t| Some(**t) != here)
                .copied()
        };
        let id = found.unwrap_or(if down {
            order[order.len() - 1]
        } else {
            order[0]
        });
        let to = order.iter().position(|t| *t == id).unwrap_or(0);
        self.sidebar_move(session, to as isize);
    }

    /// Presses a key as if the sidebar weren't there: in the tab's pane, or
    /// whatever it is bound to.
    fn press_key(&mut self, client: ClientId, key: Key, raw: &[u8]) {
        let parsed = crate::mux::input::Parsed {
            input: crate::mux::input::Input::Key(key),
            raw: raw.to_vec(),
        };
        self.dispatch(client, parsed);
    }

    // ── The mouse ───────────────────────────────────────────────────────────

    /// A mouse event, before the panes see it: on the sidebar, a click goes
    /// to a tab (a right click peeks at it), the wheel moves the cursor and
    /// the mouse lights the row it is over (hover_highlight); a press
    /// anywhere else takes the focus out of the sidebar. Drags and releases
    /// are the panes'. Gives whether the sidebar took it.
    pub fn sidebar_mouse(&mut self, client: ClientId, m: Mouse) -> bool {
        if matches!(m.kind, MouseKind::Drag | MouseKind::Release) {
            return false;
        }
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return false;
        };
        let drawn = self.sidebar.drawn.get(&client).cloned();
        let inside = drawn
            .as_ref()
            .is_some_and(|d| m.col <= d.area.right() && m.row < d.area.bottom());
        let Some(d) = drawn.filter(|_| inside) else {
            if self.sidebar.hover.remove(&client).is_some()
                && let Some(c) = self.clients.get_mut(&client)
            {
                c.dirty = true;
            }
            if m.kind == MouseKind::Press && !m.button.is_wheel() {
                self.sidebar_leave(&session);
            }
            return false;
        };
        let owner = (m.col < d.area.right())
            .then(|| d.owners.get((m.row - d.area.y) as usize).cloned().flatten())
            .flatten();
        match (m.kind, m.button) {
            (MouseKind::Move, _) => {
                let lit = owner.filter(|_| settings::is_on("hover_highlight"));
                let changed = self.sidebar.hover.get(&client) != lit.as_ref();
                match lit {
                    Some(o) => {
                        self.sidebar.hover.insert(client, o);
                    }
                    None => {
                        self.sidebar.hover.remove(&client);
                    }
                }
                if changed && let Some(c) = self.clients.get_mut(&client) {
                    c.dirty = true;
                }
            }
            (MouseKind::Press, MouseButton::WheelUp | MouseButton::WheelDown) => {
                // The wheel browses, as j and k do, the focus coming into
                // the sidebar.
                if !self.sidebar.focused(&session) {
                    self.sidebar_enter(&session, None);
                }
                let by = if m.button == MouseButton::WheelUp {
                    Key::new(KeyCode::Up, Mods::NONE)
                } else {
                    Key::new(KeyCode::Down, Mods::NONE)
                };
                self.sidebar_key_in(client, &session, by, &[]);
            }
            (MouseKind::Press, MouseButton::Left) => match owner {
                Some(Owner::Tab(t)) => self.sidebar_go(&session, t),
                Some(notice @ Owner::Notice { .. }) => self.sidebar_open_notice(client, &notice),
                None => {}
            },
            (MouseKind::Press, MouseButton::Right) => {
                if let Some(Owner::Tab(t)) = owner {
                    self.sidebar_peek(client, t);
                }
            }
            _ => {}
        }
        true
    }

    // ── What it opens ───────────────────────────────────────────────────────

    /// A look at a tab without going there; Enter in it goes there.
    fn sidebar_peek(&mut self, client: ClientId, tab: TabId) {
        let Some(session) = self.session_of_tab(tab).map(|s| s.name.clone()) else {
            return;
        };
        let Some(model) = self
            .sidebar_tabs(&session)
            .into_iter()
            .find(|t| t.id == tab)
        else {
            return;
        };
        let pane = model.claude_pane.or(model.active);
        let screen = pane
            .and_then(|p| self.panes.get(&p))
            .map(|slot| slot.pane.text(slot.pane.size().1 as usize))
            .unwrap_or_default();
        let turn = model.claude.as_ref().and(model.turn.clone());
        let peek = Peek::new(
            format!("{} {}", model.index, model.name),
            turn,
            model.state.busy(),
            screen,
        );
        self.open(
            client,
            Box::new(peek),
            Some(Box::new(move |server: &mut Server, _client, outcome| {
                if outcome == Outcome::Yes {
                    server.sidebar_go(&session, tab);
                }
            })),
        );
    }

    /// A notification, chosen: where its item goes (a tab, a link, a
    /// directory for a new tab), or, with nowhere to go, its words said.
    fn sidebar_open_notice(&mut self, client: ClientId, owner: &Owner) {
        let Owner::Notice { source, id } = owner else {
            return;
        };
        let Some(item) = self
            .notices
            .get(source)
            .and_then(|items| items.iter().find(|i| &i.id == id))
            .cloned()
        else {
            return;
        };
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        match item.target {
            Target::Tab(t) => self.sidebar_go(&session, t),
            Target::Url(url) => {
                self.sidebar_leave(&session);
                crate::links::open_in_background(url);
            }
            Target::Dir(dir) => {
                self.sidebar_leave(&session);
                if let Err(e) = self.new_tab(&session, Some(dir), None, None, false) {
                    self.say(Some(client), &format!("Couldn't open a tab there: {e}"));
                }
            }
            Target::None => {
                let words = if item.detail.is_empty() {
                    item.title
                } else {
                    format!("{}\n{}", item.title, item.detail)
                };
                self.say(Some(client), &words);
            }
        }
    }

    /// The cursor onto the first notification, in the sidebar of the
    /// client's session (sidebar.sh notices with notice_list off); with no
    /// sidebar there, it says why.
    fn sidebar_to_notices(&mut self, client: ClientId) {
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        let Some(first) = self.sidebar_notices().into_iter().next() else {
            self.say(Some(client), "No notifications");
            return;
        };
        if !self.sidebar_shown(&session) {
            let why = if settings::is_on("sidebar") {
                "The notifications are in the sidebar, which this window is too narrow for."
            } else {
                "The notifications are in the sidebar, which is off."
            };
            self.say(Some(client), why);
            return;
        }
        self.sidebar.side(&session).filter = None;
        self.sidebar_enter(&session, Some(first.owner));
    }

    // ── M-a ─────────────────────────────────────────────────────────────────

    /// M-a: the next of what needs you, in the sidebar's order, across every
    /// session: the tabs under Needs you, then the notifications while there
    /// are any (needs_notices setting). Pressed from the sidebar, the cursor
    /// stays in a sidebar, on the tab it went to.
    pub fn sidebar_next_needs_you(&mut self, client: ClientId) {
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        let Some(here) = self.session(&session).map(|s| s.current) else {
            return;
        };
        let r = Rules::now();
        let all = self.sidebar_all_tabs();
        let mut stops: Vec<Stop> = model::needs_you(&all, &r)
            .iter()
            .map(|t| Stop {
                place: Place::Tab(t.id),
                why: if t.pinned {
                    Why::Pinned
                } else if model::reminder_due(t, &r) {
                    Why::Reminder
                } else if t.state == TabState::Waiting {
                    Why::Waiting
                } else {
                    Why::Done
                },
            })
            .collect();
        let notices = self.sidebar_notices();
        if settings::is_on("needs_notices") && !notices.is_empty() {
            stops.push(Stop {
                place: Place::Notices,
                why: Why::Notices,
            });
        }
        let side = self.sidebar.sides.get(&session).filter(|s| s.focused);
        let from_sidebar = side.is_some();
        let on_notices = side
            .and_then(|s| s.cursor.as_ref())
            .is_some_and(|c| matches!(c, Owner::Notice { .. }));
        match self.sidebar.walk.next(&stops, here) {
            Next::Nothing => self.say(Some(client), "Nothing needs you."),
            Next::OnlyHere => self.say(Some(client), "This is the only tab that needs you."),
            Next::Go(i) => match stops[i].place {
                Place::Notices => self.sidebar_to_notices(client),
                Place::Tab(t) => {
                    let Some(tab) = all.iter().find(|m| m.id == t) else {
                        return;
                    };
                    // The tab's pane first, so switching sessions lands on
                    // it rather than on whatever was last focused there.
                    if let Some(pane) = tab.claude_pane.or(tab.active) {
                        self.focus_pane(pane);
                    }
                    if tab.session != session {
                        self.switch_client(client, &tab.session);
                    }
                    // Back from the notifications in this same tab's
                    // sidebar, the tab's pane takes the focus instead.
                    let back_here = on_notices && t == here;
                    if from_sidebar && !back_here {
                        self.sidebar_enter(&tab.session, Some(Owner::Tab(t)));
                    }
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_labels_pr_is_split_from_its_name() {
        assert_eq!(split_pr("#12 Fix it"), (Some(12), "Fix it".into()));
        assert_eq!(split_pr("#x y"), (None, "#x y".into()));
        assert_eq!(split_pr("plain"), (None, "plain".into()));
    }
}
