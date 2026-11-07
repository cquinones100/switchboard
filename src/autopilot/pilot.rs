//! What the mode keeps between checks and what each check decides
//! (autopilot.js's `autopilot()` and its `check`), as plain data: given the
//! mode, its tabs and the time, a check says which tabs to mark, whether to
//! read the next item again for mission control, whether to look for the
//! next item to open, and anything new to say. The looks run off the
//! server's task, since the queue reads git (`Job`), and come back to
//! `opened` and `next_read`.
//!
//! Openings are `GAP_MS` apart and one at a time, so `wtn`'s worktrees and
//! the new Claude sessions don't race. An item is never taken again while
//! a tab of the mode's is open for it (`held`, from `@autopilot_item`), nor
//! for `RECENT_MS` after it was opened (`recent`), before `wtn` has made its
//! branch, which is what claims it, nor, once its tab has landed and left,
//! until the note shows it ticked or for `LANDED_MS` (`landed`), since
//! queue-and-clean may remove the worktree a moment before it ticks it; a tab whose `wtn` failed before making
//! its branch holds its item until it is closed. A note changed in the last
//! `SETTLE_MS` isn't taken from, since an item being typed is saved as it
//! is typed and its first words would be opened as an item. A note found
//! empty is looked at again once it changes, or `RECENT_MS` later, since an
//! item a worktree had comes back once that worktree is removed.
//!
//! A tab back in the main clone once queue-and-clean has landed it leaves
//! the mode, and, with the autopilot_close setting on, is closed for you
//! (autopilot.js's `afterLanding`): marked landed (`@autopilot_left`, the
//! time) once its Claude's pane is idle in a main clone, marked seen
//! (`@autopilot_seen`) once it has been in front for `SEEN_MS`, and closed
//! as soon as you leave it, or `CLOSE_AFTER_S` after it landed if you never
//! look. Never while it is in front or while a tool stands in for one of
//! its panes; a prompt typed in it after it landed makes it yours, out of
//! the mode, so it stays open. Off, it leaves the mode as it lands.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::line::Summary;
use super::mode::{Mode, note_title};
use super::place::{Kind, Mine, kind_of};
use crate::cmd::worktree_from_note::{Item, queue};
use crate::mux::TabId;
use crate::util;

/// Openings are this far apart.
pub const GAP_MS: i64 = 5_000;
/// An item opened this recently is never taken again.
pub const RECENT_MS: i64 = 60_000;
/// A note changed this recently isn't taken from.
pub const SETTLE_MS: i64 = 60_000;
/// A landed tab's item is held until the note shows it ticked, or this
/// long, so a tick that never comes doesn't hold it for good.
pub const LANDED_MS: i64 = 10 * 60_000;
/// The next item is read again at most this often, and when the note, the
/// items skipped or the kind of items taken change.
pub const QUEUE_MS: i64 = 30_000;
/// A landed tab you never look at is closed this long after it landed.
pub const CLOSE_AFTER_S: f64 = 30.0 * 60.0;
/// A landed tab in front this long counts as seen, so passing over it, as
/// moving the sidebar's cursor does, doesn't.
pub const SEEN_MS: i64 = 1_000;

/// The item it will take next, as last read, for mission control.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Next {
    pub text: String,
    /// When it was read (ms); 0 to read it again at the next check.
    pub at: i64,
    /// What it was read for: the note, its time, the items skipped and the
    /// kind taken.
    pub key: String,
}

/// A mark a check sets on one of its tabs, or what it does to one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// Seen in its worktree: `@autopilot_in`.
    In(TabId),
    /// Its marks taken off, so it leaves the mode: landed, with the
    /// autopilot_close setting off, or made yours by a prompt.
    Left(TabId),
    /// Found back in a main clone once landed, at this time (seconds):
    /// `@autopilot_left`.
    Landed(TabId, i64),
    /// Seen since it landed: `@autopilot_seen`.
    Seen(TabId),
    /// Closed, once landed and seen and left, or long after it landed.
    Close(TabId),
}

/// A look at the note's queue, run off the server's task.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub note: String,
    pub repo: PathBuf,
    pub ready_only: bool,
    pub skip: HashSet<String>,
    /// Today's Claude spend past which nothing opens; 0 for none.
    pub spend_limit: f64,
    /// For a read for mission control, the key it is read under.
    pub key: String,
    /// When the note last changed (ms).
    pub mtime: i64,
}

