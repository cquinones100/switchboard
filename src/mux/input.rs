//! The bytes a terminal sends, as keys, mouse and paste (`Parser`), and keys
//! encoded again for a pane in the modes its program asked for (`encode_*`).
//! Ported from what tmux did for Switchboard; the key names are tmux's, so the
//! default bindings read as tmux.conf's did.
//!
//! STUB: the signatures are the contract; step 2 fills them in.

use std::time::Duration;

use super::PaneModes;

/// How long a lone ESC waits for the rest of a sequence before it counts as
/// the Escape key (tmux's `escape-time`).
pub const ESC_TIMEOUT: Duration = Duration::from_millis(10);

/// Modifier keys held with a key or mouse event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyCode {
    Char(char),
    Enter,
    Tab,
    /// Shift-Tab, as terminals send it (`\e[Z`).
    BackTab,
    Backspace,
    Esc,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8),
}

/// A key and its modifiers. Equality is on the normalized form, so Alt with
/// Shift and `j` is the same key as Alt with `J`, as tmux's `M-J` is.
#[derive(Clone, Copy, Debug, Eq)]
pub struct Key {
    pub code: KeyCode,
    pub mods: Mods,
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        let (a, b) = (self.normalized(), other.normalized());
        a.code == b.code && a.mods == b.mods
    }
}

// Hashed as compared: on the normalized form, so a map of bindings finds
// `M-J` under Alt+Shift+j.
impl std::hash::Hash for Key {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let n = self.normalized();
        n.code.hash(state);
        n.mods.hash(state);
    }
}

impl Key {
    pub fn new(code: KeyCode, mods: Mods) -> Key {
        Key { code, mods }
    }

    /// A key from tmux's notation: `M-a`, `C-w`, `M-S-Left`, `M-Space`,
    /// `M-Tab`, `Enter`, `Escape`, `BSpace`, `M-[`, `M-J`, `F1`, `C-M-x`,
    /// `BTab`; `None` when it isn't one.
    pub fn parse(notation: &str) -> Option<Key> {
        let _ = notation;
        todo!("input::Key::parse")
    }

    /// The key in tmux's notation, as `parse` reads it.
    pub fn notation(&self) -> String {
        todo!("input::Key::notation")
    }

    /// The canonical form bindings are matched in: a letter with Shift is
    /// its capital without Shift; Ctrl with a letter is lowercase.
    pub fn normalized(&self) -> Key {
        todo!("input::Key::normalized")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
    /// Movement with no button held.
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseKind {
    Press,
    Release,
    /// Movement with a button held.
    Drag,
    /// Movement with no button held (any-motion reporting).
    Move,
}

/// A mouse event at a cell, counted from 0 at the top left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mouse {
    pub kind: MouseKind,
    pub button: MouseButton,
    pub col: u16,
    pub row: u16,
    pub mods: Mods,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    Key(Key),
    Mouse(Mouse),
    /// Text pasted between bracketed-paste markers.
    Paste(String),
    FocusIn,
    FocusOut,
    /// A sequence not understood, passed to the pane as it came.
    Unknown,
}

/// One input event and the bytes it came as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parsed {
    pub input: Input,
    pub raw: Vec<u8>,
}

/// Turns what a terminal sends into events, across reads: a sequence split
/// between two reads is held until the rest comes.
#[derive(Debug, Default)]
pub struct Parser {
    pending: Vec<u8>,
}

impl Parser {
    pub fn new() -> Parser {
        Parser::default()
    }

    /// The events in `bytes`, after what was held from before. An ESC at the
    /// end is held, since it may start a sequence: `flush` it once
    /// `ESC_TIMEOUT` passes with nothing more.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Parsed> {
        let _ = bytes;
        todo!("input::Parser::feed")
    }

    /// Whether something is held waiting for the rest of a sequence.
    pub fn pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// What is held, as it stands: a lone ESC as the Escape key, the rest as
    /// keys or `Unknown`.
    pub fn flush(&mut self) -> Vec<Parsed> {
        todo!("input::Parser::flush")
    }
}

/// The bytes a key is sent to a pane as, in its modes.
pub fn encode_key(key: &Key, modes: &PaneModes) -> Vec<u8> {
    let _ = (key, modes);
    todo!("input::encode_key")
}

/// The bytes a mouse event (its position already the pane's own) is sent to
/// a pane as, or `None` when the pane didn't ask for that kind of event.
pub fn encode_mouse(mouse: &Mouse, modes: &PaneModes) -> Option<Vec<u8>> {
    let _ = (mouse, modes);
    todo!("input::encode_mouse")
}

/// Pasted text as sent to a pane: bracketed when it asked, its own markers
/// taken out so a paste can't end the bracket early.
pub fn encode_paste(text: &str, modes: &PaneModes) -> Vec<u8> {
    let _ = (text, modes);
    todo!("input::encode_paste")
}

/// Focus in or out as sent to a pane, when it asked.
pub fn encode_focus(focused: bool, modes: &PaneModes) -> Option<Vec<u8>> {
    modes.focus_events.then(|| {
        if focused {
            b"\x1b[I".to_vec()
        } else {
            b"\x1b[O".to_vec()
        }
    })
}
