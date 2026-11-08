//! The bytes a terminal sends, as keys, mouse and paste (`Parser`), and keys
//! encoded again for a pane in the modes its program asked for (`encode_*`).
//! What tmux's tty-keys.c and input-keys.c did for Switchboard; the key names
//! are tmux's, so the default bindings read as tmux.conf's did.
//!
//! A terminal sends most keys as the bytes they type and the rest as escape
//! sequences, and some sequences begin the way a key does: ESC alone is the
//! Escape key, ESC then `j` is Alt+j, and ESC `[` is Alt+[ or the start of an
//! arrow key, a mouse event or a paste. So the parser holds whatever could
//! still become a longer sequence until the rest comes, and the server
//! flushes it once `ESC_TIMEOUT` passes with nothing more, when it is read as
//! the keys it makes alone. A sequence split between two reads is read whole
//! either way, and every event keeps the exact bytes it came as, so a key no
//! binding takes, or a sequence not understood, reaches the pane unchanged.
//!
//! Replies to queries (an OSC, a DCS, a kitty graphics APC) are read whole as
//! `Unknown`, so a reply never types its body into a pane. Each begins the way
//! an Alt key does (`ESC ]` is also M-]), so it is taken for one only when the
//! byte after says it is, and anything that breaks it off is read as the keys.
//!
//! A bracketed paste is held whole, however many reads it spans, and the flush
//! never cuts one short: the rest of a paste read as keys would run every line
//! in it. Only a paste whose next bytes haven't come for `PASTE_TIMEOUT` is let
//! go as it stands, so a terminal that never ends one can't swallow every key
//! typed after it.
//!
//! The client asks your terminal for keys told apart (`client::KEYS_ON`), as
//! tmux's `extended-keys on` did, so Ctrl-Shift-P can come as the kitty
//! keyboard protocol's `CSI 112;6u` or modifyOtherKeys' `CSI 27;6;112~`
//! rather than Ctrl-P's byte; both are read as the same key, and bindings
//! match it whichever form it came in.
//!
//! Encoding goes the other way, as xterm sends keys, mouse and paste to a
//! program, in the modes the pane's program set (`PaneModes`): a key in the
//! CSI u form only to a program that asked for keys told apart, through the
//! kitty protocol or modifyOtherKeys, so Notes gets Ctrl-Shift-P while a
//! shell still gets Ctrl-P's byte for it.

use std::time::{Duration, Instant};

use super::{MouseReporting, PaneModes};

/// How long a lone ESC waits for the rest of a sequence before it counts as
/// the Escape key (tmux's `escape-time`).
pub const ESC_TIMEOUT: Duration = Duration::from_millis(10);

/// How long a bracketed paste may go without its next bytes before what came
/// of it is let go as a paste. Far longer than any gap within a real paste,
/// which arrives as fast as the terminal writes it.
pub const PASTE_TIMEOUT: Duration = Duration::from_secs(2);

/// The longest a CSI sequence is waited on before its first two bytes are
/// read as M-[ and the rest as keys; a terminal's run to a few dozen bytes.
const MAX_CSI: usize = 256;

/// The longest an OSC, DCS or APC string is waited on, likewise.
const MAX_STRING: usize = 1 << 20;

/// Lines one notch of the wheel scrolls when it sends arrow keys, as
/// alacritty's alternate scroll does.
const WHEEL_LINES: usize = 3;

const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

/// Modifier keys held with a key or mouse event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

impl Mods {
    pub const NONE: Mods = Mods {
        shift: false,
        alt: false,
        ctrl: false,
    };
    pub const SHIFT: Mods = Mods {
        shift: true,
        ..Mods::NONE
    };
    pub const ALT: Mods = Mods {
        alt: true,
        ..Mods::NONE
    };
    pub const CTRL: Mods = Mods {
        ctrl: true,
        ..Mods::NONE
    };

    /// Whether no modifier is held.
    pub fn is_empty(self) -> bool {
        self == Mods::NONE
    }

    /// The modifiers of both held at once.
    pub fn union(self, other: Mods) -> Mods {
        Mods {
            shift: self.shift || other.shift,
            alt: self.alt || other.alt,
            ctrl: self.ctrl || other.ctrl,
        }
    }

    /// xterm's modifier parameter: 1, plus 1 for Shift, 2 for Alt, 4 for Ctrl.
    fn param(self) -> u32 {
        1 + u32::from(self.shift) + 2 * u32::from(self.alt) + 4 * u32::from(self.ctrl)
    }

    /// The modifiers a parameter of that form says, `None` for an absent
    /// parameter's default of none. Super, Hyper and Meta (8, 16 and 32 in
    /// kitty's form, Meta in xterm's) have no name in tmux's notation, so a
    /// key held with one isn't read as a key; Caps Lock and Num Lock (64 and
    /// 128) don't change which key it is.
    fn from_param(param: Option<u32>) -> Option<Mods> {
        let bits = param.unwrap_or(1).saturating_sub(1);
        if bits & 0b11_1000 != 0 || bits > 0xff {
            return None;
        }
        Some(Mods {
            shift: bits & 1 != 0,
            alt: bits & 2 != 0,
            ctrl: bits & 4 != 0,
        })
    }
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

/// tmux's names for the keys that aren't a character, the first of each the
/// one `notation` writes. Matched without regard to case, as tmux does.
const NAMES: &[(&str, KeyCode)] = &[
    ("Enter", KeyCode::Enter),
    ("Escape", KeyCode::Esc),
    ("Tab", KeyCode::Tab),
    ("BTab", KeyCode::BackTab),
    ("BSpace", KeyCode::Backspace),
    ("Space", KeyCode::Char(' ')),
    ("Up", KeyCode::Up),
    ("Down", KeyCode::Down),
    ("Left", KeyCode::Left),
    ("Right", KeyCode::Right),
    ("Home", KeyCode::Home),
    ("End", KeyCode::End),
    ("PPage", KeyCode::PageUp),
    ("PageUp", KeyCode::PageUp),
    ("PgUp", KeyCode::PageUp),
    ("NPage", KeyCode::PageDown),
    ("PageDown", KeyCode::PageDown),
    ("PgDn", KeyCode::PageDown),
    ("IC", KeyCode::Insert),
    ("Insert", KeyCode::Insert),
    ("DC", KeyCode::Delete),
    ("Delete", KeyCode::Delete),
];

impl Key {
    pub fn new(code: KeyCode, mods: Mods) -> Key {
        Key { code, mods }
    }

    fn plain(code: KeyCode) -> Key {
        Key::new(code, Mods::NONE)
    }

    fn with_alt(self) -> Key {
        Key::new(self.code, self.mods.union(Mods::ALT))
    }

    /// A key from tmux's notation: `M-a`, `C-w`, `M-S-Left`, `M-Space`,
    /// `M-Tab`, `Enter`, `Escape`, `BSpace`, `M-[`, `M-J`, `F1`, `C-M-x`,
    /// `BTab`; `None` when it isn't one.
    pub fn parse(notation: &str) -> Option<Key> {
        let mut mods = Mods::NONE;
        let mut rest = notation;
        // A modifier is a letter and a dash with something after it, so `M--`
        // is Alt with `-`, and `-` alone is the key `-`.
        while let [m, b'-', _, ..] = rest.as_bytes() {
            match m.to_ascii_uppercase() {
                b'C' => mods.ctrl = true,
                b'M' => mods.alt = true,
                b'S' => mods.shift = true,
                _ => return None,
            }
            rest = &rest[2..];
        }
        // tmux's other way to write Ctrl: `^a`.
        if rest.len() > 1
            && let Some(after) = rest.strip_prefix('^')
        {
            mods.ctrl = true;
            rest = after;
        }
        let code = if let Some(&(_, code)) = NAMES
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(rest))
        {
            code
        } else if let Some(n) = rest
            .strip_prefix(['F', 'f'])
            .filter(|n| n.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|n| n.parse::<u8>().ok())
            .filter(|n| (1..=12).contains(n))
        {
            KeyCode::F(n)
        } else {
            let mut chars = rest.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => KeyCode::Char(c),
                _ => return None,
            }
        };
        Some(Key::new(code, mods))
    }

    /// The key in tmux's notation, as `parse` reads it.
    pub fn notation(&self) -> String {
        let key = self.normalized();
        let mut out = String::new();
        // tmux's order, so `C-M-x`, not `M-C-x`.
        if key.mods.ctrl {
            out.push_str("C-");
        }
        if key.mods.alt {
            out.push_str("M-");
        }
        if key.mods.shift {
            out.push_str("S-");
        }
        match key.code {
            KeyCode::Char(' ') => out.push_str("Space"),
            KeyCode::Char(c) => out.push(c),
            KeyCode::F(n) => out.push_str(&format!("F{n}")),
            code => {
                let name = NAMES
                    .iter()
                    .find(|(_, c)| *c == code)
                    .map_or("", |(name, _)| *name);
                out.push_str(name);
            }
        }
        out
    }

    /// The canonical form bindings are matched in: a letter with Shift is
    /// its capital without Shift; Ctrl with a letter is lowercase. Ctrl and
    /// Shift with a letter keep Shift as a modifier of its own, since only a
    /// terminal that tells `C-S-s` from `C-s` sends it (Ghostty's Cmd keys
    /// arrive as `C-M-S-s`). A control character is the key that types it
    /// (`\x01` is C-a, `\t` is Tab), Shift-Tab is BTab, and C-@ is C-Space.
    pub fn normalized(&self) -> Key {
        let mut key = *self;
        if let KeyCode::Char(c) = key.code
            && (c < ' ' || c == '\x7f')
        {
            let typed = c0_key(c as u8);
            key = Key::new(typed.code, key.mods.union(typed.mods));
        }
        match key.code {
            KeyCode::Char('@') if key.mods.ctrl => key.code = KeyCode::Char(' '),
            KeyCode::Char(c) if key.mods.ctrl => {
                if let Some(lower) = single(c.to_lowercase()) {
                    key.code = KeyCode::Char(lower);
                }
            }
            KeyCode::Char(c) if key.mods.shift => {
                if c.is_uppercase() {
                    key.mods.shift = false;
                } else if c.is_lowercase()
                    && let Some(upper) = single(c.to_uppercase())
                {
                    key.code = KeyCode::Char(upper);
                    key.mods.shift = false;
                }
            }
            KeyCode::Tab if key.mods.shift => {
                key.code = KeyCode::BackTab;
                key.mods.shift = false;
            }
            KeyCode::BackTab => key.mods.shift = false,
            _ => {}
        }
        key
    }
}