impl Job {
    /// The first item the queue gives, if any; `state` is the state
    /// directory, whose claims file says which items wtn started lately.
    pub fn first(&self, notes: &Path, state: &Path) -> Result<Option<Item>, String> {
        queue(
            notes,
            &self.repo,
            &self.note,
            &self.skip,
            self.ready_only,
            &crate::cmd::worktree_from_note::claims_in(state),
        )
        .map(|items| items.into_iter().next())
    }

    /// The look for the next item to open: nothing once today's spend has
    /// passed the limit, as the Usage view sums it; else the queue's first.
    pub fn find(&self, notes: &Path, state: &Path, now_ms: i64) -> Found {
        if self.spend_limit > 0.0
            && crate::usage::sessions::spent_today(state, now_ms) >= self.spend_limit
        {
            return Found::Spent;
        }
        match self.first(notes, state) {
            Ok(item) => Found::Item(item),
            Err(e) => Found::Failed(e),
        }
    }
}

/// What a look for the next item to open found.
#[derive(Clone, Debug, PartialEq)]
pub enum Found {
    /// Today's spend has passed the limit.
    Spent,
    /// The item to open, or none left.
    Item(Option<Item>),
    /// The note couldn't be read.
    Failed(String),
}

/// What a check decided.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Check {
    pub marks: Vec<Mark>,
    /// The next item to read again, for mission control.
    pub read_next: Option<Job>,
    /// The next item to look for, to open.
    pub open: Option<Job>,
    /// Something new to say in the notifications.
    pub said: Option<String>,
}

/// What a look's answer leaves to do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Opened {
    /// An item to open a tab for.
    pub take: Option<Item>,
    /// Something new to say in the notifications.
    pub said: Option<String>,
    /// Why the look failed, for the server's log.
    pub failed: Option<String>,
}

/// What the mode keeps between checks.
#[derive(Clone, Debug)]
pub struct Pilot {
    /// A look for the next item to open is under way.
    opening: bool,
    /// When the last look started (ms).
    last_open: Option<i64>,
    /// Each item opened lately, by its first line, and when (ms).
    recent: HashMap<String, i64>,
    /// When the note was last found empty: its time then, and when (ms).
    empty: Option<(i64, i64)>,
    pub next: Next,
    /// A read for mission control is under way.
    reading: bool,
    /// The day and limit today's spend was found past, so it isn't summed
    /// again until either changes.
    spent: Option<(String, f64)>,
    /// The note said to be missing, said once.
    missing_said: String,
    /// The items of its tabs still open, never taken again while there.
    held: Vec<String>,
    /// The items of its tabs that landed and left, and when each was first
    /// found so (ms).
    landed: HashMap<String, i64>,
    /// The note's unticked items as last read, and the time it was read for.
    unticked: (Option<i64>, HashSet<String>),
    /// How many of its tabs work and how many are open, at the last check.
    pub counts: (usize, usize),
    /// When each landed tab was first found in front (ms), for `SEEN_MS`.
    fronts: HashMap<TabId, i64>,
    /// Openings this far apart (ms); the tests give a shorter one.
    pub gap_ms: i64,
}

impl Default for Pilot {
    fn default() -> Pilot {
        Pilot {
            opening: false,
            last_open: None,
            recent: HashMap::new(),
            empty: None,
            next: Next::default(),
            reading: false,
            spent: None,
            missing_said: String::new(),
            held: Vec::new(),
            landed: HashMap::new(),
            unticked: (None, HashSet::new()),
            counts: (0, 0),
            fronts: HashMap::new(),
            gap_ms: GAP_MS,
        }
    }
}

/// `YYYY-MM-DD` of a time, in the local zone.
fn day_of(now_ms: i64) -> String {
    util::ymd(&util::local_at_ms(now_ms))
}

impl Pilot {
    /// The items never to take now: those skipped, those of its tabs still
    /// open, and those opened in the last `RECENT_MS`.
    pub fn skip(&self, mode: &Mode, now_ms: i64) -> HashSet<String> {
        mode.skipped
            .iter()
            .chain(&self.held)
            .chain(
                self.recent
                    .iter()
                    .filter(|(_, at)| now_ms - **at < RECENT_MS)
                    .map(|(text, _)| text),
            )
            .cloned()
            .collect()
    }

