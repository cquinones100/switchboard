//! The sidebar: every tab of the session on the left of the screen, most in
//! need of you first, what each is doing, and the notifications under them
//! (bin/tmux-sidebar and lib/sidebar/). tmux had no sidebar of its own, so a
//! daemon kept a narrow pane on the left of every window running a client
//! that drew what the daemon sent it; here the server draws it beside the
//! panes, in the columns `Session::main_area` leaves to its left, from its
//! own state, so there is no pane, no client and no socket to keep in step.
//!
//! - `text`: text measured as a terminal draws it, the badges' looks.
//! - `claude`: what each Claude session is doing, from Claude Code's files.
//! - `model`: each tab, its group and its stage, in the order of need.
//! - `render`: the sidebar's lines, a plain function of the model.
//! - `walk`: M-a's walk through Needs you, and M-A's through Idle.
//! - `notices`: the Notifications window's list, in a tab of its own.
//! - `history`: every notification of the last two weeks, by day, for
//!   mission control's Notifications view.
//! - `peek`: a look at a tab without going there.
//! - `config`: sidebar.json.
//! - `serve`: the server's side: reading, drawing, focus, keys, the mouse.
//!
//! Mission control takes the same model (`Server::sidebar_tabs`, `model`'s
//! `grouped` and `staged`, a tab's `turn` and `claude`), so its cards and
//! the sidebar never disagree about a tab.

pub mod claude;
pub mod config;
pub mod history;
pub mod model;
pub mod notices;
pub mod peek;
pub mod render;
mod serve;
pub mod walk;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use crate::mux::server::ClientId;
use crate::mux::{PaneId, TabId};

pub use switchboard_ui::text;

pub use model::{Rules, Stage, TabModel, TabState};
pub use render::{Filter, Owner};
pub use serve::{KEYS, filed_state};

/// What the server's timers tell the sidebar.
#[derive(Debug)]
pub enum SidebarEvent {
    /// Time for the spinner to turn.
    Spin,
    /// The notifications' history read from its files as the server
    /// started.
    HistoryRead(std::collections::BTreeMap<String, Vec<history::Entry>>),
    /// The history's changed days written.
    HistoryWritten,
}

/// What was read for one pane at the last refresh: the Claude conversation
/// in front of it, what that runs in the background, its last turn, and the
/// program in front.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaneRead {
    pub session: Option<claude::SessionRef>,
    pub transcript: Option<PathBuf>,
    pub work: claude::Work,
    pub turn: Option<claude::Turn>,
    /// When its transcript last changed, in seconds: when an idle Claude
    /// last did anything.
    pub said_at: Option<i64>,
    pub command: Option<String>,
    /// Whether it runs Claude (`agent::sessions`): a session in front, or
    /// a program named as Claude Code is, which may not have written its
    /// session file yet (stopped at the folder trust question).
    pub claude: bool,
}

/// A session's sidebar while it is looked at: whether it has the focus,
/// its cursor, the layout held still while you browse, a / filter, where
/// its list was scrolled to while it has the focus, and where its list was
/// last shown, which the next tab in front holds to while it is in sight
/// there (render.rs's `View::keep`); the repository it is filtered
/// to, every other one's tabs under Other repos, and whether that group
/// was unfolded, both kept with the layout until cleared; the groups
/// and the notifications folded with `z` or a click on a heading, kept
/// with the layout too; and, in select mode, the tabs selected, never
/// saved with the layout.
#[derive(Clone, Debug, Default)]
pub struct Side {
    pub focused: bool,
    pub cursor: Option<Owner>,
    pub frozen: Option<model::Frozen>,
    pub filter: Option<Filter>,
    pub top: Option<usize>,
    pub keep: Option<usize>,
    /// The main clone's directory the tabs are filtered to.
    pub repo: Option<PathBuf>,
    /// Other repos unfolded, its tabs listed one by one.
    pub repo_open: bool,
    /// The repositories it was filtered to, the latest first, at most
    /// `REPOS_RECENT`, which the filter's list leads with; kept with the
    /// layout, a clear of the filter leaving it as it is.
    pub repo_recent: Vec<PathBuf>,
    /// The groups folded to one line, Other repos aside, which folds by
    /// `repo_open`.
    pub folded: std::collections::BTreeSet<Stage>,
    /// The notifications folded to one line of their kinds.
    pub notices_folded: bool,
    /// Select mode (`v`, a Shift-click): the tabs selected, to close at
    /// once from one menu; `None` out of it.
    pub select: Option<BTreeSet<TabId>>,
}

