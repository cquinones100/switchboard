//! The routines on the server: read from their folder as the server starts
//! and again when it changes, each run started on the tick once it is due
//! as a headless Claude (`headless::run`) off the server's task, opening no
//! tab, its answer recording how long it took, what it cost and the reply's
//! first line, or the tools it needed that its list doesn't allow. A run due
//! while the last one still goes is skipped. A run due while the computer
//! slept runs on waking when it is no more than `LATE_BY` late and the
//! routine says so, else it is recorded as missed. Nothing here waits or
//! starts a process on the server's task; the files are a routine's few
//! hundred bytes and a line a run.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::routine::{self, Routine, Run};
use super::schedule::{Schedule, clock_words};
use super::{
    Command, LATE_BY, Live, Outcome, RoutinesEvent, SLEPT_AFTER, TAB_NEXT, WhenMissed, headless,
};
use crate::items::{Item, State as ItemState};
use crate::mux::TabId;
use crate::mux::server::{ClientId, Opened, Server};
use crate::ui::confirm::Confirm;
use crate::ui::{Outcome as UiOutcome, Picked};
use crate::{settings, util, worktree};

/// What a run refused in someone else's PR says.
const UNTRUSTED: &str = "its directory holds someone else's PR, so it isn't run there";

/// How long (seconds) a run told to end has before it is ended at once.
const STOP_GRACE: i64 = 30;

/// A run skipped while its last one goes says.
const STILL_RUNNING: &str = "its last run was still running, so skipped";

/// A run's record, not yet started.
fn live(due: i64, late: bool, by_hand: bool) -> Live {
    Live {
        due,
        late,
        by_hand,
        started: 0,
        checking: false,
        pid: 0,
        born: 0,
        dir: String::new(),
        stopping: 0,
        collecting: false,
    }
}