    /// What becomes of a tab back in the main clone once landed, as the
    /// module's comment says, at `now_ms`.
    fn after_landing(&mut self, m: &Mine, now_ms: i64) -> Option<Mark> {
        let now = now_ms as f64 / 1000.0;
        let Some(landed) = m.landed else {
            return m.home.then_some(Mark::Landed(m.tab, now as i64));
        };
        if m.prompted_since {
            return Some(Mark::Left(m.tab));
        }
        if m.front {
            let first = *self.fronts.entry(m.tab).or_insert(now_ms);
            return (!m.seen && now_ms - first >= SEEN_MS).then_some(Mark::Seen(m.tab));
        }
        self.fronts.remove(&m.tab);
        if m.stand_in || !(m.seen || now - landed >= CLOSE_AFTER_S) {
            return None;
        }
        Some(Mark::Close(m.tab))
    }

    /// One check, at `now_ms`: marks the mode's tabs as they reach their
    /// worktree or leave the mode (closing a landed one with `close`, the
    /// autopilot_close setting, on), then, while fewer than `slots` work
    /// and fewer than `cap` are open, looks for the next item to open. `on`
    /// is the autopilot setting, `away` whether you're away, `mtime` when
    /// the note last changed (ms), `None` when it isn't there, and `notes`
    /// the notes directory, whose note is read for its ticked items only
    /// while a landed tab's item is held.
    #[allow(clippy::too_many_arguments)]
    pub fn check(
        &mut self,
        mode: &Mode,
        on: bool,
        close: bool,
        mine: &[Mine],
        away: bool,
        now_ms: i64,
        mtime: Option<i64>,
        notes: &Path,
    ) -> Check {
        let mut out = Check::default();
        if !mode.set_up() {
            self.counts = (0, 0);
            return out;
        }
        let now = now_ms as f64 / 1000.0;
        let placed: Vec<(&Mine, Kind)> = mine.iter().map(|m| (m, kind_of(m, now))).collect();
        let mut marks = Vec::new();
        for (m, kind) in &placed {
            match kind {
                Kind::Left if close => marks.extend(self.after_landing(m, now_ms)),
                Kind::Left => marks.push(Mark::Left(m.tab)),
                _ if m.in_worktree && !m.was_in => marks.push(Mark::In(m.tab)),
                _ => {}
            }
        }
        out.marks = marks;
        self.fronts.retain(|t, _| mine.iter().any(|m| m.tab == *t));
        // A landed tab stays marked until it closes, so its item's time is
        // when it was first found left, forgotten once ticked, or once past
        // LANDED_MS with its tab gone.
        let left: HashSet<String> = placed
            .iter()
            .filter(|(_, kind)| *kind == Kind::Left)
            .filter_map(|(m, _)| m.item.clone())
            .collect();
        for item in &left {
            self.landed.entry(item.clone()).or_insert(now_ms);
        }
        if !self.landed.is_empty() {
            if self.unticked.0 != mtime || mtime.is_none() {
                let text = std::fs::read_to_string(notes.join(&mode.note)).unwrap_or_default();
                let open = text
                    .lines()
                    .filter_map(|l| l.strip_prefix("- [ ] "))
                    .map(|l| l.trim().to_string())
                    .collect();
                self.unticked = (mtime, open);
            }
            let open = &self.unticked.1;
            self.landed.retain(|item, at| {
                open.contains(item) && (now_ms - *at < LANDED_MS || left.contains(item))
            });
        }
        self.held = placed
            .iter()
            .filter(|(_, kind)| *kind != Kind::Left)
            .filter_map(|(m, _)| m.item.clone())
            .chain(
                self.landed
                    .iter()
                    .filter(|(_, at)| now_ms - **at < LANDED_MS)
                    .map(|(item, _)| item.clone()),
            )
            .collect();
        let open = placed.iter().filter(|(_, k)| *k != Kind::Left).count();
        let busy = placed.iter().filter(|(_, k)| *k == Kind::Busy).count();
        self.counts = (busy, open);
        self.recent.retain(|_, at| now_ms - *at < RECENT_MS);
        if !on || !mode.on || mode.note.is_empty() {
            return out;
        }
        let skip = self.skip(mode, now_ms);
        let job = |key: String, mtime: i64| Job {
            note: mode.note.clone(),
            repo: mode.repo.clone(),
            ready_only: mode.ready_only,
            skip: skip.clone(),
            spend_limit: mode.spend_limit,
            key,
            mtime,
        };
        // The item it will take next, read again for mission control.
        let mut skipped: Vec<&String> = skip.iter().collect();
        skipped.sort();
        let key = format!(
            "{}\t{}\t{}\t{}",
            mode.note,
            mtime.unwrap_or(0),
            skipped
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            mode.ready_only
        );
        if mtime.is_some()
            && !self.reading
            && (key != self.next.key || now_ms - self.next.at >= QUEUE_MS)
        {
            self.reading = true;
            out.read_next = Some(job(key, mtime.unwrap_or(0)));
        }
        if mode.paused
            || away
            || self.opening
            || self.last_open.is_some_and(|at| now_ms - at < self.gap_ms)
        {
            return out;
        }
        if busy >= mode.slots as usize || open >= mode.cap as usize {
            return out;
        }
        let today = day_of(now_ms);
        if mode.spend_limit > 0.0
            && self
                .spent
                .as_ref()
                .is_some_and(|(day, limit)| *day == today && *limit == mode.spend_limit)
        {
            return out;
        }
        let Some(mtime) = mtime else {
            if self.missing_said != mode.note {
                self.missing_said = mode.note.clone();
                out.said = Some(format!(
                    "Can't find the note {}: set the mode up again from M-p",
                    note_title(&mode.note)
                ));
            }
            return out;
        };
        if now_ms - mtime < SETTLE_MS {
            return out;
        }
        if self
            .empty
            .is_some_and(|(at, since)| at == mtime && now_ms - since < RECENT_MS)
        {
            return out;
        }
        self.opening = true;
        self.last_open = Some(now_ms);
        out.open = Some(job(String::new(), mtime));
        out
    }

