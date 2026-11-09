//! Where each of the mode's tabs is (autopilot.js's `placeOf` and
//! `stateOf`): busy, working in its worktree or still opening; waiting on
//! you, which frees its slot but still counts toward the cap; landing, out
//! of its worktree but not back in a main clone, as while queue-and-clean
//! removes the worktree its pane is still in, which holds its place and its
//! item as waiting does; or landed, back in the main clone once
//! queue-and-clean has landed it, which leaves the mode, and what then
//! becomes of it (`pilot`'s after landing). A directory that is gone counts
//! for nothing, since a removed worktree under the main clone's
//! `.claude/worktrees` would otherwise read as the main clone itself. Worked
//! out from plain data, which `serve` gathers from the server's tabs, so the
//! rules are tested without one.

use std::path::{Path, PathBuf};

use crate::mux::TabId;
use crate::sidebar::model::TabState;
use crate::sources::landing::git_of;

/// A tab not in its worktree yet counts as busy this long, in seconds,
/// while `wtn` makes it; past it, `wtn` failed, and the tab waits on you.
pub const OPENING_S: f64 = 10.0 * 60.0;
/// A tab counts as busy this long after it opened whatever it shows, since
/// Claude takes a moment to start in its worktree.
pub const STARTING_S: f64 = 2.0 * 60.0;

/// Where one of the mode's tabs is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Working in its worktree, or still opening: it takes a slot.
    Busy,
    /// Waiting on you (a question, a turn ended, or `wtn` failed): it frees
    /// its slot but counts toward the cap.
    Waiting,
    /// Out of its worktree but not back in a main clone, as while
    /// queue-and-clean removes it: as waiting, its item still held.
    Landing,
    /// Back in the main clone once landed: it leaves the mode.
    Left,
}

/// One of the mode's tabs, as a check needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct Mine {
    pub tab: TabId,
    /// When it opened, in seconds (`@autopilot`).
    pub since: f64,
    /// Whether it was seen in its worktree before (`@autopilot_in`).
    pub was_in: bool,
    /// The first line of the item it was opened for (`@autopilot_item`).
    pub item: Option<String>,
    /// The ID of that item's note (`@autopilot_note`); `None` on a tab
    /// opened before tabs kept it.
    pub note: Option<String>,
    /// The note the tab is linked to, by its path in the notes directory
    /// (`@tab_note`), which names the note of an older tab's item.
    pub tab_note: Option<String>,
    /// What its Claude is doing: the tab's, or, while a tool stands in for
    /// Claude's pane (M-d's diff), the pane parked behind it.
    pub state: TabState,
    /// Whether it is in a worktree of the note's repository now.
    pub in_worktree: bool,
    /// Whether it is in a main clone now (any repository's).
    pub in_main: bool,
    /// When it was found back in a main clone once landed
    /// (`@autopilot_left`), in seconds.
    pub landed: Option<f64>,
    /// Whether you have had it in front since it landed (`@autopilot_seen`).
    pub seen: bool,
    /// Whether it is in front on a terminal now.
    pub front: bool,
    /// Whether a tool stands in for one of its panes (M-d's diff, lazygit).
    pub stand_in: bool,
    /// Whether its Claude's pane is in a main clone (a directory still
    /// there), idle: landed and moved back by queue-and-clean.
    pub home: bool,
    /// Whether its Claude has been prompted since it landed: a state begun
    /// after then, which makes it yours.
    pub prompted_since: bool,
    /// Whether it is marked to land without asking (`@autopilot_land`).
    pub land: bool,
    /// Whether it is marked hands-free (`@autopilot_handsfree`).
    pub hands_free: bool,
}

/// Whether the first of a tab's directories that is in a git repository is
/// a main clone, read from git's files.
pub fn in_main_clone(dirs: &[PathBuf]) -> bool {
    dirs.iter()
        .filter(|d| d.exists())
        .find_map(|d| git_of(d))
        .is_some_and(|(root, common)| {
            common.file_name().is_some_and(|n| n == ".git") && common.parent() == Some(&root)
        })
}

/// Whether the first of a tab's directories that is in a git repository is
/// a worktree of `repo` other than `repo` itself, the main clone. Read from
/// git's files, with no process.
pub fn in_worktree_of(dirs: &[PathBuf], repo: &Path) -> bool {
    let Some((root, common)) = dirs.iter().filter(|d| d.exists()).find_map(|d| git_of(d)) else {
        return false;
    };
    let main = if common.file_name().is_some_and(|n| n == ".git") {
        common.parent().map(Path::to_path_buf)
    } else {
        Some(root.clone())
    };
    root != repo && main.as_deref() == Some(repo)
}

