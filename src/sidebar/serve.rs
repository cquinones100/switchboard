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

use std::collections::BTreeSet;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use serde_json::Value;

use super::model::{self, MergeState, Rules, Stage, TabLoop, TabModel, TabState};
use super::peek::Peek;
use super::render::{self, Filter, Notice, Owner, View};
use super::walk::{Next, Place, Stop, Why};
use super::{Drawn, PaneRead, SidebarEvent, claude, config, notices, text};
use crate::agent::State;
use crate::facts::{Merge, PrState};
use crate::mux::compose::colors;
use crate::mux::input::{Key, KeyCode, Mods, Mouse, MouseButton, MouseKind};
use crate::mux::keys::Action;
use crate::mux::server::{ClientId, Event, Server, Session};
use crate::mux::{PaneId, TabId};
use crate::ui::{Outcome, PickItem, Picker, Theme};
use crate::{paths, settings, util};

/// How long after a half-screen move by the wheel the rest of the same
/// flick is let go: a trackpad or a fast wheel sends many events at once.
const FLICK: Duration = Duration::from_millis(150);

mod history;
mod menu;
mod window;

/// How often the spinner turns.
const SPIN: Duration = Duration::from_millis(text::SPINNER_MS);

/// The sidebar's keys, for `?` (render.js's `HELP.sidebar`, less what comes
/// with mission control), and mission control's Keys view.
pub const KEYS: &[(&str, &str)] = &[
    ("j k", "show a tab"),
    ("g G", "first, last"),
    (
        "z",
        "fold its group, or the notifications; unfold a folded one",
    ),
    ("^d ^u", "half a screen down, up"),
    ("↵ l", "go to the tab"),
    ("h esc q ^c", "back to your pane"),
    ("1-9", "go to that tab"),
    ("/", "find a tab by title (↵ go, esc clear)"),
    (
        "tab ⇧tab",
        "next, last tab (with tab_focus off: mission control)",
    ),
    ("M-space", "mission control"),
    ("space", "peek at it"),
    ("m ⇧F10", "its menu, as a right click opens"),
    ("a", "answer it, or send it a message"),
    (
        "n",
        "to the notifications, when there are any, unfolding them",
    ),
    ("t", "new tab running Claude there (M-t)"),
    ("i", "its full title"),
    ("d L", "its diff, or lazygit, in its pane's place"),
    (
        "v",
        "select several tabs (space or v each, ↵ m what to do, esc)",
    ),
    ("M-x C-x", "run that key there"),
    ("M-s", "back to your pane"),
    ("?", "these keys"),
];