/// How many of the repositories filtered to a sidebar remembers.
pub const REPOS_RECENT: usize = 20;
/// The longest path kept in the history, macOS's and Linux's limit on a
/// path, so a layout file can't fill it with text no directory has.
const PATH_MAX: usize = 4096;

/// Whether a path read from a layout can be one a sidebar was filtered to.
fn keepable(repo: &std::path::Path) -> bool {
    repo.to_str().is_some_and(|p| p.len() <= PATH_MAX)
}

impl Side {
    /// The filter as a saved layout or a handover kept it. One written
    /// before `repo_recent` was kept has a filter and no history, so the
    /// repository filtered to is put at its front when it isn't in it.
    pub fn restore_repo(&mut self, repo: Option<PathBuf>, open: bool, mut recent: Vec<PathBuf>) {
        recent.retain(|r| keepable(r));
        recent.truncate(REPOS_RECENT);
        self.repo_recent = recent;
        if let Some(repo) = &repo
            && !self.repo_recent.contains(repo)
        {
            self.filtered_to(repo);
        }
        self.repo = repo;
        self.repo_open = open;
    }

    /// A saved layout's tabs added to a session already open: the
    /// repositories it was filtered to follow the session's own, which
    /// are newer, those it already holds left where they are.
    /// The layout's own filter leads them when they don't hold it, as in
    /// one written before the history was kept. No more than
    /// `REPOS_RECENT` are kept.
    pub fn add_recent(&mut self, filter: Option<PathBuf>, saved: Vec<PathBuf>) {
        let filter = filter.filter(|f| !saved.contains(f));
        let read = filter.into_iter().chain(saved).take(REPOS_RECENT + 1);
        for repo in read.filter(|r| keepable(r)) {
            if self.repo_recent.len() >= REPOS_RECENT {
                break;
            }
            if !self.repo_recent.contains(&repo) {
                self.repo_recent.push(repo);
            }
        }
    }

    /// `repo` filtered to now: the first of `repo_recent`. A path that
    /// isn't UTF-8 is left out, since the history outlives the filter and
    /// neither the saved layout nor `list` can write one.
    pub fn filtered_to(&mut self, repo: &std::path::Path) {
        if !keepable(repo) {
            return;
        }
        self.repo_recent.retain(|r| r != repo);
        self.repo_recent.insert(0, repo.to_path_buf());
        self.repo_recent.truncate(REPOS_RECENT);
    }
}

/// The order the repository filter's list gives `repos` (each a main
/// clone's directory): those filtered to before by their place in `recent`,
/// the latest first, then the rest by the tab of theirs in front most
/// recently (`fronted`, a repository's place among those the session's
/// tabs were last in, none for one never in front), then by name.
pub fn repos_newest_first<T>(
    mut repos: Vec<(PathBuf, T)>,
    recent: &[PathBuf],
    fronted: &[PathBuf],
) -> Vec<(PathBuf, T)> {
    let place =
        |list: &[PathBuf], dir: &PathBuf| list.iter().position(|r| r == dir).unwrap_or(usize::MAX);
    repos.sort_by_cached_key(|(dir, _)| {
        let name = dir
            .file_name()
            .unwrap_or(dir.as_os_str())
            .to_string_lossy()
            .to_lowercase();
        (place(recent, dir), place(fronted, dir), name)
    });
    repos
}