/// Where the tab is at `now` (seconds). A tab waiting on you frees its slot
/// at once; one idle or done counts as busy for `STARTING_S`, while its
/// Claude may not have started yet.
pub fn kind_of(mine: &Mine, now: f64) -> Kind {
    let age = now - mine.since;
    if mine.in_worktree {
        if mine.state.busy() || (mine.state != TabState::Waiting && age < STARTING_S) {
            Kind::Busy
        } else {
            Kind::Waiting
        }
    } else if mine.was_in && mine.in_main {
        Kind::Left
    } else if mine.was_in {
        Kind::Landing
    } else if age < OPENING_S {
        Kind::Busy
    } else {
        Kind::Waiting
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::fs;

    pub fn mine(tab: u32, since: f64, state: TabState, in_worktree: bool) -> Mine {
        Mine {
            tab: TabId(tab),
            since,
            was_in: in_worktree,
            item: Some(format!("item {tab}")),
            note: None,
            tab_note: None,
            state,
            in_worktree,
            in_main: !in_worktree,
            landed: None,
            seen: false,
            front: false,
            stand_in: false,
            home: false,
            prompted_since: false,
            land: false,
            hands_free: false,
        }
    }

    // test/tmux-sidebar.test.js's "a tab of autopilot still
    // counts as working while M-d or lazygit stands in for its Claude",
    // the parked pane's state given as the tab's by `serve`.
    #[test]
    fn a_tab_counts_by_its_claude_its_age_and_where_it_is() {
        let now = 10_000.0;
        let old = now - 600.0;
        let at = |state, since, inside| kind_of(&mine(1, since, state, inside), now);
        assert_eq!(
            at(TabState::Background, old, true),
            Kind::Busy,
            "background work counts"
        );
        assert_eq!(at(TabState::Waiting, old, true), Kind::Waiting);
        assert_eq!(
            at(TabState::Idle, old, true),
            Kind::Waiting,
            "idle once started"
        );
        assert_eq!(
            at(TabState::Idle, now, true),
            Kind::Busy,
            "a tab just opened counts as busy while its Claude starts"
        );
        assert_eq!(
            at(TabState::Waiting, now, true),
            Kind::Waiting,
            "but one waiting on you frees its slot at once"
        );
        assert_eq!(
            at(TabState::Idle, now - 60.0, false),
            Kind::Busy,
            "still opening"
        );
        assert_eq!(
            at(TabState::Idle, now - 900.0, false),
            Kind::Waiting,
            "wtn failed: it never reached its worktree"
        );
        let mut landed = mine(1, old, TabState::Idle, false);
        landed.was_in = true;
        assert_eq!(kind_of(&landed, now), Kind::Left, "back in the main clone");
        landed.in_main = false;
        assert_eq!(
            kind_of(&landed, now),
            Kind::Landing,
            "out of its worktree, its directory gone, and not back yet"
        );
    }

    #[test]
    fn a_worktree_of_the_repository_is_read_from_gits_files() {
        let dir = std::env::temp_dir().join(format!("sb-autopilot-place-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let repo = dir.join("repo");
        let tree = repo.join(".claude/worktrees/x");
        fs::create_dir_all(repo.join(".git/worktrees/x")).unwrap();
        fs::create_dir_all(tree.join("src")).unwrap();
        fs::write(
            tree.join(".git"),
            format!("gitdir: {}\n", repo.join(".git/worktrees/x").display()),
        )
        .unwrap();
        fs::write(repo.join(".git/worktrees/x/commondir"), "../..\n").unwrap();
        let repo = repo.canonicalize().unwrap();
        assert!(in_worktree_of(&[tree.join("src")], &repo));
        assert!(
            !in_worktree_of(std::slice::from_ref(&repo), &repo),
            "the main clone is no worktree"
        );
        assert!(
            !in_worktree_of(std::slice::from_ref(&dir), &repo),
            "nor a directory in no repository"
        );
        assert!(in_main_clone(std::slice::from_ref(&repo)));
        let gone = repo.join(".claude/worktrees/gone");
        assert!(
            !in_main_clone(std::slice::from_ref(&gone)),
            "a removed worktree under the main clone isn't the main clone"
        );
        assert!(
            !in_main_clone(&[tree.join("src")]),
            "a worktree is no main clone"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
