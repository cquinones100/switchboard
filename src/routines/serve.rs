//! The routines on the server: read from their folder as the server starts
//! and again when it changes, each run started on the tick once it is due,
//! in its tab (the same one each run, its Claude session resumed once the
//! tab closed, or a new one each run), and ended by that pane's next Stop
//! hook, which records how long it took, what it cost and the reply's
//! first line. A run due while the computer slept runs on waking when it
//! is no more than `LATE_BY` late and the routine says so, else it is
//! recorded as missed. Nothing here waits or starts a process but the run's
//! own tab; the files are a routine's few hundred bytes and a line a run.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::routine::{self, Routine, Run};
use super::schedule::{Schedule, clock_words};
use super::{
    Command, LATE_BY, Live, Outcome, RoutinesEvent, SLEPT_AFTER, START_GRACE, TAB_MARK, TAB_NEXT,
    TabPolicy, WAIT_BUSY, WhenBusy, WhenMissed,
};
use crate::agent::State;
use crate::items::{Item, State as ItemState};
use crate::mux::server::{ClientId, Opened, Server};
use crate::mux::{PaneId, TabId};
use crate::ui::confirm::Confirm;
use crate::ui::{Outcome as UiOutcome, Picked};
use crate::{settings, util, worktree};

/// A run going this long (seconds) with no Stop is recorded as failed.
const RUN_LIMIT: i64 = 2 * 3600;

/// How long (seconds) a Claude a run started has to say it started, its
/// SessionStart then taken as that Claude's rather than another's.
const EXPECT_FOR: i64 = 120;

/// What a run refused in someone else's PR says.
const UNTRUSTED: &str = "its directory holds someone else's PR, so it isn't run there";

/// A run's record, not yet started.
fn live(
    due: i64,
    late: bool,
    by_hand: bool,
    tab: Option<TabId>,
    pane: Option<PaneId>,
    started: i64,
) -> Live {
    Live {
        due,
        late,
        by_hand,
        asked: util::now_secs(),
        started,
        tab,
        pane,
        cost_before: 0.0,
        checking: false,
        quiet_until: 0,
        session: String::new(),
    }
}

/// The last time `r` was due by `now`, from `due`, its first not yet
/// handled. A long gap is searched from two days back, where the most
/// frequent schedule (every minute) has under 3,000 runs to step through,
/// so however long the server was away it counts once and never loops long
/// on the server's task.
fn latest_due(r: &Routine, due: i64, now: i64, tz: &jiff::tz::TimeZone) -> i64 {
    let mut latest = due;
    if now - due > 2 * 86_400
        && let Some(n) = r
            .schedule
            .next_after(now - 2 * 86_400, tz)
            .filter(|n| *n <= now && *n > due)
    {
        latest = n;
    }
    for _ in 0..10_000 {
        match r.schedule.next_after(latest, tz) {
            Some(n) if n <= now => latest = n,
            _ => break,
        }
    }
    latest
}

/// The notifications' source.
pub const SOURCE: &str = "routines";

/// A routine's clock time as the runs say it: `8:45 AM`.
fn clock_at(at: i64) -> String {
    let z = jiff::Timestamp::from_second(at)
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
        .to_zoned(jiff::tz::TimeZone::system());
    clock_words(z.hour() as u16 * 60 + z.minute() as u16)
}

/// Its session's cost so far, as the statusline last wrote it (the third
/// field of `usage/spend-<session>`), 0 for none.
fn session_cost(session: &str) -> f64 {
    if session.is_empty() || !crate::usage::sessions::safe_id(session) {
        return 0.0;
    }
    let file = crate::usage::sessions::usage_dir(&crate::paths::state_dir())
        .join(format!("spend-{session}"));
    std::fs::read_to_string(file)
        .ok()
        .and_then(|t| t.split('\t').nth(2).and_then(|c| c.trim().parse().ok()))
        .unwrap_or(0.0)
}

/// The command a run's Claude starts with: its session resumed when given,
/// the prompt added to its system prompt, its model, and the turn.
fn claude_command(r: &Routine, resume: &str) -> Vec<String> {
    let mut args = Vec::new();
    if !resume.is_empty() {
        args.extend(["--resume".to_string(), resume.to_string()]);
    }
    args.extend(["--append-system-prompt".to_string(), r.prompt.clone()]);
    if !r.model.is_empty() {
        args.extend(["--model".to_string(), r.model.clone()]);
    }
    args.push(r.turn());
    worktree::tab_command("claude \"$@\"", "claude", &args)
}

impl Server {
    /// Reads the routines again when their folder changed (or on the
    /// first look), with each one's runs of the last days.
    fn routines_read(&mut self) {
        let dir = routine::dir();
        if !self.routines.read {
            let was = self.before_starters();
            let home = crate::paths::home();
            super::starters::make_once(&dir, &was, &home, &util::local_now());
        }
        let seen = routine::stamp(&dir);
        if self.routines.read && seen == self.routines.seen {
            return;
        }
        self.routines.read = true;
        self.routines.seen = seen;
        let was = std::mem::replace(&mut self.routines.list, routine::load_all(&dir));
        // One moved to another directory by hand lets its session go, as
        // the box does, since `--resume` finds a session only where it was
        // made.
        let moved: Vec<Routine> = self
            .routines
            .list
            .iter()
            .filter(|r| !r.session.is_empty())
            .filter(|r| was.iter().any(|w| w.id == r.id && w.dir != r.dir))
            .cloned()
            .collect();
        for mut r in moved {
            r.session.clear();
            self.routine_save(&r);
        }
        // One whose schedule changed lets a run of the old one still
        // waiting on a busy tab go, as the box does; one under way, or
        // asked for by hand, stays.
        let rescheduled: Vec<String> = self
            .routines
            .list
            .iter()
            .filter(|r| was.iter().any(|w| w.id == r.id && w.schedule != r.schedule))
            .map(|r| r.id.clone())
            .collect();
        for id in rescheduled {
            if self
                .routines
                .live
                .get(&id)
                .is_some_and(|l| l.started == 0 && !l.by_hand)
            {
                self.routines.live.remove(&id);
            }
        }
        let since = util::now_secs() - super::KEEP_DAYS * 86_400;
        self.routines.runs = self
            .routines
            .list
            .iter()
            .map(|r| (r.id.clone(), routine::read_runs(&dir, &r.id, since)))
            .collect();
        self.mission_changed();
    }

