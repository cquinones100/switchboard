//! The multiplexer: what tmux did for Switchboard, done by Switchboard. A
//! server owns every pane (a PTY and a terminal emulator), lays them out in
//! tabs, and draws each attached client's screen; clients send it the keys
//! pressed and show what it draws. PORTING.md has the whole picture.
//!
//! - `proto`: the messages between clients and the server, and their framing.
//! - `socket`: where the server listens, connecting to it, starting it.
//! - `client`: the attached terminal: raw mode in, the drawn screen out.
//! - `input`: the bytes a terminal sends, as keys, mouse and paste, and keys
//!   encoded again for a pane in the modes its program asked for.
//! - `pane`: one pane's PTY and terminal emulator, drawn into a cell buffer.
//! - `procinfo`: a pane's processes: their directory and command.
//! - `layout`: a tab's panes as a tree of splits.

pub mod client;
pub mod input;
pub mod layout;
pub mod pane;
pub mod procinfo;
pub mod proto;
pub mod socket;

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
}

/// What a pane's reader thread sends the server.
#[derive(Debug)]
pub enum PaneEvent {
    /// The program wrote this.
    Output(PaneId, Vec<u8>),
    /// The program ended, with its exit code when it has one.
    Exited(PaneId, Option<i32>),
}
