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
//! Every item it keeps is by its note's ID and first line (`Key`), the
//! note's stable ID (src/notes/id.rs): those skipped, those of its tabs
//! (`@autopilot_note`) and those opened lately, so an item skipped or held
//! in one note never hides one worded the same in another. A tab opened
//! before tabs kept their note's ID takes the ID of the note it was opened
//! for (`@tab_note`), the mode's when it names none.
//!
//! The mode follows its note by its ID (`Keep`, run off the task): one from
//! before notes had IDs is given one; one no longer at its path is looked
//! for by its ID, at most every `LOST_MS`, and its new path written back; a
//! note at its path rewritten without its ID is given it back; and with the
//! ID nowhere, or another note's at its path, it takes nothing and says it
//! can't find the note. Nothing is opened while it looks. wtn's claims from
//! before notes had IDs are brought to IDs once a run in the mode's
//! repository.
//!
//! With "Land without asking" on (`landWithoutAsking`, with the autopilot
//! setting on), each of its tabs not landed yet is marked `@autopilot_land`,
//! which queue-and-clean reads to land on the default branch without asking
//! to push, and unmarked once it goes off; queue-and-clean marks one that
//! landed so `@autopilot_autoland`, with the commit (`switchboard land-lock
//! autolanded`).
//!
//! A tab back in the main clone once queue-and-clean has landed it leaves
//! the mode, and, unless the autopilot_close setting says never, is closed for you
//! (autopilot.js's `afterLanding`): marked landed (`@autopilot_left`, the
//! time) once its Claude's pane is idle in a main clone, marked seen
//! (`@autopilot_seen`) once it has been in front for `SEEN_MS`, and closed
//! as soon as you leave it, or the setting's delay after it landed if you
//! never look (`close_after`: when seen, half an hour; at once; after five
//! minutes). Never while it is in front or while a tool stands in for one
//! of its panes; a prompt typed in it after it landed makes it yours, out
//! of the mode, so it stays open. Never, it leaves the mode as it lands.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::line::Summary;
use super::mode::{Mode, note_title};
use super::place::{Kind, Mine, kind_of};
use crate::cmd::worktree_from_note::{Item, PlanNote, plan_notes, queue};
use crate::mux::TabId;
use crate::notes::id;
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
/// A landed tab you never look at is closed this long after it landed, by
/// the autopilot_close setting's choice; `off` never closes one.
pub fn close_after(choice: &str) -> Option<f64> {
    match choice {
        "seen" => Some(30.0 * 60.0),
        "5m" => Some(5.0 * 60.0),
        "now" => Some(0.0),
        _ => None,
    }
}
/// A landed tab in front this long counts as seen, so passing over it, as
/// moving the sidebar's cursor does, doesn't.
pub const SEEN_MS: i64 = 1_000;
/// A note lost by its ID is looked for again this often.
pub const LOST_MS: i64 = 30_000;

/// An item, by its note's ID and its first line.
pub type Key = (String, String);

/// The item it will take next, as last read, for mission control.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Next {
    pub text: String,
    /// When it was read (ms); 0 to read it again at the next check.
    pub at: i64,
    /// What it was read for: the note, its time, the items skipped and the
    /// kind taken.
    pub key: String,
    /// The items it will open, in that order, for its view (the first is
    /// `text`), at most `QUEUE_SHOWN`.
    pub queue: Vec<String>,
    /// The plan each of the note's items links, by the item's first line,
    /// read with them for the board's side panel.
    pub plans: HashMap<String, PlanNote>,
    /// Today's spend, read with them while there is a spend limit.
    pub spent: f64,
}

/// The most items of the queue its view lists.
pub const QUEUE_SHOWN: usize = 8;

/// A mark a check sets on one of its tabs, or what it does to one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mark {
    /// The ID of its item's note, on a tab opened before tabs kept it:
    /// `@autopilot_note`.
    Note(TabId, String),
    /// Seen in its worktree: `@autopilot_in`.
    In(TabId),
    /// To land without asking, or no longer: `@autopilot_land`.
    Land(TabId, bool),
    /// To take the recommended option of what it would ask, or no longer:
    /// `@autopilot_handsfree`.
    HandsFree(TabId, bool),
    /// Made yours, so it leaves the mode, its item kept: landed, with the
    /// autopilot_close setting at never, or a prompt typed in it since.
    /// `@autopilot_yours`, with the marks of what the mode has it do off.
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
    /// Items whose plan needs a choice taken too (hands-free).
    pub with_choices: bool,
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
        self.items(notes, state)
            .map(|items| items.into_iter().next())
    }

    /// Every item the queue gives, in the order it opens them.
    pub fn items(&self, notes: &Path, state: &Path) -> Result<Vec<Item>, String> {
        queue(
            notes,
            &self.repo,
            &self.note,
            &self.skip,
            self.ready_only,
            self.with_choices,
            &crate::cmd::worktree_from_note::claims_in(state),
        )
    }

    /// The plan each item of the note links, by the item's first line.
    pub fn plans(&self, notes: &Path) -> HashMap<String, PlanNote> {
        std::fs::read_to_string(notes.join(&self.note))
            .map(|text| plan_notes(notes, &text))
            .unwrap_or_default()
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

/// A path within the notes directory, never one out of it.
fn inside(notes: &Path, rel: &str) -> Option<PathBuf> {
    let rel = Path::new(rel);
    let plain = rel
        .components()
        .all(|c| matches!(c, std::path::Component::Normal(_)));
    (plain && !rel.as_os_str().is_empty()).then(|| notes.join(rel))
}

/// What following the mode's note by its ID asks for, run off the server's
/// task, since it reads and writes notes and runs git.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Keep {
    /// A repository whose wtn claims from before notes had IDs are brought
    /// to them, once a run.
    pub repo: Option<PathBuf>,
    /// The mode's note, by its path, and its ID as the mode keeps it.
    pub note: String,
    pub note_id: String,
    /// The note at its path given an ID: the mode keeps none yet.
    pub give: bool,
    /// The note looked for by its ID: it isn't at its path.
    pub find: bool,
    /// Tabs opened before tabs kept their note's ID, each with the note it
    /// is linked to (`@tab_note`), whose ID it takes.
    pub tabs: Vec<(TabId, String)>,
}

