//! Working a note's checklist continuously: with the mode on (M-p's "Work
//! the note continuously"), the server keeps `slots` tabs working through a
//! note's unticked items, each opened as the palette's "Start work on a
//! note's checklist items" opens one (a tab running `wtn --item <line>`),
//! and opens the next whenever one lands, one at a time. Ported from
//! lib/sidebar/autopilot.js, the parts of lib/sidebar/daemon.js that set it
//! up, checked it and showed it, render.js's line above mission control's
//! cards, and tmux/wtn-open.zsh's `queue` and `next`, whose order is
//! `worktree_from_note::queue`'s.
//!
//! A tab waiting on you frees its slot but still counts toward `cap`, the
//! most of the mode's tabs open at once, as does a tab that never reached
//! its worktree, so a failed `wtn` doesn't have its item opened again and
//! again. Only the tabs it opened count: each is marked `@autopilot` with
//! the time it opened, `@autopilot_item` with the item's first line, and
//! `@autopilot_in` once seen in its worktree; a tab whose pane
//! queue-and-clean has moved back to the main clone leaves the mode. Those
//! are tab options, which `src/mux/persist.rs` saves with the rest of a tab,
//! so they come back after a restart, as tmux/window-options.sh carried
//! them under tmux.
//!
//! It opens nothing while it is paused, while you're away (`Server::away_now`,
//! whatever the pause_when_away setting says, as the daemon paused it on its
//! own reading of presence), once today's Claude spend in
//! the Usage view passes its limit, or while the note changed in the last
//! minute (an item being typed is saved as it is typed); it says so, or
//! that the note has nothing left, in the notifications
//! (`notice:autopilot`, here the "autopilot" source). While it is on, its
//! line leads mission control's Tabs view, above the cards, where the
//! cursor's -1 is, and Enter there opens its menu, which pauses, skips,
//! narrows or stops it. The `autopilot` setting allows it at all; the mode
//! itself starts off.
//!
//! - `mode`: what you chose, kept in `autopilot.json` in the state directory.
//! - `place`: where each of its tabs is: busy, waiting on you, or landed.
//! - `pilot`: what it keeps between checks, and what each check decides.
//! - `line`: its line in mission control and the menu Enter opens on it.
//! - `serve`: the server's side: setting it up, the check on every tick,
//!   opening the tabs, the notifications.

pub mod line;
pub mod mode;
pub mod pilot;
pub mod place;
mod serve;

pub use line::{Act, Summary};
pub use mode::Mode;
pub use pilot::Pilot;

/// M-p's command, the one way to set the mode up or change it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// "Work the note continuously": with the mode off, which note, how
    /// many tabs at once, the most open and a spend limit, then on; with it
    /// on, its menu.
    WorkNote,
}

impl Command {
    pub const ALL: [Command; 1] = [Command::WorkNote];

    /// Its name in keys.json.
    pub fn name(self) -> &'static str {
        match self {
            Command::WorkNote => "work-note-continuously",
        }
    }

    pub fn parse(name: &str) -> Option<Command> {
        Command::ALL.into_iter().find(|c| c.name() == name)
    }

    /// What it does, as tmux.conf described its binding.
    pub fn about(self) -> &'static str {
        match self {
            Command::WorkNote => {
                "Work the note continuously: keep tabs working through a note's checklist with wtn, the next opened as each lands"
            }
        }
    }

    /// tmux.conf's tags.
    pub fn tags(self) -> &'static [&'static str] {
        match self {
            Command::WorkNote => &["notes", "worktree", "autopilot"],
        }
    }

    /// Its key by default: none, as tmux.conf bound it in the palette's
    /// table alone.
    pub fn key(self) -> Option<&'static str> {
        None
    }
}

/// What the work done off the server's task answers with
/// (`Event::Autopilot`).
#[derive(Debug)]
pub enum AutopilotEvent {
    /// The item it would take next, read again for mission control's line,
    /// under the key it was read for.
    Next { key: String, text: String },
    /// What looking for the next item to open found, for a note last
    /// changed at `mtime` (ms).
    Found { found: pilot::Found, mtime: i64 },
}

/// What the server keeps for the mode: the file as last read, and what it
/// keeps between checks.
#[derive(Debug, Default)]
pub struct Autopilot {
    pub mode: Mode,
    pub pilot: Pilot,
}