/// The one character a case mapping gives, or `None` when it gives several
/// (`ß` to `SS`), which isn't a key.
fn single(mut chars: impl Iterator<Item = char>) -> Option<char> {
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

/// The key a C0 control byte or DEL is typed as: the keys that have a byte of
/// their own (Tab, Enter, Escape, Backspace) as themselves, and Ctrl with the
/// character 64 above it for the rest (0x01 C-a, 0x1c C-\, 0x00 C-Space).
fn c0_key(byte: u8) -> Key {
    let ctrl = |c: u8| Key::new(KeyCode::Char(c as char), Mods::CTRL);
    match byte {
        0x00 => ctrl(b' '),
        b'\t' => Key::plain(KeyCode::Tab),
        b'\r' => Key::plain(KeyCode::Enter),
        0x1b => Key::plain(KeyCode::Esc),
        0x7f => Key::plain(KeyCode::Backspace),
        0x01..=0x1a => ctrl(b'a' + byte - 1),
        0x1c..=0x1f => ctrl(byte + 0x40),
        _ => Key::plain(KeyCode::Char(byte as char)),
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

impl MouseButton {
    pub fn is_wheel(self) -> bool {
        matches!(
            self,
            MouseButton::WheelUp
                | MouseButton::WheelDown
                | MouseButton::WheelLeft
                | MouseButton::WheelRight
        )
    }
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
    /// Set while a bracketed paste has begun and not ended; `pending` then
    /// starts with its start marker.
    paste: Option<OpenPaste>,
    /// The button last pressed, for the X10 form's release, which doesn't say
    /// which button it was.
    held: Option<MouseButton>,
}

#[derive(Debug)]
struct OpenPaste {
    /// How far `pending` has been searched for the end marker, so a paste
    /// arriving in many reads is searched once, not again with each.
    scanned: usize,
    /// When its last bytes came.
    since: Instant,
}

impl Parser {
    pub fn new() -> Parser {
        Parser::default()
    }

    /// The events in `bytes`, after what was held from before. An ESC at the
    /// end is held, since it may start a sequence: `flush` it once
    /// `ESC_TIMEOUT` passes with nothing more.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Parsed> {
        self.feed_at(bytes, Instant::now())
    }

    /// Whether something is held waiting for the rest of a sequence.
    pub fn pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// The bytes held waiting for the rest of a sequence (a paste's start
    /// marker and what came after it, an ESC), as they came: what a new
    /// build is given first when the server hands a terminal over.
    pub fn held(&self) -> &[u8] {
        &self.pending
    }

    /// What is held, as it stands: a lone ESC as the Escape key, the rest as
    /// keys or `Unknown`. A paste that has begun stays held for its end, until
    /// `PASTE_TIMEOUT` passes with no more of it.
    pub fn flush(&mut self) -> Vec<Parsed> {
        self.flush_at(Instant::now())
    }

    fn feed_at(&mut self, bytes: &[u8], now: Instant) -> Vec<Parsed> {
        let mut out = Vec::new();
        if self.paste_lapsed(now) {
            self.let_go_of_paste(&mut out);
        }
        self.pending.extend_from_slice(bytes);
        self.read(false, now, &mut out);
        out
    }

    fn flush_at(&mut self, now: Instant) -> Vec<Parsed> {
        let mut out = Vec::new();
        if self.paste.is_some() {
            if !self.paste_lapsed(now) {
                return out;
            }
            self.let_go_of_paste(&mut out);
        }
        self.read(true, now, &mut out);
        out
    }

    fn paste_lapsed(&self, now: Instant) -> bool {
        self.paste
            .as_ref()
            .is_some_and(|p| now.saturating_duration_since(p.since) >= PASTE_TIMEOUT)
    }

    /// A paste whose end never came, as a paste of what did.
    fn let_go_of_paste(&mut self, out: &mut Vec<Parsed>) {
        self.paste = None;
        let raw = std::mem::take(&mut self.pending);
        let text = String::from_utf8_lossy(raw.get(PASTE_START.len()..).unwrap_or_default());
        out.push(Parsed {
            input: Input::Paste(text.into_owned()),
            raw,
        });
    }

    /// Reads events off the front of `pending` until it is empty or what is
    /// left may still become a longer sequence; with `last`, nothing more is
    /// coming, so everything but an open paste is read as it stands.
    fn read(&mut self, last: bool, now: Instant, out: &mut Vec<Parsed>) {
        let mut at = 0;
        loop {
            if let Some(paste) = &mut self.paste {
                // The end marker may have begun in the bytes already searched.
                let from = paste
                    .scanned
                    .saturating_sub(PASTE_END.len() - 1)
                    .max(PASTE_START.len());
                match find(&self.pending[from..], PASTE_END) {
                    Some(i) => {
                        let end = from + i;
                        let text = String::from_utf8_lossy(&self.pending[PASTE_START.len()..end]);
                        let input = Input::Paste(text.into_owned());
                        at = end + PASTE_END.len();
                        out.push(Parsed {
                            input,
                            raw: self.pending[..at].to_vec(),
                        });
                        self.paste = None;
                    }
                    None => {
                        if self.pending.len() > paste.scanned {
                            paste.since = now;
                        }
                        paste.scanned = self.pending.len();
                        break;
                    }
                }
            }
            if at == self.pending.len() {
                break;
            }
            let rest = &self.pending[at..];
            match step(rest, last, &mut self.held) {
                Step::Event(input, n) => {
                    out.push(Parsed {
                        input,
                        raw: rest[..n].to_vec(),
                    });
                    at += n;
                }
                Step::PasteStart => {
                    self.pending.drain(..at);
                    at = 0;
                    self.paste = Some(OpenPaste {
                        scanned: PASTE_START.len(),
                        since: now,
                    });
                }
                // Every step reads something once nothing more is coming;
                // this only keeps a mistake there from losing the bytes.
                Step::More if last => {
                    out.push(Parsed {
                        input: Input::Unknown,
                        raw: rest.to_vec(),
                    });
                    at = self.pending.len();
                }
                Step::More => break,
            }
        }
        self.pending.drain(..at);
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// What reading at the start of some bytes found.
#[derive(Debug, PartialEq)]
enum Step {
    /// An event, and how many bytes it took (at least one).
    Event(Input, usize),
    /// The start of a bracketed paste (its marker's six bytes).
    PasteStart,
    /// The bytes may begin a sequence whose rest hasn't come.
    More,
}

fn key(key: Key, n: usize) -> Step {
    Step::Event(Input::Key(key), n)
}

fn unknown(n: usize) -> Step {
    Step::Event(Input::Unknown, n)
}

/// ESC and the byte after it read as Alt with that byte, for an ESC `[`,
/// `O`, `]`, `P` or `_` that turned out to begin nothing longer.
fn alt_of_second(buf: &[u8]) -> Step {
    key(Key::new(KeyCode::Char(buf[1] as char), Mods::ALT), 2)
}

/// One event from the front of `buf`, which isn't empty.
fn step(buf: &[u8], last: bool, held: &mut Option<MouseButton>) -> Step {
    match buf[0] {
        0x1b => escape(buf, last, held),
        b @ (0x00..=0x1f | 0x7f) => key(c0_key(b), 1),
        b @ 0x20..=0x7e => key(Key::plain(KeyCode::Char(b as char)), 1),
        _ => utf8(buf, last),
    }
}

/// A character of more than one byte, or `Unknown` for a byte that can't
/// begin or continue one.
fn utf8(buf: &[u8], last: bool) -> Step {
    let need = match buf[0] {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return unknown(1),
    };
    let have = buf.len().min(need);
    if buf[1..have].iter().any(|b| !(0x80..=0xbf).contains(b)) {
        return unknown(1);
    }
    if have < need {
        return if last { unknown(have) } else { Step::More };
    }
    match std::str::from_utf8(&buf[..need]) {
        Ok(s) => s
            .chars()
            .next()
            .map_or(unknown(need), |c| key(Key::plain(KeyCode::Char(c)), need)),
        // An overlong form or a surrogate: the lead byte alone is the error.
        Err(_) => unknown(1),
    }
}

/// What begins with ESC: Escape alone, Alt with a key, or a sequence.
fn escape(buf: &[u8], last: bool, held: &mut Option<MouseButton>) -> Step {
    let Some(&next) = buf.get(1) else {
        return if last {
            key(Key::plain(KeyCode::Esc), 1)
        } else {
            Step::More
        };
    };
    // The key after an ESC is read on a copy of the held button, since its
    // result may be thrown away and the same bytes read again.
    let mut scratch = *held;
    match next {
        b'[' => csi(buf, last, held),
        b'O' => ss3(buf, last),
        b']' => string(buf, last, |b| b.is_ascii_digit()),
        b'P' => string(buf, last, |b| (0x20..=0x3f).contains(&b)),
        b'_' => string(buf, last, |b| b == b'G'),
        0x1b => match buf.get(2) {
            None if last => key(Key::new(KeyCode::Esc, Mods::ALT), 2),
            None => Step::More,
            // Three in a row: the first is Escape alone. Stopping here also
            // keeps a run of ESCs from nesting a call for each.
            Some(0x1b) => key(Key::plain(KeyCode::Esc), 1),
            // ESC before a sequence is Alt with its key (`\e\e[A` is M-Up).
            Some(_) => with_alt(escape(&buf[1..], last, &mut scratch)),
        },
        _ => with_alt(step(&buf[1..], last, &mut scratch)),
    }
}

/// An ESC in front of what was read after it: Alt with that key, or the
/// Escape key alone when what follows is no key, or one with Alt already.
fn with_alt(after: Step) -> Step {
    match after {
        Step::Event(Input::Key(k), n) if !k.mods.alt => key(k.with_alt(), n + 1),
        Step::More => Step::More,
        _ => key(Key::plain(KeyCode::Esc), 1),
    }
}

/// `ESC O` and a final byte (SS3), with a modifier digit between in some
/// terminals' F1-F4; else M-O.
fn ss3(buf: &[u8], last: bool) -> Step {
    let mut i = 2;
    while i < buf.len() && i < 5 && buf[i].is_ascii_digit() {
        i += 1;
    }
    let Some(&fin) = buf.get(i) else {
        return if last { alt_of_second(buf) } else { Step::More };
    };
    let code = match fin {
        b'A' => KeyCode::Up,
        b'B' => KeyCode::Down,
        b'C' => KeyCode::Right,
        b'D' => KeyCode::Left,
        b'H' => KeyCode::Home,
        b'F' => KeyCode::End,
        b'P'..=b'S' => KeyCode::F(fin - b'P' + 1),
        _ => return alt_of_second(buf),
    };
    let param = std::str::from_utf8(&buf[2..i])
        .ok()
        .and_then(|s| s.parse().ok());
    match Mods::from_param(param) {
        Some(mods) => key(Key::new(code, mods), i + 1),
        None => unknown(i + 1),
    }
}

/// An OSC, DCS or APC string (`ESC ]`, `ESC P`, `ESC _`), read whole to its
/// BEL or ST (`ESC \`) as `Unknown` when the byte after the introducer says it
/// is one; else, or when something breaks it off, Alt with the introducer.
fn string(buf: &[u8], last: bool, starts: impl Fn(u8) -> bool) -> Step {
    match buf.get(2) {
        None if last => return alt_of_second(buf),
        None => return Step::More,
        Some(&b) if !starts(b) => return alt_of_second(buf),
        Some(_) => {}
    }
    let end = buf.len().min(MAX_STRING);
    let mut i = 2;
    while i < end {
        match buf[i] {
            0x07 => return unknown(i + 1),
            0x1b => match buf.get(i + 1) {
                Some(b'\\') => return unknown(i + 2),
                Some(_) => return alt_of_second(buf),
                None => break,
            },
            b if b < 0x20 => return alt_of_second(buf),
            _ => i += 1,
        }
    }
    if last || i >= MAX_STRING {
        alt_of_second(buf)
    } else {
        Step::More
    }
}

/// `ESC [` and what follows: a CSI sequence (parameters, intermediates and a
/// final byte), the X10 form of a mouse event, or M-[ when it is neither.
fn csi(buf: &[u8], last: bool, held: &mut Option<MouseButton>) -> Step {
    if buf.get(2) == Some(&b'M') {
        return match buf.get(3..6) {
            Some(bytes) => {
                let input = bytes[0]
                    .checked_sub(32)
                    .and_then(|code| {
                        let x = u32::from(bytes[1].saturating_sub(32));
                        let y = u32::from(bytes[2].saturating_sub(32));
                        mouse(u32::from(code), x, y, Form::X10, held)
                    })
                    .unwrap_or(Input::Unknown);
                Step::Event(input, 6)
            }
            None if last => alt_of_second(buf),
            None => Step::More,
        };
    }
    let end = buf.len().min(MAX_CSI);
    let mut i = 2;
    while i < end && (0x30..=0x3f).contains(&buf[i]) {
        i += 1;
    }
    let params_end = i;
    while i < end && (0x20..=0x2f).contains(&buf[i]) {
        i += 1;
    }
    if i >= MAX_CSI {
        return alt_of_second(buf);
    }
    match buf.get(i) {
        None if last => alt_of_second(buf),
        None => Step::More,
        Some(&fin) if (0x40..=0x7e).contains(&fin) => {
            let (params, inter) = (&buf[2..params_end], &buf[params_end..i]);
            if params == b"200" && inter.is_empty() && fin == b'~' {
                return Step::PasteStart;
            }
            let input = csi_input(params, inter, fin, held).unwrap_or(Input::Unknown);
            Step::Event(input, i + 1)
        }
        Some(_) => alt_of_second(buf),
    }
}

/// A CSI sequence's parameters: fields split by `;`, each into sub-parameters
/// by `:`, an empty one as `None`.
struct Params(Vec<Vec<Option<u32>>>);

impl Params {
    /// `None` when they hold anything but digits and separators.
    fn parse(bytes: &[u8]) -> Option<Params> {
        if bytes.is_empty() {
            return Some(Params(Vec::new()));
        }
        let fields = bytes
            .split(|&b| b == b';')
            .map(|field| {
                field
                    .split(|&b| b == b':')
                    .map(|sub| {
                        if sub.is_empty() {
                            Some(None)
                        } else if sub.iter().all(u8::is_ascii_digit) {
                            std::str::from_utf8(sub).ok()?.parse().ok().map(Some)
                        } else {
                            None
                        }
                    })
                    .collect::<Option<Vec<_>>>()
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Params(fields))
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    fn get(&self, field: usize, sub: usize) -> Option<u32> {
        self.0.get(field)?.get(sub).copied().flatten()
    }
}

/// What a complete CSI sequence is, `None` when it isn't one understood.
fn csi_input(
    params: &[u8],
    inter: &[u8],
    fin: u8,
    held: &mut Option<MouseButton>,
) -> Option<Input> {
    if !inter.is_empty() {
        return None;
    }
    if let Some(sgr) = params.strip_prefix(b"<") {
        let p = Params::parse(sgr)?;
        if !matches!(fin, b'M' | b'm') || p.len() != 3 {
            return None;
        }
        let (code, x, y) = (p.get(0, 0)?, p.get(1, 0)?, p.get(2, 0)?);
        return mouse(
            code,
            x,
            y,
            Form::Sgr {
                release: fin == b'm',
            },
            held,
        );
    }
    // Any other private marker (`?`, `>`, `=`) is a reply to a query.
    let p = Params::parse(params)?;
    match fin {
        b'A'..=b'D' | b'H' | b'F' | b'P'..=b'S' | b'Z' => {
            // No parameters, or `1;<modifiers>`.
            if p.len() > 2 || !matches!(p.get(0, 0), None | Some(1)) {
                return None;
            }
            let code = match fin {
                b'A' => KeyCode::Up,
                b'B' => KeyCode::Down,
                b'C' => KeyCode::Right,
                b'D' => KeyCode::Left,
                b'H' => KeyCode::Home,
                b'F' => KeyCode::End,
                b'Z' => KeyCode::BackTab,
                _ => KeyCode::F(fin - b'P' + 1),
            };
            Some(Input::Key(Key::new(code, Mods::from_param(p.get(1, 0))?)))
        }
        b'I' if params.is_empty() => Some(Input::FocusIn),
        b'O' if params.is_empty() => Some(Input::FocusOut),
        b'~' => tilde(&p),
        b'u' => csi_u(&p),
        // urxvt's form (1015): the X10 form's numbers in decimal.
        b'M' if p.len() == 3 => {
            let code = p.get(0, 0)?.checked_sub(32)?;
            mouse(code, p.get(1, 0)?, p.get(2, 0)?, Form::X10, held)
        }
        _ => None,
    }
}

/// `CSI <n> ~` and `CSI <n> ; <mods> ~`: the editing and function keys, and
/// xterm's modifyOtherKeys form, `CSI 27 ; <mods> ; <code> ~`.
fn tilde(p: &Params) -> Option<Input> {
    let n = p.get(0, 0)?;
    let mods = Mods::from_param(p.get(1, 0))?;
    if n == 27 {
        let key = code_key(p.get(2, 0)?)?;
        return Some(Input::Key(Key::new(key.code, key.mods.union(mods))));
    }
    if p.len() > 2 {
        return None;
    }
    let code = match n {
        1 | 7 => KeyCode::Home,
        2 => KeyCode::Insert,
        3 => KeyCode::Delete,
        4 | 8 => KeyCode::End,
        5 => KeyCode::PageUp,
        6 => KeyCode::PageDown,
        // rxvt's and old xterm's F1-F4.
        11..=15 => KeyCode::F((n - 10) as u8),
        17..=21 => KeyCode::F((n - 11) as u8),
        23 | 24 => KeyCode::F((n - 12) as u8),
        _ => return None,
    };
    Some(Input::Key(Key::new(code, mods)))
}

/// `CSI <code> ; <mods> u`, the kitty keyboard protocol's form (also sent by
/// some terminals for keys the old forms can't tell apart, Shift-Enter among
/// them). Only a press or a repeat is a key; a release isn't.
fn csi_u(p: &Params) -> Option<Input> {
    let mods = Mods::from_param(p.get(1, 0))?;
    if !matches!(p.get(1, 1), None | Some(1) | Some(2)) {
        return None;
    }
    let key = code_key(p.get(0, 0)?)?;
    Some(Input::Key(Key::new(key.code, key.mods.union(mods))))
}

/// The key a Unicode code point names in the kitty and modifyOtherKeys forms:
/// a control character as the key it is typed with, the keypad's own codes
/// as the keys they are, and any other private-use code as none.
fn code_key(code: u32) -> Option<Key> {
    let plain = |code| Some(Key::plain(code));
    let char = |c| plain(KeyCode::Char(c));
    match code {
        0x00..=0x1f | 0x7f => Some(c0_key(code as u8)),
        57399..=57408 => char(char::from(b'0' + (code - 57399) as u8)),
        57409 => char('.'),
        57410 => char('/'),
        57411 => char('*'),
        57412 => char('-'),
        57413 => char('+'),
        57414 => plain(KeyCode::Enter),
        57415 => char('='),
        57416 => char(','),
        57417 => plain(KeyCode::Left),
        57418 => plain(KeyCode::Right),
        57419 => plain(KeyCode::Up),
        57420 => plain(KeyCode::Down),
        57421 => plain(KeyCode::PageUp),
        57422 => plain(KeyCode::PageDown),
        57423 => plain(KeyCode::Home),
        57424 => plain(KeyCode::End),
        57425 => plain(KeyCode::Insert),
        57426 => plain(KeyCode::Delete),
        0xe000..=0xf8ff => None,
        _ => char::from_u32(code).and_then(char),
    }
}

/// Which form a mouse event came in, for what its release looks like.
enum Form {
    /// `CSI < ... M` or `m`, `m` the release, which names its button.
    Sgr { release: bool },
    /// The X10 and urxvt forms, whose release is button 3, naming none.
    X10,
}

/// A mouse event from its button code and 1-based position. The code's low
/// two bits are the button, 4, 8 and 16 Shift, Alt and Ctrl, 32 movement and
/// 64 the wheel; 128 (buttons 8 to 11, back and forward) has no name here.
fn mouse(code: u32, x: u32, y: u32, form: Form, held: &mut Option<MouseButton>) -> Option<Input> {
    if code & 128 != 0 || code > 255 {
        return None;
    }
    let mods = Mods {
        shift: code & 4 != 0,
        alt: code & 8 != 0,
        ctrl: code & 16 != 0,
    };
    let low = (code & 3) as usize;
    let pressed = [
        MouseButton::Left,
        MouseButton::Middle,
        MouseButton::Right,
        MouseButton::None,
    ][low];
    let (kind, button) = if code & 64 != 0 {
        let wheel = [
            MouseButton::WheelUp,
            MouseButton::WheelDown,
            MouseButton::WheelLeft,
            MouseButton::WheelRight,
        ][low];
        match form {
            Form::Sgr { release: true } => (MouseKind::Release, wheel),
            _ => (MouseKind::Press, wheel),
        }
    } else if let Form::Sgr { release: true } = form {
        *held = None;
        (MouseKind::Release, pressed)
    } else if code & 32 != 0 {
        match pressed {
            MouseButton::None => (MouseKind::Move, pressed),
            _ => (MouseKind::Drag, pressed),
        }
    } else if pressed == MouseButton::None {
        // The X10 form's release; SGR never sends button 3 pressed.
        (MouseKind::Release, held.take().unwrap_or(MouseButton::Left))
    } else {
        *held = Some(pressed);
        (MouseKind::Press, pressed)
    };
    let cell = |n: u32| u16::try_from(n.saturating_sub(1)).unwrap_or(u16::MAX);
    Some(Input::Mouse(Mouse {
        kind,
        button,
        col: cell(x),
        row: cell(y),
        mods,
    }))
}

/// The bytes a key is sent to a pane as, in its modes: in the CSI u form
/// when its program asked for keys told apart and this is one it wants so
/// (`extended`), else as a terminal has always sent it.
pub fn encode_key(key: &Key, modes: &PaneModes) -> Vec<u8> {
    let key = key.normalized();
    if let Some(bytes) = extended(&key, modes) {
        return bytes;
    }
    let mods = key.mods;
    let mut out = Vec::new();
    // The keys sent as a byte or two: Alt is an ESC in front.
    let mut prefixed = |bytes: &[u8]| {
        if mods.alt {
            out.push(0x1b);
        }
        out.extend_from_slice(bytes);
    };
    match key.code {
        KeyCode::Char(c) => match ctrl_byte(c).filter(|_| mods.ctrl) {
            Some(byte) => prefixed(&[byte]),
            None => prefixed(c.encode_utf8(&mut [0; 4]).as_bytes()),
        },
        KeyCode::Enter => prefixed(b"\r"),
        KeyCode::Tab => prefixed(b"\t"),
        KeyCode::BackTab => prefixed(b"\x1b[Z"),
        KeyCode::Backspace => prefixed(if mods.ctrl { b"\x08" } else { b"\x7f" }),
        KeyCode::Esc => prefixed(b"\x1b"),
        KeyCode::Up => return cursor(b'A', mods, modes),
        KeyCode::Down => return cursor(b'B', mods, modes),
        KeyCode::Right => return cursor(b'C', mods, modes),
        KeyCode::Left => return cursor(b'D', mods, modes),
        KeyCode::Home => return cursor(b'H', mods, modes),
        KeyCode::End => return cursor(b'F', mods, modes),
        KeyCode::Insert => return tilde_key(2, mods),
        KeyCode::Delete => return tilde_key(3, mods),
        KeyCode::PageUp => return tilde_key(5, mods),
        KeyCode::PageDown => return tilde_key(6, mods),
        KeyCode::F(n @ 1..=4) => {
            let fin = b'P' + n - 1;
            return if mods.is_empty() {
                vec![0x1b, b'O', fin]
            } else {
                format!("\x1b[1;{}{}", mods.param(), fin as char).into_bytes()
            };
        }
        KeyCode::F(n @ 5..=12) => {
            let number = [15, 17, 18, 19, 20, 21, 23, 24][usize::from(n - 5)];
            return tilde_key(number, mods);
        }
        // There is no sequence for a function key past F12.
        KeyCode::F(_) => {}
    }
    out
}

/// A key (normalized) as `CSI <code> ; <modifiers> u`, when its pane's
/// program asked for that form and wants this key in it; `None` to send it
/// the old way. The cursor, editing and function keys are left out: their
/// old forms carry every modifier already.
///
/// Under the kitty keyboard protocol (any of its flags), as its spec has a
/// terminal send them: Escape and Shift-Tab, any key with Ctrl or Alt, and
/// Enter, Tab and Backspace with any modifier, those three alone keeping
/// their bytes so a shell left in the mode still runs a command; with its
/// flag 8 every key, and the text it types after when flag 16 asks. Under
/// modifyOtherKeys level 2, every key with a modifier but Shift with a
/// character, which only types it; under level 1, only the keys whose old
/// bytes lose a modifier (`lossy`).
fn extended(key: &Key, modes: &PaneModes) -> Option<Vec<u8>> {
    let (kitty, level) = (modes.kitty_keys, modes.modify_other_keys);
    if kitty == 0 && level == 0 {
        return None;
    }
    let mods = key.mods;
    let (code, mods) = match key.code {
        // A capital as its key with Shift, as kitty names a key.
        KeyCode::Char(c) => match single(c.to_lowercase()).filter(|_| c.is_uppercase()) {
            Some(lower) => (u32::from(lower), mods.union(Mods::SHIFT)),
            None => (u32::from(c), mods),
        },
        KeyCode::Enter => (13, mods),
        KeyCode::Tab => (9, mods),
        KeyCode::BackTab => (9, mods.union(Mods::SHIFT)),
        KeyCode::Backspace => (127, mods),
        KeyCode::Esc => (27, mods),
        _ => return None,
    };
    let types_text = matches!(key.code, KeyCode::Char(_)) && !key.mods.ctrl && !key.mods.alt;
    let wanted = if kitty & 8 != 0 {
        true
    } else if kitty != 0 {
        !types_text && (!key.mods.is_empty() || matches!(key.code, KeyCode::Esc | KeyCode::BackTab))
    } else if level >= 2 {
        !types_text && !key.mods.is_empty()
    } else {
        lossy(key)
    };
    if !wanted {
        return None;
    }
    let param = mods.param();
    let text = match key.code {
        KeyCode::Char(c) if types_text && kitty & 16 != 0 => Some(u32::from(c)),
        _ => None,
    };
    Some(
        match (param, text) {
            (_, Some(text)) => format!("\x1b[{code};{param};{text}u"),
            (1, None) => format!("\x1b[{code}u"),
            (_, None) => format!("\x1b[{code};{param}u"),
        }
        .into_bytes(),
    )
}

/// Whether a key's old bytes lose one of its modifiers, so a program that
/// asked modifyOtherKeys level 1 gets it as CSI u: Ctrl with a character
/// it doesn't change, or with Shift (C-S-p is C-p's byte); Enter, Escape
/// and Backspace with Ctrl or Shift; Tab and Shift-Tab with Ctrl. The well
/// known ones (C-i is Tab's byte, C-[ Escape's) stay as they are.
fn lossy(key: &Key) -> bool {
    let m = key.mods;
    match key.code {
        KeyCode::Char(c) => m.ctrl && (m.shift || ctrl_byte(c).is_none()),
        KeyCode::Enter | KeyCode::Esc | KeyCode::Backspace => m.ctrl || m.shift,
        KeyCode::Tab | KeyCode::BackTab => m.ctrl,
        _ => false,
    }
}

/// Whether a sequence the parser didn't understand is passed to a pane as
/// it came. A key in the CSI u or modifyOtherKeys form that has no name
/// here (F13 and on, the media keys) reaches only a program that asked
/// for keys in such a form; any other would type it as text.
pub fn passes_unknown(raw: &[u8], modes: &PaneModes) -> bool {
    let Some(body) = raw.strip_prefix(b"\x1b[") else {
        return true;
    };
    let key_form = match body.split_last() {
        Some((b'u', params)) => {
            params.first().is_some_and(u8::is_ascii_digit)
                && params
                    .iter()
                    .all(|b| b.is_ascii_digit() || b":;".contains(b))
        }
        Some((b'~', params)) => params.starts_with(b"27;"),
        _ => false,
    };
    !key_form || modes.kitty_keys != 0 || modes.modify_other_keys != 0
}

/// The byte Ctrl with a character sends, as xterm sends it; `None` for a
/// character Ctrl doesn't change, which is sent as itself.
fn ctrl_byte(c: char) -> Option<u8> {
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c as u8 - b'A' + 1),
        ' ' | '@' | '2' => Some(0x00),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' | '~' => Some(0x1e),
        '_' | '7' | '/' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

/// A cursor key (or Home or End): SS3 in the cursor keys' application mode
/// and CSI otherwise, and `CSI 1 ; <mods>` with a modifier in either.
fn cursor(fin: u8, mods: Mods, modes: &PaneModes) -> Vec<u8> {
    if !mods.is_empty() {
        format!("\x1b[1;{}{}", mods.param(), fin as char).into_bytes()
    } else if modes.app_cursor {
        vec![0x1b, b'O', fin]
    } else {
        vec![0x1b, b'[', fin]
    }
}

fn tilde_key(number: u32, mods: Mods) -> Vec<u8> {
    if mods.is_empty() {
        format!("\x1b[{number}~").into_bytes()
    } else {
        format!("\x1b[{number};{}~", mods.param()).into_bytes()
    }
}

/// The bytes a mouse event (its position already the pane's own) is sent to
/// a pane as, or `None` when the pane didn't ask for that kind of event. A
/// pane that takes no mouse events but asked for alternate scroll gets the
/// wheel on its alternate screen as arrow keys, as alacritty sends it, so a
/// pager scrolls under the wheel.
pub fn encode_mouse(mouse: &Mouse, modes: &PaneModes) -> Option<Vec<u8>> {
    let takes = match modes.mouse {
        MouseReporting::Off => return alternate_scroll(mouse, modes),
        MouseReporting::Press => matches!(mouse.kind, MouseKind::Press | MouseKind::Release),
        MouseReporting::Drag => mouse.kind != MouseKind::Move,
        MouseReporting::Motion => true,
    };
    if !takes {
        return None;
    }
    let mut code: u32 = match mouse.button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::None => 3,
        MouseButton::WheelUp => 64,
        MouseButton::WheelDown => 65,
        MouseButton::WheelLeft => 66,
        MouseButton::WheelRight => 67,
    };
    if matches!(mouse.kind, MouseKind::Drag | MouseKind::Move) {
        code |= 32;
    }
    code |= 4 * u32::from(mouse.mods.shift)
        + 8 * u32::from(mouse.mods.alt)
        + 16 * u32::from(mouse.mods.ctrl);
    let (x, y) = (u32::from(mouse.col) + 1, u32::from(mouse.row) + 1);
    if modes.mouse_sgr {
        let fin = if mouse.kind == MouseKind::Release {
            'm'
        } else {
            'M'
        };
        return Some(format!("\x1b[<{code};{x};{y}{fin}").into_bytes());
    }
    if mouse.kind == MouseKind::Release {
        // The X10 form's release is button 3, and the wheel has none.
        if mouse.button.is_wheel() {
            return None;
        }
        code |= 3;
    }
    // Each value is offset by 32 into one byte, so a position past 223 can't
    // be told (it is sent as 223), or, with 1005, as a UTF-8 character, up to
    // the two bytes xterm sends.
    let (max, at_most) = if modes.mouse_utf8 {
        (0x7ff, 0x7ff - 32)
    } else {
        (0xff, 0xff - 32)
    };
    let mut out = b"\x1b[M".to_vec();
    for value in [code, x.min(at_most), y.min(at_most)] {
        let value = (32 + value).min(max);
        if modes.mouse_utf8 {
            let c = char::from_u32(value).unwrap_or(' ');
            out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
        } else {
            out.push(value as u8);
        }
    }
    Some(out)
}

fn alternate_scroll(mouse: &Mouse, modes: &PaneModes) -> Option<Vec<u8>> {
    if !(modes.alt_screen && modes.alternate_scroll) || mouse.kind != MouseKind::Press {
        return None;
    }
    let code = match mouse.button {
        MouseButton::WheelUp => KeyCode::Up,
        MouseButton::WheelDown => KeyCode::Down,
        _ => return None,
    };
    Some(encode_key(&Key::plain(code), modes).repeat(WHEEL_LINES))
}

/// Pasted text as sent to a pane: bracketed when it asked, its own markers
/// taken out so a paste can't end the bracket early. Line ends go as the
/// carriage return Enter sends, `\r\n` as one, as terminals send a paste, so
/// a shell without bracketed paste runs each line rather than sitting on it.
pub fn encode_paste(text: &str, modes: &PaneModes) -> Vec<u8> {
    let mut text = text.replace("\r\n", "\r").replace('\n', "\r");
    // Taken out until none is left, since taking one out can join the text
    // around it into another.
    loop {
        let before = text.len();
        text = text.replace("\x1b[200~", "").replace("\x1b[201~", "");
        if text.len() == before {
            break;
        }
    }
    if !modes.bracketed_paste {
        return text.into_bytes();
    }
    let mut out = PASTE_START.to_vec();
    out.extend_from_slice(text.as_bytes());
    out.extend_from_slice(PASTE_END);
    out
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn k(notation: &str) -> Key {
        Key::parse(notation).unwrap_or_else(|| panic!("{notation:?} isn't a key"))
    }

    /// Everything the bytes make, flushed as the server would once nothing
    /// more comes.
    fn all(bytes: &[u8]) -> Vec<Parsed> {
        let mut p = Parser::new();
        let mut events = p.feed(bytes);
        events.extend(p.flush());
        assert!(!p.pending(), "{bytes:?} left something held");
        events
    }

    /// Each event as a key's notation, or the event itself for the rest.
    fn names_of(events: &[Parsed]) -> Vec<String> {
        events
            .iter()
            .map(|e| match &e.input {
                Input::Key(key) => key.notation(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    fn names(bytes: &[u8]) -> Vec<String> {
        names_of(&all(bytes))
    }

    fn mods(shift: bool, alt: bool, ctrl: bool) -> Mods {
        Mods { shift, alt, ctrl }
    }

    fn at(kind: MouseKind, button: MouseButton, col: u16, row: u16) -> Mouse {
        Mouse {
            kind,
            button,
            col,
            row,
            mods: Mods::NONE,
        }
    }

    fn one_mouse(bytes: &[u8]) -> Mouse {
        match all(bytes).as_slice() {
            [
                Parsed {
                    input: Input::Mouse(m),
                    raw,
                },
            ] if raw == bytes => *m,
            other => panic!("{bytes:?} read as {other:?}"),
        }
    }

    // ── Reading ─────────────────────────────────────────────────────────────

    #[test]
    fn text_is_its_characters() {
        assert_eq!(names(b"ab ~"), ["a", "b", "Space", "~"]);
        assert_eq!(names("é日🙂".as_bytes()), ["é", "日", "🙂"]);
    }

    #[test]
    fn a_character_split_between_reads_waits_for_the_rest() {
        let bytes = "🙂".as_bytes();
        for cut in 1..bytes.len() {
            let mut p = Parser::new();
            assert!(p.feed(&bytes[..cut]).is_empty());
            assert!(p.pending());
            let got = p.feed(&bytes[cut..]);
            assert_eq!(names_of(&got), ["🙂"]);
            assert_eq!(got[0].raw, bytes);
            assert!(!p.pending());
        }
    }

    #[test]
    fn control_bytes_are_the_keys_that_type_them() {
        for b in 0x01..=0x1au8 {
            let want = match b {
                0x09 => "Tab".to_string(),
                0x0d => "Enter".to_string(),
                _ => format!("C-{}", (b'a' + b - 1) as char),
            };
            assert_eq!(names(&[b]), [want], "byte {b:#04x}");
        }
        for (b, want) in [
            (0x00, "C-Space"),
            (0x08, "C-h"),
            (0x0a, "C-j"),
            (0x1c, "C-\\"),
            (0x1d, "C-]"),
            (0x1e, "C-^"),
            (0x1f, "C-_"),
            (0x7f, "BSpace"),
        ] {
            assert_eq!(names(&[b]), [want], "byte {b:#04x}");
        }
    }

    #[test]
    fn a_lone_escape_waits_for_the_flush() {
        let mut p = Parser::new();
        assert!(!p.pending());
        assert!(p.feed(b"\x1b").is_empty());
        assert!(p.pending());
        assert_eq!(
            p.flush(),
            [Parsed {
                input: Input::Key(k("Escape")),
                raw: vec![0x1b],
            }]
        );
        assert!(!p.pending());
        assert!(p.flush().is_empty());
    }

    #[test]
    fn escape_and_a_key_are_alt_unless_flushed_between() {
        let mut p = Parser::new();
        assert!(p.feed(b"\x1b").is_empty());
        assert_eq!(names_of(&p.feed(b"j")), ["M-j"]);

        let mut p = Parser::new();
        assert!(p.feed(b"\x1b").is_empty());
        assert_eq!(names_of(&p.flush()), ["Escape"]);
        assert_eq!(names_of(&p.feed(b"j")), ["j"]);
    }

    #[test]
    fn escape_before_a_key_is_alt() {
        for (bytes, want) in [
            (&b"\x1bj"[..], "M-j"),
            (b"\x1bJ", "M-J"),
            (b"\x1b0", "M-0"),
            (b"\x1b=", "M-="),
            (b"\x1b-", "M--"),
            (b"\x1b\x01", "C-M-a"),
            (b"\x1b\x00", "C-M-Space"),
            (b"\x1b\x7f", "M-BSpace"),
            (b"\x1b\r", "M-Enter"),
            (b"\x1b\t", "M-Tab"),
            (b"\x1b ", "M-Space"),
            ("\x1bé".as_bytes(), "M-é"),
        ] {
            assert_eq!(names(bytes), [want], "{bytes:?}");
            assert_eq!(all(bytes)[0].raw, bytes);
        }
    }

    #[test]
    fn an_introducer_alone_is_an_alt_key_once_flushed() {
        for (bytes, want) in [
            (&b"\x1b["[..], "M-["),
            (b"\x1bO", "M-O"),
            (b"\x1b]", "M-]"),
            (b"\x1bP", "M-P"),
            (b"\x1b_", "M-_"),
            (b"\x1b\x1b", "M-Escape"),
        ] {
            let mut p = Parser::new();
            assert!(p.feed(bytes).is_empty(), "{bytes:?} wasn't held");
            assert!(p.pending());
            let got = p.flush();
            assert_eq!(names_of(&got), [want], "{bytes:?}");
            assert_eq!(got[0].raw, bytes);
        }
    }

    #[test]
    fn an_introducer_before_what_cant_follow_it_is_an_alt_key_at_once() {
        for (bytes, want) in [
            (&b"\x1b[\x01"[..], vec!["M-[", "C-a"]),
            (b"\x1b[\x1bj", vec!["M-[", "M-j"]),
            (b"\x1bOx", vec!["M-O", "x"]),
            (b"\x1bO2x", vec!["M-O", "2", "x"]),
            (b"\x1b]x", vec!["M-]", "x"]),
            (b"\x1bPa", vec!["M-P", "a"]),
            (b"\x1b_x", vec!["M-_", "x"]),
            // An OSC broken off by another key.
            (b"\x1b]1\r", vec!["M-]", "1", "Enter"]),
            (b"\x1b]1\x1bj", vec!["M-]", "1", "M-j"]),
        ] {
            let mut p = Parser::new();
            let got = p.feed(bytes);
            assert_eq!(names_of(&got), want, "{bytes:?}");
            assert!(!p.pending());
        }
    }

    #[test]
    fn an_unfinished_sequence_flushes_as_keys() {
        for (bytes, want) in [
            (&b"\x1b[1;"[..], vec!["M-[", "1", ";"]),
            (b"\x1b[<0;5", vec!["M-[", "<", "0", ";", "5"]),
            (b"\x1bO2", vec!["M-O", "2"]),
            (b"\x1b]11;rgb", vec!["M-]", "1", "1", ";", "r", "g", "b"]),
            (b"\x1b]11;x\x1b", vec!["M-]", "1", "1", ";", "x", "Escape"]),
            (b"\x1b[M!", vec!["M-[", "M", "!"]),
            (b"\x1b\x1b[", vec!["Escape", "M-["]),
            (b"\x1b\x1b\x1b", vec!["Escape", "M-Escape"]),
        ] {
            let mut p = Parser::new();
            let mut got = p.feed(bytes);
            assert!(p.pending(), "{bytes:?} wasn't held");
            got.extend(p.flush());
            assert_eq!(names_of(&got), want, "{bytes:?}");
            let raw: Vec<u8> = got.iter().flat_map(|e| e.raw.clone()).collect();
            assert_eq!(raw, bytes);
        }
    }

    #[test]
    fn escape_before_a_sequence_is_alt_with_its_key() {
        assert_eq!(names(b"\x1b\x1b[A"), ["M-Up"]);
        assert_eq!(names(b"\x1b\x1bOB"), ["M-Down"]);
        assert_eq!(names(b"\x1b\x1b[1;5C"), ["C-M-Right"]);
        assert_eq!(names(b"\x1b\x1b[3~"), ["M-DC"]);
        // A key that has Alt already, or no key, leaves the first ESC alone.
        assert_eq!(names(b"\x1b\x1bj"), ["Escape", "M-j"]);
        assert_eq!(names(b"\x1b\x1b[1;3A"), ["Escape", "M-Up"]);
        assert_eq!(
            names(b"\x1b\x1b[I"),
            ["Escape".to_string(), format!("{:?}", Input::FocusIn)]
        );
        assert_eq!(all(b"\x1b\x1b[A")[0].raw, b"\x1b\x1b[A");
    }

    #[test]
    fn a_sequence_too_long_to_be_one_is_read_as_keys() {
        let mut csi = b"\x1b[".to_vec();
        csi.extend([b'1'; MAX_CSI]);
        csi.push(b'A');
        let got = all(&csi);
        assert_eq!(names_of(&got[..1]), ["M-["]);
        assert_eq!(got.len(), 2 + MAX_CSI);
        let mut p = Parser::new();
        let mut split = p.feed(&csi[..100]);
        assert!(split.is_empty());
        split.extend(p.feed(&csi[100..]));
        assert_eq!(split, got);

        let mut osc = b"\x1b]1;".to_vec();
        osc.resize(MAX_STRING + 10, b'x');
        osc.push(0x07);
        let got = all(&osc);
        assert_eq!(names_of(&got[..3]), ["M-]", "1", ";"]);
        assert_eq!(got.len(), osc.len() - 1);
    }

    #[test]
    fn a_long_run_of_escapes_reads_without_nesting() {
        let bytes = vec![0x1b; 100_000];
        let got = all(&bytes);
        let (last, rest) = got.split_last().unwrap();
        assert_eq!(rest.len(), 99_998);
        assert!(rest.iter().all(|e| e.input == Input::Key(k("Escape"))));
        assert_eq!(last.input, Input::Key(k("M-Escape")));
    }

    #[test]
    fn cursor_function_and_editing_keys() {
        for (bytes, want) in [
            (&b"\x1b[A"[..], "Up"),
            (b"\x1b[B", "Down"),
            (b"\x1b[C", "Right"),
            (b"\x1b[D", "Left"),
            (b"\x1b[H", "Home"),
            (b"\x1b[F", "End"),
            (b"\x1b[1;3A", "M-Up"),
            (b"\x1b[1;2B", "S-Down"),
            (b"\x1b[1;5C", "C-Right"),
            (b"\x1b[1;4D", "M-S-Left"),
            (b"\x1b[1;6H", "C-S-Home"),
            (b"\x1b[1;7F", "C-M-End"),
            (b"\x1b[1;8A", "C-M-S-Up"),
            (b"\x1b[Z", "BTab"),
            (b"\x1b[1;3Z", "M-BTab"),
            (b"\x1bOA", "Up"),
            (b"\x1bOB", "Down"),
            (b"\x1bOC", "Right"),
            (b"\x1bOD", "Left"),
            (b"\x1bOH", "Home"),
            (b"\x1bOF", "End"),
            (b"\x1bOP", "F1"),
            (b"\x1bOQ", "F2"),
            (b"\x1bOR", "F3"),
            (b"\x1bOS", "F4"),
            (b"\x1bO2P", "S-F1"),
            (b"\x1bO5S", "C-F4"),
            (b"\x1b[P", "F1"),
            (b"\x1b[1;2P", "S-F1"),
            (b"\x1b[1;5R", "C-F3"),
            (b"\x1b[1;3S", "M-F4"),
            (b"\x1b[1~", "Home"),
            (b"\x1b[2~", "IC"),
            (b"\x1b[3~", "DC"),
            (b"\x1b[4~", "End"),
            (b"\x1b[5~", "PPage"),
            (b"\x1b[6~", "NPage"),
            (b"\x1b[7~", "Home"),
            (b"\x1b[8~", "End"),
            (b"\x1b[11~", "F1"),
            (b"\x1b[12~", "F2"),
            (b"\x1b[13~", "F3"),
            (b"\x1b[14~", "F4"),
            (b"\x1b[15~", "F5"),
            (b"\x1b[17~", "F6"),
            (b"\x1b[18~", "F7"),
            (b"\x1b[19~", "F8"),
            (b"\x1b[20~", "F9"),
            (b"\x1b[21~", "F10"),
            (b"\x1b[23~", "F11"),
            (b"\x1b[24~", "F12"),
            (b"\x1b[2;2~", "S-IC"),
            (b"\x1b[3;5~", "C-DC"),
            (b"\x1b[5;3~", "M-PPage"),
            (b"\x1b[6;7~", "C-M-NPage"),
            (b"\x1b[15;2~", "S-F5"),
            (b"\x1b[24;8~", "C-M-S-F12"),
            // Caps Lock and Num Lock don't change the key.
            (b"\x1b[1;67A", "M-Up"),
        ] {
            let got = all(bytes);
            assert_eq!(names_of(&got), [want], "{bytes:?}");
            assert_eq!(got[0].raw, bytes);
        }
    }

    #[test]
    fn every_modifier_parameter_on_the_cursor_keys() {
        for param in 1..=8u32 {
            let bits = param - 1;
            let held = mods(bits & 1 != 0, bits & 2 != 0, bits & 4 != 0);
            for (fin, code) in [
                ('A', KeyCode::Up),
                ('B', KeyCode::Down),
                ('C', KeyCode::Right),
                ('D', KeyCode::Left),
                ('H', KeyCode::Home),
                ('F', KeyCode::End),
            ] {
                let bytes = format!("\x1b[1;{param}{fin}");
                assert_eq!(
                    names(bytes.as_bytes()),
                    [Key::new(code, held).notation()],
                    "{bytes:?}"
                );
            }
        }
    }

    #[test]
    fn csi_u_keys() {
        for (bytes, want) in [
            (&b"\x1b[13;2u"[..], "S-Enter"),
            (b"\x1b[13;5u", "C-Enter"),
            (b"\x1b[106;4u", "M-J"),
            (b"\x1b[106:74;4u", "M-J"),
            (b"\x1b[97;5u", "C-a"),
            (b"\x1b[97;6u", "C-S-a"),
            (b"\x1b[115;8u", "C-M-S-s"),
            (b"\x1b[122;8u", "C-M-S-z"),
            (b"\x1b[9;2u", "BTab"),
            (b"\x1b[9;5u", "C-Tab"),
            (b"\x1b[27u", "Escape"),
            (b"\x1b[27;3u", "M-Escape"),
            (b"\x1b[127;3u", "M-BSpace"),
            (b"\x1b[32;5u", "C-Space"),
            (b"\x1b[233u", "é"),
            (b"\x1b[57414u", "Enter"),
            (b"\x1b[57399;5u", "C-0"),
            (b"\x1b[57417u", "Left"),
            (b"\x1b[57426;2u", "S-DC"),
            // Caps Lock (64) held with Ctrl.
            (b"\x1b[97;69u", "C-a"),
            // A repeat is the key again.
            (b"\x1b[97;5:2u", "C-a"),
            // Text after the modifiers is ignored.
            (b"\x1b[97;1;97u", "a"),
            // xterm's modifyOtherKeys form.
            (b"\x1b[27;2;13~", "S-Enter"),
            (b"\x1b[27;5;105~", "C-i"),
            (b"\x1b[27;3;106~", "M-j"),
        ] {
            let got = all(bytes);
            assert_eq!(names_of(&got), [want], "{bytes:?}");
            assert_eq!(got[0].raw, bytes);
        }
    }

    #[test]
    fn sequences_not_understood_are_unknown_with_their_bytes() {
        for bytes in [
            &b"\x1b[?1;2c"[..],
            b"\x1b[>1;4000;0c",
            b"\x1b[?1u",
            b"\x1b[12;40R",
            b"\x1b[2 q",
            b"\x1b[E",
            b"\x1b[99~",
            b"\x1b[1;2;3A",
            b"\x1b[2A",
            b"\x1b[1I",
            // Super (kitty's 8) or xterm's Meta has no name here.
            b"\x1b[1;9A",
            b"\x1b[97;9u",
            b"\x1b[97;33u",
            // A key's release.
            b"\x1b[97;5:3u",
            // kitty's F13, a private-use code.
            b"\x1b[57376u",
            b"\x1b[201~",
            b"\x1b]11;rgb:0000/0000/0000\x07",
            b"\x1b]11;rgb:0000/0000/0000\x1b\\",
            b"\x1b]52;c;aGVsbG8=\x07",
            b"\x1bP>|kitty(0.35.2)\x1b\\",
            b"\x1bP1$r0m\x1b\\",
            b"\x1b_Gi=31;OK\x1b\\",
            b"\x1b[<128;1;1M",
            b"\x1b[<0;1M",
            b"\x1b[M\x1f!!",
            b"\x1bO9P",
        ] {
            assert_eq!(
                all(bytes),
                [Parsed {
                    input: Input::Unknown,
                    raw: bytes.to_vec(),
                }],
                "{bytes:?}"
            );
        }
    }

    #[test]
    fn bytes_that_arent_utf8_are_unknown_one_at_a_time() {
        let unknown = "Unknown";
        assert_eq!(names(&[0xff]), [unknown]);
        assert_eq!(names(&[0x80]), [unknown]);
        assert_eq!(names(&[0xc3, b'a']), [unknown, "a"]);
        assert_eq!(names(&[0xe0, 0x80, 0x80]), [unknown; 3]);
        assert_eq!(names(&[0xed, 0xa0, 0x80]), [unknown; 3]);
        // An unfinished character is held, then let go whole.
        let mut p = Parser::new();
        assert!(p.feed(&[0xe6, 0x97]).is_empty());
        assert_eq!(
            p.flush(),
            [Parsed {
                input: Input::Unknown,
                raw: vec![0xe6, 0x97],
            }]
        );
    }

    #[test]
    fn focus_in_and_out() {
        assert_eq!(all(b"\x1b[I")[0].input, Input::FocusIn);
        assert_eq!(all(b"\x1b[O")[0].input, Input::FocusOut);
    }

    #[test]
    fn sgr_mouse_events() {
        use MouseButton::{Left, Middle, Right, WheelDown, WheelLeft, WheelRight, WheelUp};
        use MouseKind::*;
        for (bytes, want) in [
            (&b"\x1b[<0;10;5M"[..], at(Press, Left, 9, 4)),
            (b"\x1b[<0;10;5m", at(Release, Left, 9, 4)),
            (b"\x1b[<1;1;1M", at(Press, Middle, 0, 0)),
            (b"\x1b[<2;1;1M", at(Press, Right, 0, 0)),
            (b"\x1b[<2;1;1m", at(Release, Right, 0, 0)),
            (b"\x1b[<32;3;4M", at(Drag, Left, 2, 3)),
            (b"\x1b[<34;3;4M", at(Drag, Right, 2, 3)),
            (b"\x1b[<35;7;8M", at(Move, MouseButton::None, 6, 7)),
            (b"\x1b[<64;1;2M", at(Press, WheelUp, 0, 1)),
            (b"\x1b[<65;1;2M", at(Press, WheelDown, 0, 1)),
            (b"\x1b[<66;1;2M", at(Press, WheelLeft, 0, 1)),
            (b"\x1b[<67;1;2M", at(Press, WheelRight, 0, 1)),
            (b"\x1b[<0;300;400M", at(Press, Left, 299, 399)),
            (b"\x1b[<0;0;0M", at(Press, Left, 0, 0)),
            (b"\x1b[<0;70000;1M", at(Press, Left, u16::MAX, 0)),
        ] {
            assert_eq!(one_mouse(bytes), want, "{bytes:?}");
        }
        for (code, held) in [
            (4, mods(true, false, false)),
            (8, mods(false, true, false)),
            (16, mods(false, false, true)),
            (28, mods(true, true, true)),
        ] {
            let m = one_mouse(format!("\x1b[<{code};1;1M").as_bytes());
            assert_eq!((m.kind, m.button, m.mods), (Press, Left, held));
            let m = one_mouse(format!("\x1b[<{};1;1M", code + 64).as_bytes());
            assert_eq!((m.button, m.mods), (WheelUp, held));
        }
    }

    #[test]
    fn x10_and_urxvt_mouse_events() {
        use MouseButton::{Left, Right, WheelDown, WheelUp};
        use MouseKind::*;
        let x10 = |code: u8, x: u8, y: u8| vec![0x1b, b'[', b'M', code, x, y];
        assert_eq!(one_mouse(&x10(32, 33 + 9, 33 + 4)), at(Press, Left, 9, 4));
        assert_eq!(one_mouse(&x10(96, 33, 33)), at(Press, WheelUp, 0, 0));
        assert_eq!(one_mouse(&x10(97, 33, 33)), at(Press, WheelDown, 0, 0));
        assert_eq!(one_mouse(&x10(64, 34, 34)), at(Drag, Left, 1, 1));
        assert_eq!(
            one_mouse(&x10(67, 33, 33)),
            at(Move, MouseButton::None, 0, 0)
        );
        assert_eq!(one_mouse(&x10(32, 255, 255)), at(Press, Left, 222, 222));
        assert_eq!(
            one_mouse(&x10(32 + 4 + 16, 33, 33)).mods,
            mods(true, false, true)
        );
        // A release names no button: it is the one last pressed.
        assert_eq!(one_mouse(&x10(35, 33, 33)), at(Release, Left, 0, 0));
        let mut p = Parser::new();
        let mut events = p.feed(&x10(34, 40, 40));
        events.extend(p.feed(&x10(35, 41, 41)));
        let inputs: Vec<Input> = events.into_iter().map(|e| e.input).collect();
        assert_eq!(
            inputs,
            [
                Input::Mouse(at(Press, Right, 7, 7)),
                Input::Mouse(at(Release, Right, 8, 8)),
            ]
        );
        // The bytes after `\e[M` can be anything, ESC and `~` included.
        let tricky = x10(32, 0x1b + 32, b'~');
        assert_eq!(one_mouse(&tricky), at(Press, Left, 26, 93));

        assert_eq!(one_mouse(b"\x1b[32;10;5M"), at(Press, Left, 9, 4));
        assert_eq!(one_mouse(b"\x1b[35;10;5M"), at(Release, Left, 9, 4));
        assert_eq!(one_mouse(b"\x1b[96;10;5M"), at(Press, WheelUp, 9, 4));
    }

    #[test]
    fn a_paste_is_one_event_with_its_text_as_it_came() {
        let bytes = "\x1b[200~hello\nworld \x1b[A é\x1b[201~".as_bytes();
        assert_eq!(
            all(bytes),
            [Parsed {
                input: Input::Paste("hello\nworld \x1b[A é".into()),
                raw: bytes.to_vec(),
            }]
        );
        assert_eq!(
            all(b"\x1b[200~\x1b[201~")[0].input,
            Input::Paste(String::new())
        );
    }

    #[test]
    fn keys_before_between_and_after_pastes() {
        let got = all(b"a\x1b[200~x\x1b[201~\x1b[200~y\x1b[201~\x1b[Ab");
        let inputs: Vec<Input> = got.iter().map(|e| e.input.clone()).collect();
        assert_eq!(
            inputs,
            [
                Input::Key(k("a")),
                Input::Paste("x".into()),
                Input::Paste("y".into()),
                Input::Key(k("Up")),
                Input::Key(k("b")),
            ]
        );
        assert_eq!(got[1].raw, b"\x1b[200~x\x1b[201~");
        assert_eq!(names(b"\x1b\x1b[200~x\x1b[201~")[0], "Escape");
    }

    #[test]
    fn a_paste_is_held_across_reads_and_flushes_until_its_end() {
        let t = Instant::now();
        let mut p = Parser::new();
        assert!(p.feed_at(b"\x1b[200~hel", t).is_empty());
        assert!(p.pending());
        assert!(p.flush_at(t + ESC_TIMEOUT).is_empty());
        assert!(p.pending());
        assert!(p.feed_at(b"lo\n\x1b[20", t + ESC_TIMEOUT * 2).is_empty());
        assert!(p.flush_at(t + ESC_TIMEOUT * 3).is_empty());
        let got = p.feed_at(b"1~x", t + ESC_TIMEOUT * 4);
        assert_eq!(
            got,
            [
                Parsed {
                    input: Input::Paste("hello\n".into()),
                    raw: b"\x1b[200~hello\n\x1b[201~".to_vec(),
                },
                Parsed {
                    input: Input::Key(k("x")),
                    raw: b"x".to_vec(),
                },
            ]
        );
        assert!(!p.pending());
    }

    #[test]
    fn a_long_paste_in_many_small_reads() {
        let body: String = (0..20_000).map(|i| ['a', 'é', '\n', '日'][i % 4]).collect();
        let bytes = [PASTE_START, body.as_bytes(), PASTE_END].concat();
        let mut p = Parser::new();
        let mut got = Vec::new();
        for chunk in bytes.chunks(7) {
            got.extend(p.feed(chunk));
        }
        assert_eq!(
            got,
            [Parsed {
                input: Input::Paste(body),
                raw: bytes,
            }]
        );
    }

    #[test]
    fn a_paste_whose_end_never_comes_is_let_go() {
        let t = Instant::now();
        let mut p = Parser::new();
        assert!(p.feed_at(b"\x1b[200~abc", t).is_empty());
        let just_before = t + PASTE_TIMEOUT - Duration::from_millis(1);
        assert!(p.flush_at(just_before).is_empty());
        assert_eq!(
            p.flush_at(t + PASTE_TIMEOUT),
            [Parsed {
                input: Input::Paste("abc".into()),
                raw: b"\x1b[200~abc".to_vec(),
            }]
        );
        assert!(!p.pending());

        // A key typed once it has lapsed comes after it, as a key.
        let mut p = Parser::new();
        assert!(p.feed_at(b"\x1b[200~abc", t).is_empty());
        let got = p.feed_at(b"j", t + PASTE_TIMEOUT);
        assert_eq!(names_of(&got)[1], "j");
        assert_eq!(got[0].input, Input::Paste("abc".into()));

        // One still arriving isn't let go: the timeout runs from its last bytes.
        let mut p = Parser::new();
        assert!(p.feed_at(b"\x1b[200~abc", t).is_empty());
        let later = t + Duration::from_millis(1500);
        assert!(p.feed_at(b"def", later).is_empty());
        assert!(p.flush_at(t + PASTE_TIMEOUT).is_empty());
        assert!(p.feed_at(b"", t + PASTE_TIMEOUT).is_empty());
        assert_eq!(
            p.flush_at(later + PASTE_TIMEOUT)[0].input,
            Input::Paste("abcdef".into())
        );
    }

    #[test]
    fn raw_is_exactly_the_bytes_each_event_took() {
        let got = all(b"a\x1b[1;3A\x1b[<0;1;1Mb\x1b]11;x\x07\xff\x1bj");
        let raws: Vec<&[u8]> = got.iter().map(|e| e.raw.as_slice()).collect();
        assert_eq!(
            raws,
            [
                &b"a"[..],
                b"\x1b[1;3A",
                b"\x1b[<0;1;1M",
                b"b",
                b"\x1b]11;x\x07",
                b"\xff",
                b"\x1bj",
            ]
        );
    }

    /// Every way of splitting each sample between two reads, and a byte at a
    /// time, reads the same as the sample in one read.
    #[test]
    fn a_sequence_split_anywhere_reads_the_same() {
        let samples: &[&[u8]] = &[
            b"\x1b[1;3A",
            b"\x1bOP",
            b"\x1bO5S",
            b"\x1b[24;5~",
            b"\x1b[<35;120;40M",
            b"\x1b[<0;10;5m",
            b"\x1b[M #!",
            b"\x1b[32;10;5M",
            b"\x1b[200~paste\r\nme\x1b[201~",
            b"\x1b[115;8u",
            b"\x1b[27;2;13~",
            b"\x1b]11;rgb:1/2/3\x1b\\",
            b"\x1bP>|xterm\x1b\\",
            b"\x1b_Gi=1;OK\x1b\\",
            b"\x1b\x1b[A",
            b"\x1bj",
            b"\x1b[I",
            b"a\x1b[Bb",
            "é日🙂\x1bé".as_bytes(),
        ];
        let read = |chunks: &[&[u8]]| {
            let mut p = Parser::new();
            let mut events: Vec<Parsed> = chunks.iter().flat_map(|c| p.feed(c)).collect();
            events.extend(p.flush());
            events
        };
        for sample in samples {
            let whole = read(&[sample]);
            let raw: Vec<u8> = whole.iter().flat_map(|e| e.raw.clone()).collect();
            assert_eq!(&raw, sample);
            for cut in 1..sample.len() {
                assert_eq!(
                    read(&[&sample[..cut], &sample[cut..]]),
                    whole,
                    "{sample:?} cut at {cut}"
                );
            }
            let bytes: Vec<&[u8]> = sample.chunks(1).collect();
            assert_eq!(read(&bytes), whole, "{sample:?} a byte at a time");
        }
    }

    /// Random input, much of it pieces of sequences, loses no bytes and reads
    /// the same in one read as in many.
    #[test]
    fn random_input_loses_nothing_however_it_is_split() {
        let pieces: &[&[u8]] = &[
            b"\x1b",
            b"\x1b",
            b"[",
            b"O",
            b"]",
            b"P",
            b"_",
            b"<",
            b"M",
            b"m",
            b"~",
            b"u",
            b";",
            b":",
            b"0",
            b"1",
            b"2",
            b"5",
            b"9",
            b"A",
            b"Z",
            b"\\",
            b"\x07",
            b"a",
            b" ",
            b"\xc3",
            b"\xa9",
            b"\xe6\x97\xa5",
            b"\xff",
            b"\x00",
            b"\x7f",
            b"\r",
            b"\x1b[200~",
            b"\x1b[201~",
            b"\x1b[<0;5;5M",
            b"\x1b[1;3A",
        ];
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move |below: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % below as u64) as usize
        };
        let t = Instant::now();
        let read = |p: &mut Parser, chunks: &[&[u8]]| {
            let mut events: Vec<Parsed> = chunks.iter().flat_map(|c| p.feed_at(c, t)).collect();
            let mut later = t;
            while p.pending() {
                later += PASTE_TIMEOUT;
                events.extend(p.flush_at(later));
            }
            events
        };
        for _ in 0..3000 {
            let input: Vec<u8> = (0..next(40))
                .flat_map(|_| pieces[next(pieces.len())].iter().copied())
                .collect();
            let whole = read(&mut Parser::new(), &[&input]);
            let mut chunks = Vec::new();
            let mut rest = &input[..];
            while !rest.is_empty() {
                let (chunk, after) = rest.split_at(1 + next(rest.len()));
                chunks.push(chunk);
                rest = after;
            }
            let split = read(&mut Parser::new(), &chunks);
            assert!(whole.iter().all(|e| !e.raw.is_empty()), "{input:?}");
            let raw: Vec<u8> = whole.iter().flat_map(|e| e.raw.clone()).collect();
            assert_eq!(raw, input);
            assert_eq!(split, whole, "{input:?} split as {chunks:?}");
        }
    }

    // ── Notation ────────────────────────────────────────────────────────────

    #[test]
    fn notation_reads_tmux_keys() {
        use KeyCode::*;
        let none = Mods::NONE;
        for (text, code, held) in [
            ("a", Char('a'), none),
            ("M-a", Char('a'), Mods::ALT),
            ("C-w", Char('w'), Mods::CTRL),
            ("M-S-Left", Left, mods(true, true, false)),
            ("S-M-Left", Left, mods(true, true, false)),
            ("M-Space", Char(' '), Mods::ALT),
            ("M-Tab", Tab, Mods::ALT),
            ("Enter", Enter, none),
            ("enter", Enter, none),
            ("Escape", Esc, none),
            ("BSpace", Backspace, none),
            ("BTab", BackTab, none),
            ("M-[", Char('['), Mods::ALT),
            ("M-]", Char(']'), Mods::ALT),
            ("M-=", Char('='), Mods::ALT),
            ("M-0", Char('0'), Mods::ALT),
            ("M-J", Char('J'), Mods::ALT),
            ("M--", Char('-'), Mods::ALT),
            ("-", Char('-'), none),
            ("^", Char('^'), none),
            ("^a", Char('a'), Mods::CTRL),
            ("F1", F(1), none),
            ("F12", F(12), none),
            ("C-M-x", Char('x'), mods(false, true, true)),
            ("c-m-x", Char('x'), mods(false, true, true)),
            ("C-M-S-s", Char('s'), mods(true, true, true)),
            ("Space", Char(' '), none),
            ("Up", Up, none),
            ("Down", Down, none),
            ("Right", Right, none),
            ("Home", Home, none),
            ("End", End, none),
            ("PPage", PageUp, none),
            ("PageUp", PageUp, none),
            ("PgUp", PageUp, none),
            ("NPage", PageDown, none),
            ("PageDown", PageDown, none),
            ("PgDn", PageDown, none),
            ("IC", Insert, none),
            ("Insert", Insert, none),
            ("DC", Delete, none),
            ("Delete", Delete, none),
            ("M-é", Char('é'), Mods::ALT),
        ] {
            let key = k(text);
            assert_eq!((key.code, key.mods), (code, held), "{text}");
        }
    }

    #[test]
    fn notation_refuses_what_isnt_a_key() {
        for text in [
            "", "C-", "M-", "S-", "X-a", "Foo", "F0", "F13", "F01x", "ab", "C-ab", "M-Foo",
        ] {
            assert_eq!(Key::parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn notation_writes_the_canonical_form() {
        use KeyCode::*;
        for (key, want) in [
            (Key::new(Char('j'), mods(true, true, false)), "M-J"),
            (Key::new(Char('J'), Mods::ALT), "M-J"),
            (Key::new(Char(' '), Mods::ALT), "M-Space"),
            (Key::new(Char('x'), mods(false, true, true)), "C-M-x"),
            (Key::new(Char('A'), Mods::CTRL), "C-a"),
            (Key::new(Char('a'), mods(true, false, true)), "C-S-a"),
            (Key::new(Char('S'), mods(true, true, true)), "C-M-S-s"),
            (Key::new(Char('@'), Mods::CTRL), "C-Space"),
            (Key::new(Char('\x01'), Mods::NONE), "C-a"),
            (Key::new(Char('\x01'), Mods::ALT), "C-M-a"),
            (Key::new(Char('\t'), Mods::ALT), "M-Tab"),
            (Key::new(Char('\r'), Mods::NONE), "Enter"),
            (Key::new(Char('\x1b'), Mods::NONE), "Escape"),
            (Key::new(Char('\x7f'), Mods::NONE), "BSpace"),
            (Key::new(Tab, Mods::SHIFT), "BTab"),
            (Key::new(BackTab, mods(true, true, false)), "M-BTab"),
            (Key::new(Char('é'), Mods::SHIFT), "É"),
            (Key::new(Char('1'), Mods::SHIFT), "S-1"),
            (Key::new(Char('ß'), Mods::SHIFT), "S-ß"),
            (Key::new(Enter, Mods::SHIFT), "S-Enter"),
            (Key::new(Left, mods(true, true, false)), "M-S-Left"),
            (Key::new(PageUp, Mods::NONE), "PPage"),
            (Key::new(PageDown, Mods::NONE), "NPage"),
            (Key::new(Insert, Mods::NONE), "IC"),
            (Key::new(Delete, Mods::CTRL), "C-DC"),
            (Key::new(F(7), Mods::SHIFT), "S-F7"),
            (Key::new(Char('\\'), Mods::CTRL), "C-\\"),
            (Key::new(Char('-'), Mods::ALT), "M--"),
        ] {
            assert_eq!(key.notation(), want, "{key:?}");
        }
        assert_eq!(k("S-Tab").notation(), "BTab");
        assert_eq!(k("PageUp").notation(), "PPage");
        assert_eq!(k("S-M-C-left").notation(), "C-M-S-Left");
    }

    #[test]
    fn equal_keys_are_equal_and_hash_alike() {
        assert_eq!(k("M-S-j"), k("M-J"));
        assert_eq!(k("M-S-J"), k("M-J"));
        assert_eq!(k("C-A"), k("C-a"));
        assert_eq!(k("C-@"), k("C-Space"));
        assert_eq!(k("S-Tab"), k("BTab"));
        assert_ne!(k("M-j"), k("M-J"));
        assert_ne!(k("C-a"), k("C-S-a"));
        assert_ne!(k("C-M-s"), k("C-M-S-s"));
        assert_ne!(k("Tab"), k("C-i"));
        let bound: HashSet<Key> = ["M-J", "C-a", "BTab", "M-Space"].map(k).into();
        assert!(bound.contains(&Key::new(KeyCode::Char('j'), mods(true, true, false))));
        assert!(bound.contains(&Key::new(KeyCode::Char('A'), Mods::CTRL)));
        assert!(bound.contains(&Key::new(KeyCode::Tab, Mods::SHIFT)));
        assert!(bound.contains(&Key::new(KeyCode::Char(' '), Mods::ALT)));
        assert!(!bound.contains(&k("M-j")));
    }

    #[test]
    fn notation_round_trips() {
        use KeyCode::*;
        let mut codes = vec![
            Enter, Tab, BackTab, Backspace, Esc, Left, Right, Up, Down, Home, End, PageUp,
            PageDown, Insert, Delete,
        ];
        codes.extend((1..=12).map(F));
        codes.extend("azAJ09-[]=\\^@ ~/;CMSé日É\u{1}\t\u{1f}".chars().map(Char));
        for code in codes {
            for bits in 0..8 {
                let key = Key::new(code, mods(bits & 1 != 0, bits & 2 != 0, bits & 4 != 0));
                let text = key.notation();
                let back = Key::parse(&text).unwrap_or_else(|| panic!("{key:?} wrote {text:?}"));
                assert_eq!(back, key, "{text}");
                assert_eq!(back.notation(), text);
            }
        }
    }

    // ── Encoding ────────────────────────────────────────────────────────────

    #[test]
    fn keys_encode_as_xterm_sends_them() {
        let normal = PaneModes::default();
        let app = PaneModes {
            app_cursor: true,
            ..PaneModes::default()
        };
        for (text, want, want_app) in [
            ("a", &b"a"[..], &b"a"[..]),
            ("A", b"A", b"A"),
            ("é", "é".as_bytes(), "é".as_bytes()),
            ("M-j", b"\x1bj", b"\x1bj"),
            ("M-J", b"\x1bJ", b"\x1bJ"),
            ("M-S-j", b"\x1bJ", b"\x1bJ"),
            ("M-é", "\x1bé".as_bytes(), "\x1bé".as_bytes()),
            ("C-a", b"\x01", b"\x01"),
            ("C-A", b"\x01", b"\x01"),
            ("C-z", b"\x1a", b"\x1a"),
            ("C-M-x", b"\x1b\x18", b"\x1b\x18"),
            ("C-Space", b"\x00", b"\x00"),
            ("C-@", b"\x00", b"\x00"),
            ("M-Space", b"\x1b ", b"\x1b "),
            ("C-M-Space", b"\x1b\x00", b"\x1b\x00"),
            ("C-[", b"\x1b", b"\x1b"),
            ("C-\\", b"\x1c", b"\x1c"),
            ("C-]", b"\x1d", b"\x1d"),
            ("C-^", b"\x1e", b"\x1e"),
            ("C-_", b"\x1f", b"\x1f"),
            ("C-/", b"\x1f", b"\x1f"),
            ("C-?", b"\x7f", b"\x7f"),
            ("C-1", b"1", b"1"),
            ("Space", b" ", b" "),
            ("Enter", b"\r", b"\r"),
            ("M-Enter", b"\x1b\r", b"\x1b\r"),
            ("Tab", b"\t", b"\t"),
            ("M-Tab", b"\x1b\t", b"\x1b\t"),
            ("BTab", b"\x1b[Z", b"\x1b[Z"),
            ("S-Tab", b"\x1b[Z", b"\x1b[Z"),
            ("BSpace", b"\x7f", b"\x7f"),
            ("M-BSpace", b"\x1b\x7f", b"\x1b\x7f"),
            ("C-BSpace", b"\x08", b"\x08"),
            ("Escape", b"\x1b", b"\x1b"),
            ("M-Escape", b"\x1b\x1b", b"\x1b\x1b"),
            ("Up", b"\x1b[A", b"\x1bOA"),
            ("Down", b"\x1b[B", b"\x1bOB"),
            ("Right", b"\x1b[C", b"\x1bOC"),
            ("Left", b"\x1b[D", b"\x1bOD"),
            ("Home", b"\x1b[H", b"\x1bOH"),
            ("End", b"\x1b[F", b"\x1bOF"),
            ("M-Up", b"\x1b[1;3A", b"\x1b[1;3A"),
            ("S-Down", b"\x1b[1;2B", b"\x1b[1;2B"),
            ("C-Right", b"\x1b[1;5C", b"\x1b[1;5C"),
            ("C-M-S-Left", b"\x1b[1;8D", b"\x1b[1;8D"),
            ("C-Home", b"\x1b[1;5H", b"\x1b[1;5H"),
            ("M-End", b"\x1b[1;3F", b"\x1b[1;3F"),
            ("IC", b"\x1b[2~", b"\x1b[2~"),
            ("DC", b"\x1b[3~", b"\x1b[3~"),
            ("PPage", b"\x1b[5~", b"\x1b[5~"),
            ("NPage", b"\x1b[6~", b"\x1b[6~"),
            ("C-DC", b"\x1b[3;5~", b"\x1b[3;5~"),
            ("M-NPage", b"\x1b[6;3~", b"\x1b[6;3~"),
            ("F1", b"\x1bOP", b"\x1bOP"),
            ("F2", b"\x1bOQ", b"\x1bOQ"),
            ("F3", b"\x1bOR", b"\x1bOR"),
            ("F4", b"\x1bOS", b"\x1bOS"),
            ("S-F1", b"\x1b[1;2P", b"\x1b[1;2P"),
            ("C-F4", b"\x1b[1;5S", b"\x1b[1;5S"),
            ("F5", b"\x1b[15~", b"\x1b[15~"),
            ("F6", b"\x1b[17~", b"\x1b[17~"),
            ("F7", b"\x1b[18~", b"\x1b[18~"),
            ("F8", b"\x1b[19~", b"\x1b[19~"),
            ("F9", b"\x1b[20~", b"\x1b[20~"),
            ("F10", b"\x1b[21~", b"\x1b[21~"),
            ("F11", b"\x1b[23~", b"\x1b[23~"),
            ("F12", b"\x1b[24~", b"\x1b[24~"),
            ("S-F5", b"\x1b[15;2~", b"\x1b[15;2~"),
            ("C-M-F12", b"\x1b[24;7~", b"\x1b[24;7~"),
        ] {
            assert_eq!(encode_key(&k(text), &normal), want, "{text}");
            assert_eq!(encode_key(&k(text), &app), want_app, "{text} (app)");
        }
        assert!(encode_key(&Key::new(KeyCode::F(13), Mods::NONE), &normal).is_empty());
    }

    #[test]
    fn keys_encoded_read_back_as_themselves() {
        let normal = PaneModes::default();
        for text in [
            "a", "M-j", "M-J", "C-a", "C-M-x", "C-Space", "M-Space", "Enter", "M-Enter", "Tab",
            "M-Tab", "BTab", "BSpace", "M-BSpace", "Escape", "M-Escape", "Up", "M-Up", "C-S-Left",
            "Home", "End", "IC", "DC", "PPage", "NPage", "C-DC", "F1", "F4", "F5", "F12", "S-F3",
            "C-M-F9", "é", "M-é", "C-\\", "C-]", "C-^", "C-_", "M-[", "M-O", "M-]",
        ] {
            let bytes = encode_key(&k(text), &normal);
            assert_eq!(names(&bytes), [text], "{text} sent as {bytes:?}");
        }
    }

    fn kitty(flags: u8) -> PaneModes {
        PaneModes {
            kitty_keys: flags,
            ..PaneModes::default()
        }
    }

    fn other_keys(level: u8) -> PaneModes {
        PaneModes {
            modify_other_keys: level,
            ..PaneModes::default()
        }
    }

    #[test]
    fn keys_go_as_csi_u_only_to_a_program_that_asked_for_them() {
        // (key, a shell's bytes, kitty's flag 1, modifyOtherKeys 2, and 1)
        for (text, shell, flag_1, level_2, level_1) in [
            (
                "C-S-p",
                &b"\x10"[..],
                &b"\x1b[112;6u"[..],
                &b"\x1b[112;6u"[..],
                &b"\x1b[112;6u"[..],
            ),
            ("C-p", b"\x10", b"\x1b[112;5u", b"\x1b[112;5u", b"\x10"),
            ("C-i", b"\t", b"\x1b[105;5u", b"\x1b[105;5u", b"\t"),
            ("C-1", b"1", b"\x1b[49;5u", b"\x1b[49;5u", b"\x1b[49;5u"),
            ("M-j", b"\x1bj", b"\x1b[106;3u", b"\x1b[106;3u", b"\x1bj"),
            ("M-J", b"\x1bJ", b"\x1b[106;4u", b"\x1b[106;4u", b"\x1bJ"),
            ("C-Space", b"\x00", b"\x1b[32;5u", b"\x1b[32;5u", b"\x00"),
            ("a", b"a", b"a", b"a", b"a"),
            ("A", b"A", b"A", b"A", b"A"),
            ("S-1", b"1", b"1", b"1", b"1"),
            ("Enter", b"\r", b"\r", b"\r", b"\r"),
            (
                "S-Enter",
                b"\r",
                b"\x1b[13;2u",
                b"\x1b[13;2u",
                b"\x1b[13;2u",
            ),
            (
                "M-Enter",
                b"\x1b\r",
                b"\x1b[13;3u",
                b"\x1b[13;3u",
                b"\x1b\r",
            ),
            ("Tab", b"\t", b"\t", b"\t", b"\t"),
            ("BTab", b"\x1b[Z", b"\x1b[9;2u", b"\x1b[Z", b"\x1b[Z"),
            ("C-Tab", b"\t", b"\x1b[9;5u", b"\x1b[9;5u", b"\x1b[9;5u"),
            ("BSpace", b"\x7f", b"\x7f", b"\x7f", b"\x7f"),
            (
                "C-BSpace",
                b"\x08",
                b"\x1b[127;5u",
                b"\x1b[127;5u",
                b"\x1b[127;5u",
            ),
            ("Escape", b"\x1b", b"\x1b[27u", b"\x1b", b"\x1b"),
            (
                "M-Escape",
                b"\x1b\x1b",
                b"\x1b[27;3u",
                b"\x1b[27;3u",
                b"\x1b\x1b",
            ),
            ("Up", b"\x1b[A", b"\x1b[A", b"\x1b[A", b"\x1b[A"),
            (
                "C-Up",
                b"\x1b[1;5A",
                b"\x1b[1;5A",
                b"\x1b[1;5A",
                b"\x1b[1;5A",
            ),
            ("F5", b"\x1b[15~", b"\x1b[15~", b"\x1b[15~", b"\x1b[15~"),
        ] {
            let key = k(text);
            assert_eq!(encode_key(&key, &PaneModes::default()), shell, "{text}");
            assert_eq!(encode_key(&key, &kitty(1)), flag_1, "{text} (kitty)");
            assert_eq!(encode_key(&key, &other_keys(2)), level_2, "{text} (2)");
            assert_eq!(encode_key(&key, &other_keys(1)), level_1, "{text} (1)");
            // The kitty protocol wins where a program asked for both.
            let both = PaneModes {
                kitty_keys: 1,
                modify_other_keys: 2,
                ..PaneModes::default()
            };
            assert_eq!(encode_key(&key, &both), flag_1, "{text} (both)");
        }
    }

    #[test]
    fn every_key_goes_as_csi_u_when_kitty_s_flag_8_asks() {
        for (text, flags, want) in [
            ("a", 8 | 1, &b"\x1b[97u"[..]),
            ("A", 8 | 1, b"\x1b[97;2u"),
            ("A", 8 | 16 | 1, b"\x1b[97;2;65u"),
            ("a", 8 | 16, b"\x1b[97;1;97u"),
            ("C-a", 8 | 16, b"\x1b[97;5u"),
            ("Enter", 8, b"\x1b[13u"),
            ("BSpace", 8, b"\x1b[127u"),
            ("Up", 8, b"\x1b[A"),
        ] {
            assert_eq!(
                encode_key(&k(text), &kitty(flags)),
                want,
                "{text} ({flags})"
            );
        }
    }

    #[test]
    fn keys_sent_as_csi_u_read_back_as_themselves() {
        for modes in [kitty(1), kitty(8), other_keys(1), other_keys(2)] {
            for text in [
                "C-S-p", "C-p", "C-1", "M-j", "M-J", "C-Space", "a", "A", "S-Enter", "M-Enter",
                "BTab", "M-BTab", "C-Tab", "C-BSpace", "Escape", "M-Escape", "C-M-S-s", "Enter",
                "Tab", "BSpace",
            ] {
                let bytes = encode_key(&k(text), &modes);
                assert_eq!(
                    names(&bytes),
                    [text],
                    "{text} sent as {bytes:?} ({modes:?})"
                );
            }
        }
    }

    #[test]
    fn a_key_with_no_name_here_reaches_only_a_program_that_asked_for_its_form() {
        let f13 = b"\x1b[57376u";
        let modify = b"\x1b[27;5;57376~";
        for raw in [&f13[..], &modify[..]] {
            assert!(!passes_unknown(raw, &PaneModes::default()), "{raw:?}");
            assert!(passes_unknown(raw, &kitty(1)), "{raw:?}");
            assert!(passes_unknown(raw, &other_keys(1)), "{raw:?}");
        }
        // Anything else not understood passes as before.
        for raw in [
            &b"\x1b[?1u"[..],
            b"\x1b]11;rgb:0/0/0\x07",
            b"\x1b[u",
            b"\x1b[99~",
        ] {
            assert!(passes_unknown(raw, &PaneModes::default()), "{raw:?}");
        }
    }

    #[test]
    fn mouse_events_encode_for_what_the_pane_asked() {
        use MouseButton::{Left, Middle, Right, WheelDown, WheelLeft, WheelRight, WheelUp};
        use MouseKind::*;
        let sgr = |mouse| PaneModes {
            mouse,
            mouse_sgr: true,
            ..PaneModes::default()
        };
        let press = at(Press, Left, 9, 4);
        let release = at(Release, Left, 9, 4);
        let wheel = at(Press, WheelUp, 9, 4);
        let drag = at(Drag, Left, 9, 4);
        let moved = at(Move, MouseButton::None, 9, 4);
        let encode = |m: Mouse, modes: PaneModes| encode_mouse(&m, &modes);
        let some = |s: &str| Some(s.as_bytes().to_vec());

        for m in [press, release, wheel, drag, moved] {
            assert_eq!(encode(m, PaneModes::default()), None);
        }
        let modes = sgr(MouseReporting::Press);
        assert_eq!(encode(press, modes), some("\x1b[<0;10;5M"));
        assert_eq!(encode(release, modes), some("\x1b[<0;10;5m"));
        assert_eq!(encode(wheel, modes), some("\x1b[<64;10;5M"));
        assert_eq!(
            encode(at(Press, WheelDown, 0, 0), modes),
            some("\x1b[<65;1;1M")
        );
        assert_eq!(
            encode(at(Press, WheelLeft, 0, 0), modes),
            some("\x1b[<66;1;1M")
        );
        assert_eq!(
            encode(at(Press, WheelRight, 0, 0), modes),
            some("\x1b[<67;1;1M")
        );
        assert_eq!(encode(at(Press, Middle, 0, 0), modes), some("\x1b[<1;1;1M"));
        assert_eq!(
            encode(at(Release, Right, 0, 0), modes),
            some("\x1b[<2;1;1m")
        );
        assert_eq!(encode(drag, modes), None);
        assert_eq!(encode(moved, modes), None);

        let modes = sgr(MouseReporting::Drag);
        assert_eq!(encode(press, modes), some("\x1b[<0;10;5M"));
        assert_eq!(encode(drag, modes), some("\x1b[<32;10;5M"));
        assert_eq!(encode(at(Drag, Right, 0, 0), modes), some("\x1b[<34;1;1M"));
        assert_eq!(encode(moved, modes), None);

        let modes = sgr(MouseReporting::Motion);
        assert_eq!(encode(drag, modes), some("\x1b[<32;10;5M"));
        assert_eq!(encode(moved, modes), some("\x1b[<35;10;5M"));

        let held = Mouse {
            mods: mods(true, true, true),
            ..press
        };
        assert_eq!(encode(held, modes), some("\x1b[<28;10;5M"));
        let ctrl_wheel = Mouse {
            mods: Mods::CTRL,
            ..wheel
        };
        assert_eq!(encode(ctrl_wheel, modes), some("\x1b[<80;10;5M"));
    }

    #[test]
    fn mouse_events_in_the_x10_form() {
        use MouseButton::{Left, Right, WheelDown, WheelUp};
        use MouseKind::*;
        let modes = |mouse_utf8| PaneModes {
            mouse: MouseReporting::Motion,
            mouse_utf8,
            ..PaneModes::default()
        };
        let encode = |m: Mouse| encode_mouse(&m, &modes(false));
        let x10 = |code: u8, x: u8, y: u8| Some(vec![0x1b, b'[', b'M', code, x, y]);
        assert_eq!(encode(at(Press, Left, 9, 4)), x10(32, 42, 37));
        assert_eq!(encode(at(Press, Right, 0, 0)), x10(34, 33, 33));
        assert_eq!(encode(at(Release, Left, 9, 4)), x10(35, 42, 37));
        assert_eq!(encode(at(Release, Right, 9, 4)), x10(35, 42, 37));
        assert_eq!(encode(at(Press, WheelUp, 0, 0)), x10(96, 33, 33));
        assert_eq!(encode(at(Press, WheelDown, 0, 0)), x10(97, 33, 33));
        assert_eq!(encode(at(Release, WheelUp, 0, 0)), None);
        assert_eq!(encode(at(Drag, Left, 1, 1)), x10(64, 34, 34));
        assert_eq!(encode(at(Move, MouseButton::None, 0, 0)), x10(67, 33, 33));
        let shifted = Mouse {
            mods: Mods::SHIFT,
            ..at(Press, Left, 0, 0)
        };
        assert_eq!(encode(shifted), x10(36, 33, 33));
        let shifted_release = Mouse {
            mods: Mods::SHIFT,
            ..at(Release, Left, 0, 0)
        };
        assert_eq!(encode(shifted_release), x10(39, 33, 33));
        // Past column 223 the position can't be told, and is sent as 223.
        assert_eq!(encode(at(Press, Left, 221, 0)), x10(32, 254, 33));
        assert_eq!(encode(at(Press, Left, 222, 0)), x10(32, 255, 33));
        assert_eq!(encode(at(Press, Left, 300, 1000)), x10(32, 255, 255));

        // With 1005, as UTF-8 up to two bytes.
        let utf8 = |m: Mouse| encode_mouse(&m, &modes(true));
        assert_eq!(utf8(at(Press, Left, 9, 4)), x10(32, 42, 37));
        let mut want = b"\x1b[M ".to_vec();
        want.extend_from_slice("\u{14d}\u{7ff}".as_bytes());
        assert_eq!(utf8(at(Press, Left, 300, 5000)), Some(want));
    }

    #[test]
    fn mouse_events_read_back_as_themselves() {
        use MouseButton::{Left, Middle, Right, WheelDown, WheelLeft};
        use MouseKind::*;
        let modes = PaneModes {
            mouse: MouseReporting::Motion,
            mouse_sgr: true,
            ..PaneModes::default()
        };
        for m in [
            at(Press, Left, 0, 0),
            at(Release, Middle, 5, 6),
            at(Drag, Right, 200, 300),
            at(Move, MouseButton::None, 1, 2),
            at(Press, WheelDown, 7, 8),
            Mouse {
                mods: mods(true, false, true),
                ..at(Press, WheelLeft, 3, 3)
            },
        ] {
            let bytes = encode_mouse(&m, &modes).unwrap();
            assert_eq!(one_mouse(&bytes), m);
        }
    }

    #[test]
    fn the_wheel_scrolls_by_arrow_keys_on_an_alternate_screen_that_asked() {
        use MouseButton::{Left, WheelDown, WheelLeft, WheelUp};
        use MouseKind::*;
        let pager = PaneModes {
            alt_screen: true,
            alternate_scroll: true,
            ..PaneModes::default()
        };
        let up = at(Press, WheelUp, 3, 3);
        let down = at(Press, WheelDown, 3, 3);
        assert_eq!(encode_mouse(&up, &pager), Some(b"\x1b[A".repeat(3)));
        assert_eq!(encode_mouse(&down, &pager), Some(b"\x1b[B".repeat(3)));
        let app = PaneModes {
            app_cursor: true,
            ..pager
        };
        assert_eq!(encode_mouse(&up, &app), Some(b"\x1bOA".repeat(3)));
        assert_eq!(encode_mouse(&at(Press, Left, 3, 3), &pager), None);
        assert_eq!(encode_mouse(&at(Press, WheelLeft, 3, 3), &pager), None);
        let shell = PaneModes {
            alt_screen: false,
            ..pager
        };
        assert_eq!(encode_mouse(&up, &shell), None);
        let not_asked = PaneModes {
            alternate_scroll: false,
            ..pager
        };
        assert_eq!(encode_mouse(&up, &not_asked), None);
        // A program that takes the mouse gets the wheel itself.
        let takes = PaneModes {
            mouse: MouseReporting::Press,
            mouse_sgr: true,
            ..pager
        };
        assert_eq!(encode_mouse(&up, &takes), Some(b"\x1b[<64;4;4M".to_vec()));
    }

    #[test]
    fn pastes_encode_bracketed_when_asked() {
        let plain = PaneModes::default();
        let bracketed = PaneModes {
            bracketed_paste: true,
            ..PaneModes::default()
        };
        assert_eq!(encode_paste("hi", &plain), b"hi");
        assert_eq!(encode_paste("a\nb\r\nc\r", &plain), b"a\rb\rc\r");
        assert_eq!(encode_paste("hi", &bracketed), b"\x1b[200~hi\x1b[201~");
        assert_eq!(
            encode_paste("x\x1b[201~echo done\n", &bracketed),
            b"\x1b[200~xecho done\r\x1b[201~"
        );
        // A marker that taking one out would make is taken out too.
        assert_eq!(
            encode_paste("\x1b[20\x1b[201~1~y\x1b[200~", &bracketed),
            b"\x1b[200~y\x1b[201~"
        );
        assert_eq!(encode_paste("\x1b[200~", &plain), b"");
        assert_eq!(encode_paste("", &bracketed), b"\x1b[200~\x1b[201~");
        assert_eq!(
            encode_paste("é日", &bracketed),
            "\x1b[200~é日\x1b[201~".as_bytes()
        );
    }

    #[test]
    fn focus_encodes_only_when_asked() {
        let asked = PaneModes {
            focus_events: true,
            ..PaneModes::default()
        };
        assert_eq!(encode_focus(true, &asked), Some(b"\x1b[I".to_vec()));
        assert_eq!(encode_focus(false, &asked), Some(b"\x1b[O".to_vec()));
        assert_eq!(encode_focus(true, &PaneModes::default()), None);
    }
}
