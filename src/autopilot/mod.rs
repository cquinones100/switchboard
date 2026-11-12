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
//! the time it opened, `@autopilot_item` with the item's first line,
//! `@autopilot_note` with its note's stable ID (src/notes/id.rs), and
//! `@autopilot_in` once seen in its worktree; a tab whose pane
//! queue-and-clean has moved back to the main clone leaves the mode, and,
//! unless the autopilot_close setting says never, is marked landed
//! (`@autopilot_left`), shown "landed" in the sidebar instead of "auto",
//! marked seen once you have had it in front (`@autopilot_seen`), and
//! closed as the setting says: once seen and left, or half an hour after it
//! landed (when seen); as soon as it lands (at once); or five minutes after
//! (after 5 minutes). One closed unseen leaves a notification, "Closed
//! <item>: landed <short hash>", kept in `autopilot.json`'s `closed`. Those are tab options, which `src/mux/persist.rs` saves with the rest of a tab,
//! so they come back after a restart, as tmux/window-options.sh carried
//! them under tmux.
//!
//! It opens nothing while it is paused, with no client attached (a tab
//! opens in one), while you're away (`Server::away_now`, whatever the
//! pause_when_away setting says) only with the autopilot_while_away setting
//! off, so it works through the note overnight, once today's Claude spend in
//! the Usage view passes its limit, or while the note changed in the last
//! minute (an item being typed is saved as it is typed); it says so, or
//! that the note has nothing left, in the notifications
//! (`notice:autopilot`, here the "autopilot" source). While it is on, its
//! line leads mission control's Tabs view, above the cards, where the
//! cursor's -1 is, and Enter there opens its menu, which pauses, skips,
//! narrows or stops it. The `autopilot` setting allows it at all; the mode
//! itself starts off.
//!
//! Its menu's "Land without asking", after a yes naming what it skips and
//! keeps, marks its tabs not landed yet `@autopilot_land`, which
//! queue-and-clean's landing on the default branch takes as yes to the push
//! once `switchboard leak-check` finds nothing and the review loop ended on
//! two clean rounds of the change, and which the review loop reads to fix
//! medium findings without asking; "Ask before landing" unmarks them. After
//! such a landing, `switchboard land-lock autolanded` marks the tab
//! `@autopilot_autoland`, shown "landed without asking" in the sidebar once
//! it has landed, and adds "Landed <hash>: <item>" to the notifications
//! (`landed`, the "autolanded" source), whose menu goes to its tab. Both are
//! tab options too, kept across a restart.
//!
//! Its menu's "Hands-free", after a yes naming what it decides and what
//! still stops (and with the autopilot_handsfree setting on), turns "Land
//! without asking" on with it and marks its tabs not landed yet
//! `@autopilot_handsfree`, a new one as it opens: the server then answers
//! their Claude's question boxes and turns that end on a question for it
//! (src/agent/hands_free.rs), counting the turns sent back in
//! `@autopilot_nudged`, the queue takes items whose plan needs a choice
//! too, and wtn tells such a tab to take the recommended option, each
//! decision written into the item's plan as a `Decided hands-free` line,
//! which the landing's notification counts. "Ask me again" unmarks them.
//!
//! - `mode`: what you chose, kept in `autopilot.json` in the state directory,
//!   the note by its path and its ID, which a renamed or moved note is
//!   followed by.
//! - `place`: where each of its tabs is: busy, waiting on you, or landed.
//! - `pilot`: what it keeps between checks, and what each check decides.
//! - `line`: its line in mission control and the menu Enter opens on it.
//! - `landed`: the landings its tabs made without asking, for the
//!   notifications.
//! - `view`: its own view in mission control, with UX 2.0 on.
//! - `serve`: the server's side: setting it up, the check on every tick,
//!   opening the tabs, the notifications.

pub mod landed;
pub mod line;
pub mod mode;
pub mod pilot;
pub mod place;
mod serve;
pub mod view;

pub use line::{Act, Summary};
pub use mode::Mode;
pub use pilot::Pilot;
pub use serve::{AUTOLAND, HANDS_FREE, LAND, LEFT, NUDGED, OPENED, PUSHED, REFUSED};

/// Whether the mode has its own view in mission control (UX 2.0 and the
/// view_autopilot setting on), which then takes the place of its line
/// leading the Tabs view.
pub fn has_view() -> bool {
    let on = crate::settings::is_on;
    on("ux2") && on("view_autopilot") && on("autopilot")
}

/// Where mission control opens to show the mode: its own view while it has
/// one, else the Tabs view its line leads.
pub fn opening() -> crate::mission::Opening {
    if has_view() {
        crate::mission::Opening::View("autopilot".into())
    } else {
        crate::mission::Opening::Tabs
    }
}

/// M-p's command, the one way to set the mode up or change it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// "Autopilot: work a note": with the mode off, which note, how
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
                "Autopilot: work a note: keep tabs working through a note's checklist with wtn, the next opened as each lands"
            }
        }
    }

    /// More on what it does than `about`, for the palette's preview of the chosen command.
    pub fn detail(self) -> &'static str {
        match self {
            Command::WorkNote => {
                "Asked from a pane in the note's repository, it asks which note, the branch to land on, how many tabs work at once and a spend limit. With the mode on, it opens mission control on the mode, whose menu (m) changes or stops it."
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
    Next {
        key: String,
        text: String,
        /// The items it will open, in order, for its view.
        queue: Vec<String>,
        /// The plan each of the note's items links, by its first line.
        plans: std::collections::HashMap<String, crate::cmd::worktree_from_note::PlanNote>,
        /// Today's spend, while there is a spend limit.
        spent: f64,
    },
    /// What looking for the next item to open found, for a note last
    /// changed at `mtime` (ms).
    Found { found: pilot::Found, mtime: i64 },
    /// What following the note by its ID found; `followed` when it looked
    /// for the note or gave it an ID, rather than only tidying.
    Kept { kept: pilot::Kept, followed: bool },
}

/// What says a file changed: its modified time and change time, each in
/// seconds and nanoseconds, and its size.
pub type Stamp = (i64, i64, u64, i64, i64);

/// What the server keeps for the mode: the file as last read, and what it
/// keeps between checks.
#[derive(Debug, Default)]
pub struct Autopilot {
    pub mode: Mode,
    pub pilot: Pilot,
    /// The ID the mode's note held when its file was last read, by the
    /// file and what it was like then.
    pub seen_id: Option<(std::path::PathBuf, Stamp, Option<String>)>,
}
