//! The mode on the server: what lib/sidebar/daemon.js did for it (M-p's
//! `autopilotStart` and `autopilotSetup`, `pilotMenu`, `pilot.check` on
//! every refresh, its notice) and what tmux/wtn-open.zsh's `next` did, done
//! in the server task.
//!
//! M-p's "Work the note continuously" opens mission control on its tabs,
//! then, with the mode off, asks which note (the one last worked first,
//! then the newest), how many of its tabs work at once, the most open and a
//! spend limit, each typed in a box with what it was before, Enter keeping
//! it; the repository is the main clone of the pane it was asked from. With
//! the mode on, it opens the line's menu instead.
//!
//! Every tick reads `autopilot.json`, gathers the tabs it opened (their
//! directories from what the server holds and git's files, their state from
//! the sidebar's model, or from the pane parked behind M-d's diff), and
//! checks (`Pilot::check`). Reading the queue starts git, so it runs off the
//! task and answers with `Event::Autopilot`; the tab for the item found is
//! opened back on it, in the session used last, as the palette's "Start
//! work on a note's checklist items" opens one (`Server::open_item_tab`).

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use super::line::{self, Act, Summary};
use super::mode::{self, Mode, note_title};
use super::pilot::{Job, Mark};
use super::place::{Mine, in_worktree_of};
use super::{AutopilotEvent, Command};
use crate::agent::State;
use crate::cmd::work::main_clone;
use crate::cmd::worktree_from_note::{self, Item};
use crate::diff::in_place::PARKED_PANE;
use crate::mission::ask::Ask;
use crate::mission::{Mission, Opening};
use crate::mux::server::{ClientId, Event, Server};
use crate::mux::{PaneId, TabId};
use crate::sidebar::model::TabState;
use crate::ui::{Menu, MenuOption, Outcome, PickItem, Picker};
use crate::{paths, settings, util};

/// When it opened one of its tabs, in seconds.
pub const OPENED: &str = "@autopilot";
/// Seen in its worktree.
pub const IN: &str = "@autopilot_in";
/// The first line of the item it was opened for.
pub const ITEM: &str = "@autopilot_item";

/// When a file last changed, in ms; `None` when it isn't there.
fn modified_ms(path: &Path) -> Option<i64> {
    let at = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(at.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64)
}

/// A whole number, 1 or more, as typed; empty keeps `was`.
fn whole(text: &str, was: u32) -> Option<u32> {
    if text.is_empty() {
        return Some(was);
    }
    let n: f64 = text.parse().ok()?;
    (n.fract() == 0.0 && (1.0..=f64::from(u32::MAX)).contains(&n)).then_some(n as u32)
}

/// Dollars, as typed, a `$` in front or not; empty for no limit.
fn dollars(text: &str) -> Option<f64> {
    if text.is_empty() {
        return Some(0.0);
    }
    let n: f64 = text.strip_prefix('$').unwrap_or(text).trim().parse().ok()?;
    (n.is_finite() && n >= 0.0).then_some(n)
}

impl Server {
    // ── M-p ─────────────────────────────────────────────────────────────────

    /// M-p's command, pressed with `pane` current: mission control's tabs,
    /// then the mode set up, or its menu while it is on.
    pub fn autopilot_command(&mut self, client: ClientId, command: Command, pane: Option<PaneId>) {
        match command {
            Command::WorkNote => {
                let dir = pane
                    .and_then(|p| self.panes.get(&p))
                    .and_then(|s| s.pane.cwd())
                    .unwrap_or_else(paths::home);
                self.open_mission(client, Opening::Tabs);
                if !settings::is_on("autopilot") {
                    self.mission_note(
                        client,
                        "Working a note continuously is off in Settings (Work a note continuously)",
                    );
                    return;
                }
                if Mode::read(&paths::state_dir()).on {
                    self.autopilot_menu(client);
                } else {
                    self.autopilot_setup(client, &dir);
                }
            }
        }
    }

