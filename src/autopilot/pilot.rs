//! What the mode keeps between checks and what each check decides
//! (autopilot.js's `autopilot()` and its `check`), as plain data: given the
//! mode, its tabs and the time, a check says which tabs to mark, whether to
//! read the next item again for mission control, whether to look for the
//! next item to open, and anything new to say. The looks run off the
//! server's task, since the queue reads git (`Job`), and come back to
//! `opened` and `next_read`.
//!
//! Openings are `GAP_MS` apart and one at a time, so `wt`'s worktrees and
//! the new Claude sessions don't race. An item is never taken again while
//! a tab of the mode's is open for it (`held`, from `@autopilot_item`), nor
//! for `RECENT_MS` after it was opened (`recent`), before `wt` has made its
//! branch, which is what claims it. A tab that landed and left holds
//! nothing: `switchboard item-done` ticks its item before it removes the
//! worktree, so the note shows it landed by then. A tab whose `wt` stopped
//! before making its worktree (`@autopilot_failed`) is closed, its item opened
//! again on a later check, and skipped once it has failed `FAILS_SKIP`
//! times in `FAILS_MS` (`start_failed`); its card in Next up says why. A note changed in the last
//! `SETTLE_MS` isn't taken from, since an item being typed is saved as it
//! is typed and its first words would be opened as an item. A note found
//! empty is looked at again once it changes, or `RECENT_MS` later, since an
//! item a worktree had comes back once that worktree is removed.
//!
//! Every item it keeps is by its note's ID and first line (`Key`), the
//! note's stable ID (crates/notes/src/id.rs): those skipped, those of its tabs
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
//! can't find the note. Nothing is opened while it looks. wt's claims from
//! before notes had IDs are brought to IDs once a run in the mode's
//! repository.
//!
//! With "Land without asking" on (`landWithoutAsking`, with the autopilot
//! setting on), each of its tabs not landed yet is marked `@land` as it
//! turns on, which queue-and-clean reads to land on the default branch
//! without asking to push, and unmarked as it goes off; between the two a
//! tab's own menu decides for it; queue-and-clean marks one that
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
use super::mode::{Mode, Sources, note_title};
use super::place::{Kind, Mine, kind_of};
use crate::cmd::worktree_from_note::{Item, PlanNote, plan_notes, queue_and_held};
use crate::mux::TabId;
use crate::notes::id;
use crate::util;

/// Openings are this far apart.
pub const GAP_MS: i64 = 5_000;
/// An item opened this recently is never taken again.
pub const RECENT_MS: i64 = 60_000;
/// An item whose tab failed to start this many times within `FAILS_MS` is
/// skipped.
pub const FAILS_SKIP: usize = 3;
/// How long a failed start counts toward `FAILS_SKIP`.
pub const FAILS_MS: i64 = 60 * 60_000;
/// An item whose start failed is opened again no sooner than this after,
/// listed in Next up meanwhile.
pub const RETRY_MS: i64 = 5 * 60_000;
/// A note changed this recently isn't taken from.
pub const SETTLE_MS: i64 = 60_000;
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

/// An item the queue would take but for what it waits on: its first line,
/// what holds it (`Item::waits_on`: the first open item an `After:` line of
/// its names, or the review of a design board), and every open item its
/// `After:` lines name (`held_by_all`), each with where it stands in the
/// note (`Stands`), none when a review alone holds it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Held {
    pub text: String,
    pub waits_on: String,
    pub after: Vec<(String, Stands)>,
}

/// Where an open item another waits on stands in the note, read from the
/// whole queue and the note's own text, past the `QUEUE_SHOWN` the view is
/// given: its place in the queue (from 1); blocked itself by open items,
/// as its `After:` lines in the note say, whether or not the queue would
/// take it (an unplanned, skipped or parked one too); or open with the
/// queue not taking it and nothing holding it (at work in a tab, skipped,
/// parked, waiting on a choice or a review).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stands {
    Queued(usize),
    Blocked,
    Open,
}

/// The held items the view shows (`QUEUE_SHOWN` of `held`, in its order),
/// each with every open item its `After:` lines name in the note `text`
/// and where that one stands, from the whole `queue` and the note; an
/// issue with every open issue blocking it (its item's `after`), named by
/// its item's first line while the queue or `held` has it.
pub fn held_shown(text: &str, queue: &[Item], held: Vec<Item>) -> Vec<Held> {
    use crate::cmd::worktree_from_note::held_by_all;
    // An issue's item, among `items`, by its reference.
    let issue_in = |items: &[Item], on: &str| {
        let prefix = format!("{}: ", on.to_lowercase());
        items
            .iter()
            .position(|i| i.line == 0 && i.text.to_lowercase().starts_with(&prefix))
    };
    let issues_held: Vec<Item> = held.iter().filter(|i| i.line == 0).cloned().collect();
    let issue_stands = |on: &str| {
        if let Some(at) = issue_in(queue, on) {
            (queue[at].text.clone(), Stands::Queued(at + 1))
        } else if let Some(at) = issue_in(&issues_held, on) {
            (issues_held[at].text.clone(), Stands::Blocked)
        } else {
            (on.to_string(), Stands::Open)
        }
    };
    // An open item of the note, by its first line, that waits on another.
    let blocked_itself = |on: &str| {
        text.split('\n').enumerate().any(|(at, line)| {
            line.strip_prefix("- [ ] ")
                .is_some_and(|first| first.trim() == on)
                && !held_by_all(text, at + 1).is_empty()
        })
    };
    let stands = |on: &str| {
        if let Some(at) = queue.iter().position(|i| i.text == on) {
            Stands::Queued(at + 1)
        } else if blocked_itself(on) {
            Stands::Blocked
        } else {
            Stands::Open
        }
    };
    held.into_iter()
        .take(QUEUE_SHOWN)
        .filter_map(|i| {
            Some(Held {
                after: if i.line == 0 {
                    i.after.iter().map(|on| issue_stands(on)).collect()
                } else {
                    held_by_all(text, i.line)
                        .into_iter()
                        .map(|on| {
                            let stands = stands(&on);
                            (on, stands)
                        })
                        .collect()
                },
                text: i.text,
                waits_on: i.waits_on?,
            })
        })
        .collect()
}

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
    /// The items it would open but for what each waits on, for its view.
    pub held: Vec<Held>,
    /// The plan each of the note's items links, by the item's first line,
    /// read with them for the board's side panel.
    pub plans: HashMap<String, PlanNote>,
    /// Each issue's priority, by its item's first line, for its card.
    pub priorities: HashMap<String, u8>,
    /// Today's spend, read with them while there is a spend limit.
    pub spent: f64,
    /// The week's use of the weekly rate limit (percent) and when it
    /// resets (seconds), read with them while there is a weekly limit.
    pub weekly: Option<(f64, i64)>,
    /// What is left of the note, whole, for the view's estimate.
    pub left: super::eta::Left,
    /// The read for `key` failed (GitHub issues unreadable): the queue
    /// is unknown, never drained, until a read succeeds.
    pub failed: bool,
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
    /// To land without asking, or no longer: `@land`.
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
    /// Items listed in the queue but not opened yet: those whose start
    /// failed in the last `RETRY_MS`.
    pub retrying: HashSet<String>,
    /// The items put next in the queue, by their first lines.
    pub next: Vec<String>,
    /// Today's Claude spend past which nothing opens; 0 for none.
    pub spend_limit: f64,
    /// The percent of the weekly rate limit used past which nothing opens;
    /// 0 for none.
    pub weekly_limit: u32,
    /// For a read for mission control, the key it is read under.
    pub key: String,
    /// When the note last changed (ms).
    pub mtime: i64,
    /// Where the work comes from, as the repository's tickets say; `None`
    /// while they give none (`Mode::source`), which takes nothing.
    pub sources: Option<Sources>,
    /// Whether GitHub issues may be read now (`Mode::issues_on`); off, a
    /// source of issues gives none.
    pub issues_on: bool,
    /// The label an issue carries to be taken.
    pub label: String,
}