/// Where a client's sidebar was last drawn, for the mouse and C-d.
#[derive(Clone, Debug, Default)]
pub struct Drawn {
    /// Its columns and rows on the screen, its line beside the panes left
    /// out.
    pub area: ratatui::layout::Rect,
    pub owners: Vec<Option<Owner>>,
    pub scroll: render::Scroll,
    /// Whether a badge on it spins.
    pub spins: bool,
    /// The columns of its top line the ⏰ chip takes, while it shows.
    pub reminders_at: Option<(usize, usize)>,
    /// The columns of its top line each of the workspace's own buttons
    /// takes.
    pub buttons_at: Vec<(Owner, usize, usize)>,
}

/// What the server keeps for the sidebar.
#[derive(Default)]
pub struct Sidebar {
    pub config: config::Config,
    config_seen: Option<Option<SystemTime>>,
    reader: claude::Reader,
    /// What was read of each pane at the last refresh.
    pub panes: HashMap<PaneId, PaneRead>,
    /// When each pane last printed anything, in seconds: when a tab with no
    /// Claude last did anything.
    pub output_at: HashMap<PaneId, i64>,
    /// Something happened (a hook) that the next draw should read again for.
    pub stale: bool,
    /// Each session's sidebar, by name.
    pub sides: HashMap<String, Side>,
    /// What the mouse is over, per client (hover_highlight).
    pub hover: HashMap<ClientId, Owner>,
    /// Each client's last half-screen move by the wheel: its direction
    /// (down) and when, so the rest of one flick is let go.
    pub wheel: HashMap<ClientId, (bool, Instant)>,
    /// The mouse (the wheel, the scroll bar) is stepping as j and k do,
    /// which never wraps round the list (sidebar_wrap is the keys' alone).
    pub stepping_by_mouse: bool,
    /// Where each client's sidebar was last drawn.
    pub drawn: HashMap<ClientId, Drawn>,
    /// The spinner's frame, and whether its next turn is timed.
    pub frame: usize,
    spin_pending: bool,
    /// Where M-a's walk got to.
    pub walk: walk::Walk,
    /// Each session's Notifications window: its cursor, the tab it came
    /// from, its focus and the mouse.
    pub windows: notices::Windows,
    /// The tabs whose status set by hand has been seen to hold this run
    /// (`own_stage` equal to the reading kept with it), and when the server
    /// first looked: until then a reading can still be loading (a restored
    /// Claude resuming, a PR not yet looked up), so a status isn't dropped
    /// for it (src/sidebar/serve/status.rs).
    pub status_seen: HashSet<crate::mux::TabId>,
    pub status_since: Option<Instant>,
    /// The tips asked for once this run (merged, loop, sidebar).
    tips_asked: HashSet<&'static str>,
    /// The interruption each pane was last handed back after (ms).
    handed_back: HashMap<PaneId, i64>,
    /// Every notification of the last two weeks, by day.
    pub history: history::History,
    /// A write of the history's days is under way.
    history_writing: bool,
}

impl Sidebar {
    /// A pane printed something.
    pub fn output(&mut self, pane: PaneId) {
        self.output_at.insert(pane, crate::util::now_secs());
    }

    /// A session's sidebar, made when first wanted.
    pub fn side(&mut self, session: &str) -> &mut Side {
        self.sides.entry(session.to_string()).or_default()
    }

    /// Whether a session's sidebar has the focus.
    pub fn focused(&self, session: &str) -> bool {
        self.sides.get(session).is_some_and(|s| s.focused)
    }