    /// Which note, how many tabs at once, the most open and a spend limit,
    /// then the mode on, working the note in the main clone of `dir`'s
    /// repository.
    fn autopilot_setup(&mut self, client: ClientId, dir: &Path) {
        let Some(repo) = main_clone(dir) else {
            self.mission_note(
                client,
                "Ask from a pane in the note's repository: its items are worked there",
            );
            return;
        };
        let notes = worktree_from_note::notes_dir();
        let was = Mode::read(&paths::state_dir());
        let last = was.note.clone();
        self.off_task(
            client,
            move || mode::note_choices(&notes, &last),
            move |server, client, names| {
                if names.is_empty() {
                    server.mission_note(client, "No notes to work");
                    return;
                }
                let items = names
                    .iter()
                    .map(|n| PickItem::new(n.strip_suffix(".md").unwrap_or(n), n.clone()))
                    .collect();
                let picker = Picker::new("Work which note continuously?", items)
                    .named("notes")
                    .prompt("note> ");
                server.pick(client, picker, move |server, client, picked| {
                    if let Some(p) = picked {
                        server.autopilot_ask_counts(client, repo, p.value, was);
                    }
                });
            },
        );
    }

    /// The setup's slots and cap, each typed in starting from what it was.
    fn autopilot_ask_counts(&mut self, client: ClientId, repo: PathBuf, note: String, was: Mode) {
        let (slots_were, cap_was, limit_was) = (was.slots, was.cap, was.spend_limit);
        self.autopilot_number(
            client,
            "Tabs working at once",
            "How many of its tabs work at once; one waiting on you doesn't count.",
            slots_were,
            move |server, client, slots| {
                server.autopilot_number(
                    client,
                    "Tabs open at most",
                    "The most of its tabs open at once, those waiting on you included.",
                    cap_was.max(slots),
                    move |server, client, cap| {
                        if cap < slots {
                            server.mission_note(
                                client,
                                format!("Tabs open at most: at least {slots}"),
                            );
                            return;
                        }
                        server.autopilot_ask_limit(client, repo, note, (slots, cap), limit_was);
                    },
                );
            },
        );
    }

    /// A whole number asked for in a box, `was` typed in already.
    fn autopilot_number(
        &mut self,
        client: ClientId,
        title: &'static str,
        hint: &'static str,
        was: u32,
        then: impl FnOnce(&mut Server, ClientId, u32) + Send + 'static,
    ) {
        let ask = Ask::new(title, hint).text(was.to_string()).empty_ok();
        self.open(
            client,
            Box::new(ask),
            Some(Box::new(move |server: &mut Server, client, outcome| {
                let Outcome::Picked(p) = outcome else {
                    return;
                };
                match whole(&p.value, was) {
                    Some(n) => then(server, client, n),
                    None => {
                        server.mission_note(client, format!("{title}: a whole number, 1 or more"))
                    }
                }
            })),
        );
    }

    /// The setup's spend limit, then the mode on.
    fn autopilot_ask_limit(
        &mut self,
        client: ClientId,
        repo: PathBuf,
        note: String,
        (slots, cap): (u32, u32),
        was: f64,
    ) {
        let ask = Ask::new(
            "Spend limit",
            "Open no more tabs once today's Claude spend passes this many dollars, as the Usage view counts it; empty for none.",
        )
        .text(if was > 0.0 { was.to_string() } else { String::new() })
        .empty_ok();
        self.open(
            client,
            Box::new(ask),
            Some(Box::new(move |server: &mut Server, client, outcome| {
                let Outcome::Picked(p) = outcome else {
                    return;
                };
                let Some(limit) = dollars(&p.value) else {
                    server.mission_note(client, "Spend limit: dollars, or empty for none");
                    return;
                };
                let written = Mode::update(&paths::state_dir(), |m| {
                    // Skipped items are the note's, kept by their first line alone.
                    if m.note != note {
                        m.skipped.clear();
                    }
                    m.on = true;
                    m.paused = false;
                    m.said.clear();
                    m.note = note.clone();
                    m.repo = repo.clone();
                    m.slots = slots;
                    m.cap = cap;
                    m.spend_limit = limit;
                });
                match written {
                    Ok(_) => {
                        server.autopilot.pilot.reset();
                        server.autopilot_tick();
                        server.mission_note(
                            client,
                            format!(
                                "Working {}: {slots} at a time, up to {cap} tabs, in {}",
                                note_title(&note),
                                paths::tilde(&repo)
                            ),
                        );
                    }
                    Err(e) => server.mission_note(client, format!("Couldn't save it: {e}")),
                }
            })),
        );
    }