impl Job {
    /// The first item the queue gives, if any; `state` is the state
    /// directory, whose claims file says which items wt started lately.
    pub fn first(&self, notes: &Path, state: &Path) -> Result<Option<Item>, String> {
        self.items(notes, state)
            .map(|items| items.into_iter().find(|i| !self.retrying.contains(&i.text)))
    }

    /// Every item the queue gives, in the order it opens them.
    pub fn items(&self, notes: &Path, state: &Path) -> Result<Vec<Item>, String> {
        self.items_and_held(notes, state).map(|(items, _)| items)
    }

    /// `items`, and those it would give but for the open item each waits on.
    pub fn items_and_held(
        &self,
        notes: &Path,
        state: &Path,
    ) -> Result<(Vec<Item>, Vec<Item>), String> {
        match self.sources {
            Some(Sources::Note) => self.note_items(notes, state),
            Some(Sources::Issues) if self.issues_on => self
                .issue_items(state)
                .map_err(|e| format!("GitHub issues: {e}")),
            Some(Sources::Issues) | None => Ok((Vec::new(), Vec::new())),
        }
    }

    /// The repository's GitHub issues it may take, by priority and then
    /// oldest first, as items: those no worktree has, wt didn't start
    /// lately and weren't skipped. Apart from them, those blocked by an open
    /// issue, each waiting on the first (its item's `after` naming every
    /// one), and those whose body's open choice waits on your review of a
    /// design board, which hands-free never decides.
    fn issue_items(&self, state: &Path) -> Result<(Vec<Item>, Vec<Item>), String> {
        let mut issues = super::issues::issues(&self.repo, &self.label, state, util::now_ms())?;
        // Stable, so the search's oldest first holds within a priority;
        // those put next first of all, the last put there first, by their
        // reference, whatever their title says now.
        issues.sort_by_key(|i| i.priority);
        let mut front = Vec::new();
        for text in &self.next {
            let Some((repo, number)) = super::issues::reference_in(text) else {
                continue;
            };
            let named = format!("{repo}#{number}").to_lowercase();
            if let Some(at) = issues
                .iter()
                .position(|i| i.reference().to_lowercase() == named)
            {
                front.push(issues.remove(at));
            }
        }
        issues.splice(0..0, front);
        let claims = std::fs::read_to_string(crate::cmd::worktree_from_note::claims_in(state))
            .unwrap_or_default();
        // An ID that can't be read claims nothing as this computer's, so
        // every claim on GitHub is another's.
        let own = super::issues::own_claim_id()
            .ok()
            .flatten()
            .unwrap_or_default();
        let (mut ready, mut held) = (Vec::new(), Vec::new());
        for issue in super::issues::unclaimed(
            &self.repo,
            issues,
            &claims,
            &self.skip,
            &own,
            util::now_secs(),
        ) {
            let item = super::issues::item_of(&issue);
            if let Some(first) = issue.blocked_by.first() {
                held.push(Item {
                    waits_on: Some(first.clone()),
                    after: issue.blocked_by.clone(),
                    ..item
                });
                continue;
            }
            if let Some(what) = crate::cmd::worktree_from_note::review_hold(&issue.body) {
                held.push(Item {
                    waits_on: Some(what),
                    ..item
                });
                continue;
            }
            // Its body is its plan: none is no plan, which Planned only
            // leaves out, and an open choice is taken only hands-free, as a
            // note item's plan is.
            let open = crate::cmd::worktree_from_note::open_choice(&issue.body);
            if (self.ready_only && issue.body.trim().is_empty()) || (open && !self.with_choices) {
                continue;
            }
            ready.push(item);
        }
        Ok((ready, held))
    }

    /// The note's items, and those held, in the order it opens them.
    fn note_items(&self, notes: &Path, state: &Path) -> Result<(Vec<Item>, Vec<Item>), String> {
        let (mut items, held) = queue_and_held(
            notes,
            &self.repo,
            &self.note,
            &self.skip,
            self.ready_only,
            self.with_choices,
            &crate::cmd::worktree_from_note::claims_in(state),
        )?;
        if !self.next.is_empty() {
            // Only an item whose plan is settled now is put first: its plan
            // may have gained a decision since it was put next.
            use crate::cmd::worktree_from_note::{item_text, plan_open, put_next};
            let text = std::fs::read_to_string(notes.join(&self.note)).unwrap_or_default();
            put_next(&mut items, &self.next, |i| {
                !plan_open(notes, &item_text(&text, i.line))
            });
        }
        Ok((items, held))
    }