    /// What was on before the starter routines (`starters::Was`): the
    /// settings they replace, Fathom's connector, and the views' tabs the
    /// server kept.
    fn before_starters(&mut self) -> super::starters::Was {
        let todo_hour = settings::cached("todo_note_time")
            .map(|v| util::js_string(&v))
            .and_then(|v| v.parse::<u8>().ok())
            .filter(|h| (7..=11).contains(h))
            .unwrap_or(9);
        super::starters::Was {
            todo_note: settings::is_on("todo_note"),
            todo_hour,
            // Fathom seen working: its connector's last call, or a record of
            // the meetings the built-in notes wrote, which a last call that
            // failed doesn't undo.
            meeting_notes: settings::is_on("meeting_notes")
                && (crate::connector::connected("Fathom")
                    || crate::paths::state_dir().join("meetings.json").exists()),
            slack_tab: !settings::is_on("slack_background")
                && settings::is_on("view_slack")
                && !self.loop_tab_open("Slack"),
            calendar_tab: settings::is_on("view_calendar")
                && self.calendar_needs_tab()
                && !self.loop_tab_open("Calendar"),
        }
    }

    /// Whether a tab the server opened before the starters to keep a view
    /// (`@loop_tab`, named `name`) still runs Claude: its loop goes on, so the
    /// starter replacing it is made paused rather than run beside it.
    fn loop_tab_open(&self, name: &str) -> bool {
        self.sidebar_all_tabs().iter().any(|t| {
            self.tabs
                .get(&t.id)
                .is_some_and(|tab| tab.opts.get("@loop_tab").map(String::as_str) == Some("1"))
                && self.tab_own_name(t.id).unwrap_or(&t.name) == name
                && t.panes.iter().any(|p| self.runs_claude(*p))
        })
    }

    /// A routine written, the folder's time taken as read.
    fn routine_save(&mut self, r: &Routine) {
        let dir = routine::dir();
        if let Err(e) = routine::save(&dir, r) {
            eprintln!("routines: couldn't save {}: {e}", r.id);
        }
        match self.routines.list.iter_mut().find(|x| x.id == r.id) {
            Some(x) => *x = r.clone(),
            None => self.routines.list.push(r.clone()),
        }
        self.routines.seen = routine::stamp(&dir);
        if self.routines.said.contains_key(&r.id) {
            self.routines_notices();
        }
        self.mission_changed();
    }

    /// The tab a routine's runs are in, while it is open.
    /// With a new tab each run, several may be open: the one its run under
    /// way is in, else the newest.
    pub fn routine_tab(&self, id: &str) -> Option<TabId> {
        let marked = |t: &TabId| {
            self.tabs
                .get(t)
                .is_some_and(|tab| tab.opts.get(TAB_MARK).map(String::as_str) == Some(id))
        };
        self.routines
            .live
            .get(id)
            .and_then(|l| l.tab)
            .filter(marked)
            .or_else(|| self.tabs.keys().copied().filter(marked).max())
    }

    /// A tab's number, as its routine's runs name it: the one it keeps
    /// while open (`Server::tab_number`), empty for none.
    fn routine_tab_number(&self, tab: TabId) -> String {
        match self.tab_number(tab) {
            0 => String::new(),
            n => n.to_string(),
        }
    }

    /// The pane in a tab its Claude runs in: one running Claude, else its
    /// active pane.
    fn routine_pane(&self, tab: TabId) -> Option<PaneId> {
        let panes = self.tab_panes(tab);
        panes
            .iter()
            .copied()
            .find(|p| self.agents.state(*p).is_some() || self.runs_claude(*p))
            .or_else(|| self.tabs.get(&tab).map(|t| t.active))
    }

    /// The session of the Claude running in a pane and the directory it
    /// runs in, from its session file (Claude Code's `sessions/<pid>.json`).
    fn pane_claude_session(&self, pane: PaneId) -> Option<(String, String)> {
        let slot = self.panes.get(&pane)?;
        let dir = crate::agent::sessions::sessions_dir();
        slot.pane.foreground_pids().into_iter().find_map(|pid| {
            let v = crate::util::read_json(dir.join(format!("{pid}.json")))?;
            let session = v.get("sessionId")?.as_str()?.to_string();
            let cwd = v
                .get("cwd")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            (!session.is_empty()).then_some((session, cwd))
        })
    }

    /// Whether a pane's Claude is in the middle of a turn or waiting on you.
    /// A draft of yours unsent in its prompt box counts too: the run's
    /// Enter would send it.
    fn claude_busy(&self, pane: PaneId) -> bool {
        // As is a dialog of Claude's own start (trusting a folder, an MCP
        // server), which runs no hook: the run would answer or end it.
        let waits_on_screen = self.panes.get(&pane).is_some_and(|slot| {
            let screen = slot.pane.text(0);
            crate::relay::tabs::draft_in(&screen, &slot.pane.typed_text(0)).is_some()
                || crate::relay::tabs::dialog_in(&screen).is_some()
        });
        waits_on_screen
            || matches!(
                self.agents.state(pane),
                Some(State::Working | State::Waiting)
            )
    }

    /// Whether its Claude exited and something else of yours runs in the
    /// pane now (an editor, a build, a script, the diff in its place,
    /// anything in front but the pane's own shell at its prompt): busy, as a Claude at work is, and
    /// never ended for a run.
    fn other_program(&self, pane: PaneId) -> bool {
        // A tool standing in its place (the diff, lazygit) is in front of
        // it, whatever runs behind.
        let covered = self
            .panes
            .get(&pane)
            .is_some_and(|slot| slot.opts.contains_key(crate::diff::in_place::PANE_PARKED));
        if covered {
            return true;
        }
        // Anything not known to be the shell at its prompt could lose
        // something if ended.
        !self.claude_in(pane)
            && !self
                .panes
                .get(&pane)
                .is_some_and(|slot| crate::mux::persist::at_its_shell(&slot.pane))
    }

