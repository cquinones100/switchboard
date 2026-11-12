//! Routines (UX 2.0 boards 29 and 30): a prompt of yours run on a
//! schedule, each run a turn of Claude in a tab of its own, its prompt
//! added to Claude's system prompt and the turn saying "Run <name>". New;
//! the tmux version had nothing like it, only a tab's reminder
//! (src/mission/remind.rs), which starts nothing.
//!
//! - `schedule`: when a routine runs, read from what you type ("weekdays
//!   at 8:45", "every 30m 9-6", "nov 20 3pm"), its next runs and its words.
//! - `routine`: a routine as it is kept, a file each in the state
//!   directory, and its runs, a line each.
//! - `view`: mission control's Routines view, its items and, with UX 2.0,
//!   its board (board 29).
//! - `form`: the box a routine is made and changed in (board 30).
//! - `serve`: the server's side: the runs on the tick, the hooks that end
//!   them, the view's keys and the palette's commands.

pub mod form;
pub mod routine;
pub mod schedule;
mod serve;
pub mod view;

use std::collections::HashMap;
use std::time::SystemTime;

use crate::items::Item;
use crate::mux::{PaneId, TabId};
pub use routine::{Outcome, Routine, Run, TabPolicy, WhenBusy, WhenMissed};

/// The tab option marking a routine's tab with its id, saved with the
/// layout so a restart still finds it.
pub const TAB_MARK: &str = "@routine";
/// The tab option holding when its routine runs next (seconds), for the
/// sidebar's "routine · next 2:30".
pub const TAB_NEXT: &str = "@routine_next";

/// A run waits on a busy tab at most this long, in seconds.
pub const WAIT_BUSY: i64 = 30 * 60;
/// A gap this long between two ticks (seconds) is the computer having
/// slept: a run due before the ticks went on again was missed, as one due
/// before the server started was.
pub const SLEPT_AFTER: i64 = 60;
/// A run's Claude not running this long (seconds) after the run started
/// has exited, Stop hook or none.
pub const START_GRACE: i64 = 60;
/// A missed run is run on waking only this late at most, in seconds.
pub const LATE_BY: i64 = 3600;
/// How many days of runs the server keeps for the view.
pub const KEEP_DAYS: i64 = 8;

/// A run under way, or waiting on its tab.
#[derive(Clone, Debug)]
pub struct Live {
    /// When it was due.
    pub due: i64,
    /// Missed while the computer slept, run on waking.
    pub late: bool,
    /// Asked for with r or Run now, which runs whatever the schedule says.
    pub by_hand: bool,
    /// When it became due, which a wait on a busy tab counts from.
    pub asked: i64,
    /// When its turn started, 0 while it waits on a busy tab.
    pub started: i64,
    pub tab: Option<TabId>,
    pub pane: Option<PaneId>,
    /// Its session's cost when the turn started.
    pub cost_before: f64,
    /// Its directory being checked for someone else's PR, off the task,
    /// before anything starts.
    pub checking: bool,
    /// Until when (seconds) a SessionEnd is the Claude it replaced ending.
    pub quiet_until: i64,
    /// The session its own pane's hooks named, which its cost is read
    /// from; empty before one.
    pub session: String,
}

/// What the server keeps for them.
#[derive(Debug, Default)]
pub struct Routines {
    /// Every routine, as last read or written.
    pub list: Vec<Routine>,
    /// Each routine's runs of the last `KEEP_DAYS`, by its id.
    pub runs: HashMap<String, Vec<Run>>,
    /// The runs under way or waiting, by routine id.
    pub live: HashMap<String, Live>,
    /// A run that failed or was skipped, by routine id, until put away,
    /// for the notifications.
    pub said: HashMap<String, Item>,
    /// The prompt's hash and the directory each pane's Claude was started with here
    /// (`Routine::prompt_hash`). Kept in memory alone: a tab brought back
    /// by a restart resumes its Claude without the prompt, so its next run
    /// starts it again with it.
    pub started: HashMap<PaneId, (String, String)>,
    /// The panes whose Claude a run just started, until when (seconds):
    /// their SessionStart is that Claude's, any other a Claude started
    /// otherwise, which never had the prompt.
    pub(crate) expect: HashMap<PaneId, i64>,
    /// The session each routine pane's hooks last named, with the
    /// directory it was made in, so a run that types into a pane counts its
    /// cost from that pane's own session, and one that starts its Claude
    /// again resumes it only there.
    pub(crate) pane_sessions: HashMap<PaneId, (String, String)>,
    /// The last tick (seconds), and since when the ticks have gone on
    /// without a gap of `SLEPT_AFTER`: a run due before that was missed.
    pub(crate) ticked: i64,
    pub(crate) awake_since: i64,
    /// The folder's modified time when last read.
    pub(crate) seen: Option<SystemTime>,
    pub(crate) read: bool,
}

impl Routines {
    pub fn get(&self, id: &str) -> Option<&Routine> {
        self.list.iter().find(|r| r.id == id)
    }
}

/// What the work done off the server's task answers with
/// (`Event::Routines`).
#[derive(Debug)]
pub enum RoutinesEvent {
    /// Whether a run's directory is marked as holding someone else's PR
    /// (`trust::is_untrusted`, which runs git).
    Checked {
        id: String,
        /// The directory it checked.
        dir: String,
        untrusted: bool,
    },
}

/// What a routine's menu does (`mission::choices::Do::Routine`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    RunNow,
    GoToTab,
    Edit,
    Pause,
    Resume,
    Delete,
}

/// The commands for the palette, `mux::keys::Action::Routines`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    /// "New routine": the routine's box.
    New,
}

impl Command {
    pub const ALL: [Command; 1] = [Command::New];

    /// Its name in keys.json.
    pub fn name(self) -> &'static str {
        match self {
            Command::New => "new-routine",
        }
    }

    pub fn parse(name: &str) -> Option<Command> {
        Command::ALL.into_iter().find(|c| c.name() == name)
    }

    pub fn about(self) -> &'static str {
        match self {
            Command::New => {
                "New routine: a prompt of yours run on a schedule, each run in a Claude tab of its own"
            }
        }
    }

    /// More on what it does, for the palette's preview.
    pub fn detail(self) -> &'static str {
        match self {
            Command::New => {
                "Asks its name, its directory, when it runs (weekdays at 8:45, every 30m 9-6, nov 20 3pm), which tab, what to do when that tab is busy or the computer slept, and the prompt added to Claude's system prompt. Mission control's Routines view lists them. Settings' \"Routines\" (routines)."
            }
        }
    }

    pub fn tags(self) -> &'static [&'static str] {
        match self {
            Command::New => &["routines", "claude", "schedule"],
        }
    }

    /// Its key by default: none, the palette's alone.
    pub fn key(self) -> Option<&'static str> {
        None
    }
}