    // ── Its menu ────────────────────────────────────────────────────────────

    /// The menu Enter on its line in mission control opens, moved through
    /// with j and k as mission control's other menus are.
    pub fn autopilot_menu(&mut self, client: ClientId) {
        let mode = Mode::read(&paths::state_dir());
        let list = line::choices(&mode, &self.autopilot.pilot.next.text);
        let options = list
            .iter()
            .map(|(key, label, _)| MenuOption::new(label.clone()).key(*key))
            .collect();
        let menu = Menu::new(format!("Working {}", note_title(&mode.note)), options);
        self.choose(client, menu, move |server, client, chose| {
            if let Some((_, _, act)) = chose.and_then(|i| list.into_iter().nth(i)) {
                server.autopilot_act(client, act);
            }
        });
    }

    /// Does what an option of its menu, or of its notification's, says.
    pub fn autopilot_act(&mut self, client: ClientId, act: Act) {
        let state = paths::state_dir();
        match act {
            Act::Change => {
                let repo = Mode::read(&state).repo;
                let dir = if repo.as_os_str().is_empty() {
                    paths::home()
                } else {
                    repo
                };
                self.autopilot_setup(client, &dir);
            }
            Act::ToTabs => self.open_mission(client, Opening::Tabs),
            Act::Dismiss => {
                self.autopilot_say("");
                self.mission_note(client, "Dismissed.");
            }
            act => {
                let mut said = None;
                match Mode::update(&state, |m| said = line::apply(&act, m)) {
                    Ok(_) => {
                        self.autopilot.pilot.reset();
                        self.autopilot_tick();
                        if let Some(said) = said {
                            self.mission_note(client, said);
                        }
                    }
                    Err(e) => self.mission_note(client, format!("Couldn't save it: {e}")),
                }
            }
        }
    }

    /// What mission control's line shows, while the mode is on.
    pub fn autopilot_summary(&self) -> Option<Summary> {
        let mode = &self.autopilot.mode;
        (settings::is_on("autopilot") && mode.on)
            .then(|| self.autopilot.pilot.summary(mode, self.away()))
    }

    // ── The check ───────────────────────────────────────────────────────────

    /// Every tick: the mode read again, its tabs marked, and the next item
    /// read or opened when it is time.
    pub fn autopilot_tick(&mut self) {
        let state = paths::state_dir();
        let mode = Mode::read(&state);
        self.autopilot_notices(&mode);
        let on = settings::is_on("autopilot");
        let repo = mode
            .repo
            .canonicalize()
            .unwrap_or_else(|_| mode.repo.clone());
        let mine = if mode.set_up() {
            self.autopilot_mine(&repo)
        } else {
            Vec::new()
        };
        let notes = worktree_from_note::notes_dir();
        let mtime = (on && mode.on && !mode.note.is_empty())
            .then(|| modified_ms(&notes.join(&mode.note)))
            .flatten();
        let away = self.away();
        let counts = self.autopilot.pilot.counts;
        let check = self
            .autopilot
            .pilot
            .check(&mode, on, &mine, away, util::now_ms(), mtime);
        let changed = self.autopilot.mode != mode || self.autopilot.pilot.counts != counts;
        self.autopilot.mode = mode;
        self.autopilot_marks(&check.marks);
        if let Some(said) = check.said {
            self.autopilot_say(&said);
        }
        if let Some(job) = check.read_next {
            self.autopilot_read_next(job, notes.clone());
        }
        if let Some(job) = check.open {
            self.autopilot_look(job, notes);
        }
        if changed {
            self.autopilot_redraw();
        }
    }