    /// Whether a pane runs Claude, by the process in front (its session
    /// file, or its name), never by what its hooks last said: a Claude
    /// gone without a SessionEnd leaves that behind.
    fn claude_in(&self, pane: PaneId) -> bool {
        self.runs_claude(pane)
    }

    /// The routines on the tick: read again when changed; each one due
    /// started, or recorded as missed; each run waiting on a busy tab
    /// started once it is idle or skipped after `WAIT_BUSY`; a run whose
    /// tab closed recorded as failed.
    pub fn routines_tick(&mut self) {
        if !settings::is_on("routines") {
            return;
        }
        self.routines_read();
        // A restore still to type its panes' commands (a Claude resumed)
        // goes first, so a run never starts a pane's Claude over the one
        // about to be typed there.
        if self.restore_typing() {
            return;
        }
        let now = util::now_secs();
        // Ticks with a gap between them: the computer slept.
        if self.routines.ticked == 0 || now - self.routines.ticked > SLEPT_AFTER {
            self.routines.awake_since = now;
        }
        self.routines.ticked = now;
        let tz = jiff::tz::TimeZone::system();
        let routines = self.routines.list.clone();
        for r in &routines {
            if r.paused {
                continue;
            }
            let Some(due) = r.next(&tz) else {
                continue;
            };
            if due > now {
                continue;
            }
            if self.routines.live.contains_key(&r.id) {
                // Due again while its last run goes on: skipped, as a busy
                // tab is, or run once that one ends.
                // Waited on like a busy tab: skipped once it has waited
                // `WAIT_BUSY`, the last one still going.
                if r.when_busy == WhenBusy::Skip || now - due >= WAIT_BUSY {
                    let latest = latest_due(r, due, now, &tz);
                    let mut handled = r.clone();
                    handled.handled = latest;
                    self.routine_save(&handled);
                    self.routine_record(
                        &handled,
                        Run {
                            due: latest,
                            ran: 0,
                            tab: String::new(),
                            secs: 0,
                            cost: 0.0,
                            outcome: Outcome::Busy,
                            words: "its last run was still going, so skipped".into(),
                        },
                    );
                }
                continue;
            }
            // The todo note's starter waits for you, as the note it writes
            // gathers what is there when you are, and runs on your return.
            if self.away() && super::starters::is_todo_note(&routine::dir(), r) {
                continue;
            }
            // The last time it was due, when it missed several.
            let latest = latest_due(r, due, now, &tz);
            let mut handled = r.clone();
            handled.handled = latest;
            self.routine_save(&handled);
            if super::starters::done_today(&routine::dir(), r)
                || super::starters::from_another_day(&routine::dir(), r, latest, &tz)
            {
                continue;
            }
            let missed = latest < self.routines.awake_since;
            if !missed {
                self.routine_start(&handled, latest, false, false);
            } else if r.when_missed == WhenMissed::OnWaking && now - latest <= LATE_BY {
                self.routine_start(&handled, latest, true, false);
            } else {
                self.routine_record(
                    &handled,
                    Run {
                        due: latest,
                        ran: 0,
                        tab: String::new(),
                        secs: 0,
                        cost: 0.0,
                        outcome: Outcome::Missed,
                        words: format!("asleep past {}, so skipped", clock_at(latest)),
                    },
                );
            }
        }
        let panes = &self.panes;
        self.routines.started.retain(|p, _| panes.contains_key(p));
        self.routines
            .pane_sessions
            .retain(|p, _| panes.contains_key(p));
        self.routines
            .expect
            .retain(|p, until| panes.contains_key(p) && *until >= now);
        self.routines_waiting(now);
        // Its notifications follow its tab opening and closing.
        if !self.routines.said.is_empty() {
            self.routines_notices();
        }
        self.routines_next_on_tabs(&tz);
    }

    /// The runs waiting on a busy tab, and those whose tab closed.
    fn routines_waiting(&mut self, now: i64) {
        let live: Vec<(String, Live)> = self
            .routines
            .live
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (id, run) in live {
            let Some(r) = self.routines.get(&id).cloned() else {
                self.routines.live.remove(&id);
                continue;
            };
            if run.checking {
                continue;
            }
            // Paused meanwhile (its file changed by hand): one not started
            // goes.
            if r.paused && run.started == 0 && !run.by_hand {
                self.routines.live.remove(&id);
                continue;
            }
            let tab_open = run.tab.is_some_and(|t| self.tabs.contains_key(&t));
            if run.started > 0 {
                let pane = run
                    .pane
                    .or_else(|| run.tab.and_then(|t| self.routine_pane(t)));
                if !tab_open {
                    self.routine_end(&r, Outcome::Failed, "its tab closed mid-run".into());
                } else if now - run.started > START_GRACE
                    && pane.is_some_and(|p| !self.runs_claude(p))
                {
                    // Its Claude gone back to the shell, with no hook said:
                    // what its hooks left of it goes too, so the next run
                    // finds the shell idle and starts Claude again.
                    if let Some(p) = pane {
                        self.agents.drop_state(p);
                        self.routines.started.remove(&p);
                    }
                    self.routine_end(&r, Outcome::Failed, "Claude exited mid-run".into());
                } else if now - run.started > RUN_LIMIT
                    && pane.is_none_or(|p| {
                        // No hook from it in that long either: a turn at
                        // work runs hooks with each tool it calls.
                        let heard = self.agents.get(p).map_or(0, |a| a.heard / 1000);
                        now - heard > RUN_LIMIT
                    })
                {
                    // A turn that ended with no Stop (an API error) holds
                    // nothing up past this, nor the working state its last
                    // hook left, which no Stop will now clear. A question
                    // or permission prompt still waiting on you stays, so
                    // no run's Enter answers it.
                    if let Some(p) = pane.filter(|p| self.agents.state(*p) == Some(State::Working))
                    {
                        self.agents.drop_state(p);
                    }
                    self.routine_end(&r, Outcome::Failed, "no answer in 2 hours".into());
                }
                continue;
            }
            // Waiting on a busy tab.
            let tab = run.tab.filter(|_| tab_open);
            // A Claude gone without a word is no longer busy.
            if let Some(p) = tab.and_then(|t| self.routine_pane(t))
                && !self.claude_in(p)
                && self.agents.drop_state(p)
            {
                self.routines.started.remove(&p);
            }
            let idle = tab
                .and_then(|t| self.routine_pane(t))
                .is_some_and(|p| !self.claude_busy(p) && !self.other_program(p));
            // Its 30 minutes are up first, however idle the tab is now (a
            // computer that slept through the wait).
            if now - run.asked < WAIT_BUSY && (tab.is_none() || idle) {
                self.routines.live.remove(&id);
                self.routine_start(&r, run.due, run.late, run.by_hand);
            } else if now - run.asked >= WAIT_BUSY {
                self.routines.live.remove(&id);
                let number = tab.map(|t| self.routine_tab_number(t)).unwrap_or_default();
                self.routine_record(
                    &r,
                    Run {
                        due: run.due,
                        ran: 0,
                        tab: number.clone(),
                        secs: 0,
                        cost: 0.0,
                        outcome: Outcome::Busy,
                        words: format!("tab {number} was busy for 30 minutes, so skipped"),
                    },
                );
            }
        }
    }