    /// A look for the next item to open has answered, for a note last
    /// changed at `mtime`: what to open, or say.
    pub fn opened(&mut self, mode: &Mode, found: Found, mtime: i64, now_ms: i64) -> Opened {
        self.opening = false;
        self.next.at = 0;
        let mut out = Opened::default();
        match found {
            Found::Spent => {
                self.spent = Some((day_of(now_ms), mode.spend_limit));
                out.said = Some(format!(
                    "Stopped opening tabs: today's Claude spend passed ${}",
                    mode.spend_limit
                ));
            }
            Found::Failed(e) => out.failed = Some(e),
            Found::Item(None) => {
                // Said once, when it first finds the note empty.
                if self.empty.is_none() {
                    out.said = Some(format!(
                        "Nothing left to take in {}",
                        note_title(&mode.note)
                    ));
                }
                self.empty = Some((mtime, now_ms));
            }
            Found::Item(Some(item)) => {
                self.empty = None;
                out.take = Some(item);
            }
        }
        out
    }

    /// A tab was opened for an item: it isn't taken again for a while.
    pub fn took(&mut self, text: &str, now_ms: i64) {
        self.recent.insert(text.to_string(), now_ms);
    }

    /// The next item, read again for mission control.
    pub fn next_read(&mut self, key: String, text: String, now_ms: i64) {
        self.reading = false;
        self.next = Next {
            text,
            at: now_ms,
            key,
        };
    }

    /// A look that couldn't start, let go.
    pub fn cancel(&mut self) {
        self.opening = false;
        self.reading = false;
    }

    /// Something changed what it would take (a menu's choice, the mode set
    /// up again): the next item read again, an empty note looked at again,
    /// a missing one said again.
    pub fn reset(&mut self) {
        self.next.at = 0;
        self.empty = None;
        self.missing_said.clear();
    }