/// What following the note found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Kept {
    pub note: String,
    pub note_id: String,
    /// The ID the note at its path was given.
    pub given: Option<String>,
    /// Whether it was looked for, and where it was found, by its path
    /// within the notes directory.
    pub looked: bool,
    pub found: Option<String>,
    /// Not found anywhere, the note at its path, holding no ID, was given
    /// it back.
    pub gave_back: bool,
    /// Each older tab and its note's ID.
    pub tabs: Vec<(TabId, String)>,
    /// The claims whose note couldn't be found, left as they were.
    pub unplaced: Vec<String>,
}

impl Keep {
    /// Whether it follows the note, rather than only tidying.
    pub fn follows(&self) -> bool {
        self.give || self.find
    }

    /// Does what it asks, in the notes directory `notes`.
    pub fn run(&self, notes: &Path) -> Kept {
        let mut kept = Kept {
            note: self.note.clone(),
            note_id: self.note_id.clone(),
            ..Kept::default()
        };
        if let Some(repo) = &self.repo {
            kept.unplaced = id::migrate_claims(repo, notes);
        }
        let file = inside(notes, &self.note);
        if self.give
            && let Some(file) = file.as_ref().filter(|f| f.is_file())
        {
            kept.given = id::ensure_id(file, notes).ok();
        }
        if self.find && !self.note_id.is_empty() {
            kept.looked = true;
            let root = notes.canonicalize().unwrap_or_else(|_| notes.to_path_buf());
            kept.found = id::find_by_id(&self.note_id, notes)
                .and_then(|f| Some(f.strip_prefix(&root).ok()?.to_string_lossy().into_owned()));
            if kept.found.is_none()
                && let Some(file) = file.filter(|f| f.is_file() && id::of_file(f).is_none())
            {
                kept.gave_back = id::give_id(&file, &self.note_id).is_ok();
            }
        }
        kept.tabs = self
            .tabs
            .iter()
            .filter_map(|(tab, rel)| {
                let file = inside(notes, rel).filter(|f| f.is_file())?;
                Some((*tab, id::ensure_id(&file, notes).ok()?))
            })
            .collect();
        kept
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
    /// The note to follow by its ID, off the task.
    pub keep: Option<Keep>,
    /// The note, lost, is at its path again: what was said of it goes.
    pub found_again: bool,
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

/// What it says once today's spend has passed the limit, the limit after
/// it; a change of the limit takes it back.
pub const SPENT: &str = "Stopped opening tabs: today's Claude spend passed";

/// What the mode keeps between checks.
#[derive(Clone, Debug)]
pub struct Pilot {
    /// A look for the next item to open is under way.
    opening: bool,
    /// When the last look started (ms).
    last_open: Option<i64>,
    /// Each item opened lately, and when (ms).
    recent: HashMap<Key, i64>,
    /// When the note was last found empty: its time then, and when (ms).
    empty: Option<(i64, i64)>,
    /// "Nothing left to take" said since the note last had an item to take.
    empty_said: bool,
    pub next: Next,
    /// A read for mission control is under way.
    reading: bool,
    /// The day and limit today's spend was found past, so it isn't summed
    /// again until either changes.
    spent: Option<(String, f64)>,
    /// The note said to be missing, said once.
    missing_said: String,
    /// The items of its tabs still open, never taken again while there.
    held: Vec<Key>,
    /// The items of its tabs that landed and left, and when each was first
    /// found so (ms).
    landed: HashMap<Key, i64>,
    /// The note's unticked items as last read, and the time it was read for.
    unticked: (Option<i64>, HashSet<String>),
    /// How many of its tabs work and how many are open, at the last check.
    pub counts: (usize, usize),
    /// When each landed tab was first found in front (ms), for `SEEN_MS`.
    fronts: HashMap<TabId, i64>,
    /// Openings this far apart (ms); the tests give a shorter one.
    pub gap_ms: i64,
    /// Following the note by its ID: a `Keep` under way, one following the
    /// note (nothing opens meanwhile), when the last that followed it ended
    /// (ms), and whether the note was lost then.
    keeping: bool,
    following: bool,
    kept_at: Option<i64>,
    lost: bool,
    /// The repositories whose wtn claims were brought to IDs this run.
    migrated: HashSet<PathBuf>,
    /// The older tabs whose note was asked for its ID.
    asked: HashSet<TabId>,
}

impl Default for Pilot {
    fn default() -> Pilot {
        Pilot {
            opening: false,
            last_open: None,
            recent: HashMap::new(),
            empty: None,
            empty_said: false,
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
            keeping: false,
            following: false,
            kept_at: None,
            lost: false,
            migrated: HashSet::new(),
            asked: HashSet::new(),
        }
    }
}

/// `YYYY-MM-DD` of a time, in the local zone.
fn day_of(now_ms: i64) -> String {
    util::ymd(&util::local_at_ms(now_ms))
}

impl Pilot {
    /// The items of the mode's note never to take now, by their first
    /// line: those skipped, those of its tabs still open, and those opened
    /// in the last `RECENT_MS`.
    pub fn skip(&self, mode: &Mode, now_ms: i64) -> HashSet<String> {
        let here = |key: &&Key| key.0 == mode.note_id;
        mode.skipped_here()
            .map(str::to_string)
            .chain(self.held.iter().filter(here).map(|k| k.1.clone()))
            .chain(
                self.recent
                    .iter()
                    .filter(|(key, at)| here(key) && now_ms - **at < RECENT_MS)
                    .map(|(key, _)| key.1.clone()),
            )
            .collect()
    }

    /// What becomes of a tab back in the main clone once landed, as the
    /// module's comment says, at `now_ms`, one never looked at closed
    /// `close_after` seconds after it landed; with `None` (never closed),
    /// made yours on the check after it is marked landed.
    fn after_landing(&mut self, m: &Mine, now_ms: i64, close_after: Option<f64>) -> Option<Mark> {
        let now = now_ms as f64 / 1000.0;
        let Some(landed) = m.landed else {
            return m.home.then_some(Mark::Landed(m.tab, now as i64));
        };
        let Some(close_after) = close_after.filter(|_| !m.prompted_since) else {
            return Some(Mark::Left(m.tab));
        };
        if m.front {
            let first = *self.fronts.entry(m.tab).or_insert(now_ms);
            return (!m.seen && now_ms - first >= SEEN_MS).then_some(Mark::Seen(m.tab));
        }
        self.fronts.remove(&m.tab);
        if m.stand_in || !(m.seen || now - landed >= close_after) {
            return None;
        }
        Some(Mark::Close(m.tab))
    }

    /// One check, at `now_ms`: marks the mode's tabs as they reach their
    /// worktree or leave the mode (closing a landed one `close` seconds after
    /// it landed when never seen, the autopilot_close setting's choice, none
    /// for never), then, while fewer than `slots` work
    /// and fewer than `cap` are open, looks for the next item to open. `on`
    /// is the autopilot setting, `away` whether you're away, `mtime` when
    /// the note last changed (ms), `None` when it isn't there, `here` the ID
    /// the note at its path holds, and `notes` the notes directory, whose
    /// note is read for its ticked items only while a landed tab's item is
    /// held.
    #[allow(clippy::too_many_arguments)]
    pub fn check(
        &mut self,
        mode: &Mode,
        on: bool,
        close: Option<f64>,
        mine: &[Mine],
        away: bool,
        now_ms: i64,
        mtime: Option<i64>,
        here: Option<&str>,
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
        let mut keep = Keep::default();
        let working = on && mode.on && !mode.note.is_empty();
        // A tab from before tabs kept their note's ID gets the ID of the
        // note it was opened for, kept on it now, so it stays that note's
        // once the note is renamed or the mode works another: the mode's
        // note's when its @tab_note names none or the mode's, another note's
        // asked for off the task (given one when it has none). Until then,
        // and for good once that note is gone, its item is no note's.
        let other_note = |m: &Mine| {
            m.tab_note
                .clone()
                .filter(|t| !t.is_empty() && *t != mode.note)
        };
        for m in mine.iter().filter(|m| m.item.is_some() && m.note.is_none()) {
            match other_note(m) {
                None if !mode.note_id.is_empty() => {
                    marks.push(Mark::Note(m.tab, mode.note_id.clone()));
                }
                Some(other) if working && !self.asked.contains(&m.tab) => {
                    keep.tabs.push((m.tab, other));
                }
                _ => {}
            }
        }
        let key_of = |m: &Mine| -> Option<Key> {
            let note = m.note.clone().unwrap_or_else(|| match other_note(m) {
                Some(other) => format!("gone:{other}"),
                None => mode.note_id.clone(),
            });
            Some((note, m.item.clone()?))
        };
        // Its tabs not landed yet land without asking while the switch is on.
        let land = on && mode.land_without_asking;
        // And decide for you while hands-free is on (the server leaves it off
        // while the autopilot_handsfree setting is).
        let hands_free = on && mode.hands_free;
        for (m, kind) in &placed {
            // Hands-free turned off reaches a tab finishing its landing too.
            if *kind == Kind::Left && m.hands_free && !hands_free {
                marks.push(Mark::HandsFree(m.tab, false));
            }
            match kind {
                Kind::Left => marks.extend(self.after_landing(m, now_ms, close)),
                _ => {
                    if m.in_worktree && !m.was_in {
                        marks.push(Mark::In(m.tab));
                    }
                    if m.land != land {
                        marks.push(Mark::Land(m.tab, land));
                    }
                    if m.hands_free != hands_free {
                        marks.push(Mark::HandsFree(m.tab, hands_free));
                    }
                }
            }
        }
        out.marks = marks;
        self.fronts.retain(|t, _| mine.iter().any(|m| m.tab == *t));
        // A landed tab stays marked until it closes, so its item's time is
        // when it was first found left, forgotten once its note shows it
        // ticked (only the mode's note is read), or once past LANDED_MS with
        // its tab gone.
        let left: HashSet<Key> = placed
            .iter()
            .filter(|(_, kind)| *kind == Kind::Left)
            .filter_map(|(m, _)| key_of(m))
            .collect();
        for key in &left {
            self.landed.entry(key.clone()).or_insert(now_ms);
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
            let ticked = |key: &Key| key.0 == mode.note_id && !open.contains(&key.1);
            self.landed
                .retain(|key, at| !ticked(key) && (now_ms - *at < LANDED_MS || left.contains(key)));
        }
        self.held = placed
            .iter()
            .filter(|(_, kind)| *kind != Kind::Left)
            .filter_map(|(m, _)| key_of(m))
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
        if !working {
            return out;
        }
        // The note followed by its ID, off the task: given one when the mode
        // keeps none, looked for when it isn't at its path.
        if !self.migrated.contains(&mode.repo) {
            keep.repo = Some(mode.repo.clone());
        }
        let at_path = mode.note_id.is_empty() || here == Some(mode.note_id.as_str());
        if !mode.note_id.is_empty() && at_path && self.lost {
            self.lost = false;
            self.missing_said.clear();
            out.found_again = true;
        }
        let due = self.kept_at.is_none_or(|at| now_ms - at >= LOST_MS);
        if !self.keeping && due {
            if mode.note_id.is_empty() {
                keep.give = mtime.is_some();
            } else {
                keep.find = !at_path;
            }
        }
        if !self.keeping && keep != Keep::default() {
            keep.note = mode.note.clone();
            keep.note_id = mode.note_id.clone();
            self.migrated.extend(keep.repo.clone());
            self.asked.extend(keep.tabs.iter().map(|(tab, _)| *tab));
            self.keeping = true;
            self.following = keep.follows();
            out.keep = Some(keep);
        }
        // Nothing taken while it looks, nor from another note at its path.
        if self.following || !at_path {
            if self.lost && self.missing_said != mode.note_id {
                self.missing_said = mode.note_id.clone();
                out.said = Some(format!(
                    "Can't find the note {}: set the mode up again from M-p",
                    note_title(&mode.note)
                ));
            }
            return out;
        }
        let skip = self.skip(mode, now_ms);
        let job = |key: String, mtime: i64| Job {
            note: mode.note.clone(),
            repo: mode.repo.clone(),
            ready_only: mode.ready_only,
            with_choices: mode.hands_free,
            skip: skip.clone(),
            spend_limit: mode.spend_limit,
            key,
            mtime,
        };
        // The item it will take next, read again for mission control.
        let mut skipped: Vec<&String> = skip.iter().collect();
        skipped.sort();
        let key = format!(
            "{}\t{}\t{}\t{}\t{}",
            mode.note,
            mtime.unwrap_or(0),
            skipped
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            mode.ready_only,
            mode.hands_free
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
                out.said = Some(format!("{SPENT} ${}", mode.spend_limit));
            }
            Found::Failed(e) => out.failed = Some(e),
            Found::Item(None) => {
                // Said once, and only with none of its tabs open: while some
                // still work, the view's heading says nothing more starts.
                if !self.empty_said && self.counts.1 == 0 {
                    self.empty_said = true;
                    out.said = Some(format!(
                        "Nothing left to take in {}",
                        note_title(&mode.note)
                    ));
                }
                self.empty = Some((mtime, now_ms));
            }
            Found::Item(Some(item)) => {
                self.empty = None;
                self.empty_said = false;
                out.take = Some(item);
            }
        }
        out
    }

    /// A tab was opened for an item of the note whose ID is `note`: it
    /// isn't taken again for a while.
    pub fn took(&mut self, note: &str, text: &str, now_ms: i64) {
        self.recent
            .insert((note.to_string(), text.to_string()), now_ms);
    }

    /// Following the note has answered: lost when it was looked for and is
    /// nowhere, and its path held nothing to give it back to.
    pub fn kept(&mut self, kept: &Kept, followed: bool, now_ms: i64) {
        self.keeping = false;
        self.following = false;
        if followed {
            self.kept_at = Some(now_ms);
        }
        if kept.looked {
            self.lost = kept.found.is_none() && !kept.gave_back;
            if !self.lost {
                self.missing_said.clear();
            }
        }
        if kept.given.is_some() {
            self.kept_at = None;
        }
    }

    /// The next item, read again for mission control: one found means the
    /// note is no longer empty, though its tabs may fill every slot.
    pub fn next_read(&mut self, key: String, text: String, now_ms: i64) {
        self.reading = false;
        if !text.is_empty() {
            self.empty = None;
        }
        self.next = Next {
            text,
            at: now_ms,
            key,
            queue: Vec::new(),
            plans: HashMap::new(),
            spent: 0.0,
        };
    }

    /// A look that couldn't start, let go.
    pub fn cancel(&mut self) {
        self.opening = false;
        self.reading = false;
    }

    /// The next item read again at the next check, its note having moved.
    pub fn reset_next(&mut self) {
        self.next.at = 0;
    }

    /// Something changed what it would take (a menu's choice, the mode set
    /// up again): the next item read again, an empty note looked at again,
    /// a missing one said again.
    pub fn reset(&mut self) {
        self.next.at = 0;
        self.empty = None;
        self.empty_said = false;
        self.missing_said.clear();
        self.lost = false;
        self.kept_at = None;
    }

    /// What mission control's line shows while the mode is on.
    pub fn summary(&self, mode: &Mode, away: bool) -> Summary {
        // The queue read finds nothing even while its tabs fill every slot,
        // when no look to open one runs.
        let read_empty = !self.next.key.is_empty() && self.next.text.is_empty();
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
            skipped: mode.skipped_here().count(),
            spend_limit: mode.spend_limit,
            spent: self.next.spent,
            lands_alone: mode.land_without_asking,
            hands_free: mode.hands_free,
            target: mode.target.clone(),
            drained: (self.empty.is_some() || read_empty) && self.counts.1 > 0,
            failing: String::new(),
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
        /// The ID the note at its path holds.
        here: Option<String>,
    }

    const ID: &str = "n-000000000000";

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
                    note_id: ID.into(),
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
                here: Some(ID.into()),
            }
        }

        fn check(&mut self) {
            self.now += 1_000;
            let check = self.pilot.check(
                &self.mode,
                true,
                None,
                &self.tabs,
                false,
                self.now,
                Some(self.mtime),
                self.here.as_deref(),
                Path::new("/nowhere"),
            );
            if let Some(keep) = &check.keep {
                self.pilot.kept(&Kept::default(), keep.follows(), self.now);
            }
            for mark in check.marks {
                match mark {
                    Mark::Note(t, id) => self.tab(t).note = Some(id),
                    Mark::Land(t, on) => self.tab(t).land = on,
                    Mark::HandsFree(t, on) => self.tab(t).hands_free = on,
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
                let next = if self.taken < self.max {
                    "next thing"
                } else {
                    ""
                };
                self.pilot.next_read(job.key, next.into(), self.now);
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
                tab.note = Some(self.mode.note_id.clone());
                self.tabs.push(tab);
                self.pilot.took(&self.mode.note_id, &item.text, self.now);
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

    // test/tmux-sidebar.test.js's "autopilot keeps 3 tabs
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
        // Then it moves the pane back to the main clone, its Claude idle
        // there: marked landed, then made yours.
        w.tab(first).in_main = true;
        w.tab(first).state = TabState::Idle;
        w.tab(first).home = true;
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
        assert_eq!(w.said, "", "its tabs still work: it says nothing");
        let summary = w.pilot.summary(&w.mode, false);
        assert_eq!(
            (
                summary.note.as_str(),
                summary.open,
                summary.cap,
                summary.slots,
                summary.drained
            ),
            ("Work", 4, 5, 3, true),
            "the heading says nothing more starts"
        );
        assert_eq!(summary.next, "");
        w.pilot
            .next_read("new".into(), "added meanwhile".into(), w.now);
        assert!(
            !w.pilot.summary(&w.mode, false).drained,
            "an item read for the queue takes it back"
        );
        w.settle();
        assert!(w.pilot.summary(&w.mode, false).drained);
        assert_eq!(w.said, "");

        w.tabs.clear();
        w.now += RECENT_MS;
        w.settle();
        assert_eq!(
            w.said, "Nothing left to take in Work",
            "with none of its tabs open, it says so"
        );
        assert!(!w.pilot.summary(&w.mode, false).drained);
        w.said.clear();
        w.now += RECENT_MS;
        w.settle();
        assert_eq!(w.said, "", "once");
    }

    #[test]
    fn with_every_slot_full_the_queue_read_says_nothing_more_starts() {
        let mode = World::new().mode;
        let mut pilot = Pilot {
            counts: (3, 3),
            ..Pilot::default()
        };
        assert!(
            !pilot.summary(&mode, false).drained,
            "nothing read yet: nothing said"
        );
        pilot.next_read("k".into(), "item".into(), 1);
        assert!(!pilot.summary(&mode, false).drained);
        pilot.next_read("k".into(), String::new(), 2);
        assert!(pilot.summary(&mode, false).drained);
        pilot.counts = (0, 0);
        assert!(
            !pilot.summary(&mode, false).drained,
            "with no tab open it is said in the notifications instead"
        );
    }

    #[test]
    fn it_opens_nothing_while_away_or_past_the_spend_limit_and_says_a_missing_note_once() {
        let mut w = World::new();
        let none = Path::new("/nowhere");
        let check = w.pilot.check(
            &w.mode,
            true,
            None,
            &[],
            true,
            w.now,
            Some(w.mtime),
            Some(ID),
            none,
        );
        assert_eq!(check.open, None, "away");
        let check = w.pilot.check(
            &w.mode,
            false,
            None,
            &[],
            false,
            w.now,
            Some(w.mtime),
            Some(ID),
            none,
        );
        assert_eq!(check.open, None, "the setting off");

        w.mode.spend_limit = 5.0;
        let check = w.pilot.check(
            &w.mode,
            true,
            None,
            &[],
            false,
            w.now,
            Some(w.mtime),
            Some(ID),
            none,
        );
        let job = check.open.expect("a look, which sums the spend");
        assert_eq!(job.spend_limit, 5.0);
        let opened = w.pilot.opened(&w.mode, Found::Spent, w.mtime, w.now);
        assert_eq!(
            opened.said.as_deref(),
            Some("Stopped opening tabs: today's Claude spend passed $5")
        );
        let later = w.now + 60_000;
        let check = w.pilot.check(
            &w.mode,
            true,
            None,
            &[],
            false,
            later,
            Some(w.mtime),
            Some(ID),
            none,
        );
        assert_eq!(check.open, None, "not summed again today");
        w.mode.spend_limit = 50.0;
        let check = w.pilot.check(
            &w.mode,
            true,
            None,
            &[],
            false,
            later,
            Some(w.mtime),
            Some(ID),
            none,
        );
        assert!(check.open.is_some(), "a new limit is looked at again");
        w.pilot.cancel();
        // The claims brought to IDs, as the first check asked.
        w.pilot.kept(&Kept::default(), false, later);

        // Gone from its path, and looked for by its ID in vain.
        let missing = |w: &mut World, at: i64| {
            let check = w
                .pilot
                .check(&w.mode, true, None, &[], false, at, None, None, none);
            assert_eq!(check.open, None, "nothing opened while it is missing");
            if let Some(keep) = &check.keep {
                assert!(keep.find && keep.note_id == ID, "{keep:?}");
                let nowhere = Kept {
                    looked: true,
                    ..Kept::default()
                };
                w.pilot.kept(&nowhere, keep.follows(), at);
            }
            check.said
        };
        assert_eq!(missing(&mut w, later + 10_000), None, "not while it looks");
        assert_eq!(
            missing(&mut w, later + 20_000).as_deref(),
            Some("Can't find the note Work: set the mode up again from M-p")
        );
        assert_eq!(missing(&mut w, later + 30_000), None, "said once");
    }

    // test/tmux-sidebar.test.js's "the mode follows its note by its ID when
    // the note is renamed outside Notes, and a skipped item hides only in
    // its own note", the looks answered here; Keep::run's own are tested
    // below.
    // test/tmux-sidebar.test.js's "Land without asking, after a yes, marks
    // the mode's tabs not landed yet, and only those, and its line says so;
    // Ask before landing unmarks them".
    #[test]
    fn landing_without_asking_marks_its_tabs_not_landed_yet_and_only_those() {
        let mut w = World::new();
        let now = w.now;
        let working = mine(1, (now / 1000) as f64 - 600.0, TabState::Working, true);
        let mut landed = mine(3, (now / 1000) as f64 - 600.0, TabState::Idle, false);
        landed.was_in = true;
        landed.home = true;
        let marks = |w: &mut World, tabs: &[Mine], on: bool| {
            w.pilot
                .check(
                    &w.mode,
                    on,
                    super::close_after("seen"),
                    tabs,
                    false,
                    now,
                    Some(w.mtime),
                    Some(ID),
                    Path::new("/nowhere"),
                )
                .marks
                .into_iter()
                .filter(|m| matches!(m, Mark::Land(..)))
                .collect::<Vec<_>>()
        };
        let tabs = [working.clone(), landed.clone()];
        assert!(marks(&mut w, &tabs, true).is_empty(), "off: nothing marked");
        w.mode.land_without_asking = true;
        assert_eq!(
            marks(&mut w, &tabs, true),
            [Mark::Land(TabId(1), true)],
            "only the mode's tab still in its worktree"
        );
        assert!(
            marks(&mut w, &tabs, false).is_empty(),
            "nor with the autopilot setting off"
        );
        let mut marked = working.clone();
        marked.land = true;
        assert!(
            marks(&mut w, std::slice::from_ref(&marked), true).is_empty(),
            "already marked: nothing to do"
        );
        assert!(w.pilot.summary(&w.mode, false).lands_alone);
        w.mode.land_without_asking = false;
        assert_eq!(
            marks(&mut w, std::slice::from_ref(&marked), true),
            [Mark::Land(TabId(1), false)],
            "unmarked on the open tabs"
        );
    }

    #[test]
    fn hands_free_marks_its_tabs_and_turned_off_unmarks_one_finishing_its_landing_too() {
        let mut w = World::new();
        let now = w.now;
        let working = mine(1, (now / 1000) as f64 - 600.0, TabState::Working, true);
        let mut landed = mine(3, (now / 1000) as f64 - 600.0, TabState::Working, false);
        landed.was_in = true;
        let marks = |w: &mut World, tabs: &[Mine]| {
            w.pilot
                .check(
                    &w.mode,
                    true,
                    super::close_after("seen"),
                    tabs,
                    false,
                    now,
                    Some(w.mtime),
                    Some(ID),
                    Path::new("/nowhere"),
                )
                .marks
                .into_iter()
                .filter(|m| matches!(m, Mark::HandsFree(..)))
                .collect::<Vec<_>>()
        };
        w.mode.hands_free = true;
        assert_eq!(
            marks(&mut w, &[working.clone(), landed.clone()]),
            [Mark::HandsFree(TabId(1), true)],
            "only the tab still in its worktree is marked"
        );
        w.mode.hands_free = false;
        let mut was = working.clone();
        was.hands_free = true;
        landed.hands_free = true;
        assert_eq!(
            marks(&mut w, &[was, landed]),
            [
                Mark::HandsFree(TabId(1), false),
                Mark::HandsFree(TabId(3), false)
            ],
            "unmarked in the worktree and while finishing its landing"
        );
    }

    #[test]
    fn it_follows_its_note_by_its_id_taking_nothing_from_another_note_at_its_path() {
        fn look(w: &mut World, at: &mut i64, here: Option<&str>) -> Check {
            *at += 1_000;
            let mtime = Some(w.mtime);
            w.pilot.check(
                &w.mode,
                true,
                None,
                &[],
                false,
                *at,
                mtime,
                here,
                Path::new("/nowhere"),
            )
        }
        let mut w = World::new();
        let mut at = w.now;
        let first = look(&mut w, &mut at, Some(ID));
        assert!(first.open.is_some(), "at its path: worked");
        assert_eq!(
            first.keep.as_ref().and_then(|k| k.repo.clone()),
            Some(PathBuf::from("/repo")),
            "the repository's claims brought to IDs, once"
        );
        w.pilot.cancel();
        w.pilot.kept(&Kept::default(), false, at);

        // Another note at its path: looked for, nothing taken meanwhile.
        let other = look(&mut w, &mut at, Some("n-ffffffffffff"));
        assert_eq!(other.open, None);
        let keep = other.keep.expect("looked for by its ID");
        assert!(keep.find && !keep.give && keep.repo.is_none());
        // Found elsewhere: the server writes its new path back.
        let found = Kept {
            looked: true,
            found: Some("Projects/Renamed.md".into()),
            ..Kept::default()
        };
        w.pilot.kept(&found, true, at);
        w.mode.note = "Projects/Renamed.md".into();
        assert!(look(&mut w, &mut at, Some(ID)).open.is_some());
        w.pilot.cancel();

        // Later nowhere, another note at its path: it says so, and takes
        // nothing from that one.
        at += LOST_MS;
        let keep = look(&mut w, &mut at, Some("n-ffffffffffff"))
            .keep
            .expect("looked for again");
        let lost = Kept {
            looked: true,
            ..Kept::default()
        };
        w.pilot.kept(&lost, keep.follows(), at);
        let said = look(&mut w, &mut at, Some("n-ffffffffffff"));
        assert_eq!(said.open, None);
        assert!(said.keep.is_none(), "looked for again only every LOST_MS");
        assert_eq!(
            said.said.as_deref(),
            Some("Can't find the note Renamed: set the mode up again from M-p")
        );
        // Back at its path (renamed back, or given back its ID): worked, and
        // what was said of it goes.
        let back = look(&mut w, &mut at, Some(ID));
        assert!(back.found_again && back.open.is_some());

        // A note from before notes had IDs is given one first.
        w.mode.note_id.clear();
        w.pilot.cancel();
        at += LOST_MS;
        let give = look(&mut w, &mut at, None);
        assert!(give.keep.is_some_and(|k| k.give && !k.find));
        assert_eq!(give.open, None, "nothing taken until it has one");
    }

    #[test]
    fn a_tab_from_before_tabs_kept_their_notes_id_takes_its_notes_and_holds_its_item_there_alone() {
        let mut w = World::new();
        let now = w.now;
        let mut ours = mine(1, (now / 1000) as f64 - 300.0, TabState::Working, true);
        ours.item = Some("Shared".into());
        let mut other = mine(2, (now / 1000) as f64 - 300.0, TabState::Working, true);
        other.item = Some("Elsewhere".into());
        other.tab_note = Some("Other.md".into());
        let mut gone = mine(3, (now / 1000) as f64 - 300.0, TabState::Working, true);
        gone.item = Some("Gone".into());
        gone.tab_note = Some("Gone.md".into());
        let tabs = [ours, other, gone];
        let check = w.pilot.check(
            &w.mode,
            true,
            None,
            &tabs,
            false,
            now,
            Some(w.mtime),
            Some(ID),
            Path::new("/nowhere"),
        );
        assert_eq!(
            check.marks,
            [Mark::Note(TabId(1), ID.into())],
            "the mode's note's, kept on it"
        );
        let keep = check.keep.expect("the other notes asked for their IDs");
        assert_eq!(
            keep.tabs,
            [(TabId(2), "Other.md".into()), (TabId(3), "Gone.md".into())]
        );
        let skip = w.pilot.skip(&w.mode, now);
        assert!(skip.contains("Shared"));
        assert!(
            !skip.contains("Elsewhere") && !skip.contains("Gone"),
            "another note's items aren't held in this one"
        );
        let again = w.pilot.check(
            &w.mode,
            true,
            None,
            &tabs,
            false,
            now + 1_000,
            Some(w.mtime),
            Some(ID),
            Path::new("/nowhere"),
        );
        assert!(again.keep.is_none(), "asked once");
    }

    #[test]
    fn following_finds_the_note_by_its_id_gives_one_back_and_gives_older_tabs_their_notes() {
        let dir = std::env::temp_dir().join(format!("sb-autopilot-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Projects")).unwrap();
        let notes = dir.canonicalize().unwrap();
        let line = id::line(ID);
        std::fs::write(
            notes.join("Projects/Renamed.md"),
            format!(
                "# Work

- [ ] One

{line}
"
            ),
        )
        .unwrap();
        std::fs::write(
            notes.join("Other.md"),
            "# Other

- [ ] One
",
        )
        .unwrap();
        let keep = Keep {
            note: "Work.md".into(),
            note_id: ID.into(),
            find: true,
            tabs: vec![
                (TabId(2), "Other.md".into()),
                (TabId(3), "../outside.md".into()),
            ],
            ..Keep::default()
        };
        let kept = keep.run(&notes);
        assert_eq!(kept.found.as_deref(), Some("Projects/Renamed.md"));
        let other = id::of_file(&notes.join("Other.md")).expect("given one");
        assert_eq!(kept.tabs, [(TabId(2), other)], "never a note outside");

        // Rewritten at its path without its ID, and nowhere else: given it back.
        std::fs::remove_file(notes.join("Projects/Renamed.md")).unwrap();
        std::fs::write(
            notes.join("Work.md"),
            "# Work

- [ ] One
",
        )
        .unwrap();
        let kept = Keep {
            tabs: Vec::new(),
            ..keep.clone()
        }
        .run(&notes);
        assert!(kept.looked && kept.found.is_none() && kept.gave_back);
        assert_eq!(id::of_file(&notes.join("Work.md")).as_deref(), Some(ID));

        // Another note's at its path: lost, and that note left alone.
        std::fs::write(
            notes.join("Work.md"),
            "# Else

<!-- note-id: n-ffffffffffff -->
",
        )
        .unwrap();
        let kept = Keep {
            tabs: Vec::new(),
            ..keep
        }
        .run(&notes);
        assert!(kept.found.is_none() && !kept.gave_back);
        let _ = std::fs::remove_dir_all(&dir);
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
            pilot.check(&mode, true, None, tabs, false, at, Some(mtime), None, &dir);
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

    // test/tmux-sidebar.test.js's "a landed tab of autopilot
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
        let marks = |pilot: &mut Pilot, tab: &Mine, at: i64, close: Option<f64>| {
            pilot
                .check(
                    &mode,
                    true,
                    close,
                    std::slice::from_ref(tab),
                    false,
                    at,
                    None,
                    None,
                    Path::new("/nowhere"),
                )
                .marks
        };
        assert!(
            marks(&mut pilot, &tab, now, None).is_empty(),
            "with the setting off, not left before it is marked landed"
        );
        assert!(
            marks(&mut pilot, &tab, now, close_after("seen")).is_empty(),
            "not marked landed while its Claude isn't idle in a main clone"
        );
        tab.home = true;
        assert_eq!(
            marks(&mut pilot, &tab, now, close_after("seen")),
            [Mark::Landed(TabId(1), secs as i64)]
        );
        tab.landed = Some(secs);
        assert!(
            marks(&mut pilot, &tab, now + 1_000, close_after("seen")).is_empty(),
            "not yet"
        );

        // In front: seen after a second, never closed while there.
        tab.front = true;
        assert!(marks(&mut pilot, &tab, now + 2_000, close_after("seen")).is_empty());
        assert_eq!(
            marks(&mut pilot, &tab, now + 3_500, close_after("seen")),
            [Mark::Seen(TabId(1))]
        );
        tab.seen = true;
        assert!(marks(&mut pilot, &tab, now + 4_000, close_after("seen")).is_empty());
        // Left: closed, unless a tool stands in for its pane.
        tab.front = false;
        tab.stand_in = true;
        assert!(marks(&mut pilot, &tab, now + 5_000, close_after("seen")).is_empty());
        tab.stand_in = false;
        assert_eq!(
            marks(&mut pilot, &tab, now + 5_000, close_after("seen")),
            [Mark::Close(TabId(1))]
        );

        // Never looked at: closed half an hour after it landed.
        let mut unseen = tab.clone();
        unseen.seen = false;
        assert!(marks(&mut pilot, &unseen, now + 60_000, close_after("seen")).is_empty());
        assert_eq!(
            marks(&mut pilot, &unseen, now + 1_800_000, close_after("seen")),
            [Mark::Close(TabId(1))]
        );
        // A prompt typed in it since makes it yours.
        unseen.prompted_since = true;
        assert_eq!(
            marks(&mut pilot, &unseen, now + 1_800_000, close_after("seen")),
            [Mark::Left(TabId(1))]
        );

        // At once: as soon as it lands, unless in front; after five minutes,
        // or sooner once seen and left.
        let mut fresh = tab.clone();
        fresh.seen = false;
        fresh.landed = Some(secs);
        assert_eq!(
            marks(&mut pilot, &fresh, now, close_after("now")),
            [Mark::Close(TabId(1))]
        );
        assert!(marks(&mut pilot, &fresh, now + 60_000, close_after("5m")).is_empty());
        assert_eq!(
            marks(&mut pilot, &fresh, now + 300_000, close_after("5m")),
            [Mark::Close(TabId(1))]
        );
        assert_eq!(close_after("off"), None, "never");
    }

    #[test]
    fn with_never_a_landed_tab_is_marked_landed_first_then_made_yours() {
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
        tab.home = true;
        let mut marks = |tab: &Mine, at: i64| {
            pilot
                .check(
                    &mode,
                    true,
                    None,
                    std::slice::from_ref(tab),
                    false,
                    at,
                    None,
                    None,
                    Path::new("/nowhere"),
                )
                .marks
        };
        assert_eq!(marks(&tab, now), [Mark::Landed(TabId(1), secs as i64)]);
        tab.landed = Some(secs);
        assert_eq!(marks(&tab, now + 1_000), [Mark::Left(TabId(1))]);
    }
}