    /// Each issue's priority, by its item's first line, from the read the
    /// queue took (no request of its own while that read is fresh).
    pub fn priorities(&self, state: &Path) -> HashMap<String, u8> {
        if !(self.sources == Some(Sources::Issues) && self.issues_on) {
            return HashMap::new();
        }
        super::issues::issues(&self.repo, &self.label, state, util::now_ms())
            .unwrap_or_default()
            .into_iter()
            .map(|i| (i.text(), i.priority))
            .collect()
    }

    /// The plan each item of the note links, by the item's first line.
    /// An issue's plan is its body, from the read the queue took (no
    /// request of its own while that read is fresh).
    pub fn plans(&self, notes: &Path, state: &Path) -> HashMap<String, PlanNote> {
        let mut plans: HashMap<String, PlanNote> = if self.sources == Some(Sources::Note) {
            std::fs::read_to_string(notes.join(&self.note))
                .map(|text| plan_notes(notes, &text))
                .unwrap_or_default()
        } else {
            HashMap::new()
        };
        if self.sources == Some(Sources::Issues)
            && self.issues_on
            && let Ok(issues) =
                super::issues::issues(&self.repo, &self.label, state, util::now_ms())
        {
            for issue in issues {
                plans.insert(
                    issue.text(),
                    PlanNote {
                        name: issue.reference(),
                        text: Some(issue.body.clone()),
                        file: None,
                    },
                );
            }
        }
        plans
    }