    /// Each routine's tab told when it runs next, for the sidebar.
    fn routines_next_on_tabs(&mut self, tz: &jiff::tz::TimeZone) {
        let marks: Vec<(TabId, String)> = self
            .tabs
            .iter()
            .filter_map(|(t, tab)| tab.opts.get(TAB_MARK).map(|id| (*t, id.clone())))
            .collect();
        for (tab, id) in marks {
            let next = self
                .routines
                .get(&id)
                .filter(|r| !r.paused)
                .and_then(|r| r.next(tz))
                .map(|n| n.to_string());
            let was = self
                .tabs
                .get(&tab)
                .and_then(|t| t.opts.get(TAB_NEXT))
                .cloned();
            if was != next {
                self.set_tab_opt(tab, TAB_NEXT, next.as_deref());
            }
        }
    }

    /// Starts a run of `r` due at `due`, once its directory is found not
    /// to hold someone else's PR: by its path here, and by the mark in its
    /// worktree config off the task (`trust::is_untrusted` runs git).
    fn routine_start(&mut self, r: &Routine, due: i64, late: bool, by_hand: bool) {
        let dir = PathBuf::from(&r.dir);
        let in_review = crate::trust::in_review_dir(&dir)
            || dir
                .canonicalize()
                .is_ok_and(|real| crate::trust::in_review_dir(&real));
        if in_review {
            self.routine_refused(r, due, UNTRUSTED.into());
            return;
        }
        if !dir.is_dir() {
            self.routine_refused(r, due, format!("{} is gone", r.dir));
            return;
        }
        if tokio::runtime::Handle::try_current().is_err() {
            let untrusted = crate::trust::marked(&dir);
            self.routine_checked(r, due, late, by_hand, untrusted);
            return;
        }
        let mut held = live(due, late, by_hand, None, None, 0);
        held.checking = true;
        self.routines.live.insert(r.id.clone(), held);
        let tx = self.tx.clone();
        let id = r.id.clone();
        tokio::task::spawn_blocking(move || {
            let untrusted = crate::trust::marked(&dir);
            let _ = tx.send(crate::mux::server::Event::Routines(
                RoutinesEvent::Checked {
                    id,
                    dir: dir.to_string_lossy().into_owned(),
                    untrusted,
                },
            ));
        });
    }

    /// What was found off the task.
    pub fn routines_event(&mut self, event: RoutinesEvent) {
        match event {
            RoutinesEvent::Checked { id, dir, untrusted } => {
                let Some(run) = self.routines.live.remove(&id).filter(|l| l.checking) else {
                    return;
                };
                // Turned off meanwhile: no routine runs.
                if !settings::is_on("routines") {
                    return;
                }
                let Some(r) = self
                    .routines
                    .get(&id)
                    .cloned()
                    .filter(|r| !r.paused || run.by_hand)
                else {
                    return;
                };
                // Moved meanwhile: the new directory is checked in its turn.
                if r.dir != dir {
                    self.routine_start(&r, run.due, run.late, run.by_hand);
                    return;
                }
                self.routine_checked(&r, run.due, run.late, run.by_hand, untrusted);
            }
        }
    }

    fn routine_refused(&mut self, r: &Routine, due: i64, words: String) {
        self.routine_record(
            r,
            Run {
                due,
                ran: 0,
                tab: String::new(),
                secs: 0,
                cost: 0.0,
                outcome: Outcome::Refused,
                words,
            },
        );
    }