/// The shades the sidebar is drawn in: the light ones when the theme is
/// light, the canvas's for a truecolor terminal (src/status/theme.rs), as
/// the boxes are.
fn theme() -> Theme {
    crate::status::theme::theme()
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
        // A setting that changes what the sidebar lists (sidebar_reminders)
        // may leave a focused cursor on a tab it no longer lists.
        let focused: Vec<String> = self
            .sidebar
            .sides
            .iter()
            .filter(|(_, s)| s.focused)
            .map(|(n, _)| n.clone())
            .collect();
        for name in focused {
            // And a held layout keeps no place for a tab now left out for
            // its reminder, so it shows under Needs you once it comes due.
            if !settings::is_on("sidebar_reminders") {
                let r = Rules::now();
                let hidden: Vec<TabId> = self
                    .sidebar_marked(&name)
                    .iter()
                    .filter(|t| model::own_stage(t, &r) == Stage::Later)
                    .map(|t| t.id)
                    .collect();
                if let Some(frozen) = self.sidebar.side(&name).frozen.as_mut() {
                    frozen.retain(|id, _| !hidden.contains(id));
                }
            }
            self.sidebar_cursor_back(&name);
            self.dirty_session(&name);
        }
    }

    // ── Reading ─────────────────────────────────────────────────────────────

    /// Asks Haiku for a sentence on each pane's last turn that has ended
    /// (a reply, no call waiting, its Claude neither working nor waiting),
    /// once a turn, in a task of its own, with Settings' `turn_summaries`
    /// and `ux2` on, the cards that show it being UX 2.0's
    /// (src/mission/summary.rs). A turn begun over a day ago is left as
    /// it is, so a server starting doesn't send every old tab's.
    fn summarize_turns(&mut self, reads: &std::collections::HashMap<PaneId, PaneRead>) {
        if !settings::is_on("turn_summaries") || !settings::is_on("ux2") {
            return;
        }
        let day_ago = util::now_ms() - 86_400_000;
        let away = self.away();
        for (pane, read) in reads {
            let (Some(session), Some(turn)) = (&read.session, &read.turn) else {
                continue;
            };
            // What was kept for the session shows whatever the turn's age.
            self.mission.summaries.load(&session.session_id);
            let (Some(at), Some(reply)) = (turn.at, &turn.reply) else {
                continue;
            };
            let ended = turn.pending.is_none()
                && !matches!(
                    self.agents.state(*pane),
                    Some(State::Working | State::Waiting)
                );
            // Held while you're away (`pause_when_away`), as the lookups
            // are, and asked once you're back.
            if !ended
                || at < day_ago
                || away
                || !self.mission.summaries.ask_now(&session.session_id, at)
            {
                continue;
            }
            let program = util::which("claude").unwrap_or_else(|| "claude".into());
            let (session, prompt, reply) = (
                session.session_id.clone(),
                turn.prompt.clone(),
                reply.clone(),
            );
            let tx = self.tx.clone();
            tokio::spawn(async move {
                let text = crate::mission::summary::ask(
                    &program,
                    &crate::mission::summary::dir(),
                    &prompt,
                    &reply,
                )
                .await;
                let _ = tx.send(Event::Mission(crate::mission::MissionEvent::Summary {
                    session,
                    turn_at: at,
                    text,
                }));
            });
        }
    }

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
                claude: !in_front.is_empty()
                    || crate::agent::sessions::named_like_claude(&slot.pane),
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
        let interrupted: Vec<(PaneId, i64)> = reads
            .iter()
            .filter_map(|(p, r)| Some((*p, r.work.interrupted_at?)))
            .collect();
        // When Claude Code says each pane's session went idle.
        let idle: Vec<(PaneId, i64)> = reads
            .iter()
            .filter_map(|(p, r)| Some((*p, r.session.as_ref()?.idle_since?)))
            .collect();
        // A question asked since the session started, still unanswered.
        let asks: Vec<(PaneId, String, i64, String)> = reads
            .iter()
            .filter_map(|(p, r)| {
                let (question, at, call) = r.turn.as_ref()?.asks.clone()?;
                let started = r.session.as_ref()?.started_at.unwrap_or(0);
                (at >= started).then_some((*p, question, at, call))
            })
            .collect();
        // The call each pane's turn is stopped on, for a wait whose hook
        // named no tool.
        let pending: Vec<(PaneId, String, bool, String)> = reads
            .iter()
            .filter_map(|(p, r)| {
                let turn = r.turn.as_ref()?;
                let only = turn.unanswered == 1;
                Some((*p, turn.pending_call.clone(), only, turn.pending.clone()?))
            })
            .collect();
        let changed = reads != self.sidebar.panes;
        self.summarize_turns(&reads);
        self.sidebar.panes = reads;
        self.sidebar.stale = false;
        self.sidebar_hand_back(&interrupted);
        for (pane, at) in idle {
            if self.agent_went_idle(pane, at) {
                self.dirty_sidebars();
            }
        }
        for (pane, question, at, call) in asks {
            if self.agent_asked(pane, at, &call, &question) {
                self.dirty_sidebars();
            }
        }
        for (pane, call, only, words) in pending {
            if self.agent_name_wait(pane, &call, only, &words) {
                self.dirty_sidebars();
            }
        }
        if changed {
            self.dirty_sidebars();
        }
    }

    /// A turn you interrupted, waiting on a request or working, runs no
    /// hook, so its pane is left as it was: each pane whose transcript says so is handed
    /// back as idle (`agent_interrupted`, the daemon's `handBack`), once for
    /// each interruption (`at`, ms), a parked one included, as every pane is
    /// read.
    fn sidebar_hand_back(&mut self, interrupted: &[(PaneId, i64)]) {
        let panes = &self.panes;
        self.sidebar
            .handed_back
            .retain(|p, _| panes.contains_key(p));
        for &(pane, at) in interrupted {
            if self.sidebar.handed_back.get(&pane) != Some(&at) && self.agent_interrupted(pane, at)
            {
                self.sidebar.handed_back.insert(pane, at);
                self.dirty_sidebars();
            }
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
        // The Notifications window is no tab (src/sidebar/notices.rs).
        s.tabs
            .iter()
            .enumerate()
            .filter(|(_, id)| !self.is_notices_tab(**id))
            .filter_map(|(i, id)| {
                let mut tab = self.tab_model(s, attached, *id, self.tab_number(*id))?;
                tab.place = i + 1;
                Some(tab)
            })
            .collect()
    }

    /// Every session's tabs, one list.
    pub fn sidebar_all_tabs(&self) -> Vec<TabModel> {
        self.sessions
            .iter()
            .flat_map(|s| self.sidebar_tabs(&s.name))
            .collect()
    }

    /// The repository a session's sidebar is filtered to, while Settings'
    /// sidebar_repo_filter is on.
    pub(crate) fn sidebar_repo(&self, session: &str) -> Option<&std::path::Path> {
        if !settings::is_on("sidebar_repo_filter") {
            return None;
        }
        self.sidebar.sides.get(session)?.repo.as_deref()
    }

    /// A session's tabs as its sidebar groups them: those of another
    /// repository than its filter marked `elsewhere`, for Other repos.
    pub(crate) fn sidebar_marked(&self, session: &str) -> Vec<TabModel> {
        let mut tabs = self.sidebar_tabs(session);
        if let Some(repo) = self.sidebar_repo(session) {
            for t in &mut tabs {
                t.elsewhere = t.repo_dir.as_deref() != Some(repo);
            }
        }
        tabs
    }

    /// Every session's tabs as M-a and M-A walk them: those under a
    /// session's Other repos left out of Needs you and Idle.
    fn sidebar_walked_tabs(&self) -> Vec<TabModel> {
        self.sessions
            .iter()
            .flat_map(|s| self.sidebar_marked(&s.name))
            .collect()
    }

    /// The groups a session's sidebar draws folded to one line, of those
    /// it draws (the held layout's included): each folded with `z` or a
    /// click, and Other repos while its filter is on and it wasn't
    /// unfolded; none while a / search is on, from the / that starts it,
    /// so its matches are all reachable.
    pub(crate) fn sidebar_folded(&self, session: &str, tabs: &[TabModel]) -> BTreeSet<Stage> {
        let side = self.sidebar.sides.get(session);
        let searching = side
            .filter(|s| s.focused)
            .is_some_and(|s| s.filter.is_some());
        if searching {
            return BTreeSet::new();
        }
        let filtered = self.sidebar_repo(session).is_some();
        let frozen = side.and_then(|s| s.frozen.as_ref());
        model::grouped(tabs, frozen, &Rules::now())
            .iter()
            .map(|g| g.stage)
            .filter(|stage| match stage {
                Stage::OtherRepos => filtered && !side.is_some_and(|s| s.repo_open),
                _ => side.is_some_and(|s| s.folded.contains(stage)),
            })
            .collect()
    }

    /// How many tabs wait on a reminder while the sidebar leaves its
    /// Reminders group out (sidebar_reminders off), for the top line's
    /// chip; 0 while the group shows.
    fn sidebar_reminders_hidden(&self, session: &str) -> usize {
        if settings::is_on("sidebar_reminders") {
            return 0;
        }
        let r = Rules::now();
        self.sidebar_marked(session)
            .iter()
            .filter(|t| model::own_stage(t, &r) == Stage::Later)
            .count()
    }

    /// One tab (model.js's `tabsFrom` for one window).
    fn tab_model(&self, s: &Session, attached: bool, id: TabId, index: usize) -> Option<TabModel> {
        let tab = self.tabs.get(&id)?;
        let (pr, name) = split_pr(&self.tab_label(id));
        // A note item's tab shows its whole first line, which the sidebar
        // wraps to its three lines, while its name is that item cut short.
        let name = self.tab_full_title(id).unwrap_or(name);
        let panes = tab.layout.panes();
        // Its Claude is looked for behind a tool in its pane's place too
        // (the diff, lazygit, a view), so the tab keeps the stage its
        // Claude is in rather than falling to Other.
        let with_parked = self.tab_panes(id);
        // The pane whose state the tab shows, and since when: the longest
        // there of those in that state; else the first running Claude.
        let leader = self.agents.leader(&with_parked);
        let read_of = |p: PaneId| self.sidebar.panes.get(&p);
        let claude_pane = leader.map(|(p, _, _)| p).or_else(|| {
            with_parked
                .iter()
                .copied()
                .find(|p| read_of(*p).is_some_and(|r| r.session.is_some()))
                .or_else(|| {
                    with_parked
                        .iter()
                        .copied()
                        .find(|p| read_of(*p).is_some_and(|r| r.claude))
                })
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
        // A working tab's loop counts only for its round, which its card
        // shows.
        let round = work.loop_run.as_ref().map_or(0, |r| r.round);
        if base == TabState::Working {
            work = claude::Work {
                api_error: work.api_error,
                ..claude::Work::default()
            };
        }
        let stopped = work.api_error.clone();
        // A turn that ends while only watches run is Done, under Needs you,
        // until you see it (the hook state goes Idle), as a question is
        // Waiting whatever runs behind it; then Monitoring.
        let finished_over_watches = base == TabState::Done
            && work.count > 0
            && work.monitors.len() == work.count
            && work.loop_run.is_none()
            && work.scheduled.is_none();
        let state = if stopped.is_some() {
            TabState::Waiting
        } else if finished_over_watches {
            base
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
        t.opened = settings::is_on("tab_age")
            .then(|| crate::tabs::opened(&tab.opts))
            .flatten();
        t.pinned = opt("@pinned").is_some_and(|v| v != "0");
        t.remind_at = opt("@remind_at")
            .and_then(|v| v.trim().parse::<i64>().ok())
            .filter(|n| *n != 0);
        t.remind_note = opt("@remind_note");
        t.routine_next = opt(crate::routines::TAB_NEXT).and_then(|v| v.trim().parse::<i64>().ok());
        // A landed tab made yours (`@autopilot_yours`) is a plain tab here.
        t.autopilot = opt("@autopilot")
            .and_then(|v| v.trim().parse::<f64>().ok())
            .is_some_and(|n| n != 0.0)
            && opt("@autopilot_yours").is_none();
        t.autopilot_landed = t.autopilot && opt("@autopilot_left").is_some();
        t.autopilot_autoland = t.autopilot && opt("@autopilot_autoland").is_some();
        // Notes beside this tab's work, or, on Notes' own tab while it is
        // away, where it went (model.js's notesHere, notesAway, notesOwner).
        let marked = |p: &PaneId, name: &str| {
            self.panes
                .get(p)
                .and_then(|slot| slot.opts.get(name))
                .is_some_and(|v| !v.is_empty() && v != "0")
        };
        t.notes_here = panes.len() > 1 && panes.iter().any(|p| marked(p, "@notes_pane"));
        t.notes_away = panes.iter().any(|p| marked(p, "@notes_placeholder"));
        if t.notes_away {
            t.notes_owner = self.notes_owner(&s.name);
        }
        t.background = work.count;
        t.monitors = work.monitors.clone();
        t.round = round;
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
        t.ask = agent
            .filter(|a| a.wait.is_some())
            .and_then(|a| a.ask.clone());
        t.doing = agent
            .filter(|a| a.state == Some(crate::agent::State::Working))
            .and_then(|a| a.doing.clone());
        t.context = agent.and_then(|a| a.context.as_ref()).map(|c| c.used);
        t.loop_skill = agent.and_then(|a| some(&a.loop_name));
        // A tab with no Claude (update-target's) shows the step its program
        // set on its pane.
        t.phase = agent.and_then(|a| some(&a.phase)).or_else(|| {
            with_parked
                .iter()
                .find_map(|p| self.agents.get(*p).and_then(|a| some(&a.phase)))
        });
        t.since = since;
        t.path = self.tab_path(id).map(std::path::Path::to_path_buf);
        t.repo = t.path.as_deref().and_then(|p| self.sources.origin_kept(p));
        t.repo_dir = self
            .tab_names
            .shown
            .get(&id)
            .and_then(|shown| shown.repo_dir.clone());
        t.command = read_of(tab.active).and_then(|r| r.command.clone());
        t.panes = panes;
        t.active = Some(tab.active);
        t.claude_pane = claude_pane;
        t.claude = session;
        t.turn = read.and_then(|r| r.turn.clone());
        // Shown only while the setting is on, whatever was kept before.
        t.summary = read
            .filter(|_| settings::is_on("turn_summaries"))
            .and_then(|r| Some((r.session.as_ref()?, r.turn.as_ref()?.at?)))
            .and_then(|(s, at)| self.mission.summaries.get(&s.session_id, at))
            .map(str::to_string);
        // The PR its label shows, else the one GitHub says its branch has.
        t.pr = pr;
        t.pr_repo = self.tab_pr_repo(id).map(ToString::to_string);
        // What GitHub and the landing queue say of it, as prs.js's mergeOf
        // and landing.js's landingOf gave it; none of GitHub's with the
        // github setting off.
        let facts = self.facts.get(&id);
        if let Some(p) = facts
            .and_then(|f| f.pr.as_ref())
            .filter(|_| settings::is_on("github"))
        {
            // The repository GitHub answered from, with its number, when the
            // tab's name didn't say (a gone worktree's branch found in a
            // fork's upstream).
            if t.pr_repo.is_none() && t.pr.is_none_or(|n| n == p.number) {
                t.pr_repo = Some(p.repo.clone());
            }
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
        // Landed straight on a branch (`land-lock landed`): Merged, as a
        // merged PR is, unless GitHub answered for a PR of its own.
        // Only while the tab is still in the landed repository's main
        // clone, on its branch then, read from git's files.
        if t.pr_state.is_none()
            && let Some(mark) = opt(model::LANDED).as_deref().and_then(model::landed_mark)
            && t.path
                .as_deref()
                .and_then(crate::tabs::git::locate)
                .is_some_and(|p| model::landed_here(&mark, &p))
        {
            t.landed_on = Some((mark.hash, mark.branch));
            t.merge = Some(MergeState::Merged);
        }
        Some(t)
    }

    /// The notifications the sources have given, each source's list in the
    /// order the kinds are shown (`notices::ORDER`): none with the
    /// sidebar_notifications setting off.
    pub(crate) fn notice_lists(&self) -> Vec<(&str, &[crate::items::Item])> {
        if !settings::is_on("sidebar_notifications") {
            return Vec::new();
        }
        let mut lists: Vec<(&str, &[crate::items::Item])> = self
            .notices
            .iter()
            .filter(|(_, items)| !items.is_empty())
            .map(|(source, items)| (source.as_str(), items.as_slice()))
            .collect();
        lists.sort_by_key(|(source, _)| notices::rank(source));
        lists
    }

    /// A notification as the Notifications view and window draw it: with a
    /// time, its own, else when the history says it came; none while the
    /// history is off, since what it read before isn't kept up to date.
    pub(crate) fn notice_dated(
        &self,
        source: &str,
        item: &crate::items::Item,
    ) -> crate::items::Item {
        let mut item = item.clone();
        if item.time.is_none() && Self::notice_history_on() {
            item.time = self.sidebar.history.came(source, &item.id);
        }
        item
    }

    /// The notifications in the sidebar's shape: a row for each kind, with
    /// how many there are (`notices::summary`), named by its first, so the
    /// tabs keep the room however many come in; the Notifications window
    /// lists each.
    fn sidebar_notices(&self) -> Vec<Notice> {
        self.notice_lists()
            .into_iter()
            .map(|(source, items)| {
                let (title, detail) = notices::summary(source, items);
                Notice {
                    owner: Owner::Notice {
                        source: source.to_string(),
                        id: items[0].id.clone(),
                    },
                    title,
                    detail,
                    // The newest one's time; a meeting's is when it starts,
                    // not when it came.
                    time: (source != "meeting")
                        .then(|| items.iter().filter_map(|i| i.time).max())
                        .flatten()
                        .map(|ms| ms.div_euclid(1000)),
                }
            })
            .collect()
    }

    /// A session's tabs as its sidebar lists them: all, or those whose name
    /// has its / filter's text; those waiting on a reminder left out while
    /// the sidebar leaves its Reminders group out (sidebar_reminders off).
    fn sidebar_listed(&self, session: &str) -> Vec<TabModel> {
        let text = self
            .sidebar
            .sides
            .get(session)
            .filter(|s| s.focused)
            .and_then(|s| s.filter.as_ref())
            .map(|f| f.text.trim().to_lowercase())
            .unwrap_or_default();
        let mut tabs = self.sidebar_marked(session);
        if !settings::is_on("sidebar_reminders") {
            let r = Rules::now();
            tabs.retain(|t| model::own_stage(t, &r) != Stage::Later);
        }
        if text.is_empty() {
            return tabs;
        }
        tabs.into_iter()
            .filter(|t| t.name.to_lowercase().contains(&text))
            .collect()
    }

    /// The layout to hold while you browse: every tab where it is now,
    /// those the sidebar leaves out for their reminders aside, so one that
    /// comes due meanwhile shows under Needs you rather than Reminders.
    fn sidebar_frozen_now(&self, session: &str, r: &Rules) -> model::Frozen {
        let mut tabs = self.sidebar_marked(session);
        if !settings::is_on("sidebar_reminders") {
            tabs.retain(|t| model::own_stage(t, r) != Stage::Later);
        }
        model::frozen_from(&tabs, r)
    }

    /// What j and k step through in a session's sidebar, in the order it
    /// draws them, by the groups as drawn, the held layout's included, so
    /// a tab that moved while you browse is where it shows: each tab, and
    /// a folded group's line in its tabs' place.
    fn sidebar_stops(&self, session: &str) -> Vec<Owner> {
        let tabs = self.sidebar_listed(session);
        let folded = self.sidebar_folded(session, &tabs);
        let frozen = self
            .sidebar
            .sides
            .get(session)
            .and_then(|s| s.frozen.as_ref());
        model::grouped(&tabs, frozen, &Rules::now())
            .into_iter()
            .flat_map(|g| {
                if folded.contains(&g.stage) {
                    vec![Owner::Group(g.stage)]
                } else {
                    g.tabs.iter().map(|t| Owner::Tab(t.id)).collect()
                }
            })
            .collect()
    }

    /// The tabs in the order a session's sidebar lists them, those under
    /// a folded group left out.
    fn sidebar_order(&self, session: &str) -> Vec<TabId> {
        self.sidebar_stops(session)
            .iter()
            .filter_map(Owner::tab)
            .collect()
    }

    /// The group a tab is under as the sidebar draws it, the held layout's
    /// included, whatever a / search shows.
    fn sidebar_group_of(&self, session: &str, tab: TabId) -> Option<Stage> {
        let tabs = self.sidebar_marked(session);
        let frozen = self
            .sidebar
            .sides
            .get(session)
            .and_then(|s| s.frozen.as_ref());
        model::grouped(&tabs, frozen, &Rules::now())
            .iter()
            .find(|g| g.tabs.iter().any(|t| t.id == tab))
            .map(|g| g.stage)
    }

    /// Unfolds the group a tab is under, so a tab gone to (M-a, M-1, a
    /// / search's match, one chosen in a menu) is listed.
    pub(crate) fn sidebar_unfold_to(&mut self, session: &str, tab: TabId) {
        // The group it shows under while you browse, and the one it is in
        // now, which it goes back to when the held layout is let go.
        let held = self.sidebar_group_of(session, tab);
        let live = model::grouped(&self.sidebar_marked(session), None, &Rules::now())
            .iter()
            .find(|g| g.tabs.iter().any(|t| t.id == tab))
            .map(|g| g.stage);
        let side = self.sidebar.side(session);
        let mut changed = false;
        for stage in [held, live].into_iter().flatten() {
            changed |= match stage {
                Stage::OtherRepos => !std::mem::replace(&mut side.repo_open, true),
                _ => side.folded.remove(&stage),
            };
        }
        if changed {
            self.dirty_session(session);
        }
    }

    /// The notifications j, k and Tab step through: none while they are
    /// folded.
    fn sidebar_notice_stops(&self, session: &str) -> Vec<Owner> {
        if self
            .sidebar
            .sides
            .get(session)
            .is_some_and(|s| s.notices_folded)
        {
            return Vec::new();
        }
        self.sidebar_notices()
            .into_iter()
            .map(|n| n.owner)
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
        let folded = self.sidebar_folded(&s.name, &tabs);
        let repo_name = self.sidebar_repo(&s.name).map(|d| {
            d.file_name()
                .unwrap_or(d.as_os_str())
                .to_string_lossy()
                .into_owned()
        });
        let notices = self.sidebar_notices();
        // Each plugin that asks for a list under the tabs (plugins.js's
        // `sidebar` count), from what its last run gave.
        let sections: Vec<render::Section> = self
            .usage
            .plugins
            .iter()
            .filter(|p| p.sidebar > 0)
            .map(|p| render::Section::of(&p.title, &p.items, p.error.as_deref(), p.sidebar))
            .collect();
        let build = self.build_row();
        let view = View {
            focused,
            cursor: side.and_then(|x| x.cursor.as_ref()),
            frame: self.sidebar.frame,
            here: s.current,
            filter: side.and_then(|x| x.filter.as_ref()).filter(|_| focused),
            top: side.filter(|x| x.focused).and_then(|x| x.top),
            keep: side.and_then(|x| x.keep),
            hover: self.sidebar.hover.get(&client),
            awake: self.awake.holding(),
            repo: repo_name.as_deref(),
            folded: &folded,
            notices_folded: side.is_some_and(|x| x.notices_folded),
            reminders: self.sidebar_reminders_hidden(&s.name),
            build: build.as_ref(),
            selected: side
                .and_then(|x| x.select.as_ref())
                .filter(|_| focused && settings::is_on("sidebar_select")),
        };
        let out = render::sidebar_lines(
            &tabs,
            side.and_then(|x| x.frozen.as_ref()),
            &notices,
            &sections,
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
            // The line beside the list is its scroll bar while the list runs
            // past its room: the thumb a heavier line where the list is.
            let sc = &out.scroll;
            if crate::ui::kit::scroll_bars()
                && let Some((at, len)) =
                    crate::ui::kit::thumb(sc.length, sc.top, sc.space, sc.space)
            {
                for y in at..at + len {
                    let y = (sc.list_y + y) as u16;
                    if y < rows {
                        buf[(width, y)].set_symbol("┃");
                    }
                }
            }
        }
        Some(Drawn {
            area,
            owners: out.owners,
            scroll: out.scroll,
            spins: out.spins,
            reminders_at: out.reminders_at,
        })
    }

    /// After a client is drawn: where its sidebar went, kept for the mouse
    /// and C-d; the list's place, kept while it has the focus, and kept for
    /// the next tab in front to hold to (`View::keep`); and the spinner's
    /// next turn, timed only while something spins.
    pub fn sidebar_drawn(&mut self, client: ClientId, session: &str, drawn: Option<Drawn>) {
        let Some(d) = drawn else {
            self.sidebar.drawn.remove(&client);
            return;
        };
        let side = self.sidebar.side(session);
        if side.focused {
            side.top = Some(d.scroll.top);
        }
        side.keep = Some(d.scroll.top);
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
            SidebarEvent::HistoryRead(read) => self.notice_history_read(read),
            SidebarEvent::HistoryWritten => self.sidebar.history_writing = false,
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
        let frozen = self.sidebar_frozen_now(session, &r);
        let side = self.sidebar.side(session);
        if !side.focused {
            side.frozen = Some(frozen);
            side.top = None;
        }
        side.focused = true;
        // In the Notifications window, the cursor starts on its notification.
        let shown = self
            .is_notices_tab(here)
            .then(|| self.sidebar.windows.at.get(session).cloned())
            .flatten();
        // A tab or a notification brought into the sidebar (M-a, M-A, a
        // click) unfolds what it is under, so the cursor is on it.
        match &cursor {
            Some(Owner::Tab(t)) => self.sidebar_unfold_to(session, *t),
            Some(Owner::Notice { .. }) => self.sidebar.side(session).notices_folded = false,
            _ => {}
        }
        let side = self.sidebar.side(session);
        side.cursor = Some(cursor.or(shown).unwrap_or(Owner::Tab(here)));
        self.sidebar_cursor_back(session);
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
        side.select = None;
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
        if *action == Action::ContextMenu && !self.mission_open(client) {
            self.sidebar_menu_key(client, &session);
            return (true, true);
        }
        // M-f and the reply copy act on the tab in front, which on a folded
        // group's line, a tab the sidebar leaves out, or a cursor left on
        // another tab, is no tab under the cursor: there they do nothing.
        let current = self.session(&session).map(|s| s.current);
        let off_a_listed_tab = match self
            .sidebar
            .sides
            .get(&session)
            .and_then(|s| s.cursor.as_ref())
        {
            Some(Owner::Group(_)) => true,
            Some(Owner::Tab(t)) => Some(*t) != current || !self.sidebar_order(&session).contains(t),
            _ => false,
        };
        if matches!(action, Action::TogglePin | Action::CopyReply) && off_a_listed_tab {
            return (true, true);
        }
        match action {
            Action::SelectPane(dir) => {
                if *dir == crate::mux::Direction::Right {
                    self.sidebar_leave(&session);
                }
                (true, true)
            }
            Action::NextNeedsYou
            | Action::NextIdle
            | Action::SelectTab(_)
            | Action::Palette
            | Action::ListKeys
            | Action::Detach
            | Action::ResetTips
            | Action::TogglePin
            | Action::CopyReply
            // C-w's next key moves from the sidebar, and M-0's tab keeps
            // the cursor in a sidebar, as M-1 to M-9 do (src/extras).
            | Action::Extra(crate::extras::Command::PaneNav | crate::extras::Command::GoToTab) => {
                (false, true)
            }
            _ => {
                self.sidebar_leave(&session);
                (false, true)
            }
        }
    }

    /// After a key's action: M-1 to M-9 unfold the group of the tab they
    /// went to, and pressed from the sidebar leave its cursor on that tab
    /// (sidebar.sh refocus).
    pub fn sidebar_after_action(&mut self, client: ClientId, was_focused: bool, action: &Action) {
        if !matches!(action, Action::SelectTab(_)) {
            return;
        }
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        let Some(here) = self.session(&session).map(|s| s.current) else {
            return;
        };
        // A tab under a folded group unfolds it, so the sidebar lists it.
        self.sidebar_unfold_to(&session, here);
        if was_focused
            && let Some(side) = self.sidebar.sides.get_mut(&session)
            && side.focused
        {
            side.cursor = Some(Owner::Tab(here));
            // One the sidebar leaves out (a hidden reminder) is gone to: the
            // focus into its pane, as the sidebar has no row for it.
            let r = Rules::now();
            let hidden = !settings::is_on("sidebar_reminders")
                && self
                    .sidebar_marked(&session)
                    .iter()
                    .any(|t| t.id == here && model::own_stage(t, &r) == Stage::Later);
            if hidden {
                self.sidebar_leave(&session);
            } else {
                // One a search leaves out clears the search, so the cursor
                // stays on the tab gone to.
                if !self.sidebar_order(&session).contains(&here) {
                    self.sidebar.side(&session).filter = None;
                }
                self.sidebar_cursor_back(&session);
            }
            self.dirty_session(&session);
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
        // A cleared search folds its groups again, so the tab in front may
        // be folded away: onto its group's line, or a stop listed.
        self.sidebar_cursor_back(session);
        self.dirty_session(session);
    }

    /// Moves the cursor to the `to`th stop: a tab, which comes to the front
    /// with the focus still in the sidebar, or a folded group's line; or
    /// past the last of them, one of the notifications.
    fn sidebar_move(&mut self, session: &str, to: isize) {
        let stops = self.sidebar_stops(session);
        let notices = self.sidebar_notice_stops(session);
        let typing = self
            .sidebar
            .sides
            .get(session)
            .and_then(|s| s.filter.as_ref())
            .is_some_and(|f| f.typing);
        if to >= stops.len() as isize && !notices.is_empty() && !typing {
            let i = ((to - stops.len() as isize) as usize).min(notices.len() - 1);
            self.sidebar.side(session).cursor = Some(notices[i].clone());
            // Onto a notification, the Notifications window comes to the
            // front on it, as a tab would (notice_list setting).
            if settings::is_on("notice_list") {
                self.show_notices(session, &notices[i]);
            }
            self.dirty_session(session);
            return;
        }
        if stops.is_empty() {
            return;
        }
        let stop = stops[to.clamp(0, stops.len() as isize - 1) as usize].clone();
        self.sidebar.side(session).cursor = Some(stop.clone());
        if let Owner::Tab(tab) = stop
            && self.session(session).is_some_and(|s| s.current != tab)
        {
            self.select_tab(session, tab);
        }
        self.dirty_session(session);
    }

    /// Tab and Shift-Tab on the notifications with the Notifications
    /// window: the next or last of them one by one, the window following; up
    /// from the first goes back to the last tab, and down from the last stays.
    fn sidebar_notice_step(&mut self, session: &str, tabs: usize, cursor: &Owner, by: isize) {
        let entries = self.notice_entries();
        let at = entries
            .iter()
            .position(|(o, _)| o == cursor)
            .unwrap_or_else(|| self.notice_cursor(session, &entries)) as isize;
        let to = at + by;
        if to < 0 {
            self.sidebar_move(session, tabs as isize - 1);
            return;
        }
        if let Some((owner, _)) = entries.get(to as usize) {
            self.sidebar_notice_to(session, owner);
        }
    }

    /// j and k with UX 2.0, whose notifications sit above the tabs: up
    /// from the first tab onto the last kind of notification, down from the
    /// last kind onto the first tab, and no further either way, a kind's row
    /// at a time, the Notifications window on that kind's first.
    fn sidebar_step_ux2(
        &mut self,
        session: &str,
        tabs: usize,
        on_notice: Option<usize>,
        at: isize,
        by: isize,
    ) {
        let notices = self.sidebar_notice_stops(session).len();
        match on_notice {
            None if by < 0 && at == 0 && notices > 0 => {
                self.sidebar_move(session, (tabs + notices) as isize - 1);
            }
            None => self.sidebar_move(session, (at + by).clamp(0, tabs.max(1) as isize - 1)),
            Some(i) => {
                let to = i as isize + by;
                if to >= notices as isize {
                    self.sidebar_move(session, 0);
                } else if to >= 0 {
                    self.sidebar_move(session, tabs as isize + to);
                }
            }
        }
    }

    /// The sidebar's cursor and the Notifications window's onto one
    /// notification.
    fn sidebar_notice_to(&mut self, session: &str, owner: &Owner) {
        let w = &mut self.sidebar.windows;
        w.at.insert(session.to_string(), owner.clone());
        w.back.remove(session);
        w.menu.remove(session);
        self.sidebar.side(session).cursor = Some(owner.clone());
        self.dirty_session(session);
    }

    /// Goes to a tab from the sidebar, the focus into its pane.
    fn sidebar_go(&mut self, session: &str, tab: TabId) {
        // A tab of a folded group gone to (a / search's match) unfolds
        // it, so the tab in front stays listed: the group it is in now,
        // once the layout held while browsing is let go.
        self.sidebar_leave(session);
        self.sidebar_unfold_to(session, tab);
        if let Some(s) = self.session_of_tab(tab).map(|s| s.name.clone()) {
            self.select_tab(&s, tab);
        }
    }

    fn sidebar_key_in(&mut self, client: ClientId, session: &str, key: Key, raw: &[u8]) {
        let stop_list = self.sidebar_stops(session);
        let order: Vec<TabId> = stop_list.iter().filter_map(Owner::tab).collect();
        let notices = self.sidebar_notice_stops(session);
        let Some(here) = self.session(session).map(|s| s.current) else {
            return;
        };
        // Any key puts out what the mouse lit.
        if self.sidebar.hover.remove(&client).is_some() {
            self.dirty_session(session);
        }
        let side = self.sidebar.side(session).clone();
        let cursor = side.cursor.clone().unwrap_or(Owner::Tab(here));
        // A kind's row holds the cursor while it is on any of its
        // notifications, as the Notifications window goes through them.
        let on_notice = notices.iter().position(|o| o.same_row(&cursor));
        let by_item = on_notice.is_some() && settings::is_on("notice_list");
        // A folded group's line is a stop in its tabs' place.
        let stops = stop_list.len();
        let on_fold = match &cursor {
            Owner::Group(stage) if stop_list.contains(&cursor) => Some(*stage),
            _ => None,
        };
        let at = match on_notice {
            Some(i) => stops + i,
            None => stop_list.iter().position(|o| *o == cursor).unwrap_or(0),
        } as isize;
        let k = key.normalized();
        let none = k.mods.is_empty();
        let ctrl = k.mods == Mods::CTRL;
        let tab_focus = settings::is_on("tab_focus");
        // The tab under the cursor, only while the sidebar lists it, so a
        // key never acts on another tab in its place.
        let chosen = cursor.tab().filter(|t| order.contains(t));

        // Typing a / filter: letters go into it and the cursor to the first
        // tab it leaves; the arrows move, Enter goes to the chosen tab, and
        // Esc clears it.
        if side.filter.as_ref().is_some_and(|f| f.typing) {
            match k.code {
                KeyCode::Up if none => self.sidebar_move(session, at - 1),
                KeyCode::Down if none => self.sidebar_move(session, at + 1),
                KeyCode::Char('p') if ctrl => self.sidebar_move(session, at - 1),
                KeyCode::Char('n') if ctrl => self.sidebar_move(session, at + 1),
                KeyCode::Tab | KeyCode::BackTab if tab_focus && stops > 0 => {
                    let n = stops as isize;
                    let by = if k.code == KeyCode::Tab { 1 } else { -1 };
                    self.sidebar_move(session, (at + by).rem_euclid(n));
                }
                // In select mode, Enter keeps the filter and goes back to
                // select mode's keys, the tabs still selected.
                KeyCode::Enter if side.select.is_some() && settings::is_on("sidebar_select") => {
                    if let Some(f) = self.sidebar.side(session).filter.as_mut() {
                        f.typing = false;
                    }
                    self.dirty_session(session);
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
                        self.sidebar_refilter(session);
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

        // Select mode: Space or v selects or unselects the tab under the
        // cursor, Enter or m opens what to do with them, Esc leaves it; j,
        // k and the rest move as ever.
        if let Some(picked) = side
            .select
            .as_ref()
            .filter(|_| settings::is_on("sidebar_select"))
        {
            match k.code {
                KeyCode::Char(' ' | 'v') if none => {
                    if let Some(t) = chosen.filter(|_| on_notice.is_none()) {
                        let mut picked = picked.clone();
                        if !picked.remove(&t) {
                            picked.insert(t);
                        }
                        self.sidebar.side(session).select = Some(picked);
                        self.dirty_session(session);
                    }
                    return;
                }
                KeyCode::Enter | KeyCode::Char('m') if none => {
                    self.sidebar_select_menu(client, session);
                    return;
                }
                // Esc clears a kept filter first, then leaves select mode.
                KeyCode::Esc if none && side.filter.is_some() => {
                    self.sidebar.side(session).filter = None;
                    self.sidebar_refilter(session);
                    return;
                }
                KeyCode::Esc if none => {
                    self.sidebar.side(session).select = None;
                    self.dirty_session(session);
                    return;
                }
                _ => {}
            }
        }

        let ux2 = settings::is_on("ux2");
        let up_down = match k.code {
            KeyCode::Char('j') | KeyCode::Down if none => Some(1),
            KeyCode::Char('n') if ctrl => Some(1),
            KeyCode::Char('k') | KeyCode::Up if none => Some(-1),
            KeyCode::Char('p') if ctrl => Some(-1),
            _ => None,
        };
        if ux2 && let Some(by) = up_down {
            self.sidebar_step_ux2(session, stops, on_notice, at, by);
            return;
        }
        // On a folded group's line, Enter, l or the right arrow unfolds it.
        if let Some(stage) = on_fold
            && (matches!(k.code, KeyCode::Right | KeyCode::Char('l')) && none
                || k.code == KeyCode::Enter)
        {
            self.sidebar_fold(session, stage, false);
            return;
        }
        match k.code {
            // On the notifications, j and k step a kind at a time, the
            // Notifications window following onto the kind's first.
            KeyCode::Char('j') | KeyCode::Down if none => self.sidebar_move(session, at + 1),
            KeyCode::Char('n') if ctrl => self.sidebar_move(session, at + 1),
            KeyCode::Char('k') | KeyCode::Up if none => self.sidebar_move(session, at - 1),
            KeyCode::Char('p') if ctrl => self.sidebar_move(session, at - 1),
            KeyCode::Char('g') | KeyCode::Home if none => self.sidebar_move(session, 0),
            KeyCode::Char('G') | KeyCode::End if none => {
                self.sidebar_move(session, stops as isize - 1)
            }
            // z folds the chosen tab's group to one line, or the
            // notifications; on a folded line, unfolds it.
            KeyCode::Char('z') if none => {
                if on_notice.is_some() {
                    self.sidebar_fold_notices(session, true);
                } else if let Some(stage) = on_fold {
                    self.sidebar_fold(session, stage, false);
                } else if let Some(stage) = chosen.and_then(|t| self.sidebar_group_of(session, t)) {
                    self.sidebar_fold(session, stage, true);
                }
            }
            KeyCode::Char('d' | 'u') if ctrl => {
                self.sidebar_half_page(client, session, k, raw, &order, on_notice.is_some())
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l')
                if none || k.code == KeyCode::Enter =>
            {
                if on_notice.is_some() && settings::is_on("notice_list") {
                    // Into the Notifications window, as Enter goes into a
                    // tab's pane.
                    self.notices_enter(session, &cursor);
                } else if on_notice.is_some() {
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
            // webpage, the last wrapping to the first, one notification at a
            // time with the Notifications window; with tab_focus off, Tab
            // opens mission control, as M-Space does.
            KeyCode::Tab if tab_focus => {
                let entries = self.notice_entries();
                let last = match on_notice {
                    Some(_) if by_item => {
                        entries.iter().position(|(o, _)| *o == cursor)
                            == Some(entries.len().saturating_sub(1))
                    }
                    Some(i) => i + 1 >= notices.len(),
                    None => at + 1 >= stops as isize && notices.is_empty(),
                };
                if last {
                    self.sidebar_move(session, 0);
                } else if by_item {
                    self.sidebar_notice_step(session, stops, &cursor, 1);
                } else {
                    self.sidebar_move(session, at + 1);
                }
            }
            KeyCode::Tab => {
                self.sidebar_leave(session);
                self.open_mission(client, crate::mission::Opening::Tabs);
            }
            KeyCode::BackTab if tab_focus => {
                let last = stops as isize + notices.len() as isize - 1;
                let entries = self.notice_entries();
                if by_item {
                    self.sidebar_notice_step(session, stops, &cursor, -1);
                } else if on_notice.is_some() || at != 0 {
                    self.sidebar_move(session, at - 1);
                } else {
                    // From the first tab round to the last notification,
                    // the last one by one with the Notifications window.
                    self.sidebar_move(session, last);
                    // Folded notifications are no stop, so none is gone to.
                    if settings::is_on("notice_list")
                        && !notices.is_empty()
                        && let Some((owner, _)) = entries.last()
                    {
                        self.sidebar_notice_to(session, owner);
                    }
                }
            }
            KeyCode::Char(' ') if none => {
                if let Some(t) = chosen.filter(|_| on_notice.is_none()) {
                    self.sidebar_peek(client, t);
                }
            }
            // v: select mode, the chosen tab selected.
            KeyCode::Char('v') if none && settings::is_on("sidebar_select") => {
                if let Some(t) = chosen.filter(|_| on_notice.is_none()) {
                    self.sidebar.side(session).select = Some([t].into());
                    self.dirty_session(session);
                }
            }
            // m: the menu a right click on the cursor's row opens.
            KeyCode::Char('m') if none => self.sidebar_menu_key(client, session),
            // Its Claude answered from here, in mission control's box, as
            // the sidebar's `a` answered the chosen tab in a popup.
            KeyCode::Char('a') if none => {
                if let Some(t) = chosen.filter(|_| on_notice.is_none()) {
                    self.open_answer(client, t, None);
                }
            }
            // n unfolds folded notifications as it goes to them.
            KeyCode::Char('n') if none => {
                if self.sidebar_notices().is_empty() {
                    self.say(Some(client), "No notifications");
                } else {
                    self.sidebar.side(session).notices_folded = false;
                    self.sidebar_move(session, stops as isize);
                }
            }
            // d and L: the chosen tab gone to, its diff or lazygit in its
            // pane's place, as the tmux version's sidebar and mission
            // control's menu on a tab do.
            KeyCode::Char(c @ ('d' | 'L')) if none => {
                if let Some(t) = chosen.filter(|_| on_notice.is_none()) {
                    self.sidebar_go(session, t);
                    self.tool_in_front(client, c == 'L');
                }
            }
            // t: a new tab running Claude, as M-t opens one, from the chosen
            // tab, so its directory comes first.
            KeyCode::Char('t') if none => {
                if let Some(t) = chosen {
                    self.sidebar_go(session, t);
                    self.act(client, Action::NewClaudeTab);
                }
            }
            // i: its whole title in a box, as M-T shows it.
            KeyCode::Char('i') if none && settings::is_on("tab_title") => {
                if let Some(t) = chosen.filter(|_| on_notice.is_none()) {
                    self.show_tab_title(client, t);
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
                // One under a folded group unfolds it, the cursor onto it
                // in the sidebar.
                if let Some(t) = target.filter(|t| !order.contains(t)) {
                    self.sidebar_unfold_to(session, t);
                }
                let stop_list = self.sidebar_stops(session);
                match target.and_then(|id| stop_list.iter().position(|o| *o == Owner::Tab(id))) {
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

    /// Whether a session's sidebar is in select mode.
    fn sidebar_selecting(&self, session: &str) -> bool {
        self.sidebar
            .sides
            .get(session)
            .is_some_and(|s| s.focused && s.select.is_some())
            && settings::is_on("sidebar_select")
    }

    /// A tab selected, or unselected, by a click: the focus into the
    /// sidebar on it, starting select mode when it isn't on.
    fn sidebar_select_toggle(&mut self, session: &str, tab: TabId) {
        if !self.sidebar.focused(session) {
            self.sidebar_enter(session, Some(Owner::Tab(tab)));
        }
        let side = self.sidebar.side(session);
        side.cursor = Some(Owner::Tab(tab));
        // A / filter being typed is kept but no longer typed, so select
        // mode's keys reach it.
        if let Some(f) = side.filter.as_mut() {
            f.typing = false;
        }
        let picked = side.select.get_or_insert_default();
        if !picked.remove(&tab) {
            picked.insert(tab);
        }
        self.dirty_session(session);
    }

    /// M-p's "Select several tabs in the sidebar": the focus into the
    /// sidebar, select mode on with the tab in front selected when the
    /// sidebar lists it. M-p has
    /// already taken the focus out, clearing any / filter, as every M-p
    /// command that acts on the pane does.
    pub(crate) fn sidebar_select_start(&mut self, client: ClientId) {
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        if !self.sidebar_shown(&session) {
            self.say(Some(client), "The sidebar isn't shown here.");
            return;
        }
        let Some(here) = self.session(&session).map(|s| s.current) else {
            return;
        };
        // Only a tab the sidebar lists, never the Notifications window:
        // with none in front, select mode starts on the first, nothing
        // selected.
        let order = self.sidebar_order(&session);
        let listed = order.contains(&here);
        let start = if listed {
            Some(here)
        } else {
            order.first().copied()
        };
        self.sidebar_enter(&session, start.map(Owner::Tab));
        self.sidebar.side(&session).select = Some(start.filter(|_| listed).into_iter().collect());
        self.dirty_session(&session);
    }

    /// Enter or m in select mode, or a click on its chip: what to do with
    /// the tabs selected, as one menu.
    fn sidebar_select_menu(&mut self, client: ClientId, session: &str) {
        let picked: Vec<TabId> = self
            .sidebar
            .sides
            .get(session)
            .and_then(|s| s.select.as_ref())
            .map(|p| {
                p.iter()
                    .copied()
                    .filter(|t| self.tabs.contains_key(t))
                    .collect()
            })
            .unwrap_or_default();
        if picked.is_empty() {
            self.say(Some(client), "No tabs selected: Space or v selects one.");
            return;
        }
        let n = picked.len();
        let title = format!("{n} {} selected", if n == 1 { "tab" } else { "tabs" });
        let menu = crate::ui::Menu::of(title, &["Close these tabs"]);
        let session = session.to_string();
        self.choose(client, menu, move |server, client, chose| {
            if chose == Some(0) {
                server.close_tabs(client, &session, picked);
            }
        });
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
            // Only to a tab the cursor is on and the sidebar lists, never
            // another in its place (a folded group's line has none).
            let at = match cursor.as_ref() {
                Some(c) => c.tab().filter(|t| order.contains(t)),
                None => order.first().copied(),
            };
            if let Some(t) = at {
                self.sidebar_go(session, t);
                self.press_key(client, key, raw);
            }
            return;
        }
        let stops = self.sidebar_stops(session);
        if stops.is_empty() {
            return;
        }
        if on_notice {
            if !down {
                self.sidebar_move(session, stops.len() as isize - 1);
            }
            return;
        }
        let Some(scroll) = self.sidebar.drawn.get(&client).map(|d| d.scroll.clone()) else {
            return;
        };
        let by = (scroll.room / 2).max(1) as isize * if down { 1 } else { -1 };
        let most = scroll.length.saturating_sub(scroll.room) as isize;
        self.sidebar.side(session).top = Some((scroll.top as isize + by).clamp(0, most) as usize);
        // The stop (a tab, or a folded group's line) on the line half a
        // screen on, else the nearest one past it, never the one the cursor
        // is on, which a tab taller than half the list would otherwise hold
        // it on press after press.
        let here = cursor.as_ref();
        let line = scroll.cursor_at as isize + by;
        let found = if down {
            scroll
                .owners
                .iter()
                .skip(line.max(0) as usize)
                .flatten()
                .find(|o| Some(*o) != here && stops.contains(o))
                .cloned()
        } else {
            scroll
                .owners
                .iter()
                .take((line + 1).max(0) as usize)
                .rev()
                .flatten()
                .find(|o| Some(*o) != here && stops.contains(o))
                .cloned()
        };
        let to = match found {
            Some(o) => stops.iter().position(|s| *s == o).unwrap_or(0),
            None if down => stops.len() - 1,
            None => 0,
        };
        self.sidebar_move(session, to as isize);
    }

    /// A click on the ▲ or ▼ more row: the list half a screen up or down,
    /// the cursor with it, as C-u and C-d, whether or not a filter is being
    /// typed, the focus coming into the sidebar as the wheel brings it;
    /// nothing with sidebar_half_page off.
    fn sidebar_scroll_click(&mut self, client: ClientId, session: &str, down: bool) {
        if !settings::is_on("sidebar_half_page") {
            return;
        }
        if !self.sidebar.focused(session) {
            self.sidebar_enter(session, None);
        }
        let order = self.sidebar_order(session);
        let on_notice = self
            .sidebar
            .side(session)
            .cursor
            .as_ref()
            .is_some_and(|c| matches!(c, Owner::Notice { .. }));
        let key = Key::new(KeyCode::Char(if down { 'd' } else { 'u' }), Mods::CTRL);
        self.sidebar_half_page(client, session, key, &[], &order, on_notice);
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

    // ── The repository filter ───────────────────────────────────────────────

    /// Filters a session's sidebar to one repository (a main clone's
    /// directory), every other one's tabs folded under Other repos, or
    /// clears the filter with `None`. While the sidebar has the focus, the
    /// tabs are held where the filter puts them, and a cursor left on what
    /// folded away goes back to the tab in front.
    pub(crate) fn sidebar_set_repo(&mut self, session: &str, repo: Option<std::path::PathBuf>) {
        let side = self.sidebar.side(session);
        side.repo = repo;
        side.repo_open = false;
        if side.focused {
            let frozen = self.sidebar_frozen_now(session, &Rules::now());
            self.sidebar.side(session).frozen = Some(frozen);
        }
        self.sidebar_cursor_back(session);
        self.dirty_session(session);
    }

    /// M-p's "Filter the sidebar to a repository": a picker of the
    /// repositories the session's tabs are in, each with how many tabs,
    /// "Every repository" first to clear it, the one filtered to now
    /// marked; picking one filters to it and says so.
    pub(crate) fn sidebar_pick_repo(&mut self, client: ClientId) {
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        let now = self
            .sidebar_repo(&session)
            .map(std::path::Path::to_path_buf);
        let mut repos: Vec<(std::path::PathBuf, usize)> = Vec::new();
        for t in self.sidebar_tabs(&session) {
            let Some(dir) = t.repo_dir else { continue };
            match repos.iter_mut().find(|(d, _)| *d == dir) {
                Some((_, n)) => *n += 1,
                None => repos.push((dir, 1)),
            }
        }
        let name = |d: &std::path::Path| {
            d.file_name()
                .unwrap_or(d.as_os_str())
                .to_string_lossy()
                .into_owned()
        };
        repos.sort_by_key(|(d, _)| name(d).to_lowercase());
        let mut items = vec![PickItem {
            text: "Every repository".into(),
            tags: if now.is_none() {
                "now".into()
            } else {
                String::new()
            },
            ..PickItem::default()
        }];
        for (dir, n) in &repos {
            items.push(PickItem {
                text: format!(
                    "{}  {n} {}",
                    name(dir),
                    if *n == 1 { "tab" } else { "tabs" }
                ),
                tags: if now.as_ref() == Some(dir) {
                    "now".into()
                } else {
                    String::new()
                },
                value: dir.to_string_lossy().into_owned(),
                ..PickItem::default()
            });
        }
        let picker = Picker::new("Filter the sidebar to a repository", items)
            .named("sidebar-repo")
            .prompt("repository> ");
        self.pick(client, picker, move |server, client, picked| {
            let Some(picked) = picked else {
                return;
            };
            if picked.value.is_empty() {
                server.sidebar_set_repo(&session, None);
                server.say(Some(client), "Sidebar: every repository");
                return;
            }
            let dir = std::path::PathBuf::from(&picked.value);
            let said = format!("Sidebar: {} only", name(&dir));
            server.sidebar_set_repo(&session, Some(dir));
            server.say(Some(client), &said);
        });
    }

    /// Folds a session's group to one line, or unfolds it: Other repos by
    /// `repo_open`, any other in `folded`. The cursor goes onto the first
    /// of its tabs as it unfolds under the cursor, leaving the tab in front
    /// as it was, and onto its line as it folds over the cursor.
    fn sidebar_fold(&mut self, session: &str, stage: Stage, fold: bool) {
        let side = self.sidebar.side(session);
        if stage == Stage::OtherRepos {
            side.repo_open = !fold;
        } else if fold {
            side.folded.insert(stage);
        } else {
            side.folded.remove(&stage);
        }
        // Unfolded under the cursor, the cursor goes onto its first tab
        // without bringing it to the front: opening a group to look at its
        // tabs keeps you on the tab you were on until j, k or Enter moves.
        if !fold && self.sidebar.side(session).cursor == Some(Owner::Group(stage)) {
            let first = self.sidebar_stops(session).into_iter().find(|o| {
                o.tab()
                    .is_some_and(|t| self.sidebar_group_of(session, t) == Some(stage))
            });
            if let Some(first) = first {
                self.sidebar.side(session).cursor = Some(first);
            }
        }
        if fold {
            let on_folded = self
                .sidebar
                .side(session)
                .cursor
                .as_ref()
                .and_then(Owner::tab)
                .is_some_and(|t| !self.sidebar_order(session).contains(&t));
            if on_folded {
                self.sidebar.side(session).cursor = Some(Owner::Group(stage));
            }
        }
        self.dirty_session(session);
    }

    /// Folds a session's notifications to one line of their kinds, or
    /// unfolds them; a cursor on one as they fold goes to the last stop
    /// above them, else the tab in front.
    fn sidebar_fold_notices(&mut self, session: &str, fold: bool) {
        self.sidebar.side(session).notices_folded = fold;
        let on_notice = matches!(
            self.sidebar.side(session).cursor,
            Some(Owner::Notice { .. })
        );
        if fold && on_notice && self.sidebar.focused(session) {
            // Onto the last stop as j and k bring it, its tab to the front.
            let stops = self.sidebar_stops(session).len();
            if stops > 0 {
                self.sidebar_move(session, stops as isize - 1);
            } else {
                self.sidebar.side(session).cursor =
                    self.session(session).map(|s| Owner::Tab(s.current));
                self.sidebar_cursor_back(session);
            }
        }
        self.dirty_session(session);
    }

    /// The cursor back to the tab in front when it is on a tab or a line
    /// the sidebar no longer lists; on a tab of a folded group, onto that
    /// group's line.
    fn sidebar_cursor_back(&mut self, session: &str) {
        let stops = self.sidebar_stops(session);
        // With every tab left out for its reminder, the cursor goes onto a
        // notification, or with none, the sidebar gives the focus back.
        // A search matching nothing isn't that: it keeps the focus.
        let searching = self
            .sidebar
            .sides
            .get(session)
            .is_some_and(|s| s.filter.is_some());
        if stops.is_empty() && !searching && self.sidebar.focused(session) {
            let notices = self.sidebar_notice_stops(session);
            let cursor = self.sidebar.side(session).cursor.clone();
            let on_notice = cursor
                .as_ref()
                .is_some_and(|c| notices.iter().any(|n| n.same_row(c)));
            match notices.first() {
                _ if on_notice => {}
                Some(first) => self.sidebar.side(session).cursor = Some(first.clone()),
                None => self.sidebar_leave(session),
            }
            return;
        }
        // A tab a / search leaves out stays under the cursor; one the
        // repository filter or hidden reminders leave out does not.
        let searching = self
            .sidebar
            .sides
            .get(session)
            .and_then(|s| s.filter.as_ref())
            .is_some_and(|f| !f.text.trim().is_empty());
        let filtered = self.sidebar_repo(session).is_some() || !searching;
        let cursor = self.sidebar.side(session).cursor.clone();
        let folded_over = cursor
            .as_ref()
            .and_then(Owner::tab)
            .and_then(|t| self.sidebar_group_of(session, t))
            .map(Owner::Group)
            .filter(|g| stops.contains(g));
        if let Some(group) = folded_over {
            self.sidebar.side(session).cursor = Some(group);
            return;
        }
        let gone = match cursor.as_ref() {
            Some(Owner::Tab(t)) => filtered && !stops.contains(&Owner::Tab(*t)),
            Some(group @ Owner::Group(_)) => !stops.contains(group),
            // A notification the Notifications window kept, while they are
            // folded.
            Some(Owner::Notice { .. }) => self
                .sidebar
                .sides
                .get(session)
                .is_some_and(|s| s.notices_folded),
            _ => false,
        };
        if !gone {
            return;
        }
        // The tab in front when the sidebar lists it, else its first stop.
        let here = self.session(session).map(|s| Owner::Tab(s.current));
        let back = here
            .clone()
            .filter(|h| stops.contains(h))
            .or_else(|| stops.first().cloned())
            .or(here);
        // Onto another tab than the one in front, it comes to the front, as
        // j and k bring one, so the cursor and the pane agree.
        let focused = self.sidebar.focused(session);
        let current = self.session(session).map(|s| s.current);
        self.sidebar.side(session).cursor = back.clone();
        if focused
            && let Some(Owner::Tab(t)) = back
            && Some(t) != current
        {
            self.select_tab(session, t);
        }
    }

    // ── The mouse ───────────────────────────────────────────────────────────

    /// What a cell of a client's sidebar is, where it last drew it: the row
    /// or chip a click there presses (`None` for one it does nothing on),
    /// or `None` outside the sidebar.
    pub fn sidebar_under(&self, client: ClientId, col: u16, row: u16) -> Option<Option<Owner>> {
        let d = self
            .sidebar
            .drawn
            .get(&client)
            .filter(|d| col <= d.area.right() && row < d.area.bottom())?;
        // The top line's ⏰ chip is a target of its own on a line the
        // repository filter's chip may share.
        let on_reminders = row == d.area.y
            && d.reminders_at
                .is_some_and(|(from, to)| (from..to).contains(&usize::from(col - d.area.x)));
        Some(if on_reminders {
            Some(Owner::Reminders)
        } else {
            (col < d.area.right())
                .then(|| d.owners.get((row - d.area.y) as usize).cloned().flatten())
                .flatten()
        })
    }

    /// A mouse event, before the panes see it: on the sidebar, a click goes
    /// to a tab (a right click opens a menu by it, serve/menu.rs) or, on the
    /// ▲ or ▼ more row,
    /// scrolls half a screen, as the wheel does, and
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
        let Some(owner) = self.sidebar_under(client, m.col, m.row) else {
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
        let Some(d) = self.sidebar.drawn.get(&client).cloned() else {
            return false;
        };
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
                // The wheel moves half a screen through the tabs, as C-d and
                // C-u do, the focus coming into the sidebar; the rest of one
                // flick (the same way within FLICK of the move) is let go.
                // On a notification, or at the first or last tab going on
                // past it, it steps as j and k do, so the notifications stay
                // in reach, one step a flick; with sidebar_half_page off it
                // always steps, every event.
                let down = m.button == MouseButton::WheelDown;
                if !self.sidebar.focused(&session) {
                    self.sidebar_enter(&session, None);
                }
                let half_page = settings::is_on("sidebar_half_page");
                if half_page {
                    let now = std::time::Instant::now();
                    let same_flick = self
                        .sidebar
                        .wheel
                        .get(&client)
                        .is_some_and(|(was, at)| *was == down && now - *at < FLICK);
                    if same_flick {
                        return true;
                    }
                    self.sidebar.wheel.insert(client, (down, now));
                }
                let stops = self.sidebar_stops(&session);
                let edge = if down { stops.last() } else { stops.first() };
                // On a tab or a folded group's line short of the edge.
                let on_tab_short_of_the_edge = match &self.sidebar.side(&session).cursor {
                    Some(c @ (Owner::Tab(_) | Owner::Group(_))) => Some(c) != edge,
                    _ => false,
                };
                if half_page && on_tab_short_of_the_edge {
                    self.sidebar_scroll_click(client, &session, down);
                } else {
                    let code = if down { KeyCode::Down } else { KeyCode::Up };
                    self.sidebar_key_in(client, &session, Key::new(code, Mods::NONE), &[]);
                }
            }
            // A press on the line beside the list, where its scroll bar is,
            // pages it toward the press, as C-u and C-d do (with half-page
            // scrolling off, a tab at a time, as the wheel does).
            (MouseKind::Press, MouseButton::Left)
                if m.col == d.area.right() && crate::ui::kit::scroll_bars() =>
            {
                let sc = &d.scroll;
                let row = usize::from(m.row - d.area.y);
                if let Some((at, len)) =
                    crate::ui::kit::thumb(sc.length, sc.top, sc.space, sc.space)
                    && let Some(row) = row.checked_sub(sc.list_y).filter(|r| *r < sc.space)
                    && !(at..at + len).contains(&row)
                {
                    let down = row >= at + len;
                    if settings::is_on("sidebar_half_page") {
                        self.sidebar_scroll_click(client, &session, down);
                    } else {
                        // With half-page scrolling off, a tab at a time, as
                        // the wheel moves then.
                        if !self.sidebar.focused(&session) {
                            self.sidebar_enter(&session, None);
                        }
                        let code = if down { KeyCode::Down } else { KeyCode::Up };
                        self.sidebar_key_in(client, &session, Key::new(code, Mods::NONE), &[]);
                    }
                }
            }
            (MouseKind::Press, MouseButton::Left) => match owner {
                // In select mode a click selects or unselects a tab, and a
                // Shift-click starts it.
                Some(Owner::Tab(t))
                    if m.mods.shift && settings::is_on("sidebar_select")
                        || self.sidebar_selecting(&session) =>
                {
                    self.sidebar_select_toggle(&session, t)
                }
                Some(Owner::Tab(t)) => self.sidebar_go(&session, t),
                Some(Owner::SelectChip) => self.sidebar_select_menu(client, &session),
                // A notification clicked: the Notifications window, on it.
                Some(notice @ Owner::Notice { .. }) if settings::is_on("notice_list") => {
                    self.sidebar_enter(&session, Some(notice.clone()));
                    self.show_notices(&session, &notice);
                }
                Some(notice @ Owner::Notice { .. }) => self.sidebar_open_notice(client, &notice),
                // The ▲ and ▼ more rows scroll half a screen, as C-u and C-d.
                Some(Owner::ScrollUp) => self.sidebar_scroll_click(client, &session, false),
                Some(Owner::ScrollDown) => self.sidebar_scroll_click(client, &session, true),
                // The filter's chip clears it; Other repos folds or unfolds.
                Some(Owner::BuildRow) => self.build_steps_open(client),
                Some(Owner::RepoChip) => {
                    self.sidebar_set_repo(&session, None);
                    self.say(Some(client), "Sidebar: every repository");
                }
                // A group folded: a click unfolds it as Enter on its line
                // does, the focus coming into the sidebar and its first tab
                // to the front; unfolded, a click on its heading folds it.
                Some(group @ Owner::Group(stage)) => {
                    let folded = self.sidebar_stops(&session).contains(&group);
                    if folded {
                        self.sidebar_enter(&session, Some(group));
                    }
                    self.sidebar_fold(&session, stage, !folded);
                }
                // The notifications' heading or folded line folds or
                // unfolds them.
                Some(Owner::Notices) => {
                    let folded = self
                        .sidebar
                        .sides
                        .get(&session)
                        .is_some_and(|s| s.notices_folded);
                    self.sidebar_fold_notices(&session, !folded);
                }
                // The ⏰ chip: mission control's Reminders view.
                Some(Owner::Reminders) => {
                    self.sidebar_leave(&session);
                    self.open_mission(client, crate::mission::Opening::View("reminders".into()));
                }
                None => {}
            },
            (MouseKind::Press, MouseButton::Right) => {
                self.sidebar_right_press(client, &session, owner);
            }
            _ => {}
        }
        true
    }

    // ── What it opens ───────────────────────────────────────────────────────

    /// A look at a tab without going there; Enter in it goes there.
    fn sidebar_peek(&mut self, client: ClientId, tab: TabId) {
        self.open_peek(client, tab, None, false);
    }

    /// A look at a tab, from the sidebar or mission control (`mission`,
    /// whose Enter goes there as its menus do): its last turn, or its screen
    /// with `SCROLLBACK` lines above it, on `mode` when given. Its `r` asks
    /// for a new look at the screen, which is read again here.
    pub(crate) fn open_peek(
        &mut self,
        client: ClientId,
        tab: TabId,
        mode: Option<super::peek::Mode>,
        mission: bool,
    ) {
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
            .map(|slot| slot.pane.text(super::peek::SCROLLBACK))
            .unwrap_or_default();
        let turn = model.claude.as_ref().and(model.turn.clone());
        let mut peek = Peek::new(
            format!("{} {}", model.index, model.name),
            turn,
            model.state.busy(),
            screen,
        );
        if let Some(mode) = mode {
            peek = peek.mode(mode);
        }
        self.open(
            client,
            Box::new(peek),
            Some(Box::new(
                move |server: &mut Server, client, outcome| match outcome {
                    Outcome::Yes if mission => server.mission_go(client, tab),
                    Outcome::Yes => server.sidebar_go(&session, tab),
                    Outcome::No => {
                        server.open_peek(client, tab, Some(super::peek::Mode::Screen), mission)
                    }
                    _ => {}
                },
            )),
        );
    }

    /// A notification chosen with the Notifications window off: mission
    /// control's Notifications view, on it, doing nothing else; its menu,
    /// dismissing and every other choice are the view's and the window's.
    fn sidebar_open_notice(&mut self, client: ClientId, owner: &Owner) {
        let Owner::Notice { source, id } = owner else {
            return;
        };
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        // The landings made without asking and the landed tabs closed unseen
        // show the newest on their line (`notices::summary`), which is the
        // one it leads to.
        let id = match source.as_str() {
            "autolanded" | "landed" => self
                .notices
                .get(source)
                .and_then(|items| items.last())
                .map_or(id.clone(), |i| i.id.clone()),
            _ => id.clone(),
        };
        self.sidebar_leave(&session);
        self.mission_on_notice(client, source, &id);
    }

    /// The view mission control shows a kind of notification in, while that
    /// view is on (goToNotices), for "Go to the notifications" with no
    /// sidebar to go to.
    fn notice_view(source: &str) -> Option<crate::mission::Opening> {
        let on = settings::is_on;
        let view = |slug: &str| Some(crate::mission::Opening::View(slug.to_string()));
        match source {
            "slack" if on("view_slack") => view("slack"),
            "inbox" if on("linear") && on("view_inbox") => view("inbox"),
            "prs" if crate::sources::enabled(crate::sources::PRS) => view("prs"),
            "meeting" | "calendar" if on("view_calendar") => view("calendar"),
            "habits" => Some(crate::mission::Opening::Settings),
            "autopilot" | "autolanded" => Some(crate::autopilot::opening()),
            _ => None,
        }
    }

    /// The cursor onto the first notification, in the sidebar of the
    /// client's session, the Notifications window coming to the front on it
    /// (goToNotices); with notice_list off and no sidebar there, it says
    /// why.
    pub(crate) fn sidebar_to_notices(&mut self, client: ClientId) {
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        let Some(first) = self.sidebar_notices().into_iter().next() else {
            self.say(Some(client), "No notifications");
            return;
        };
        // With the Notifications window, it comes to the front on the
        // first, browsed from the sidebar, or from its own keys with no
        // sidebar to browse from.
        if settings::is_on("notice_list") {
            if self.sidebar_shown(&session) {
                self.sidebar.side(&session).filter = None;
                self.sidebar_enter(&session, Some(first.owner.clone()));
                self.show_notices(&session, &first.owner);
            } else {
                self.notices_enter(&session, &first.owner);
            }
            return;
        }
        if !self.sidebar_shown(&session) {
            // The first one's view, when that view is on; else why there is
            // nowhere to show them.
            let source = match &first.owner {
                Owner::Notice { source, .. } => source.as_str(),
                _ => "",
            };
            if let Some(view) = Self::notice_view(source) {
                self.open_mission(client, view);
                return;
            }
            let why = if settings::is_on("sidebar") {
                "The notifications are in the sidebar, which this window is too narrow for; Settings' Notifications window shows them in a tab of their own"
            } else {
                "The notifications are in the sidebar, which is off; Settings' Notifications window shows them in a tab of their own"
            };
            self.say(Some(client), why);
            return;
        }
        self.sidebar.side(&session).filter = None;
        self.sidebar_enter(&session, Some(first.owner));
    }

    // ── M-a ─────────────────────────────────────────────────────────────────

    /// What M-a walks, in its order: the tabs under Needs you, then the
    /// notifications while there are any (needs_notices setting).
    fn sidebar_needs_stops(&self, all: &[TabModel]) -> Vec<Stop> {
        let r = Rules::now();
        let mut stops: Vec<Stop> = model::needs_you(all, &r)
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
        if settings::is_on("needs_notices") && !self.sidebar_notices().is_empty() {
            stops.push(Stop {
                place: Place::Notices,
                why: Why::Notices,
            });
        }
        stops
    }

    /// The tab M-a would go to next from `here`, its number and name, from
    /// the same walk without taking it (UX 2.0's header says it): none when
    /// it would go to the notifications, or nowhere else.
    pub fn sidebar_needs_next(&self, here: TabId) -> Option<(usize, String)> {
        let all = self.sidebar_walked_tabs();
        let stops = self.sidebar_needs_stops(&all);
        let Next::Go(i) = self.sidebar.walk.clone().next(&stops, here) else {
            return None;
        };
        let Place::Tab(t) = stops[i].place else {
            return None;
        };
        all.iter()
            .find(|m| m.id == t && t != here)
            .map(|m| (m.index, m.name.clone()))
    }

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
        let all = self.sidebar_walked_tabs();
        let stops = self.sidebar_needs_stops(&all);
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
                    // A folded group it is under unfolds, so it is listed.
                    self.sidebar_unfold_to(&tab.session, t);
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

    /// M-A: the next tab under Idle, in the sidebar's order, wrapping at the
    /// end (agent-state.sh's `idle-next` on the list the daemon wrote,
    /// `writeIdle`); pressed from a sidebar, its cursor stays in the new
    /// tab's sidebar, as with M-a.
    pub fn sidebar_next_idle(&mut self, client: ClientId) {
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        let Some(here) = self.session(&session).map(|s| s.current) else {
            return;
        };
        let all = self.sidebar_walked_tabs();
        let idle: Vec<TabId> = model::idle_tabs(&all, &Rules::now())
            .iter()
            .map(|t| t.id)
            .collect();
        let from_sidebar = self.sidebar.focused(&session);
        match super::walk::next_idle(&idle, here) {
            Next::Nothing => self.say(Some(client), "No idle tabs."),
            Next::OnlyHere => self.say(Some(client), "This is the only idle tab."),
            Next::Go(i) => {
                let Some(tab) = all.iter().find(|m| m.id == idle[i]) else {
                    return;
                };
                if let Some(pane) = tab.claude_pane.or(tab.active) {
                    self.focus_pane(pane);
                }
                self.sidebar_unfold_to(&tab.session, tab.id);
                if tab.session != session {
                    self.switch_client(client, &tab.session);
                }
                if from_sidebar {
                    self.sidebar_enter(&tab.session, Some(Owner::Tab(tab.id)));
                }
            }
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