    /// What mission control's line shows while the mode is on.
    pub fn summary(&self, mode: &Mode, away: bool) -> Summary {
        Summary {
            note: note_title(&mode.note),
            busy: self.counts.0,
            open: self.counts.1,
            slots: mode.slots,
            cap: mode.cap,
            next: self.next.text.clone(),
            paused: mode.paused,
            away,
            ready_only: mode.ready_only,
            skipped: mode.skipped.len(),
            spend_limit: mode.spend_limit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::autopilot::place::tests::mine;
    use crate::sidebar::model::TabState;

    /// A stand-in for the server: its tabs, the note's items and what was
    /// said, each look answered as wtn-open.zsh's stand-in did in
    /// test/tmux-sidebar.test.js, a tab opened for each item taken, working
    /// in its worktree, opened five minutes ago.
    struct World {
        pilot: Pilot,
        mode: Mode,
        tabs: Vec<Mine>,
        taken: usize,
        max: usize,
        now: i64,
        mtime: i64,
        said: String,
        skip: HashSet<String>,
    }

    impl World {
        fn new() -> World {
            let now = 1_700_000_000_000;
            World {
                pilot: Pilot {
                    gap_ms: 0,
                    ..Pilot::default()
                },
                mode: Mode {
                    on: true,
                    note: "Work.md".into(),
                    repo: "/repo".into(),
                    ..Mode::default()
                },
                tabs: Vec::new(),
                taken: 0,
                max: 99,
                now,
                mtime: now - 120_000,
                said: String::new(),
                skip: HashSet::new(),
            }
        }

        fn check(&mut self) {
            self.now += 1_000;
            let check = self.pilot.check(
                &self.mode,
                true,
                false,
                &self.tabs,
                false,
                self.now,
                Some(self.mtime),
                Path::new("/nowhere"),
            );
            for mark in check.marks {
                match mark {
                    Mark::In(t) => self.tab(t).was_in = true,
                    Mark::Left(t) | Mark::Close(t) => self.tabs.retain(|m| m.tab != t),
                    Mark::Landed(t, at) => self.tab(t).landed = Some(at as f64),
                    Mark::Seen(t) => self.tab(t).seen = true,
                }
            }
            if let Some(said) = check.said {
                self.said = said;
            }
            if let Some(job) = check.read_next {
                self.pilot.next_read(job.key, "next thing".into(), self.now);
            }
            let Some(job) = check.open else {
                return;
            };
            self.skip = job.skip.clone();
            let found = if self.taken < self.max {
                self.taken += 1;
                Found::Item(Some(Item {
                    line: self.taken,
                    text: format!("item {}", self.taken),
                    needs_choice: false,
                }))
            } else {
                Found::Item(None)
            };
            let opened = self.pilot.opened(&self.mode, found, job.mtime, self.now);
            if let Some(said) = opened.said {
                self.said = said;
            }
            if let Some(item) = opened.take {
                let n = 100 + item.line as u32;
                let mut tab = mine(n, (self.now / 1000) as f64 - 300.0, TabState::Working, true);
                tab.was_in = false;
                tab.item = Some(item.text.clone());
                self.tabs.push(tab);
                self.pilot.took(&item.text, self.now);
                if self.said.starts_with("Nothing left") {
                    self.said.clear();
                }
            }
        }

        /// Checks until its tabs hold still.
        fn settle(&mut self) -> usize {
            for _ in 0..20 {
                self.check();
            }
            self.tabs.len()
        }

        fn tab(&mut self, t: TabId) -> &mut Mine {
            self.tabs.iter_mut().find(|m| m.tab == t).unwrap()
        }
    }

    // test/tmux-sidebar.test.js's "working a note continuously keeps 3 tabs
    // working and at most 5 open, a tab waiting on you freeing its slot, one
    // landed leaving the mode, and stops on pause or an empty note".
    #[test]
    fn three_tabs_working_and_at_most_five_open_a_waiting_tab_freeing_its_slot_one_landed_leaving()
    {
        let mut w = World::new();
        w.mtime = w.now;
        assert_eq!(
            w.settle(),
            0,
            "nothing taken from a note changed a moment ago, whose item may still be being typed"
        );
        w.mtime = w.now - 120_000;
        assert_eq!(w.settle(), 3, "three tabs open, one for each slot");
        let first = w.tabs[0].tab;
        w.tab(first).state = TabState::Waiting;
        assert_eq!(w.settle(), 4, "a tab waiting on you frees its slot");
        let second = w.tabs[1].tab;
        w.tab(second).state = TabState::Done;
        assert_eq!(w.settle(), 5, "a fifth once another waits");
        let third = w.tabs[2].tab;
        w.tab(third).state = TabState::Waiting;
        assert_eq!(
            w.settle(),
            5,
            "never a sixth: the cap counts the tabs waiting on you"
        );
        assert!(
            w.tabs.iter().all(|m| m.was_in),
            "each marked once seen in its worktree"
        );
        assert!(
            ["item 1", "item 2", "item 3", "item 4"]
                .iter()
                .all(|i| w.skip.contains(*i)),
            "the items of the tabs still open are never taken again"
        );

        // queue-and-clean's cleanup takes the worktree away under the pane:
        // landing, it keeps its place and its item.
        w.tab(first).in_worktree = false;
        w.tab(first).in_main = false;
        assert_eq!(w.settle(), 5, "a tab landing holds its place");
        assert!(w.skip.contains("item 1"), "and its item");
        // Then it moves the pane back to the main clone.
        w.tab(first).in_main = true;
        w.settle();
        assert!(
            !w.tabs.iter().any(|m| m.tab == first),
            "the landed tab leaves the mode"
        );
        assert_eq!(w.tabs.len(), 5, "and the next item opens in its place");
        assert_eq!(w.said, "");

        w.mode.paused = true;
        let fourth = w.tabs[3].tab;
        w.tab(fourth).state = TabState::Waiting;
        let gone = w.tabs[4].tab;
        w.tabs.retain(|m| m.tab != gone);
        assert_eq!(w.settle(), 4, "paused, it opens nothing");

        w.mode.paused = false;
        w.max = 0;
        assert_eq!(w.settle(), 4, "nothing left in the note: nothing opens");
        assert_eq!(w.said, "Nothing left to take in Work", "and it says so");
        let summary = w.pilot.summary(&w.mode, false);
        assert_eq!(
            (
                summary.note.as_str(),
                summary.open,
                summary.cap,
                summary.slots
            ),
            ("Work", 4, 5, 3)
        );
        assert_eq!(summary.next, "next thing");
    }

    #[test]
    fn it_opens_nothing_while_away_or_past_the_spend_limit_and_says_a_missing_note_once() {
        let mut w = World::new();
        let none = Path::new("/nowhere");
        let check = w
            .pilot
            .check(&w.mode, true, false, &[], true, w.now, Some(w.mtime), none);
        assert_eq!(check.open, None, "away");
        let check = w.pilot.check(
            &w.mode,
            false,
            false,
            &[],
            false,
            w.now,
            Some(w.mtime),
            none,
        );
        assert_eq!(check.open, None, "the setting off");

        w.mode.spend_limit = 5.0;
        let check = w
            .pilot
            .check(&w.mode, true, false, &[], false, w.now, Some(w.mtime), none);
        let job = check.open.expect("a look, which sums the spend");
        assert_eq!(job.spend_limit, 5.0);
        let opened = w.pilot.opened(&w.mode, Found::Spent, w.mtime, w.now);
        assert_eq!(
            opened.said.as_deref(),
            Some("Stopped opening tabs: today's Claude spend passed $5")
        );
        let later = w.now + 60_000;
        let check = w
            .pilot
            .check(&w.mode, true, false, &[], false, later, Some(w.mtime), none);
        assert_eq!(check.open, None, "not summed again today");
        w.mode.spend_limit = 50.0;
        let check = w
            .pilot
            .check(&w.mode, true, false, &[], false, later, Some(w.mtime), none);
        assert!(check.open.is_some(), "a new limit is looked at again");
        w.pilot.cancel();

        let missing = |w: &mut World, at: i64| {
            w.pilot
                .check(&w.mode, true, false, &[], false, at, None, none)
                .said
        };
        assert_eq!(
            missing(&mut w, later + 10_000).as_deref(),
            Some("Can't find the note Work: set the mode up again from M-p")
        );
        assert_eq!(missing(&mut w, later + 20_000), None, "said once");
    }

    // test/tmux-sidebar.test.js's "a tab that left keeps its item held until
    // the note shows it ticked, or for ten minutes".
    #[test]
    fn a_landed_tabs_item_is_held_until_the_note_ticks_it_or_ten_minutes_pass() {
        let dir = std::env::temp_dir().join(format!("sb-autopilot-held-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Work.md"), "- [ ] item 1\n- [ ] item 2\n").unwrap();
        let mut pilot = Pilot::default();
        let mode = Mode {
            on: true,
            note: "Work.md".into(),
            repo: "/repo".into(),
            ..Mode::default()
        };
        let now = 1_700_000_000_000_i64;
        let mut tab = mine(1, (now / 1000) as f64 - 3_600.0, TabState::Idle, false);
        tab.was_in = true;
        let mut check = |tabs: &[Mine], at: i64, mtime: i64| {
            pilot.check(&mode, true, false, tabs, false, at, Some(mtime), &dir);
            pilot.skip(&mode, at)
        };
        assert!(
            check(std::slice::from_ref(&tab), now, 1).contains("item 1"),
            "held as it leaves"
        );
        assert!(check(&[], now + 60_000, 1).contains("item 1"), "and after");
        assert!(
            !check(&[], now + LANDED_MS, 1).contains("item 1"),
            "for ten minutes at most"
        );
        assert!(check(std::slice::from_ref(&tab), now + LANDED_MS, 1).contains("item 1"));
        std::fs::write(dir.join("Work.md"), "- [ ] item 2\n- [x] item 1\n").unwrap();
        assert!(
            !check(&[], now + LANDED_MS + 1_000, 2).contains("item 1"),
            "or until the note shows it ticked"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // test/tmux-sidebar.test.js's "a landed tab of the mode working a note
    // closes once seen and left, or half an hour after landing".
    #[test]
    fn a_landed_tab_is_marked_then_closed_once_seen_and_left_or_long_after_it_landed() {
        let mut pilot = Pilot::default();
        let mode = Mode {
            on: true,
            note: "Work.md".into(),
            repo: "/repo".into(),
            ..Mode::default()
        };
        let now = 1_700_000_000_000_i64;
        let secs = (now / 1000) as f64;
        let mut tab = mine(1, secs - 3_600.0, TabState::Idle, false);
        tab.was_in = true;
        let marks = |pilot: &mut Pilot, tab: &Mine, at: i64, close: bool| {
            pilot
                .check(
                    &mode,
                    true,
                    close,
                    std::slice::from_ref(tab),
                    false,
                    at,
                    None,
                    Path::new("/nowhere"),
                )
                .marks
        };
        assert_eq!(
            marks(&mut pilot, &tab, now, false),
            [Mark::Left(TabId(1))],
            "with the setting off, it leaves the mode as it lands"
        );
        assert!(
            marks(&mut pilot, &tab, now, true).is_empty(),
            "not marked landed while its Claude isn't idle in a main clone"
        );
        tab.home = true;
        assert_eq!(
            marks(&mut pilot, &tab, now, true),
            [Mark::Landed(TabId(1), secs as i64)]
        );
        tab.landed = Some(secs);
        assert!(
            marks(&mut pilot, &tab, now + 1_000, true).is_empty(),
            "not yet"
        );

        // In front: seen after a second, never closed while there.
        tab.front = true;
        assert!(marks(&mut pilot, &tab, now + 2_000, true).is_empty());
        assert_eq!(
            marks(&mut pilot, &tab, now + 3_500, true),
            [Mark::Seen(TabId(1))]
        );
        tab.seen = true;
        assert!(marks(&mut pilot, &tab, now + 4_000, true).is_empty());
        // Left: closed, unless a tool stands in for its pane.
        tab.front = false;
        tab.stand_in = true;
        assert!(marks(&mut pilot, &tab, now + 5_000, true).is_empty());
        tab.stand_in = false;
        assert_eq!(
            marks(&mut pilot, &tab, now + 5_000, true),
            [Mark::Close(TabId(1))]
        );

        // Never looked at: closed half an hour after it landed.
        let mut unseen = tab.clone();
        unseen.seen = false;
        assert!(marks(&mut pilot, &unseen, now + 60_000, true).is_empty());
        assert_eq!(
            marks(&mut pilot, &unseen, now + 1_800_000, true),
            [Mark::Close(TabId(1))]
        );
        // A prompt typed in it since makes it yours.
        unseen.prompted_since = true;
        assert_eq!(
            marks(&mut pilot, &unseen, now + 1_800_000, true),
            [Mark::Left(TabId(1))]
        );
    }
}
