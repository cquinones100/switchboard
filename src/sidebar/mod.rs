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
pub mod text;
pub mod walk;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use crate::mux::server::ClientId;
use crate::mux::{PaneId, TabId};

pub use model::{Rules, Stage, TabModel, TabState};
pub use render::{Filter, Owner};
pub use serve::KEYS;

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
    /// The groups folded to one line, Other repos aside, which folds by
    /// `repo_open`.
    pub folded: std::collections::BTreeSet<Stage>,
    /// The notifications folded to one line of their kinds.
    pub notices_folded: bool,
    /// Select mode (`v`, a Shift-click): the tabs selected, to close at
    /// once from one menu; `None` out of it.
    pub select: Option<BTreeSet<TabId>>,
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