    /// A run started in its tab as its policy says, held while that tab
    /// is busy, or skipped; refused when its directory turned out to hold
    /// someone else's PR.
    fn routine_checked(
        &mut self,
        r: &Routine,
        due: i64,
        late: bool,
        by_hand: bool,
        untrusted: bool,
    ) {
        if untrusted {
            self.routine_refused(r, due, UNTRUSTED.into());
            return;
        }
        let now = util::now_secs();
        let dir = PathBuf::from(&r.dir);
        let live = |tab: Option<TabId>, pane: Option<PaneId>, started: i64| {
            live(due, late, by_hand, tab, pane, started)
        };
        let open = match r.tab_policy {
            TabPolicy::SameTab => self.routine_tab(&r.id),
            TabPolicy::NewTab => None,
        };
        let Some(tab) = open else {
            let resume = r.tab_policy == TabPolicy::SameTab;
            if let Some((tab, pane)) = self.routine_open_tab(r, &dir, resume) {
                eprintln!("routines: {} runs in a new tab, {tab}", r.id);
                // Bound to its own pane, so another of the routine's tabs
                // ending meanwhile isn't taken for it.
                let mut run = live(Some(tab), Some(pane), now);
                // From its session's cost so far, which the statusline
                // carries on across a resume (a new session's is nothing);
                // a cost gone down by the end is a count started over.
                run.cost_before = if resume {
                    session_cost(r.resumable())
                } else {
                    0.0
                };
                self.routines.live.insert(r.id.clone(), run);
            } else {
                // No session to open it in (a server with no workspace
                // yet), or the tab couldn't start: said, never lost.
                self.routine_record(
                    r,
                    Run {
                        due,
                        ran: 0,
                        tab: String::new(),
                        secs: 0,
                        cost: 0.0,
                        outcome: Outcome::Failed,
                        words: "couldn't open its tab (no workspace open?), see the server's log"
                            .into(),
                    },
                );
            }
            return;
        };
        let pane = self.routine_pane(tab);
        let Some(pane) = pane else {
            return;
        };
        // What the hooks of a Claude gone without a word left behind goes,
        // so the pane is judged by what runs in it now.
        if !self.claude_in(pane) && self.agents.drop_state(pane) {
            self.routines.started.remove(&pane);
        }
        let other = self.other_program(pane);
        if !self.claude_in(pane) && !other {
            // Its Claude exited to the shell: started again in its pane.
            let session = self.routine_respawn(r, tab, pane, &dir);
            eprintln!(
                "routines: {} starts Claude again in {pane}, which had exited",
                r.id
            );
            let mut run = live(Some(tab), Some(pane), now);
            // From the resumed session's cost so far (see above).
            run.cost_before = session_cost(&session);
            run.session = session;
            run.quiet_until = now + 10;
            self.routines.live.insert(r.id.clone(), run);
            return;
        }
        if other || self.claude_busy(pane) {
            if r.when_busy == WhenBusy::Skip && !by_hand {
                let number = self.routine_tab_number(tab);
                self.routine_record(
                    r,
                    Run {
                        due,
                        ran: 0,
                        tab: number.clone(),
                        secs: 0,
                        cost: 0.0,
                        outcome: Outcome::Busy,
                        words: format!("tab {number} was busy, so skipped"),
                    },
                );
            } else {
                self.routines
                    .live
                    .insert(r.id.clone(), live(Some(tab), Some(pane), 0));
                eprintln!("routines: {} waits on {pane}, busy", r.id);
            }
            return;
        }
        let mut run = live(Some(tab), Some(pane), now);
        if self.routines.started.get(&pane) != Some(&(r.prompt_hash(), r.dir.clone())) {
            // New words or a new directory, or a Claude this server didn't
            // start with them (a tab brought back by a restart): started
            // again with them, resuming its session (a directory changed
            // left none), so they take.
            let session = self.routine_respawn(r, tab, pane, &dir);
            eprintln!(
                "routines: {} starts Claude again in {pane} with its prompt",
                r.id
            );
            run.quiet_until = now + 10;
            run.cost_before = session_cost(&session);
            run.session = session;
        } else {
            self.type_into_claude(pane, crate::mission::answer::message_presses(&r.turn()));
            eprintln!("routines: {} runs in {pane}", r.id);
            // The session of the Claude typed into, whatever another of the
            // routine's tabs last named.
            run.session = self
                .routines
                .pane_sessions
                .get(&pane)
                .map(|(session, _)| session.clone())
                .unwrap_or_else(|| r.session.clone());
            run.cost_before = session_cost(&run.session);
        }
        self.routines.live.insert(r.id.clone(), run);
    }

    /// A new tab for a run, in the background at the end of the session
    /// used last (the first one with no client), named after the routine
    /// and marked as its.
    fn routine_open_tab(
        &mut self,
        r: &Routine,
        dir: &Path,
        resume: bool,
    ) -> Option<(TabId, PaneId)> {
        let session = self
            .latest_client()
            .and_then(|c| self.clients.get(&c))
            .map(|c| c.session.clone())
            .or_else(|| self.sessions.first().map(|s| s.name.clone()))?;
        let command = claude_command(r, if resume { r.resumable() } else { "" });
        let (tab, pane) = match self.new_tab(
            &session,
            Some(dir.to_path_buf()),
            Some(command),
            None,
            Opened::Background,
        ) {
            Ok((tab, pane)) => {
                self.routines
                    .started
                    .insert(pane, (r.prompt_hash(), r.dir.clone()));
                self.routines
                    .expect
                    .insert(pane, util::now_secs() + EXPECT_FOR);
                (tab, pane)
            }
            Err(e) => {
                eprintln!("routines: couldn't open a tab for {}: {e}", r.id);
                return None;
            }
        };
        self.tab_to_end(tab);
        self.routine_mark(r, tab);
        Some((tab, pane))
    }

    /// A routine's tab marked as its, named after it.
    fn routine_mark(&mut self, r: &Routine, tab: TabId) {
        if let Some(t) = self.tabs.get_mut(&tab) {
            t.opts.insert(TAB_MARK.into(), r.id.clone());
            t.opts.insert("@tab_name".into(), r.name.clone());
            t.opts.insert("@tab_title".into(), r.name.clone());
        }
        self.name_tab(tab);
        self.dirty_everywhere(tab);
    }

    /// Its Claude started again in `pane`, its session resumed.
    /// Gives the session it resumed: the pane's own, as its hooks last
    /// named it, while it was started in the routine's directory as it is
    /// now, else the routine's.
    fn routine_respawn(&mut self, r: &Routine, tab: TabId, pane: PaneId, dir: &Path) -> String {
        // What the server remembers of the pane, else (after an upgrade,
        // which keeps none of it) the session file of the Claude running
        // there, else the routine's own.
        let session = self
            .routines
            .pane_sessions
            .get(&pane)
            .cloned()
            .or_else(|| self.pane_claude_session(pane))
            .filter(|(_, made_in)| *made_in == r.dir)
            .map(|(session, _)| session)
            .unwrap_or_else(|| r.resumable().to_string());
        let command = claude_command(r, &session);
        if let Err(e) = self.respawn_pane(pane, Some(dir.to_path_buf()), Some(command), true) {
            eprintln!("routines: couldn't start {} again: {e}", r.id);
            return session;
        }
        self.routines
            .started
            .insert(pane, (r.prompt_hash(), r.dir.clone()));
        self.routines
            .expect
            .insert(pane, util::now_secs() + EXPECT_FOR);
        self.routine_mark(r, tab);
        session
    }

