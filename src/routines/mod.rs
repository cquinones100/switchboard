//! Routines (UX 2.0 boards 29 and 30): a prompt of yours run on a
//! schedule, each run a headless Claude (`claude -p`) in its directory
//! that opens no tab, allowed only the routine's own tools, its prompt
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
//! - `headless`: a run's `claude -p`, its arguments and its JSON read.
//! - `starters`: the routines Switchboard starts you with (today's todo
//!   note, meeting notes, the Slack and Calendar views kept up to date),
//!   made once, each with the tools it needs.
//! - `serve`: the server's side: the runs on the tick and their answers,
//!   the view's keys and the palette's commands.

pub mod form;
pub mod headless;
pub mod routine;
pub mod schedule;
mod serve;
pub mod starters;
pub mod view;

use std::collections::HashMap;
use std::time::SystemTime;

use crate::items::Item;
pub use routine::{Outcome, Routine, Run, WhenMissed};

/// The tab option a build before runs were headless marked a routine's tab
/// with, saved with the layout: such a tab, brought back, is still taken
/// for a tab kept for a routine (the relay and the Slack view pass it over).
pub const TAB_MARK: &str = "@routine";
/// The tab option such a build kept when its routine runs next, for the
/// sidebar's "routine · next 2:30", taken off any tab still carrying it.
pub const TAB_NEXT: &str = "@routine_next";

/// A gap this long between two ticks (seconds) is the computer having
/// slept: a run due before the ticks went on again was missed, as one due
/// before the server started was.
pub const SLEPT_AFTER: i64 = 60;
/// A missed run is run on waking only this late at most, in seconds.
pub const LATE_BY: i64 = 3600;
/// How many days of runs the server keeps for the view.
pub const KEEP_DAYS: i64 = 8;

/// A run under way.
#[derive(Clone, Debug)]
pub struct Live {
    /// When it was due.
    pub due: i64,
    /// Missed while the computer slept, run on waking.
    pub late: bool,
    /// Asked for with r or Run now, which runs whatever the schedule says.
    pub by_hand: bool,
    /// When its Claude started, 0 while its directory is checked.
    pub started: i64,
    /// Its Claude's process, 0 until it has started, and when that
    /// process started (`headless::Started::born`).
    pub pid: u32,
    pub born: u64,
    /// The directory it runs in, where its session is made.
    pub dir: String,
    /// When (seconds) it was told to end, having run past its limit or its
    /// routine being deleted; 0 while it runs. It is watched until it has.
    pub stopping: i64,
    /// Its Claude ended, and its answer is being read off the task.
    pub collecting: bool,
    /// Its directory being checked for someone else's PR, off the task,
    /// before anything starts.
    pub checking: bool,
}

/// What the server keeps for them.
#[derive(Debug, Default)]
pub struct Routines {
    /// Every routine, as last read or written.
    pub list: Vec<Routine>,
    /// Each routine's runs of the last `KEEP_DAYS`, by its id.
    pub runs: HashMap<String, Vec<Run>>,
    /// The runs under way, by routine id.
    pub live: HashMap<String, Live>,
    /// A run that failed or was skipped, by routine id, until put away,
    /// for the notifications.
    pub said: HashMap<String, Item>,
    /// The last tick (seconds), and since when the ticks have gone on
    /// without a gap of `SLEPT_AFTER`: a run due before that was missed.
    pub(crate) ticked: i64,
    pub(crate) awake_since: i64,
    /// The folder's modified time when last read.
    pub(crate) seen: Option<SystemTime>,
    pub(crate) read: bool,
    /// Whether the runs a server before this one left going were taken up.
    pub(crate) taken_up: bool,
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
    /// A run's Claude started, its process id, or why it didn't.
    Started {
        id: String,
        /// When it started, which tells it from a later run.
        started: i64,
        pid: Result<u32, String>,
    },
    /// A run's answer, read from its files once its Claude ended.
    Answered {
        id: String,
        started: i64,
        answer: Result<headless::Reply, String>,
    },
}

/// What a routine's menu does (`mission::choices::Do::Routine`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    RunNow,
    /// Its session opened in a tab, `claude --resume`.
    OpenSession,
    /// The tools its last run needed added to its own, and run again.
    AllowNeeded,
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
                "New routine: a prompt of yours run on a schedule by a headless Claude, opening no tab"
            }
        }
    }

    /// More on what it does, for the palette's preview.
    pub fn detail(self) -> &'static str {
        match self {
            Command::New => {
                "Asks its name, its directory, when it runs (weekdays at 8:45, every 30m 9-6, nov 20 3pm), the tools a run may use, what to do when the computer slept, and the prompt added to Claude's system prompt. Mission control's Routines view lists them. Settings' \"Routines\" (routines)."
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
