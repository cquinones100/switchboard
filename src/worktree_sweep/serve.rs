//! Landed worktrees swept on the server (src/worktree_sweep): a repository
//! swept when a tab in one of its worktrees closes, and each repository a
//! tab has been in once every `RESTING`, after fetching its default branch
//! (only while Settings' GitHub is on). The sweep runs off the server's
//! task, since it runs git, in three steps (fetch, find, remove): the
//! server hands the find the directories its panes have open, removes what
//! it found one worktree at a time, each checked against the panes again
//! just before, since one may have been opened meanwhile, and says what
//! went once the last is done. No Claude is started. Settings'
//! `sweep_landed_worktrees` off, nothing is swept, and Reopen's menu still
//! removes a worktree by hand.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::{Ask, RESTING};
use crate::mux::server::{Event, Server};
use crate::settings;

/// The setting that turns the sweep on.
pub const SETTING: &str = "sweep_landed_worktrees";

/// Whether this server sweeps: the setting on, and not a `switchboard try`'s
/// server (`SWITCHBOARD_CHECKOUT` set in its environment), which knows none
/// of the real workspace's tabs, whatever an older try home's settings
/// say.
fn sweeps() -> bool {
    let in_a_try = std::env::var("SWITCHBOARD_CHECKOUT").is_ok_and(|c| !c.is_empty());
    settings::is_on(SETTING) && !in_a_try
}

/// How often the tabs' repositories are looked at again.
const LOOK_EVERY: Duration = Duration::from_secs(60);

/// What the sweep keeps on the server: the main clones tabs have been in
/// since it started, when each was last swept with a fetch, the sweeps
/// running, and when the tabs' repositories were last looked at.
#[derive(Debug, Default)]
pub struct Sweep {
    repos: HashSet<PathBuf>,
    swept: HashMap<PathBuf, Instant>,
    running: HashSet<PathBuf>,
    looked: Option<Instant>,
}

/// What the work off the task answers.
pub enum SweepEvent {
    /// The tabs' directories' main clones, found.
    Repos(Vec<PathBuf>),
    /// A repository's default branch fetched.
    Fetched(PathBuf),
    /// A repository's landed worktrees found.
    Found { main: PathBuf, trees: Vec<PathBuf> },
    /// One worktree removed (or kept, when git refused): those still to
    /// remove, and those removed so far.
    Removed {
        main: PathBuf,
        left: Vec<PathBuf>,
        removed: Vec<PathBuf>,
    },
}

impl Server {
    /// On every tick: once a minute, the main clones of the tabs'
    /// directories found; each not swept for `RESTING` swept, after a fetch.
    pub fn sweep_tick(&mut self) {
        if !sweeps() {
            return;
        }
        if self
            .sweep
            .looked
            .is_none_or(|at| at.elapsed() >= LOOK_EVERY)
        {
            self.sweep.looked = Some(Instant::now());
            let dirs: Vec<PathBuf> = self
                .sessions
                .iter()
                .flat_map(|s| s.tabs.clone())
                .filter_map(|t| self.tab_path(t).map(PathBuf::from))
                .collect();
            let tx = self.tx.clone();
            tokio::task::spawn_blocking(move || {
                let mut repos: Vec<PathBuf> = dirs
                    .iter()
                    .filter_map(|d| super::main_clone_of(d))
                    .collect();
                repos.sort();
                repos.dedup();
                let _ = tx.send(Event::Sweep(SweepEvent::Repos(repos)));
            });
        }
        let due: Vec<PathBuf> = self
            .sweep
            .repos
            .iter()
            .filter(|r| {
                self.sweep
                    .swept
                    .get(*r)
                    .is_none_or(|at| at.elapsed() >= RESTING)
            })
            .cloned()
            .collect();
        for main in due {
            self.sweep.swept.insert(main.clone(), Instant::now());
            self.sweep_start(main, settings::is_on("github"));
        }
    }

    /// A tab closed in `dir`: its repository swept, with no fetch.
    pub fn sweep_after_close(&mut self, dir: Option<PathBuf>) {
        if !sweeps() {
            return;
        }
        let Some(main) = dir.as_deref().and_then(super::main_clone_of) else {
            return;
        };
        self.sweep.repos.insert(main.clone());
        self.sweep_start(main, false);
    }

    /// A repository swept off the task, unless a sweep of it still runs:
    /// the default branch fetched first when `fetch`.
    fn sweep_start(&mut self, main: PathBuf, fetch: bool) {
        if !self.sweep.running.insert(main.clone()) {
            return;
        }
        if !fetch {
            self.sweep_find(main);
            return;
        }
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            super::fetch(&main);
            let _ = tx.send(Event::Sweep(SweepEvent::Fetched(main)));
        });
    }

    /// The directories the panes have open: where its Claude says it works,
    /// where the program in front runs (a Claude whose Bash tool moved
    /// elsewhere still runs where it started), where the pane's own shell
    /// is under it, and its tab's `@tab_dir`.
    fn sweep_open(&self) -> Vec<PathBuf> {
        self.panes
            .iter()
            .flat_map(|(p, slot)| {
                [
                    self.pane_dir_as_said(*p),
                    self.pane_own_dir_as_said(*p),
                    slot.pane.cwd(),
                    slot.pane.pid().and_then(crate::mux::procinfo::cwd_of),
                ]
            })
            .flatten()
            .collect()
    }

    /// The repository's landed worktrees found off the task.
    fn sweep_find(&mut self, main: PathBuf) {
        let ask = Ask {
            main: main.clone(),
            open: self.sweep_open(),
        };
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let trees = super::find(&ask);
            let _ = tx.send(Event::Sweep(SweepEvent::Found { main, trees }));
        });
    }

    pub fn sweep_event(&mut self, event: SweepEvent) {
        match event {
            SweepEvent::Repos(repos) => self.sweep.repos.extend(repos),
            // Turned off meanwhile: nothing more is done.
            SweepEvent::Fetched(main) | SweepEvent::Found { main, .. } if !sweeps() => {
                self.sweep.running.remove(&main);
            }
            SweepEvent::Fetched(main) => self.sweep_find(main),
            SweepEvent::Found { main, trees } => self.sweep_next(main, trees, Vec::new()),
            SweepEvent::Removed {
                main,
                left,
                removed,
            } => self.sweep_next(main, left, removed),
        }
    }

    /// The next worktree found removed off the task, one at a time, each
    /// checked again just before, against the panes as they are and every
    /// rule of `landed_worktrees`, since one may have been opened, changed
    /// and closed while git looked or removed another; once none is
    /// left (or the setting was turned off), what went is said.
    fn sweep_next(&mut self, main: PathBuf, mut left: Vec<PathBuf>, removed: Vec<PathBuf>) {
        let open = self.sweep_open();
        left.retain(|t| !super::is_open(t, &open));
        if left.is_empty() || !sweeps() {
            self.sweep.running.remove(&main);
            if removed.is_empty() {
                return;
            }
            self.say(None, &super::words(&removed));
            self.mission.revived.retain(|r| r.tree.is_dir());
            self.mission_worktrees(true);
            self.mission_revived(true);
            return;
        }
        let tree = left.remove(0);
        let ask = Ask {
            main: main.clone(),
            open,
        };
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let mut removed = removed;
            if super::still_landed(&ask, &tree) {
                removed.extend(super::remove(&main, std::slice::from_ref(&tree)));
            }
            let _ = tx.send(Event::Sweep(SweepEvent::Removed {
                main,
                left,
                removed,
            }));
        });
    }
}