    /// A run's end recorded, from what the server kept of it.
    fn routine_end(&mut self, r: &Routine, outcome: Outcome, words: String) {
        let Some(run) = self.routines.live.remove(&r.id) else {
            return;
        };
        let now = util::now_secs();
        // Its own session's, never what another of the routine's tabs
        // last named.
        let session = if run.session.is_empty() {
            &r.session
        } else {
            &run.session
        };
        // A cost below where it started is a new process's count started
        // over (the statusline's `next_spend`), counted from nothing.
        let now_cost = session_cost(session);
        let cost = if now_cost < run.cost_before {
            now_cost
        } else {
            now_cost - run.cost_before
        };
        let outcome = match outcome {
            Outcome::Ran if run.late => Outcome::Late,
            o => o,
        };
        let words = if run.late && outcome.ran() {
            format!("missed at {}, ran on waking · {words}", clock_at(run.due))
        } else {
            words
        };
        let tab = run
            .tab
            .map(|t| self.routine_tab_number(t))
            .unwrap_or_default();
        self.routine_record(
            r,
            Run {
                due: run.due,
                ran: run.started,
                tab,
                secs: (now - run.started).max(0),
                cost: (cost * 100.0).round() / 100.0,
                outcome,
                words,
            },
        );
    }

    /// A run written to its routine's runs; one that failed or was skipped
    /// said once and kept in the notifications until put away.
    fn routine_record(&mut self, r: &Routine, run: Run) {
        eprintln!(
            "routines: {} {:?} (due {}): {}",
            r.id, run.outcome, run.due, run.words
        );
        if let Err(e) = routine::append_run(&routine::dir(), &r.id, &run) {
            eprintln!("routines: couldn't record a run of {}: {e}", r.id);
        }
        if !run.outcome.ran() {
            let words = format!("Routine {}: {}.", r.name, run.words);
            let client = self.latest_client();
            self.say(client, &words);
            let mut item = Item::new(format!("routine:{}", r.id), format!("Routine {}", r.name));
            item.detail = run.words.clone();
            item.time = Some(run.at() * 1000);
            item.state = ItemState::Attention;
            item.data = json!({ "routine": r.id });
            self.routines.said.insert(r.id.clone(), item);
            self.routines_notices();
        }
        // Only the last `KEEP_DAYS` are kept, as a read keeps them.
        let since = util::now_secs() - super::KEEP_DAYS * 86_400;
        let runs = self.routines.runs.entry(r.id.clone()).or_default();
        runs.push(run);
        runs.retain(|run| run.at() >= since);
        self.mission_changed();
    }

    /// The notifications of runs that failed or were skipped.
    /// Each carries what the routine's item in the view does now (whether it
    /// is paused, its tab), so its menu is that item's.
    fn routines_notices(&mut self) {
        let mut items: Vec<Item> = self.routines.said.values().cloned().collect();
        for item in items.iter_mut() {
            let id = item.data["routine"].as_str().unwrap_or("").to_string();
            let paused = self.routines.get(&id).is_some_and(|r| r.paused);
            item.data["paused"] = json!(paused);
            item.target = self
                .routine_tab(&id)
                .map(crate::items::Target::Tab)
                .unwrap_or_default();
        }
        items.sort_by_key(|i| std::cmp::Reverse(i.time));
        if self.notices.get(SOURCE).map(Vec::as_slice).unwrap_or(&[]) == items.as_slice() {
            return;
        }
        self.set_notices(SOURCE, items);
    }