    /// The look for the next item to open: nothing once today's spend has
    /// passed the limit, as the Usage view sums it, or the week's use of the
    /// weekly rate limit has reached its limit, as the statusline last read
    /// it (a missing or past reading holds nothing); else the queue's first.
    pub fn find(&self, notes: &Path, state: &Path, now_ms: i64) -> Found {
        if self.spend_limit > 0.0
            && crate::usage::sessions::spent_today(state, now_ms) >= self.spend_limit
        {
            return Found::Spent;
        }
        if self.weekly_limit > 0
            && let Some((used, resets_at)) = crate::usage::sessions::weekly_used(state, now_ms)
            && used >= f64::from(self.weekly_limit)
        {
            return Found::Weekly { used, resets_at };
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
    /// A repository whose wt claims from before notes had IDs are brought
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
    /// The week's use of the weekly rate limit has reached its limit, until
    /// it resets (seconds).
    Weekly { used: f64, resets_at: i64 },
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
    /// The repository's tickets give it work it may take: what was said
    /// of their giving none goes (`unsay_source`).
    pub source_ok: bool,
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

/// What it says once the week's use of the weekly rate limit has reached
/// its limit, the limit and when it resets after it; a change of the limit,
/// or a tab opened again, takes it back.
pub const WEEKLY: &str = "Stopped opening tabs: weekly limit";

/// What it says without its sentence starting `what` (`SPENT` or `WEEKLY`),
/// wherever that sentence is, since what it says after a refusal is added
/// to it (`autopilot_say`).
pub fn unsay(said: &str, what: &str) -> String {
    let Some(at) = said.find(what) else {
        return said.to_string();
    };
    let end = said[at..].find(". ").map_or(said.len(), |e| at + e + 2);
    format!("{}{}", &said[..at], &said[end..])
        .trim_end_matches(". ")
        .to_string()
}

/// `said` without what it said of the repository's tickets giving it no
/// work (`Mode::source` and `ISSUES_OFF`), each a sentence of its own.
pub fn unsay_source(said: &str) -> String {
    const OF_SOURCE: [&str; 4] = [
        "ticket source isn't set",
        "which autopilot can't take",
        "its issue label isn't set",
        ISSUES_OFF,
    ];
    said.split(". ")
        .filter(|s| !OF_SOURCE.iter().any(|m| s.contains(m)))
        .collect::<Vec<_>>()
        .join(". ")
}

/// What it says while a read of GitHub issues fails (`e` starting
/// "GitHub issues:"), as one sentence, so a good read takes it back whole
/// (`unsay`).
pub fn unread_said(e: &str) -> String {
    format!("Can't read {}", e.replace(". ", "; ").trim_end_matches('.'))
}

/// What it says of the weekly limit reached: "… 85% reached, resets in 2h
/// 10m".
pub fn weekly_said(limit: u32, resets_at: i64) -> String {
    // When, not how long until, since it stays said until the week resets.
    let when = util::local_at_ms(resets_at * 1000);
    format!(
        "{WEEKLY} {limit}% reached, resets {}",
        when.strftime("%a %-I:%M %p")
    )
}

/// What the mode keeps between checks.
#[derive(Clone, Debug)]
pub struct Pilot {
    /// A look for the next item to open is under way.
    opening: bool,
    /// When the last look started (ms).
    last_open: Option<i64>,
    /// Each item opened lately, and when (ms).
    recent: HashMap<Key, i64>,
    /// Each item whose tab's start failed in the last `FAILS_MS`: when (ms)
    /// and why, oldest first.
    failed: HashMap<Key, Vec<(i64, String)>>,
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
    /// When the week's use last found at the weekly limit resets (seconds),
    /// and that limit, so it is said once; forgotten as the limit changes,
    /// so going back to it says it again.
    weekly: Option<(i64, u32)>,
    /// The note said to be missing, said once.
    missing_said: String,
    /// Why the repository's tickets give it no work, as last said.
    source_said: String,
    /// Where its work comes from, as its repository's tickets say
    /// (`Mode::source`), and the label its issues carry, which the server
    /// reads from repos.json before each check; why it takes none, else.
    pub source: Result<Sources, String>,
    pub label: String,
    /// The items of its tabs still open, never taken again while there.
    held: Vec<Key>,
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
    /// The repositories whose wt claims were brought to IDs this run.
    migrated: HashSet<PathBuf>,
    /// The older tabs whose note was asked for its ID.
    asked: HashSet<TabId>,
    /// "Land without asking" (with the autopilot setting on) at the last
    /// check, so its tabs are marked only as it changes and a tab's own
    /// menu or the land_without_asking setting decides for it meanwhile.
    landing: Option<bool>,
}

/// What it says taking GitHub issues while Settings' autopilot or GitHub
/// keeps it from reading them.
pub const ISSUES_OFF: &str =
    "GitHub is off in Settings, so autopilot takes none of the repository's issues";

impl Default for Pilot {
    fn default() -> Pilot {
        Pilot {
            opening: false,
            last_open: None,
            recent: HashMap::new(),
            failed: HashMap::new(),
            empty: None,
            empty_said: false,
            next: Next::default(),
            reading: false,
            spent: None,
            weekly: None,
            missing_said: String::new(),
            source_said: String::new(),
            source: Ok(Sources::Note),
            label: String::new(),
            held: Vec::new(),
            counts: (0, 0),
            fronts: HashMap::new(),
            gap_ms: GAP_MS,
            keeping: false,
            following: false,
            kept_at: None,
            lost: false,
            migrated: HashSet::new(),
            asked: HashSet::new(),
            landing: None,
        }
    }
}

/// `YYYY-MM-DD` of a time, in the local zone.
fn day_of(now_ms: i64) -> String {
    util::ymd(&util::local_at_ms(now_ms))
}

impl Pilot {
    /// A tab opened for the item `key` stopped before making its worktree at
    /// `now_ms`, for `why`: whether that makes `FAILS_SKIP` in the last
    /// `FAILS_MS`, which skips the item (and forgets its failures, so one
    /// taken back gets its tries again), a GitHub issue's as a note's.
    pub fn start_failed(&mut self, key: Key, why: &str, now_ms: i64) -> bool {
        // Listed in Next up again at once, `retrying` holding its opening.
        self.recent.remove(&key);
        let fails = self.failed.entry(key.clone()).or_default();
        fails.retain(|(at, _)| now_ms - at < FAILS_MS);
        fails.push((now_ms, why.to_string()));
        let skip = fails.len() >= FAILS_SKIP;
        if skip {
            self.failed.remove(&key);
        }
        skip
    }

    /// The items of the mode's note whose start failed in the last
    /// `RETRY_MS`, by their first lines: listed, not opened yet.
    pub fn retrying(&self, mode: &Mode, now_ms: i64) -> HashSet<String> {
        self.failed
            .iter()
            .filter(|(key, fails)| {
                key.0 == mode.note_id && fails.last().is_some_and(|(at, _)| now_ms - at < RETRY_MS)
            })
            .map(|(key, _)| key.1.clone())
            .collect()
    }

    /// Why each item of the mode's note last failed to start, in the last
    /// `FAILS_MS`, by its first line, for its card in Next up.
    pub fn start_failures(&self, mode: &Mode, now_ms: i64) -> HashMap<String, String> {
        self.failed
            .iter()
            .filter(|(key, _)| key.0 == mode.note_id)
            .filter_map(|(key, fails)| {
                let (at, why) = fails.last()?;
                (now_ms - at < FAILS_MS).then(|| (key.1.clone(), why.clone()))
            })
            .collect()
    }

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
    /// and the note at its path holds.
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
    ) -> Check {
        let mut out = Check::default();
        // A limit found reached is forgotten once the limit changes, before
        // anything else holds the check (paused, away, its slots full), so
        // going back to it looks, and says it, again.
        if self
            .spent
            .as_ref()
            .is_some_and(|(_, limit)| *limit != mode.spend_limit)
        {
            self.spent = None;
        }
        if self
            .weekly
            .is_some_and(|(_, limit)| limit != mode.weekly_limit)
        {
            self.weekly = None;
        }
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
        // Its tabs not landed yet are set to land without asking, or not, as
        // the switch changes (a tab opened meanwhile is set as it opens).
        let land = on && mode.land_without_asking;
        let switched = self.landing.is_some_and(|was| was != land);
        self.landing = Some(land);
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
                    if switched && m.land != land {
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
        // A tab that landed and left holds nothing: its item was ticked
        // before its worktree went (`switchboard item-done`).
        self.held = placed
            .iter()
            .filter(|(_, kind)| *kind != Kind::Left)
            .filter_map(|(m, _)| key_of(m))
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
        let retrying = self.retrying(mode, now_ms);
        let source = self.source.clone();
        let label = self.label.clone();
        let job = |key: String, mtime: i64| Job {
            note: mode.note.clone(),
            repo: mode.repo.clone(),
            ready_only: mode.ready_only,
            with_choices: mode.hands_free,
            skip: skip.clone(),
            retrying: retrying.clone(),
            next: mode.next_here(),
            spend_limit: mode.spend_limit,
            weekly_limit: mode.weekly_limit,
            key,
            mtime,
            sources: source.clone().ok(),
            issues_on: mode.issues_on(),
            label: label.clone(),
        };
        // The item it will take next, read again for mission control.
        let mut skipped: Vec<&String> = skip.iter().collect();
        skipped.sort();
        let key = format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            mode.repo.display(),
            mode.note,
            mtime.unwrap_or(0),
            skipped
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            mode.ready_only,
            mode.hands_free,
            mode.next_here().join("\n"),
            source.as_ref().map_or("none", |s| s.label()),
            mode.issues_on(),
            label
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
        // Each limit holds only while the mode still says it was reached:
        // stopped, set up again or a tab opened since, it is looked at again.
        if mode.spend_limit > 0.0
            && mode.said.contains(SPENT)
            && self.spent.as_ref().is_some_and(|(day, _)| *day == today)
        {
            return out;
        }
        // Held while the week's last reading, read again with the next
        // items, is at the limit and hasn't reset, once a look has said so
        // for that reset and limit: one gone, reset early or under a new
        // limit lets it look again, and so does one not yet said.
        // Today's spend past its limit, read with the next items and not yet
        // said, is looked for first, since it is said first.
        let spend_unsaid = mode.spend_limit > 0.0
            && self.next.spent >= mode.spend_limit
            && !mode.said.contains(SPENT);
        if mode.weekly_limit > 0
            && !spend_unsaid
            && self.next.weekly.is_some_and(|(used, resets_at)| {
                used >= f64::from(mode.weekly_limit)
                    && now_ms < resets_at * 1000
                    && self.weekly == Some((resets_at, mode.weekly_limit))
                    && mode.said.contains(WEEKLY)
            })
        {
            return out;
        }
        // Nothing taken while the repository's tickets give it no work, or
        // give it GitHub issues while Settings keep it from reading them.
        let taking = match &source {
            Ok(Sources::Issues) if !mode.issues_on() => Err(ISSUES_OFF.to_string()),
            other => other.clone(),
        };
        if let Err(why) = &taking {
            if self.source_said != *why {
                self.source_said = why.clone();
                out.said = Some(why.clone());
            }
            return out;
        }
        self.source_said.clear();
        out.source_ok = true;
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
            Found::Weekly { used, resets_at } => {
                self.next.weekly = Some((used, resets_at));
                let key = (resets_at, mode.weekly_limit);
                if self.weekly != Some(key) || !mode.said.contains(WEEKLY) {
                    self.weekly = Some(key);
                    out.said = Some(weekly_said(mode.weekly_limit, resets_at));
                }
            }
            Found::Failed(e) => {
                if e.starts_with("GitHub issues:") && !mode.said.contains(&unread_said(&e)) {
                    out.said = Some(unread_said(&e));
                    // Nothing left is said again once a read succeeds.
                    self.empty_said = false;
                }
                out.failed = Some(e);
            }
            Found::Item(None) => {
                // Said once, and only with none of its tabs open: while some
                // still work, the view's heading says nothing more starts, nor
                // while an item whose start failed waits to be tried again.
                if !self.empty_said && self.counts.1 == 0 && self.retrying(mode, now_ms).is_empty()
                {
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
            held: Vec::new(),
            plans: HashMap::new(),
            priorities: HashMap::new(),
            spent: 0.0,
            weekly: None,
            left: super::eta::Left::default(),
            failed: false,
        };
    }

    /// A read of the queue that failed (GitHub issues unreadable): the queue
    /// as last read stands, read again after `QUEUE_MS` as a good read is.
    /// With no read yet, the queue stays unknown rather than drained.
    /// The queue last read stands only for what it was read for: another
    /// source or setting since leaves it unknown.
    pub fn read_failed(&mut self, key: String, now_ms: i64) {
        self.reading = false;
        // Nothing left is said again once a read succeeds.
        self.empty_said = false;
        if self.next.key != key {
            self.next = Next {
                key,
                ..Next::default()
            };
        }
        self.next.failed = true;
        self.next.at = now_ms;
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
        let read_empty =
            !self.next.key.is_empty() && self.next.text.is_empty() && !self.next.failed;
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
            weekly_limit: mode.weekly_limit,
            weekly: self.next.weekly,
            lands_alone: mode.land_without_asking,
            hands_free: mode.hands_free,
            target: mode.target.clone(),
            drained: (self.empty.is_some() || read_empty) && !self.next.failed && self.counts.1 > 0,
            failing: String::new(),
            way: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::autopilot::place::tests::mine;
    use crate::sidebar::model::TabState;

    #[test]
    fn a_held_item_says_where_each_item_it_waits_on_stands_past_what_the_view_shows() {
        // Nine ready items, then ten held: the first waits on the ninth
        // ready one, on the last held one and on one a tab works; the rest
        // each wait on the one a tab works.
        let mut note = String::from("# Work\n\n- [ ] At work\n");
        for n in 1..=9 {
            note.push_str(&format!("- [ ] Ready {n}\n"));
        }
        note.push_str("- [ ] Held 1\n  After: Ready 9\n  After: Held 10\n  After: At work\n");
        for n in 2..=10 {
            note.push_str(&format!("- [ ] Held {n}\n  After: At work\n"));
        }
        let all = crate::cmd::worktree_from_note::items(&note);
        // The queue leaves out the item a tab works and those held.
        let (held, free): (Vec<Item>, Vec<Item>) =
            all.into_iter().partition(|i| i.waits_on.is_some());
        let queue: Vec<Item> = free.into_iter().filter(|i| i.text != "At work").collect();
        // The last held one is left out of what the queue holds, as an
        // unplanned item is with only planned ones taken.
        let held: Vec<Item> = held.into_iter().filter(|i| i.text != "Held 10").collect();
        let shown = held_shown(&note, &queue, held);
        assert_eq!(shown.len(), QUEUE_SHOWN, "the view's share of them");
        assert_eq!(shown[0].text, "Held 1");
        assert_eq!(shown[0].waits_on, "Ready 9");
        assert_eq!(
            shown[0].after,
            [
                ("Ready 9".to_string(), Stands::Queued(9)),
                ("Held 10".to_string(), Stands::Blocked),
                ("At work".to_string(), Stands::Open),
            ],
            "from the whole queue and the note itself, not the eight shown"
        );
        assert_eq!(shown[1].after, [("At work".to_string(), Stands::Open)]);
    }

    #[test]
    fn a_start_that_fails_is_tried_again_and_the_third_in_an_hour_skips_the_item() {
        let mut pilot = Pilot::default();
        let mode = Mode {
            note_id: "n-1".into(),
            ..Mode::default()
        };
        let key = || ("n-1".to_string(), "Item".to_string());
        let hour = FAILS_MS;
        pilot.took("n-1", "Item", 0);
        assert!(!pilot.start_failed(key(), "offline", 0));
        assert!(
            !pilot.skip(&mode, 1).contains("Item"),
            "listed in Next up again at once"
        );
        assert!(
            pilot.retrying(&mode, 1).contains("Item"),
            "but not opened yet"
        );
        assert!(
            pilot.retrying(&mode, RETRY_MS).is_empty(),
            "until a later check"
        );
        assert_eq!(
            pilot
                .start_failures(&mode, 1)
                .get("Item")
                .map(String::as_str),
            Some("offline"),
            "its card says why"
        );
        assert!(!pilot.start_failed(key(), "offline", 1_000));
        assert!(
            !pilot.start_failed(key(), "offline", hour + 500),
            "the first is over an hour old by the third"
        );
        assert!(
            pilot.start_failed(key(), "still offline", hour + 900),
            "the third in an hour skips it"
        );
        assert!(
            pilot.start_failures(&mode, hour + 900).is_empty(),
            "and its tries start over once it is taken back"
        );
        assert!(
            pilot.start_failures(&mode, 3 * hour).is_empty(),
            "an old failure says nothing"
        );
    }

    #[test]
    fn each_source_alone_gives_only_its_own_items() {
        let dir = std::env::temp_dir().join(format!("sb-pilot-issues-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Work.md"), "# Work\n\n- [ ] From the note\n").unwrap();
        let issue = |n: u64| {
            let body = if n == 5 {
                "## Decision needed\n\n- [ ] Option A, once boards 21 and 22 are reviewed\n- [ ] Option B\n"
            } else {
                ""
            };
            serde_json::json!({ "repo": "acme/widgets", "number": n, "title": format!("Issue {n}"),
                "body": body, "url": "" })
        };
        // A fresh read kept, so no request is made.
        std::fs::write(
            dir.join(super::super::issues::FILE),
            serde_json::json!({
                "at": util::now_ms(), "dir": dir.to_string_lossy(), "label": "autopilot",
                "issues": [issue(4), issue(5), issue(9)],
            })
            .to_string(),
        )
        .unwrap();
        let mut job = Job {
            note: "Work.md".into(),
            repo: dir.clone(),
            ready_only: false,
            with_choices: false,
            skip: ["acme/widgets#9: Issue 9".to_string()].into(),
            retrying: HashSet::new(),
            next: Vec::new(),
            spend_limit: 0.0,
            weekly_limit: 0,
            key: String::new(),
            mtime: 0,
            sources: Some(Sources::Issues),
            issues_on: true,
            label: "autopilot".into(),
        };
        let texts = |job: &Job| -> Vec<String> {
            job.items(&dir, &dir)
                .unwrap()
                .into_iter()
                .map(|i| i.text)
                .collect()
        };
        assert_eq!(texts(&job), ["acme/widgets#4: Issue 4"]);
        let (_, held) = job.items_and_held(&dir, &dir).unwrap();
        assert_eq!(held.len(), 1, "a design review holds issue 5");
        assert_eq!(
            held[0].waits_on.as_deref(),
            Some("your review of boards 21 and 22")
        );
        assert_eq!(
            job.first(&dir, &dir).unwrap().map(|i| i.text).as_deref(),
            Some("acme/widgets#4: Issue 4")
        );
        job.sources = Some(Sources::Note);
        assert_eq!(texts(&job), ["From the note"]);
        assert!(job.items_and_held(&dir, &dir).unwrap().1.is_empty());
        assert!(
            job.priorities(&dir).is_empty(),
            "no issue read for the note"
        );
        // GitHub off: issues alone take nothing, never the note's items.
        job.sources = Some(Sources::Issues);
        job.issues_on = false;
        assert!(texts(&job).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // A job taking the issues kept in `dir` alone.
    fn issues_job(dir: &Path) -> Job {
        Job {
            note: String::new(),
            repo: dir.to_path_buf(),
            ready_only: false,
            with_choices: false,
            skip: HashSet::new(),
            retrying: HashSet::new(),
            next: Vec::new(),
            spend_limit: 0.0,
            weekly_limit: 0,
            key: String::new(),
            mtime: 0,
            sources: Some(Sources::Issues),
            issues_on: true,
            label: "autopilot".into(),
        }
    }

    // The issues kept in `dir` as a fresh read, so no request is made.
    fn keep_issues(dir: &Path, issues: Vec<serde_json::Value>) {
        std::fs::write(
            dir.join(super::super::issues::FILE),
            serde_json::json!({
                "at": util::now_ms(), "dir": dir.to_string_lossy(), "label": "autopilot",
                "issues": issues,
            })
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn a_priority_1_issue_comes_before_an_older_priority_3_one() {
        let dir = std::env::temp_dir().join(format!("sb-pilot-priority-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let issue = |n: u64, priority: Option<u8>| {
            let mut i = serde_json::json!({ "repo": "acme/widgets", "number": n,
                "title": format!("Issue {n}"), "body": "", "url": "" });
            if let Some(p) = priority {
                i["priority"] = serde_json::json!(p);
            }
            i
        };
        // Oldest first, as the search answers; one kept with no priority
        // counts as 3.
        keep_issues(
            &dir,
            vec![
                issue(2, None),
                issue(3, Some(3)),
                issue(7, Some(1)),
                issue(8, Some(4)),
            ],
        );
        let texts = |job: &Job| -> Vec<String> {
            job.items(&dir, &dir)
                .unwrap()
                .into_iter()
                .map(|i| i.text)
                .collect()
        };
        let mut job = issues_job(&dir);
        assert_eq!(
            texts(&job),
            [
                "acme/widgets#7: Issue 7",
                "acme/widgets#2: Issue 2",
                "acme/widgets#3: Issue 3",
                "acme/widgets#8: Issue 8",
            ]
        );
        // Put next: first of all the issues, by its reference whatever its
        // title says now.
        job.next = vec!["acme/widgets#3: An older title".to_string()];
        let first = texts(&job);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(first[0], "acme/widgets#3: Issue 3");
        assert_eq!(first[1], "acme/widgets#7: Issue 7");
    }

    #[test]
    fn an_issue_blocked_by_an_open_one_waits_and_goes_once_it_closes() {
        let dir = std::env::temp_dir().join(format!("sb-pilot-blocked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let issue = |n: u64, blocked_by: &[&str]| {
            serde_json::json!({ "repo": "acme/widgets", "number": n,
                "title": format!("Issue {n}"), "body": "", "url": "", "blocked_by": blocked_by })
        };
        keep_issues(
            &dir,
            vec![issue(3, &[]), issue(4, &["acme/widgets#3", "acme/other#9"])],
        );
        let job = issues_job(&dir);
        let (queue, held) = job.items_and_held(&dir, &dir).unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].waits_on.as_deref(), Some("acme/widgets#3"));
        // The card names the one in line by its first line and its place.
        let shown = held_shown("", &queue, held);
        assert_eq!(
            shown[0].after,
            [
                ("acme/widgets#3: Issue 3".to_string(), Stands::Queued(1)),
                ("acme/other#9".to_string(), Stands::Open),
            ]
        );
        // Both closed: the next read lists no blockers, and it goes.
        keep_issues(&dir, vec![issue(4, &[])]);
        let (queue, held) = job.items_and_held(&dir, &dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(held.is_empty());
        assert_eq!(queue[0].text, "acme/widgets#4: Issue 4");
    }

    #[test]
    fn with_the_first_item_held_find_opens_the_second() {
        let dir = std::env::temp_dir().join(format!("sb-pilot-held-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Work.md"),
            "# Work\n\n- [ ] Builds on it\n  After: The base\n- [ ] Free\n\n## Later\n- [ ] The base\n",
        )
        .unwrap();
        let job = Job {
            note: "Work.md".into(),
            repo: dir.clone(),
            ready_only: false,
            with_choices: false,
            skip: HashSet::new(),
            retrying: HashSet::new(),
            next: Vec::new(),
            spend_limit: 0.0,
            weekly_limit: 0,
            key: String::new(),
            mtime: 0,
            sources: Some(Sources::Note),
            issues_on: true,
            label: String::new(),
        };
        let found = job.find(&dir, &dir, 0);
        let _ = std::fs::remove_dir_all(&dir);
        match found {
            Found::Item(Some(item)) => assert_eq!(item.text, "Free"),
            other => panic!("expected the free item, found {other:?}"),
        }
    }

    #[test]
    fn a_limit_s_sentence_goes_wherever_it_is_said() {
        let weekly = format!("{WEEKLY} 85% reached, resets in 1h 0m");
        assert_eq!(unsay(&weekly, WEEKLY), "");
        assert_eq!(
            unsay(
                &format!("Skipped One: wt refused it, taken. {weekly}"),
                WEEKLY
            ),
            "Skipped One: wt refused it, taken"
        );
        assert_eq!(unsay(&format!("{weekly}. Later"), WEEKLY), "Later");
        assert_eq!(
            unsay("Nothing left to take in Work", WEEKLY),
            "Nothing left to take in Work"
        );
    }

    #[test]
    fn find_stops_at_the_weekly_limit_while_the_reading_is_current() {
        let dir = std::env::temp_dir().join(format!("sb-pilot-weekly-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let usage = crate::usage::sessions::usage_dir(&dir);
        std::fs::create_dir_all(&usage).unwrap();
        std::fs::write(
            dir.join("Work.md"),
            "# Work

- [ ] One
",
        )
        .unwrap();
        let now_ms = util::now_ms();
        let now = now_ms.div_euclid(1000);
        let limits = |used: f64, resets_at: i64| {
            std::fs::write(
                usage.join("limits"),
                serde_json::json!({
                    "seven_day": { "used_percentage": used, "resets_at": resets_at },
                })
                .to_string(),
            )
            .unwrap();
        };
        let mut job = Job {
            note: "Work.md".into(),
            repo: dir.clone(),
            ready_only: false,
            with_choices: false,
            skip: HashSet::new(),
            retrying: HashSet::new(),
            next: Vec::new(),
            spend_limit: 0.0,
            weekly_limit: 80,
            key: String::new(),
            mtime: 0,
            sources: Some(Sources::Note),
            issues_on: true,
            label: String::new(),
        };
        let item = |found: Found| matches!(found, Found::Item(Some(ref i)) if i.text == "One");

        assert!(
            item(job.find(&dir, &dir, now_ms)),
            "no reading holds nothing"
        );
        limits(85.0, now + 3_600);
        assert_eq!(
            job.find(&dir, &dir, now_ms),
            Found::Weekly {
                used: 85.0,
                resets_at: now + 3_600
            }
        );
        limits(70.0, now + 3_600);
        assert!(item(job.find(&dir, &dir, now_ms)), "under the limit");
        limits(85.0, now - 5);
        assert!(
            item(job.find(&dir, &dir, now_ms)),
            "a reading past its reset"
        );
        limits(85.0, now + 3_600);
        job.weekly_limit = 0;
        assert!(item(job.find(&dir, &dir, now_ms)), "no weekly limit");

        // Past both, the spend limit is said first.
        job.weekly_limit = 80;
        job.spend_limit = 1.0;
        let today = util::ymd(&util::local_at_ms(now_ms));
        std::fs::write(usage.join("spend-a"), format!("{today}	5.0000	5	300")).unwrap();
        assert_eq!(job.find(&dir, &dir, now_ms), Found::Spent);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn at_the_weekly_limit_it_says_so_once_and_holds_while_the_reading_says_so() {
        let mut w = World::new();
        w.mode.weekly_limit = 85;
        let check = |w: &mut World, at: i64| {
            w.pilot
                .check(&w.mode, true, None, &[], false, at, Some(w.mtime), Some(ID))
        };
        // A look answered, what it says kept in the mode as the server keeps it.
        let reach = |w: &mut World, used: f64, resets_at: i64, at: i64| {
            w.pilot.cancel();
            let said = w
                .pilot
                .opened(&w.mode, Found::Weekly { used, resets_at }, w.mtime, at)
                .said;
            if let Some(said) = &said {
                w.mode.said = said.clone();
            }
            said
        };
        let now = w.now;
        let job = check(&mut w, now)
            .open
            .expect("a look, which reads the week");
        assert_eq!(job.weekly_limit, 85);
        let resets_at = w.now.div_euclid(1000) + 3_600;
        assert_eq!(
            reach(&mut w, 90.0, resets_at, now).as_deref(),
            Some(
                format!(
                    "Stopped opening tabs: weekly limit 85% reached, resets {}",
                    util::local_at_ms(resets_at * 1000).strftime("%a %-I:%M %p")
                )
                .as_str()
            )
        );
        let later = w.now + 60_000;
        assert_eq!(check(&mut w, later).open, None, "held by the reading");

        // A reading past it read with the next items, not yet said: looked
        // for, so the look says it.
        w.pilot.next.weekly = Some((92.0, resets_at + 60));
        assert!(check(&mut w, later).open.is_some(), "not yet said");
        assert!(reach(&mut w, 92.0, resets_at + 60, later).is_some());
        assert_eq!(check(&mut w, later).open, None, "said, so held");
        reach(&mut w, 90.0, resets_at, later);

        // Gone from what the mode says (stopped and started again, a tab
        // opened): looked at, and said, again.
        w.mode.said.clear();
        assert!(check(&mut w, later).open.is_some(), "no longer said");
        assert!(reach(&mut w, 90.0, resets_at, later).is_some());

        // The reading read again: gone, it holds nothing.
        w.pilot.next.weekly = None;
        assert!(check(&mut w, later).open.is_some(), "no reading");
        assert_eq!(reach(&mut w, 90.0, resets_at, later), None, "said once");
        w.pilot.next.weekly = Some((3.0, resets_at + 7 * 86_400));
        assert!(check(&mut w, later).open.is_some(), "the week reset early");
        reach(&mut w, 90.0, resets_at, later);

        // Changed and changed back while paused: looked at and said again.
        w.mode.paused = true;
        w.mode.weekly_limit = 95;
        assert_eq!(check(&mut w, later).open, None, "paused");
        w.mode.weekly_limit = 85;
        check(&mut w, later);
        w.mode.paused = false;
        w.mode.weekly_limit = 95;
        assert!(
            check(&mut w, later).open.is_some(),
            "a limit past the reading"
        );
        w.mode.weekly_limit = 85;
        assert!(
            reach(&mut w, 90.0, resets_at, later).is_some(),
            "said again at the old limit"
        );
        assert_eq!(check(&mut w, later).open, None, "held");
        // Today's spend past a spend limit too, not yet said: looked for.
        w.mode.spend_limit = 5.0;
        w.pilot.next.spent = 6.0;
        assert!(check(&mut w, later).open.is_some(), "the spend limit first");
        w.pilot.cancel();
        w.mode.spend_limit = 0.0;
        let reset = resets_at * 1000 + 60_000;
        assert!(check(&mut w, reset).open.is_some(), "it resets");
    }

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
                    after: Vec::new(),
                    waits_on: None,
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
    fn what_it_said_of_the_source_goes_and_the_rest_stays() {
        let said = format!("Skipped X: refused. {ISSUES_OFF}");
        assert_eq!(unsay_source(&said), "Skipped X: refused");
        assert_eq!(
            unsay_source("acme/app keeps its work in Linear, which autopilot can't take"),
            ""
        );
        assert_eq!(
            unsay_source("Nothing left to take in Work"),
            "Nothing left to take in Work"
        );
    }

    #[test]
    fn it_opens_nothing_while_away_or_past_the_spend_limit_and_says_a_missing_note_once() {
        let mut w = World::new();
        let check = w.pilot.check(
            &w.mode,
            true,
            None,
            &[],
            true,
            w.now,
            Some(w.mtime),
            Some(ID),
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
        );
        let job = check.open.expect("a look, which sums the spend");
        assert_eq!(job.spend_limit, 5.0);
        let opened = w.pilot.opened(&w.mode, Found::Spent, w.mtime, w.now);
        assert_eq!(
            opened.said.as_deref(),
            Some("Stopped opening tabs: today's Claude spend passed $5")
        );
        w.mode.said = opened.said.unwrap_or_default();
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
        );
        assert!(check.open.is_some(), "a new limit is looked at again");
        w.pilot.cancel();
        w.mode.spend_limit = 5.0;
        let check = w.pilot.check(
            &w.mode,
            true,
            None,
            &[],
            false,
            later,
            Some(w.mtime),
            Some(ID),
        );
        assert!(check.open.is_some(), "and so is the old one again");
        w.pilot.cancel();
        // The claims brought to IDs, as the first check asked.
        w.pilot.kept(&Kept::default(), false, later);

        // Gone from its path, and looked for by its ID in vain.
        let missing = |w: &mut World, at: i64| {
            let check = w
                .pilot
                .check(&w.mode, true, None, &[], false, at, None, None);
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
        assert!(
            marks(&mut w, &tabs, true).is_empty(),
            "the switch unchanged: a tab its menu turned off stays off"
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
            w.pilot
                .check(&w.mode, true, None, &[], false, *at, mtime, here)
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

    // A tab that landed and left holds nothing: item-done ticked its item
    // before its worktree went, so the queue no longer lists it.
    #[test]
    fn a_landed_tab_holds_its_item_only_while_it_is_open_in_its_worktree() {
        let mut pilot = Pilot::default();
        let mode = Mode {
            on: true,
            note: "Work.md".into(),
            repo: "/repo".into(),
            ..Mode::default()
        };
        let now = 1_700_000_000_000_i64;
        let mut tab = mine(1, (now / 1000) as f64 - 3_600.0, TabState::Idle, false);
        let mut check = |tabs: &[Mine], at: i64| {
            pilot.check(&mode, true, None, tabs, false, at, Some(1), None);
            pilot.skip(&mode, at)
        };
        assert!(
            check(std::slice::from_ref(&tab), now).contains("item 1"),
            "held while its tab works in its worktree"
        );
        tab.was_in = true;
        assert!(
            !check(std::slice::from_ref(&tab), now + 1_000).contains("item 1"),
            "not once it has left"
        );
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
                )
                .marks
        };
        assert_eq!(marks(&tab, now), [Mark::Landed(TabId(1), secs as i64)]);
        tab.landed = Some(secs);
        assert_eq!(marks(&tab, now + 1_000), [Mark::Left(TabId(1))]);
    }
}