/// A run that didn't happen, as its line keeps it.
fn not_run(due: i64, outcome: Outcome, words: impl Into<String>) -> Run {
    Run {
        due,
        ran: 0,
        tab: String::new(),
        secs: 0,
        cost: 0.0,
        outcome,
        words: words.into(),
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

    /// The routines on the tick: read again when changed; each one due
    /// started, recorded as missed, or skipped while its last run goes.
    pub fn routines_tick(&mut self) {
        // A run under way is seen to its end, routines turned off or not.
        self.routines_watch();
        if !settings::is_on("routines") {
            return;
        }
        self.routines_read();
        self.routines_untab();
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
            // The todo note's starter waits for you, as the note it writes
            // gathers what is there when you are, and runs on your return.
            if !self.routines.live.contains_key(&r.id)
                && self.away()
                && super::starters::is_todo_note(&routine::dir(), r)
            {
                continue;
            }
            // The last time it was due, when it missed several.
            let latest = latest_due(r, due, now, &tz);
            let mut handled = r.clone();
            handled.handled = latest;
            self.routine_save(&handled);
            if self.routines.live.contains_key(&r.id) {
                self.routine_record(&handled, not_run(latest, Outcome::Busy, STILL_RUNNING));
                continue;
            }
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
                let words = format!("asleep past {}, so skipped", clock_at(latest));
                self.routine_record(&handled, not_run(latest, Outcome::Missed, words));
            }
        }
        // A run not started whose routine went is let go; one started is
        // watched to its end, its answer finding no routine.
        let list = &self.routines.list;
        self.routines
            .live
            .retain(|id, l| l.started > 0 || list.iter().any(|r| r.id == *id));
    }

    /// Takes off any tab what a build before runs were headless put there
    /// for the sidebar's "routine · next", which nothing keeps now.
    fn routines_untab(&mut self) {
        let marked: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|(_, t)| t.opts.contains_key(TAB_NEXT))
            .map(|(id, _)| *id)
            .collect();
        for tab in marked {
            self.set_tab_opt(tab, TAB_NEXT, None);
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
            self.routine_record(r, not_run(due, Outcome::Refused, UNTRUSTED));
            return;
        }
        if !dir.is_dir() {
            self.routine_record(
                r,
                not_run(due, Outcome::Refused, format!("{} is gone", r.dir)),
            );
            return;
        }
        if tokio::runtime::Handle::try_current().is_err() {
            let untrusted = crate::trust::marked(&dir);
            self.routine_checked(r, live(due, late, by_hand), untrusted);
            return;
        }
        let mut held = live(due, late, by_hand);
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

    /// What was found or answered off the task.
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
                self.routine_checked(&r, run, untrusted);
            }
            RoutinesEvent::Started { id, started, pid } => self.routine_started(&id, started, pid),
            RoutinesEvent::Answered {
                id,
                started,
                answer,
            } => {
                let dir = self
                    .routines
                    .live
                    .get(&id)
                    .map(|l| l.dir.clone())
                    .unwrap_or_default();
                self.routine_ran(&id, started, &dir, answer);
            }
        }
    }

    /// A run started as a headless Claude off the task, or refused when
    /// its directory turned out to hold someone else's PR.
    fn routine_checked(&mut self, r: &Routine, mut run: Live, untrusted: bool) {
        if untrusted {
            self.routine_record(r, not_run(run.due, Outcome::Refused, UNTRUSTED));
            return;
        }
        let started = util::now_secs();
        run.checking = false;
        run.started = started;
        run.dir = r.dir.clone();
        self.routines.live.insert(r.id.clone(), run);
        eprintln!("routines: {} runs in {}", r.id, r.dir);
        let (id, cwd, args, turn, name) = (
            r.id.clone(),
            r.dir.clone(),
            headless::args(r),
            r.turn(),
            r.name.clone(),
        );
        let start = move || headless::start(&routine::dir(), &id, &cwd, &args, &turn, &name);
        if tokio::runtime::Handle::try_current().is_err() {
            let pid = start();
            self.routine_started(&r.id, started, pid);
            return;
        }
        let tx = self.tx.clone();
        let id = r.id.clone();
        tokio::task::spawn_blocking(move || {
            let pid = start();
            let _ = tx.send(crate::mux::server::Event::Routines(
                RoutinesEvent::Started { id, started, pid },
            ));
        });
    }

    /// A run's Claude started: its process kept, in memory and in its
    /// `.live` file, for the tick to watch; or the run failed.
    fn routine_started(&mut self, id: &str, started: i64, pid: Result<u32, String>) {
        let gone = self.routines.get(id).is_none();
        let Some(run) = self
            .routines
            .live
            .get_mut(id)
            .filter(|l| l.started == started)
        else {
            return;
        };
        // Its routine deleted meanwhile: ended, and watched until it has.
        if gone && let Ok(pid) = &pid {
            headless::stop(*pid, 0, false);
        }
        match pid {
            Ok(pid) => {
                run.pid = pid;
                run.born = headless::born_of(pid).unwrap_or(0);
                let kept = headless::Started {
                    pid,
                    born: run.born,
                    started,
                    due: run.due,
                    late: run.late,
                    dir: run.dir.clone(),
                };
                let [_, _, live] = headless::files(&routine::dir(), id);
                if let Err(e) = util::write_atomic(&live, util::json_compact(&json!(kept))) {
                    eprintln!("routines: couldn't keep the run of {id}: {e}");
                }
            }
            Err(why) => {
                let run = self.routines.live.remove(id).expect("found above");
                if let Some(r) = self.routines.get(id).cloned() {
                    self.routine_record(&r, not_run(run.due, Outcome::Failed, why));
                }
            }
        }
    }

    /// The runs under way watched on the tick: one whose Claude ended
    /// recorded from its files, one going past `RUN_LIMIT` ended and
    /// recorded as failed. A server started again (or an upgrade's) takes
    /// up the runs the one before left going, from their `.live` files.
    fn routines_watch(&mut self) {
        let dir = routine::dir();
        if !self.routines.taken_up {
            self.routines.taken_up = true;
            for (id, kept) in headless::left_going(&dir) {
                eprintln!("routines: takes up the run of {id}, process {}", kept.pid);
                self.routines.live.insert(
                    id,
                    Live {
                        due: kept.due,
                        late: kept.late,
                        by_hand: false,
                        started: kept.started,
                        checking: false,
                        pid: kept.pid,
                        born: kept.born,
                        dir: kept.dir,
                        stopping: 0,
                        collecting: false,
                    },
                );
            }
        }
        let now = util::now_secs();
        let going: Vec<(String, Live)> = self
            .routines
            .live
            .iter()
            .filter(|(_, l)| l.pid > 0 && !l.collecting)
            .map(|(id, l)| (id.clone(), l.clone()))
            .collect();
        for (id, run) in going {
            if headless::ended(run.pid, run.born) {
                // Its files read off the task, of whatever size.
                let timed_out = run.stopping > 0;
                let collect = {
                    let (dir, id) = (dir.clone(), id.clone());
                    move || {
                        let answer = headless::answer(&dir, &id);
                        // One told to end ran out of time, whatever it printed.
                        if timed_out {
                            Err("no answer in 2 hours".to_string())
                        } else {
                            answer
                        }
                    }
                };
                if tokio::runtime::Handle::try_current().is_err() {
                    let answer = collect();
                    self.routine_ran(&id, run.started, &run.dir, answer);
                    continue;
                }
                if let Some(l) = self.routines.live.get_mut(&id) {
                    l.collecting = true;
                }
                let tx = self.tx.clone();
                let (id, started) = (id.clone(), run.started);
                tokio::task::spawn_blocking(move || {
                    let answer = collect();
                    let _ = tx.send(crate::mux::server::Event::Routines(
                        RoutinesEvent::Answered {
                            id,
                            started,
                            answer,
                        },
                    ));
                });
            } else if run.stopping > 0 {
                // Still there half a minute after being told: ended at once.
                if now - run.stopping > STOP_GRACE {
                    headless::stop(run.pid, run.born, true);
                }
            } else if now - run.started > headless::RUN_LIMIT {
                headless::stop(run.pid, run.born, false);
                if let Some(l) = self.routines.live.get_mut(&id) {
                    l.stopping = now;
                }
            }
        }
    }

    /// A run's answer recorded: its reply, or a failure naming the tools it
    /// needed, or why Claude gave none; its session kept for the next run.
    fn routine_ran(
        &mut self,
        id: &str,
        started: i64,
        dir: &str,
        answer: Result<headless::Reply, String>,
    ) {
        if !self
            .routines
            .live
            .get(id)
            .is_some_and(|l| l.started == started)
        {
            return;
        }
        let Some(run) = self.routines.live.remove(id) else {
            return;
        };
        // Recorded below on this same turn of the server's, so a restart
        // finds either its files or its record.
        headless::clear(&routine::dir(), id);
        // As its file has it now, which may have changed while it ran
        // (routines off, so not read again); deleted meanwhile, nothing is
        // recorded.
        let Some(mut r) = routine::load_all(&routine::dir())
            .into_iter()
            .find(|r| r.id == id)
        else {
            return;
        };
        let was = r.clone();
        let (outcome, words, cost) = match answer {
            Ok(reply) => {
                // Kept only from the directory it is in now, the only one
                // `--resume` finds it from.
                if !reply.session.is_empty() && r.dir == dir {
                    r.session = reply.session.clone();
                    r.session_dir = r.dir.clone();
                }
                r.needs = reply.denied.clone();
                if !reply.denied.is_empty() {
                    let words = format!("needs {}", reply.denied.join(", "));
                    (Outcome::Failed, words, reply.cost)
                } else if reply.error {
                    let said = headless::first_line(&reply.text);
                    let words = if said.is_empty() {
                        "Claude's turn ended in an error".to_string()
                    } else {
                        said
                    };
                    (Outcome::Failed, words, reply.cost)
                } else {
                    let ran = if run.late {
                        Outcome::Late
                    } else {
                        Outcome::Ran
                    };
                    (ran, headless::first_line(&reply.text), reply.cost)
                }
            }
            Err(why) => {
                // A session Claude Code no longer has is let go, so the
                // next run starts one.
                if why.contains("No conversation found") {
                    r.session.clear();
                }
                (Outcome::Failed, why, 0.0)
            }
        };
        if r != was {
            self.routine_save(&r);
        }
        let words = if run.late && outcome.ran() {
            format!("missed at {}, ran on waking · {words}", clock_at(run.due))
        } else {
            words
        };
        self.routine_record(
            &r,
            Run {
                due: run.due,
                ran: started,
                tab: String::new(),
                secs: (util::now_secs() - started).max(0),
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
            let needs = run.outcome == Outcome::Failed && !r.needs.is_empty();
            let detail = if needs {
                format!("couldn't run: it needs {}", r.needs.join(", "))
            } else {
                run.words.clone()
            };
            let client = self.latest_client();
            self.say(client, &format!("Routine {}: {detail}.", r.name));
            let mut item = Item::new(format!("routine:{}", r.id), format!("Routine {}", r.name));
            item.detail = detail;
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
    /// is paused, its session, the tools it needs), so its menu is that
    /// item's.
    fn routines_notices(&mut self) {
        let mut items: Vec<Item> = self.routines.said.values().cloned().collect();
        for item in items.iter_mut() {
            let id = item.data["routine"].as_str().unwrap_or("").to_string();
            let r = self.routines.get(&id);
            item.data["paused"] = json!(r.is_some_and(|r| r.paused));
            item.data["session"] = json!(r.is_some_and(|r| !r.resumable().is_empty()));
            item.data["needs"] = json!(r.map(|r| r.needs.clone()).unwrap_or_default());
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
            when_missed: draft.when_missed,
            prompt: draft.prompt.trim_end().to_string(),
            model: draft.model.trim().to_string(),
            tools: draft
                .tools
                .iter()
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect(),
            // What its last run needed stays until a run needs nothing.
            needs: was.as_ref().map(|w| w.needs.clone()).unwrap_or_default(),
            paused: was.as_ref().is_some_and(|w| w.paused),
            // Kept with where it was made, `resumable` resuming it only there.
            session: was.as_ref().map(|w| w.session.clone()).unwrap_or_default(),
            session_dir: was
                .as_ref()
                .map(|w| w.session_dir.clone())
                .unwrap_or_default(),
        };
        self.routine_save(&r);
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

    /// r: a run now, whatever the schedule says, as the schedule's would.
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
        let words = if r.paused { "paused" } else { "resumed" };
        Some(format!("Routine {} {words}.", r.name))
    }

    /// x: taken away after asking, its sessions kept in Claude Code.
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
            // A run under way is ended, and watched until it has, so its
            // process is reaped and its files go; one not started goes.
            match server.routines.live.get_mut(&id) {
                Some(l) if l.started > 0 => {
                    if l.pid > 0 {
                        headless::stop(l.pid, l.born, false);
                        l.stopping = util::now_secs();
                    }
                }
                Some(_) => {
                    server.routines.live.remove(&id);
                }
                None => {}
            }
            server.routines.said.remove(&id);
            server.routines_notices();
            server.routines.seen = routine::stamp(&routine::dir());
            server.mission_changed();
            server.say(Some(client), &format!("Routine {} deleted.", r.name));
        });
    }

    /// o: its last session opened in a new tab in its directory, `claude
    /// --resume`, to read the run or carry on from it.
    pub fn routine_open_session(&mut self, client: ClientId, id: &str) {
        let Some(r) = self.routines.get(id).cloned() else {
            return;
        };
        let session = r.resumable().to_string();
        if session.is_empty() {
            self.say(
                Some(client),
                &format!("{} has no session yet: its first run makes one.", r.name),
            );
            return;
        }
        let Some(name) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        let command = worktree::tab_command(
            "claude \"$@\"",
            "claude",
            &["--resume".to_string(), session],
        );
        match self.new_tab(
            &name,
            Some(PathBuf::from(&r.dir)),
            Some(command),
            Some(r.name.clone()),
            Opened::Front,
        ) {
            Ok((tab, _)) => self.mission_go(client, tab),
            Err(e) => self.say(Some(client), &format!("Couldn't open its session: {e}")),
        }
    }

    /// Allow what its last run needed: the tools added to its own, and a
    /// run now. What it did, for mission control's footer.
    pub fn routine_allow_needed(&mut self, client: ClientId, id: &str) -> Option<String> {
        let mut r = self.routines.get(id).cloned()?;
        if r.needs.is_empty() {
            return self.routine_run_now(client, id);
        }
        let needs = std::mem::take(&mut r.needs);
        for tool in &needs {
            if !r.tools.contains(tool) {
                r.tools.push(tool.clone());
            }
        }
        self.routine_save(&r);
        self.routines.said.remove(id);
        self.routines_notices();
        let ran = self.routine_run_now(client, id);
        Some(format!(
            "{} may use {} now. {}",
            r.name,
            needs.join(", "),
            ran.unwrap_or_default()
        ))
    }

    /// The Routines view's items, from what the server holds.
    pub fn routines_items(&self) -> Vec<Item> {
        let now = util::local_now();
        let facts = super::view::Facts {
            now,
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
