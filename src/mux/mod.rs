//! The multiplexer: what tmux did for Switchboard, done by Switchboard. A
//! server owns every pane (a PTY and a terminal emulator), lays them out in
//! tabs, and draws each attached client's screen; clients send it the keys
//! pressed and show what it draws. ARCHITECTURE.md has the whole picture.
//!
//! - `proto`: the messages between clients and the server, and their framing.
//! - `socket`: where the server listens, connecting to it, starting it.
//! - `client`: the attached terminal: raw mode in, the drawn screen out.
//! - `input`: the bytes a terminal sends, as keys, mouse and paste, and keys
//!   encoded again for a pane in the modes its program asked for.
//! - `extkeys`: xterm's modifyOtherKeys as a pane's program sets it.
//! - `pane`: one pane's PTY and terminal emulator, drawn into a cell buffer.
//! - `pty`: a pane's terminal: its master, its program, and the threads
//!   reading and writing it, which can be paused and handed on.
//! - `procinfo`: a pane's processes: their directory and command.
//! - `layout`: a tab's panes as a tree of splits.
//! - `keys`: which key does what.
//! - `compose`: drawing a client's screen, sending only what changed.
//! - `server`: the server task, owning all of it.
//! - `commands`: what shells, hooks and skills ask of the server.
//! - `respawn`: a pane's program ended and another started in its place.
//! - `passthrough`: what one pane may pass to the terminal or take from it
//!   that others may not: kitty's graphics commands, easy mode's Cmd keys.
//! - `graphics`: how every picture reaches a terminal (kitty's protocol,
//!   sixel or text), decided in one place, and kitty's commands written.
//! - `sixel`: pictures drawn by the server as sixel, the panes' and its
//!   own, where their placeholders show, for a terminal without kitty's
//!   (Windows Terminal).
//! - `terminal`: what the attached terminal says of itself (its cell's
//!   size, its colours), which the panes' programs are told when they ask.
//! - `persist`: the layout saved as it changes and restored on start.
//! - `replay`: what a pane shows, as the bytes that draw it again.
//! - `upgrade`: the server restarted on a new build in place, its panes'
//!   programs, its socket and its terminals handed over across an exec.
//! - `trail`: the tabs and mission control views a session visited, for
//!   M-{ and M-} to go back and forward through and M-Tab to go back to.
//! - `latest`: M-p's restart building the checkout first, and telling
//!   whether there is anything new to restart on.
//! - `behind`: the notification that main has landings the running build
//!   lacks.

pub mod behind;
pub mod beside;
pub mod build_row;
pub mod client;
pub mod commands;
pub mod compose;
pub mod extkeys;
pub mod graphics;
pub mod header;
pub mod keys;
pub mod latest;
pub mod layout;
pub mod main_clone;
pub mod pane;
pub mod passthrough;
pub mod persist;
pub mod procinfo;
pub mod proto;
pub mod pty;
pub mod replay;
pub mod respawn;
pub mod server;
pub mod sixel;
pub mod socket;
pub mod terminal;
pub mod trail;
pub mod upgrade;

pub use switchboard_ui::input;
pub use switchboard_ui::input::{MouseReporting, PaneModes};

use serde::{Deserialize, Serialize};

/// A pane, by the number the server gave it; never reused while the server
/// runs, and kept across a restore. Shown and passed as `%<n>`, as tmux
/// named panes, so a pane's id reads the same in every place it appears.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PaneId(pub u32);

impl std::fmt::Display for PaneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "%{}", self.0)
    }
}

impl std::str::FromStr for PaneId {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        s.strip_prefix('%')
            .unwrap_or(s)
            .parse()
            .map(PaneId)
            .map_err(|_| ())
    }
}

/// A tab (tmux's window), by number, shown as `@<n>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TabId(pub u32);

impl std::fmt::Display for TabId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "@{}", self.0)
    }
}

impl std::str::FromStr for TabId {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        s.strip_prefix('@')
            .unwrap_or(s)
            .parse()
            .map(TabId)
            .map_err(|_| ())
    }
}

/// A direction on the screen: where a split goes, which neighbor to move
/// to, which edge a resize moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// What a pane's terminal emulator reports to the server after it has read
/// some output, beyond the screen it now shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TermNotice {
    /// The program set the title (OSC 0 or 2); `None` when it reset it.
    Title(Option<String>),
    /// The bell rang.
    Bell,
    /// The program put text on the clipboard (OSC 52).
    Clipboard(String),
    /// A kitty graphics command (`ESC _ G ... ESC \`), whole, from a pane
    /// allowed to send them on to the terminal (`passthrough`).
    Graphics(Vec<u8>),
}

/// What a pane's reader thread sends the server. Each carries which of the
/// pane's programs it is about (`Pane::generation`), since a pane whose
/// program is replaced (`respawn`) keeps its id: what the old program wrote
/// last, and its end, come after the new one has started, and are dropped.
#[derive(Debug)]
pub enum PaneEvent {
    /// The program wrote this.
    Output(PaneId, u64, Vec<u8>),
    /// The program ended, with its exit code when it has one.
    Exited(PaneId, u64, Option<i32>),
}
