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
//! - `sixel`: those pictures drawn by the server as sixel, where their
//!   placeholders show, for a terminal without kitty's (Windows Terminal).
//! - `terminal`: what the attached terminal says of itself (its cell's
//!   size, its colours), which the panes' programs are told when they ask.
//! - `persist`: the layout saved as it changes and restored on start.
//! - `replay`: what a pane shows, as the bytes that draw it again.
//! - `upgrade`: the server restarted on a new build in place, its panes'
//!   programs, its socket and its terminals handed over across an exec.
//! - `latest`: M-p's restart building the checkout first, and telling
//!   whether there is anything new to restart on.

pub mod build_row;
pub mod client;
pub mod commands;
pub mod compose;
pub mod extkeys;
pub mod header;
pub mod input;
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
pub mod upgrade;

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

/// Which mouse events a pane's program asked to be told of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseReporting {
    /// None: the multiplexer keeps the mouse (selecting, scrolling).
    #[default]
    Off,
    /// Presses and releases (mode 1000).
    Press,
    /// And movement with a button held (1002).
    Drag,
    /// And every movement (1003).
    Motion,
}

/// How a pane's program asked the terminal to behave, as far as what is sent
/// to it depends on it: `input`'s encoders read this, `pane` reads it from
/// the emulator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PaneModes {
    /// Cursor keys as SS3 (`\eOA`) rather than CSI (`\e[A`): DECCKM.
    pub app_cursor: bool,
    /// The keypad's application mode: DECKPAM.
    pub app_keypad: bool,
    /// Pasted text between `\e[200~` and `\e[201~` (2004).
    pub bracketed_paste: bool,
    /// Focus in and out as `\e[I` and `\e[O` (1004).
    pub focus_events: bool,
    pub mouse: MouseReporting,
    /// Mouse events in SGR form (1006), else the X10 form.
    pub mouse_sgr: bool,
    /// Mouse coordinates as UTF-8 (1005), for the X10 form.
    pub mouse_utf8: bool,
    /// The alternate screen is up (a full-screen program runs).
    pub alt_screen: bool,
    /// The wheel sends arrow keys on the alternate screen (alacritty's
    /// ALTERNATE_SCROLL, 1007), for programs that don't take the mouse.
    pub alternate_scroll: bool,
    /// The kitty keyboard protocol's flags the program set (`CSI > 1 u`
    /// and the rest): 1 has keys the old forms can't tell apart sent as
    /// CSI u, 8 every key, 16 with the text it types. 0 when it set none.
    pub kitty_keys: u8,
    /// xterm's modifyOtherKeys level the program set (`CSI > 4 ; 2 m`): 1
    /// has the keys the old forms lose a modifier of sent as CSI u, 2 every
    /// key with Ctrl or Alt. 0 when it set none.
    pub modify_other_keys: u8,
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