    /// The tabs it opened, as a check needs them.
    fn autopilot_mine(&self, repo: &Path) -> Vec<Mine> {
        let opened = |opts: &std::collections::BTreeMap<String, String>| {
            opts.get(OPENED)
                .and_then(|v| v.trim().parse::<f64>().ok())
                .filter(|n| *n != 0.0)
        };
        if !self.tabs.values().any(|t| opened(&t.opts).is_some()) {
            return Vec::new();
        }
        let models: std::collections::HashMap<TabId, _> = self
            .sidebar_all_tabs()
            .into_iter()
            .map(|t| (t.id, t))
            .collect();
        let mut out = Vec::new();
        for session in &self.sessions {
            for id in &session.tabs {
                let Some(tab) = self.tabs.get(id) else {
                    continue;
                };
                let Some(since) = opened(&tab.opts) else {
                    continue;
                };
                let model = models.get(id);
                let panes = tab.layout.panes();
                let state = self
                    .parked_state(&panes)
                    .or(model.map(|m| m.state))
                    .unwrap_or_default();
                // Where it works: its Claude's directory, else its panes'
                // (Notes, which M-e may have put beside it, runs in your home).
                let mut dirs: Vec<PathBuf> = model
                    .and_then(|m| m.claude.as_ref()?.cwd.as_ref().map(PathBuf::from))
                    .into_iter()
                    .chain(model.and_then(|m| m.path.clone()))
                    .collect();
                for pane in &panes {
                    let Some(slot) = self.panes.get(pane) else {
                        continue;
                    };
                    let marked = |name: &str| {
                        slot.opts
                            .get(name)
                            .is_some_and(|v| !v.is_empty() && v != "0")
                    };
                    if marked("@sidebar") || marked("@notes_pane") {
                        continue;
                    }
                    dirs.extend(slot.pane.cwd());
                }
                out.push(Mine {
                    tab: *id,
                    since,
                    was_in: tab.opts.get(IN).is_some_and(|v| !v.is_empty() && v != "0"),
                    item: tab.opts.get(ITEM).filter(|v| !v.is_empty()).cloned(),
                    state,
                    in_worktree: in_worktree_of(&dirs, repo),
                });
            }
        }
        out
    }

    /// While a tool stands in for a tab's pane (M-d's diff), the state of
    /// the pane parked behind it, background work included, as the tab's
    /// own would be.
    fn parked_state(&self, panes: &[PaneId]) -> Option<TabState> {
        let parked: PaneId = panes
            .iter()
            .find_map(|p| self.panes.get(p)?.opts.get(PARKED_PANE)?.parse().ok())?;
        self.panes.get(&parked)?;
        let work = self.sidebar.panes.get(&parked).map(|r| &r.work);
        let agent = self.agents.state(parked);
        Some(
            if agent == Some(State::Waiting) || work.is_some_and(|w| w.api_error.is_some()) {
                TabState::Waiting
            } else if agent == Some(State::Working) {
                TabState::Working
            } else if work.is_some_and(|w| w.count > 0 || w.scheduled.is_some()) {
                TabState::Background
            } else if agent == Some(State::Done) {
                TabState::Done
            } else {
                TabState::Idle
            },
        )
    }

    /// Its tabs marked as they reach their worktree, and unmarked as they
    /// land.
    fn autopilot_marks(&mut self, marks: &[Mark]) {
        for mark in marks {
            let tab = match *mark {
                Mark::In(t) | Mark::Left(t) => t,
            };
            let Some(t) = self.tabs.get_mut(&tab) else {
                continue;
            };
            match mark {
                Mark::In(_) => {
                    t.opts.insert(IN.into(), "1".into());
                }
                Mark::Left(_) => {
                    for option in [OPENED, IN, ITEM] {
                        t.opts.remove(option);
                    }
                }
            }
            self.dirty_tab(tab);
        }
    }

