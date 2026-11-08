//! What Switchboard knows of how it is used, on this computer alone: the
//! keys and commands you use (`habits`, and the `report` of them), what each
//! tab costs the computer (`resources`), each Claude session's context and
//! the day's spend and requests (`sessions`), the tabs that closed
//! (`closed`), the settings kept in your own files (`own`), and the views
//! plugins bring in (`plugins`, with the built-in `github_plugin`).
//!
//! Each part was a file of the sidebar daemon (lib/sidebar/habits.js,
//! resources.js, sessions.js, closed.js, own-config.js, plugins.js, and
//! lib/habits-report.js); each says which. They work on files and plain
//! data, so their tests need no server; `serve` puts them on the server:
//! counting keys as they are pressed, reading processes and session files
//! off its task only while a view that shows them is in front, running the
//! plugins on their intervals, and keeping each view's `ViewData` for
//! mission control to draw.

pub mod closed;
pub mod github_plugin;
pub mod habits;
pub mod own;
pub mod plugins;
pub mod report;
pub mod resources;
pub mod serve;
pub mod sessions;

use std::collections::HashMap;
use std::time::{Instant, SystemTime};

use crate::items::ViewData;
use crate::mux::PaneId;
use crate::mux::procinfo::ProcUsage;

pub use serve::{ViewKind, WANT_FOR};

/// What the work done off the server's task answers with
/// (`Event::Usage`).
#[derive(Debug)]
pub enum UsageEvent {
    /// Which pane runs which Claude session, as the session files say.
    Claudes(HashMap<PaneId, sessions::Claude>),
    /// A reading of every process, and each one's CPU since the last.
    Reading {
        at: Instant,
        list: Vec<ProcUsage>,
        procs: Vec<resources::Proc>,
    },
    /// A view worked out again.
    View(ViewKind, ViewData),
    /// Time for a view still in front to be read again.
    Refresh(ViewKind),
    /// The habits' suggestions, worked out again.
    Suggested(Vec<habits::Suggestion>),
    /// A plugin's run ended.
    Plugin {
        name: String,
        result: Result<plugins::Output, String>,
    },
    /// Text to type into a pane: `claude --resume` into a reopened tab's
    /// shell, once it has drawn its prompt.
    Type { pane: PaneId, text: String },
}

/// What the server keeps for this part.
#[derive(Debug, Default)]
pub struct Usage {
    /// Keys pressed, picks and views shown since they were last written, by
    /// kind and name.
    pending: HashMap<(String, String), u64>,
    /// When the counts were last written, and whether the habits setting was
    /// on then.
    kept: Option<Instant>,
    habits_were: Option<bool>,
    /// When the suggestions were last worked out; `None` to do it on the
    /// next tick.
    suggested: Option<Instant>,
    /// The suggestions now, which the sidebar's notifications show.
    pub suggestions: Vec<habits::Suggestion>,
    /// When mission control last showed each view, and when each was last
    /// asked to be worked out (`asked`) or answered.
    wanted: HashMap<ViewKind, Instant>,
    asked: HashMap<ViewKind, Instant>,
    in_flight: HashMap<ViewKind, bool>,
    /// Each view as last worked out.
    views: HashMap<ViewKind, ViewData>,
    /// The last reading of every process, and each one's CPU then.
    reading: Option<(Instant, Vec<ProcUsage>)>,
    procs: Option<Vec<resources::Proc>>,
    /// Which pane runs which Claude session, as last read.
    pub claudes: HashMap<PaneId, sessions::Claude>,
    surveying: bool,
    surveyed: Option<Instant>,
    /// The tabs as last seen, and the ones closed.
    pub closed: closed::Closed,
    /// The plugins sidebar.json names, and the file's time when read.
    pub plugins: Vec<plugins::Plugin>,
    plugins_seen: Option<Option<SystemTime>>,
}