    /// The tab a session's sidebar cursor is on, while it has the focus.
    pub fn cursor_tab(&self, session: &str) -> Option<TabId> {
        self.sides
            .get(session)
            .filter(|s| s.focused)
            .and_then(|s| s.cursor.as_ref())
            .and_then(Owner::tab)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(names: &[&str]) -> Vec<PathBuf> {
        names
            .iter()
            .map(|n| PathBuf::from(format!("/code/{n}")))
            .collect()
    }

    fn names(repos: Vec<(PathBuf, usize)>) -> Vec<String> {
        repos
            .iter()
            .map(|(d, _)| d.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn the_repositories_filtered_to_lead_the_list_the_latest_first() {
        let repos = |names: &[&str]| dirs(names).into_iter().map(|d| (d, 1)).collect();
        // None filtered to: the one in front most recently, then by name.
        assert_eq!(
            names(repos_newest_first(
                repos(&["delta", "Beta", "alpha", "gamma"]),
                &[],
                &dirs(&["gamma"]),
            )),
            ["gamma", "alpha", "Beta", "delta"]
        );
        // Those filtered to first, one with no tabs left naming no row.
        assert_eq!(
            names(repos_newest_first(
                repos(&["alpha", "beta", "gamma", "delta"]),
                &dirs(&["gamma", "gone", "beta"]),
                &dirs(&["delta", "beta"]),
            )),
            ["gamma", "beta", "delta", "alpha"]
        );
    }

    #[test]
    fn a_filter_restored_with_no_history_starts_it() {
        let zeta = dirs(&["zeta"]).remove(0);
        let mut side = Side::default();
        side.restore_repo(Some(zeta.clone()), true, Vec::new());
        assert_eq!(side.repo_recent, std::slice::from_ref(&zeta));
        assert!(side.repo_open);
        // One kept with its history is left as it was.
        let kept = dirs(&["alpha", "zeta"]);
        side.restore_repo(Some(zeta), false, kept.clone());
        assert_eq!(side.repo_recent, kept);
        side.restore_repo(None, false, kept.clone());
        assert_eq!((side.repo.clone(), side.repo_recent.clone()), (None, kept));
        // A layout's history added to an open session's goes after it.
        side.add_recent(None, dirs(&["zeta", "beta"]));
        assert_eq!(side.repo_recent, dirs(&["alpha", "zeta", "beta"]));
        // With the layout's own filter first, and never past the cap.
        side.add_recent(Some(dirs(&["gamma"]).remove(0)), dirs(&["delta"]));
        assert_eq!(
            side.repo_recent,
            dirs(&["alpha", "zeta", "beta", "gamma", "delta"])
        );
        // A filter the layout's history holds stays where it is in it.
        let mut open = Side::default();
        open.add_recent(Some(dirs(&["zeta"]).remove(0)), dirs(&["alpha", "zeta"]));
        assert_eq!(open.repo_recent, dirs(&["alpha", "zeta"]));
        // A path longer than any directory's is never read in.
        let long = PathBuf::from(format!("/{}", "x".repeat(PATH_MAX)));
        open.add_recent(None, vec![long.clone()]);
        open.restore_repo(None, false, vec![long.clone()]);
        assert!(!open.repo_recent.contains(&long));
        let many: Vec<PathBuf> = (0..100)
            .map(|n| PathBuf::from(format!("/code/r{n}")))
            .collect();
        side.add_recent(None, many.clone());
        assert_eq!(side.repo_recent.len(), REPOS_RECENT);
        side.restore_repo(None, false, many);
        assert_eq!(side.repo_recent.len(), REPOS_RECENT);
    }

    #[test]
    fn filtering_to_a_repository_again_moves_it_to_the_front_once() {
        let mut side = Side::default();
        for name in ["alpha", "beta", "alpha"] {
            side.filtered_to(&dirs(&[name])[0]);
        }
        assert_eq!(side.repo_recent, dirs(&["alpha", "beta"]));
        for n in 0..REPOS_RECENT + 5 {
            side.filtered_to(&PathBuf::from(format!("/code/r{n}")));
        }
        assert_eq!(side.repo_recent.len(), REPOS_RECENT);
        // A path that isn't UTF-8 is never kept.
        use std::os::unix::ffi::OsStrExt;
        let odd = PathBuf::from(std::ffi::OsStr::from_bytes(b"/code/\xff"));
        side.filtered_to(&odd);
        assert!(!side.repo_recent.contains(&odd));
        assert_eq!(side.repo_recent[0], PathBuf::from("/code/r24"));
    }
}