    /// A routine's notification put away.
    pub fn routines_dismiss(&mut self, item: &Item) {
        let id = item
            .data
            .get("routine")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| item.id.strip_prefix("routine:").map(str::to_string));
        if let Some(id) = id {
            self.routines.said.remove(&id);
        }
        self.routines_notices();
    }

    /// What a hook says of a routine's tab: its session id kept; a Stop
    /// ending the run under way there; Claude exiting ending it as failed.
    pub fn routines_hook(&mut self, pane: PaneId, tab: TabId, event: &str, payload: &Value) {
        let Some(id) = self
            .tabs
            .get(&tab)
            .and_then(|t| t.opts.get(TAB_MARK))
            .cloned()
        else {
            return;
        };
        let Some(mut r) = self.routines.get(&id).cloned() else {
            return;
        };
        // A Claude started in its pane by anything but a run (by hand, or a
        // restart's resume) never had the prompt, and one ending leaves
        // none: the next run starts it again with the prompt.
        let source = payload.get("source").and_then(Value::as_str).unwrap_or("");
        let expected = self
            .routines
            .expect
            .get(&pane)
            .is_some_and(|until| *until >= util::now_secs());
        match event {
            "SessionStart" if matches!(source, "startup" | "resume") => {
                if expected {
                    self.routines.expect.remove(&pane);
                } else {
                    self.routines.started.remove(&pane);
                }
            }
            "SessionEnd" if !expected => {
                self.routines.started.remove(&pane);
            }
            _ => {}
        }
        let session = payload
            .get("session_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        // Where the session was made: the directory a run started this
        // pane's Claude in; for one started otherwise, the directory its
        // hooks first said, kept from then on (empty when none did, which
        // is never resumed).
        let made_in = match (
            self.routines.started.get(&pane),
            self.routines.pane_sessions.get(&pane),
        ) {
            (Some((_, dir)), _) => dir.clone(),
            (None, Some((known, dir))) if known == session => dir.clone(),
            _ => payload
                .get("cwd")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        };
        if !session.is_empty() && event != "SessionEnd" {
            self.routines
                .pane_sessions
                .insert(pane, (session.to_string(), made_in));
        }
        // Only from a Claude a run started in the routine's directory as
        // it is now: an older turn in a directory it moved from would put
        // back a session `--resume` can't find here.
        let here = self
            .routines
            .started
            .get(&pane)
            .is_some_and(|(_, dir)| *dir == r.dir);
        // And only from its one tab while it runs in the same tab each run:
        // with several open, another's conversation isn't its own.
        let its_tab = r.tab_policy == TabPolicy::SameTab && self.routine_tab(&id) == Some(tab);
        if here && its_tab && !session.is_empty() && r.session != session && event != "SessionEnd" {
            r.session = session.to_string();
            r.session_dir = r.dir.clone();
            self.routine_save(&r);
        }
        let Some(run) = self.routines.live.get_mut(&id) else {
            return;
        };
        if run.started == 0 {
            return;
        }
        if run.pane.is_none() {
            run.pane = Some(pane);
        }
        if run.pane != Some(pane) {
            return;
        }
        if !session.is_empty() && event != "SessionEnd" {
            run.session = session.to_string();
        }
        let quiet_until = run.quiet_until;
        match event {
            "Stop" => {
                let reply = payload
                    .get("last_assistant_message")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let first = reply
                    .lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty())
                    .unwrap_or("")
                    .to_string();
                self.routine_end(&r, Outcome::Ran, first);
            }
            // A Claude ended as it started again with new words is no end.
            "SessionEnd" if util::now_secs() >= quiet_until => {
                self.routine_end(&r, Outcome::Failed, "Claude exited mid-run".into());
            }
            _ => {}
        }
    }

    // ── What you do with them ───────────────────────────────────────────────

    /// The palette's commands.
    pub fn routines_command(&mut self, client: ClientId, command: Command) {
        match command {
            Command::New => self.routine_new(client),
        }
    }

    /// Whether they are on, said when they aren't.
    fn routines_on(&mut self, client: ClientId) -> bool {
        if settings::is_on("routines") {
            self.routines_read();
            return true;
        }
        self.say(Some(client), "Routines are off in Settings.");
        false
    }

    /// The box for a new routine, in the directory of the pane in front.
    pub fn routine_new(&mut self, client: ClientId) {
        if !self.routines_on(client) {
            return;
        }
        let dir = self
            .current_pane(client)
            .and_then(|p| self.pane_dir(p))
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_default();
        let draft = super::form::Draft {
            dir,
            ..super::form::Draft::default()
        };
        self.routine_form(client, None, draft, None);
    }

    /// The box for changing a routine.
    pub fn routine_edit(&mut self, client: ClientId, id: &str) {
        if !self.routines_on(client) {
            return;
        }
        let Some(r) = self.routines.get(id).cloned() else {
            return;
        };
        self.routine_form(
            client,
            Some(r.id.clone()),
            super::form::Draft::of(&r),
            Some(r.schedule.clone()),
        );
    }

    fn routine_form(
        &mut self,
        client: ClientId,
        id: Option<String>,
        draft: super::form::Draft,
        kept: Option<Schedule>,
    ) {
        let places = self.routine_places();
        let boxed = super::form::RoutineForm::new(id.is_some(), draft, places, kept);
        self.open(
            client,
            Box::new(boxed),
            Some(Box::new(move |server: &mut Server, client, outcome| {
                let UiOutcome::Picked(Picked { index, value, .. }) = outcome else {
                    return;
                };
                let Ok(draft) = serde_json::from_str::<super::form::Draft>(&value) else {
                    return;
                };
                server.routine_saved(client, id.as_deref(), draft, index == 1);
            })),
        );
    }

    /// The directories the box suggests: the tabs', in their order.
    fn routine_places(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for t in self.sidebar_all_tabs() {
            if let Some(d) = t.path.as_ref().map(|d| d.to_string_lossy().into_owned())
                && !out.contains(&d)
            {
                out.push(d);
            }
        }
        out
    }

    /// What the box gave, saved as a routine (a new one, or `id` changed),
    /// and run now when asked.
    fn routine_saved(
        &mut self,
        client: ClientId,
        id: Option<&str>,
        draft: super::form::Draft,
        run_now: bool,
    ) {
        let now = util::local_now();
        let was = id.and_then(|id| self.routines.get(id).cloned());
        // When left as it was keeps its schedule, rather than reading it
        // again against today.
        let kept = was
            .as_ref()
            .filter(|w| w.when.trim() == draft.when.trim())
            .map(|w| w.schedule.clone());
        let schedule = match kept.map_or_else(|| Schedule::parse(&draft.when, &now), Ok) {
            Ok(s) => s,
            Err(e) => {
                self.say(Some(client), &e);
                return;
            }
        };
        let typed = draft.dir.trim();
        let dir = match typed.strip_prefix('~') {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => {
                crate::paths::home().join(rest.trim_start_matches('/'))
            }
            _ => PathBuf::from(typed),
        };
        if !dir.is_absolute() {
            self.say(
                Some(client),
                "A routine's directory is its whole path, or one from ~.",
            );
            return;
        }
        let r = Routine {
            id: was
                .as_ref()
                .map(|w| w.id.clone())
                .unwrap_or_else(|| routine::new_id(util::now_ms(), &self.routines.list)),
            name: draft.name.trim().to_string(),
            dir: dir.to_string_lossy().into_owned(),
            when: draft.when.trim().to_string(),
            // A schedule changed counts from now, so it doesn't run at once
            // for the times it would have run before.
            handled: match &was {
                Some(w) if w.schedule == schedule => w.handled,
                _ => now.timestamp().as_second(),
            },
            schedule,
            tab_policy: draft.tab_policy,
            when_busy: draft.when_busy,
            when_missed: draft.when_missed,
            prompt: draft.prompt.trim_end().to_string(),
            model: draft.model.trim().to_string(),
            paused: was.as_ref().is_some_and(|w| w.paused),
            // Kept with where it was made, `resumable` resuming it only there.
            session: was.as_ref().map(|w| w.session.clone()).unwrap_or_default(),
            session_dir: was
                .as_ref()
                .map(|w| w.session_dir.clone())
                .unwrap_or_default(),
        };
        self.routine_save(&r);
        // A run of the schedule before still waiting on a busy tab goes
        // with it; one under way, or asked for by hand, stays.
        if was.as_ref().is_some_and(|w| w.schedule != r.schedule)
            && self
                .routines
                .live
                .get(&r.id)
                .is_some_and(|l| l.started == 0 && !l.by_hand)
        {
            self.routines.live.remove(&r.id);
        }
        // A new name shows on its open tab at once.
        if was.as_ref().is_some_and(|w| w.name != r.name)
            && let Some(tab) = self.routine_tab(&r.id)
        {
            self.routine_mark(&r, tab);
        }
        let tz = jiff::tz::TimeZone::system();
        let next = r
            .next(&tz)
            .map(|n| format!(" Next: {}.", crate::mission::remind::said(n)))
            .unwrap_or_default();
        let verb = if was.is_some() { "changed" } else { "made" };
        if run_now {
            if let Some(note) = self.routine_run_now(client, &r.id) {
                self.mission_note(client, &note);
            }
        } else {
            self.mission_note(client, format!("Routine {} {verb}.{next}", r.name));
        }
    }

    /// r: a run now, whatever the schedule says, as the schedule's would
    /// (a busy tab waits, whatever the routine says).
    /// What it did, for mission control's footer.
    pub fn routine_run_now(&mut self, client: ClientId, id: &str) -> Option<String> {
        if !self.routines_on(client) {
            return None;
        }
        let r = self.routines.get(id).cloned()?;
        if self.routines.live.contains_key(id) {
            return Some(format!("{} is running already.", r.name));
        }
        self.routine_start(&r, util::now_secs(), false, true);
        Some(format!("Running {}.", r.name))
    }

    /// A run now of a starter routine, as its schedule would: the Slack view
    /// asking its routine to look. `None` when that starter was never made
    /// or is deleted; else whether a run started, none while routines are
    /// off, it is paused or a run of it is under way.
    pub(crate) fn routine_run_starter(&mut self, kind: super::starters::Kind) -> Option<bool> {
        self.routines_read();
        let id = super::starters::id_of(&routine::dir(), kind)?;
        let r = self.routines.get(&id).cloned()?;
        if !settings::is_on("routines") || r.paused || self.routines.live.contains_key(&r.id) {
            return Some(false);
        }
        // Not by hand: the routine's own rule for a busy tab holds.
        self.routine_start(&r, util::now_secs(), false, false);
        Some(true)
    }

    /// p: paused, or resumed counting from now, so it doesn't run at once
    /// for the times it would have run meanwhile.
    /// What it did, for mission control's footer.
    /// `to` pauses (`Some(true)`) or resumes it, as a menu chosen from
    /// says whatever it was meanwhile; `None` turns it over (p).
    pub fn routine_pause(&mut self, id: &str, to: Option<bool>) -> Option<String> {
        let mut r = self.routines.get(id).cloned()?;
        let paused = to.unwrap_or(!r.paused);
        if paused == r.paused {
            let words = if paused { "paused" } else { "running" };
            return Some(format!("Routine {} is {words} already.", r.name));
        }
        r.paused = paused;
        if !r.paused {
            r.handled = r.handled.max(util::now_secs());
        }
        self.routine_save(&r);
        // A run waiting on its busy tab goes with it; one under way ends
        // as it would.
        if r.paused && self.routines.live.get(id).is_some_and(|l| l.started == 0) {
            self.routines.live.remove(id);
        }
        let words = if r.paused { "paused" } else { "resumed" };
        Some(format!("Routine {} {words}.", r.name))
    }

    /// x: taken away after asking, its tab left as it is.
    pub fn routine_delete(&mut self, client: ClientId, id: &str) {
        let Some(r) = self.routines.get(id).cloned() else {
            return;
        };
        let question = Confirm::new(format!("Delete the routine {}?", r.name));
        let id = id.to_string();
        self.ask(client, question, move |server, client, yes| {
            if !yes {
                return;
            }
            routine::remove(&routine::dir(), &id);
            server.routines.list.retain(|r| r.id != id);
            server.routines.runs.remove(&id);
            server.routines.live.remove(&id);
            server.routines.said.remove(&id);
            server.routines_notices();
            server.routines.seen = routine::stamp(&routine::dir());
            if let Some(tab) = server.routine_tab(&id)
                && let Some(t) = server.tabs.get_mut(&tab)
            {
                t.opts.remove(TAB_MARK);
                t.opts.remove(TAB_NEXT);
            }
            server.mission_changed();
            server.say(Some(client), &format!("Routine {} deleted.", r.name));
        });
    }

    /// o: its tab, while it is open.
    pub fn routine_go(&mut self, client: ClientId, id: &str) {
        match self.routine_tab(id) {
            Some(tab) => self.mission_go(client, tab),
            None => {
                let name = self
                    .routines
                    .get(id)
                    .map(|r| r.name.clone())
                    .unwrap_or_default();
                self.say(
                    Some(client),
                    &format!("{name} has no tab open; r runs it in one."),
                );
            }
        }
    }

    /// The Routines view's items, from what the server holds.
    pub fn routines_items(&self) -> Vec<Item> {
        let now = util::local_now();
        let tabs: Vec<(String, Option<TabId>, String)> = self
            .routines
            .list
            .iter()
            .map(|r| {
                let tab = self.routine_tab(&r.id);
                let number = tab.map(|t| self.routine_tab_number(t)).unwrap_or_default();
                (r.id.clone(), tab, number)
            })
            .collect();
        let facts = super::view::Facts {
            now,
            tabs,
            live: &self.routines.live,
            runs: &self.routines.runs,
        };
        super::view::items(&self.routines.list, &facts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routines::schedule::{Days, Schedule};

    #[test]
    fn a_long_gap_counts_once_at_its_last_run() {
        // Every 15 minutes, due on the quarter hour.
        let tz = jiff::tz::TimeZone::UTC;
        let mut r = crate::routines::view::tests_routine();
        r.schedule = Schedule::Every {
            minutes: 15,
            days: Days::ALL,
            from: 0,
            to: 24 * 60 - 1,
        };
        let due = 1_700_000_100;
        let now = due + 14 * 86_400 + 30;
        assert_eq!(latest_due(&r, due, now, &tz), due + 14 * 86_400);
    }
}