    /// The next item read again, off the task, for mission control's line.
    fn autopilot_read_next(&mut self, job: Job, notes: PathBuf) {
        if tokio::runtime::Handle::try_current().is_err() {
            self.autopilot.pilot.cancel();
            return;
        }
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let text = job
                .first(&notes)
                .ok()
                .flatten()
                .map(|i| i.text)
                .unwrap_or_default();
            let _ = tx.send(Event::Autopilot(AutopilotEvent::Next {
                key: job.key,
                text,
            }));
        });
    }

    /// The next item to open looked for, off the task.
    fn autopilot_look(&mut self, job: Job, notes: PathBuf) {
        if tokio::runtime::Handle::try_current().is_err() {
            self.autopilot.pilot.cancel();
            return;
        }
        let state = paths::state_dir();
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let found = job.find(&notes, &state, util::now_ms());
            let _ = tx.send(Event::Autopilot(AutopilotEvent::Found {
                found,
                mtime: job.mtime,
            }));
        });
    }

    /// What the looks off the task found.
    pub fn autopilot_event(&mut self, event: AutopilotEvent) {
        match event {
            AutopilotEvent::Next { key, text } => {
                let changed = self.autopilot.pilot.next.text != text;
                self.autopilot.pilot.next_read(key, text, util::now_ms());
                if changed {
                    self.autopilot_redraw();
                }
            }
            AutopilotEvent::Found { found, mtime } => {
                let mode = Mode::read(&paths::state_dir());
                let opened = self
                    .autopilot
                    .pilot
                    .opened(&mode, found, mtime, util::now_ms());
                if let Some(e) = opened.failed {
                    eprintln!("autopilot: {e}");
                }
                if let Some(said) = opened.said {
                    self.autopilot_say(&said);
                }
                if let Some(item) = opened.take {
                    self.autopilot_open(&mode, item);
                }
                self.autopilot_redraw();
            }
        }
    }

    /// A tab for the item found, unless the mode was paused or stopped
    /// meanwhile: in the session used last, in the background, marked as
    /// the mode's.
    fn autopilot_open(&mut self, mode: &Mode, item: Item) {
        if !settings::is_on("autopilot") || !mode.on || mode.paused || self.away() {
            return;
        }
        let Some(client) = self.latest_client() else {
            return;
        };
        let Some(tab) = self.open_item_tab(client, &mode.repo, &mode.note, &item) else {
            return;
        };
        if let Some(t) = self.tabs.get_mut(&tab) {
            t.opts.insert(OPENED.into(), util::now_secs().to_string());
            t.opts.insert(ITEM.into(), item.text.clone());
        }
        self.dirty_tab(tab);
        self.autopilot.pilot.took(&item.text, util::now_ms());
        eprintln!(
            "autopilot: opened line {} of {} in {tab}",
            item.line, mode.note
        );
        if mode.said.starts_with("Nothing left") {
            self.autopilot_say("");
        }
    }

    // ── What it says ────────────────────────────────────────────────────────

    /// Its notification put away: what it said, forgotten.
    pub fn autopilot_dismiss(&mut self) {
        self.autopilot_say("");
    }

    /// What it has to say, kept in its file for the notifications; empty
    /// for nothing.
    fn autopilot_say(&mut self, said: &str) {
        match Mode::update(&paths::state_dir(), |m| m.said = said.to_string()) {
            Ok(mode) => {
                self.autopilot_notices(&mode);
                self.autopilot.mode = mode;
            }
            Err(e) => eprintln!("autopilot: couldn't save what it said: {e}"),
        }
    }

    /// Its notification: what it said, while the autopilot setting is on.
    fn autopilot_notices(&mut self, mode: &Mode) {
        let items = if settings::is_on("autopilot") && !mode.said.is_empty() {
            vec![crate::items::Item::new("said", &mode.said).detail("working a note continuously")]
        } else {
            Vec::new()
        };
        self.set_notices("autopilot", items);
    }

    /// Mission control drawn again wherever it is open, for its line.
    fn autopilot_redraw(&mut self) {
        for c in self.clients.values_mut() {
            if c.overlays.find::<Mission>().is_some() {
                c.dirty = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_dollars_as_typed() {
        assert_eq!(whole("", 3), Some(3), "empty keeps what it was");
        assert_eq!(whole("4", 3), Some(4));
        assert_eq!(whole("4.0", 3), Some(4));
        for bad in ["0", "-1", "2.5", "two"] {
            assert_eq!(whole(bad, 3), None, "{bad}");
        }
        assert_eq!(dollars(""), Some(0.0), "empty for none");
        assert_eq!(dollars("$20"), Some(20.0));
        assert_eq!(dollars("7.5"), Some(7.5));
        assert_eq!(dollars("-1"), None);
        assert_eq!(dollars("lots"), None);
    }
}
