//! One pane: a program on a PTY (portable-pty) and the terminal emulator
//! that keeps its screen (alacritty_terminal), drawn into a cell buffer.
//! A thread per pane reads the PTY and sends what it reads to the server
//! (`PaneEvent`), which hands it to `process` on its own task, so the
//! emulator is only ever touched there. The emulator's answers to the
//! program's queries (cursor position, device attributes, colors) go
//! straight back to the PTY.
//!
//! What is written to the program goes through a second thread rather than
//! straight to the PTY: a program that stops reading its input (a long
//! build, a stopped process) fills the terminal's input buffer, and a write
//! then blocks until it reads again. tmux never waits on a pane, and neither
//! may the server, so its writes are queued and the thread waits instead.
//!
//! The program is in a session of its own with the PTY as its controlling
//! terminal (portable-pty's `setsid`), so its pid is its process group's,
//! and the group in front of the terminal is the program you're using:
//! the pane's directory and command are read from that (`procinfo`).
//!
//! Each program a pane runs has a number of its own (`generation`), which
//! its reader thread sends with everything it reads, so when the program is
//! replaced (`respawn`, as tmux's `respawn-pane`) what the old one still
//! writes, and its end, are told from the new one's and dropped.
//!
//! A pane is drawn the way tmux drew one: the named ANSI colors stay
//! palette indexes, so your own terminal's colors apply, and the program's
//! default colors are your terminal's default; only a color the program
//! set itself (OSC 4, 10, 11) is drawn as the color it set.
//!
//! What is sent to the program follows the modes it set (`modes`), keys
//! among them: in the CSI u form when it asked for keys told apart, through
//! the kitty keyboard protocol (which the emulator keeps) or modifyOtherKeys
//! (which `extkeys` watches the output for), the old bytes otherwise, as
//! tmux's `extended-keys on` gave them.

use std::ffi::OsString;
use std::io;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{
    Boundary, Column, Direction as SearchWay, Line, Point as GridPoint, Side,
};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::{Cell, Flags, Hyperlink};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::search::RegexSearch;
use alacritty_terminal::term::{Config, Osc52, Term, TermMode};
use alacritty_terminal::vi_mode::ViMotion;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, CursorShape, NamedColor, Processor, Rgb};
use anyhow::{Context, Result};
use crossterm::cursor::SetCursorStyle;
use portable_pty::{CommandBuilder, PtySize};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use tokio::sync::mpsc::UnboundedSender;

use super::compose::Links;
use super::extkeys::ModifyOtherKeys;
use super::pty::Pty;
use super::{MouseReporting, PaneEvent, PaneId, PaneModes, TermNotice};

/// What to start in a new pane.
#[derive(Clone, Debug, Default)]
pub struct PaneSpawn {
    pub id: Option<PaneId>,
    /// The program and its arguments; `None` for the user's login shell
    /// (`$SHELL`, else the password database's, else `/bin/sh`).
    pub command: Option<Vec<String>>,
    pub cwd: Option<PathBuf>,
    /// Added to the server's environment, after `TERM=xterm-256color` and
    /// `COLORTERM=truecolor`.
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
    /// Lines of scrollback kept.
    pub scrollback: usize,
    /// Whether the program may ask for keys the old forms can't tell apart
    /// (the kitty keyboard protocol, modifyOtherKeys) and get them so: the
    /// `extended_keys` setting.
    pub extended_keys: bool,
}

/// A pane taken up from a server that handed it over (src/mux/upgrade.rs):
/// its terminal's master and the program on it, which go on as they were,
/// and what is needed to show what it showed.
#[derive(Debug)]
pub struct PaneAdopt {
    pub id: PaneId,
    pub master: std::os::fd::OwnedFd,
    pub pid: Option<u32>,
    pub cols: u16,
    pub rows: u16,
    pub scrollback: usize,
    pub extended_keys: bool,
    /// What was still to be written to the program.
    pub input: Vec<u8>,
    /// Whether the program had already closed the terminal.
    pub hung_up: bool,
    /// What the pane showed, as the bytes that draw it again (`replay`).
    pub screen: String,
    /// What the emulator held of a sequence or character not yet whole
    /// (`replay::unfinished`), given after the screen.
    pub tail: Vec<u8>,
}

/// What a pane hands over to a new build: `PaneAdopt`'s parts, the master
/// by its descriptor's number, which the new build finds open.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HandedPane {
    pub id: PaneId,
    pub fd: i32,
    #[serde(default)]
    pub pid: Option<u32>,
    pub cols: u16,
    pub rows: u16,
    #[serde(default)]
    pub input: Vec<u8>,
    #[serde(default)]
    pub hung_up: bool,
    #[serde(default)]
    pub screen: String,
    #[serde(default)]
    pub tail: Vec<u8>,
}

/// How a pane is drawn this time.
#[derive(Clone, Copy, Debug, Default)]
pub struct PaneView {
    /// Whether it has the focus (its cursor is drawn only then).
    pub focused: bool,
    /// Drawn dimmed, as an inactive pane is.
    pub dim: bool,
}

/// A span of the pane's screen, in its own coordinates (column, line, with
/// line 0 the top of the visible screen and negative lines in scrollback).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub col: u16,
    pub line: i32,
}

/// The links in a line of a pane's text: each run's bytes and address.
type Linked = Vec<(Range<usize>, String)>;

/// A line of a pane's text ends: its trailing blanks go, and any link's
/// share of them, and it joins the lines before it.
fn end_line(current: &mut String, found: &mut Linked, out: &mut Vec<(String, Linked)>) {
    current.truncate(current.trim_end().len());
    let len = current.len();
    found.retain_mut(|(range, _)| {
        range.end = range.end.min(len);
        range.start < range.end
    });
    out.push((std::mem::take(current), std::mem::take(found)));
}

/// A move of copy mode's cursor, as tmux's copy-mode-vi bound them: h j k
/// l, w b e (and W B E for words between blanks), 0 $ ^, g G, H M L, and
/// the view scrolled with the cursor by C-u C-d C-b C-f (`Scroll`, lines
/// back into the history when positive).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordNext,
    WordBack,
    WordEnd,
    BigWordNext,
    BigWordBack,
    BigWordEnd,
    LineStart,
    LineEnd,
    FirstChar,
    Top,
    Bottom,
    ScreenTop,
    ScreenMiddle,
    ScreenBottom,
    Scroll(i32),
}

/// What copy mode selects: characters (v), whole lines (V), or a rectangle
/// (C-v).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectKind {
    Chars,
    Lines,
    Block,
}

/// The size a cell is taken to be, in pixels, for programs that ask (a
/// window's size in pixels, `CSI 14 t`, or the PTY's), until a client's
/// terminal says its own (`set_cell`): a common one stands in, enough for
/// a program that divides one by the other to find the grid.
pub const CELL: (u16, u16) = (8, 16);

/// The next pane's id when the server doesn't give one.
static NEXT_ID: AtomicU32 = AtomicU32::new(0);

/// The next program's number; never 0, so no event matches a pane by
/// accident.
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

/// The next number a pane's `version` takes, shared by every pane, so a
/// pane respawned in another's place never has a version it had.
static NEXT_VERSION: AtomicU64 = AtomicU64::new(1);

pub struct Pane {
    id: PaneId,
    /// Which program of the pane's this is, as its events carry it.
    generation: u64,
    /// Changed whenever what the pane draws may have (its output, its size,
    /// colors, scrolling, a selection, copy mode), so mission control's
    /// cards draw its screen again only then (src/mission/serve.rs).
    version: u64,
    term: Term<Listener>,
    parser: Processor,
    /// What the emulator reported while reading, until `process` handles it.
    events: Arc<Mutex<Vec<Event>>>,
    /// The terminal, its program, and the threads reading and writing it.
    pty: Pty,
    cols: u16,
    rows: u16,
    title: String,
    exited: Option<Option<i32>>,
    /// Which side of its cell the selection's fixed end is on; see
    /// `select_extend`.
    anchor_side: Side,
    /// Picking kitty's graphics commands out of the output, while the pane
    /// is allowed to send them on (`pass_graphics`).
    graphics: Option<super::passthrough::Scanner>,
    /// The modifyOtherKeys level the program set, watched for in its output
    /// when it may ask for extended keys (`extkeys`).
    other_keys: Option<ModifyOtherKeys>,
    /// A cell's width and height in pixels, as the terminal attached last
    /// said (src/mux/terminal.rs), else `CELL`.
    cell: (u16, u16),
    /// The colours the terminal attached last said it draws in, which the
    /// program is told when it asks.
    colors: super::terminal::Colors,
    /// The last `RECENT` bytes the program wrote, for what of them the
    /// emulator holds unfinished when the pane is handed over.
    recent: Vec<u8>,
}

/// How much of the program's latest output a pane keeps (`recent`).
const RECENT: usize = 4096;

/// Collects what the emulator reports while it reads, for `process` to
/// handle once it has. The emulator calls this from inside its parser,
/// where nothing can be answered yet, so it only queues.
#[derive(Clone)]
struct Listener(Arc<Mutex<Vec<Event>>>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        // Only for a window with a mouse pointer and a blinking cursor of
        // its own, which the server draws for itself.
        if matches!(
            event,
            Event::MouseCursorDirty | Event::Wakeup | Event::CursorBlinkingChange
        ) {
            return;
        }
        lock(&self.0).push(event);
    }
}

/// The lock, whether or not a thread panicked holding it: the queue is
/// only ever pushed to and drained, so it is never left half-changed.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A grid's size, as the emulator takes it.
struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// The kitty keyboard protocol's flags as the protocol numbers them, from
/// the emulator's modes.
pub(super) fn kitty_flags(mode: TermMode) -> u8 {
    [
        (TermMode::DISAMBIGUATE_ESC_CODES, 1),
        (TermMode::REPORT_EVENT_TYPES, 2),
        (TermMode::REPORT_ALTERNATE_KEYS, 4),
        (TermMode::REPORT_ALL_KEYS_AS_ESC, 8),
        (TermMode::REPORT_ASSOCIATED_TEXT, 16),
    ]
    .iter()
    .filter(|(flag, _)| mode.contains(*flag))
    .map(|(_, bit)| bit)
    .sum()
}

/// The emulator needs two columns and a line at least; a pane squeezed
/// smaller is given that much, and drawn cut to its area.
fn clamp_size(cols: u16, rows: u16) -> (u16, u16) {
    (cols.max(2), rows.max(1))
}

fn pty_size(cols: u16, rows: u16, cell: (u16, u16)) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: cols.saturating_mul(cell.0),
        pixel_height: rows.saturating_mul(cell.1),
    }
}

impl Pane {
    /// Starts the program on a new PTY of the given size, and a thread that
    /// sends its output (and its end) to `events`.
    pub fn spawn(spec: PaneSpawn, events: UnboundedSender<PaneEvent>) -> Result<Pane> {
        let id = match spec.id {
            Some(id) => {
                // So an id handed out later never repeats one restored.
                NEXT_ID.fetch_max(id.0.saturating_add(1), Ordering::Relaxed);
                id
            }
            None => PaneId(NEXT_ID.fetch_add(1, Ordering::Relaxed)),
        };
        let (cols, rows) = clamp_size(spec.cols, spec.rows);
        let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);

        // portable-pty starts the shell as a login shell (`-zsh` as its
        // argv[0]), as terminals do, and in $HOME when no directory is
        // given or the one given is gone.
        let mut cmd = match spec.command.filter(|argv| !argv.is_empty()) {
            Some(argv) => CommandBuilder::from_argv(argv.into_iter().map(OsString::from).collect()),
            None => CommandBuilder::new_default_prog(),
        };
        if let Some(cwd) = &spec.cwd {
            cmd.cwd(cwd);
        }
        // A server started from inside tmux mustn't make its panes think
        // they are in tmux: the hooks and scripts would talk to it.
        cmd.env_remove("TMUX");
        cmd.env_remove("TMUX_PANE");
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        // The pane's id as the hooks read it, as tmux's TMUX_PANE was; set
        // here since a pane numbered by the counter has its id only now.
        cmd.env("SWITCHBOARD_PANE", id.to_string());
        for (key, value) in &spec.env {
            cmd.env(key, value);
        }
        let (master, pid) = super::pty::open(cmd, pty_size(cols, rows, CELL))?;
        let pty = match Pty::start(master, pid, Vec::new(), false, id, generation, events) {
            Ok(pty) => pty,
            Err(err) => {
                if let Some(pid) = pid.and_then(|p| libc::pid_t::try_from(p).ok()) {
                    // SAFETY: a signal to the child just started, not yet
                    // reaped.
                    unsafe { libc::kill(pid, libc::SIGHUP) };
                }
                return Err(err).context("starting the pane's threads");
            }
        };

        Ok(Pane::around(
            id,
            generation,
            pty,
            (cols, rows),
            spec.scrollback,
            spec.extended_keys,
        ))
    }

    /// Takes up a pane a server handed over as it replaced itself with a
    /// new build: its terminal and program go on as they were, and what it
    /// showed is drawn again in its emulator. Nothing is done to the
    /// program if this fails.
    pub fn adopt(spec: PaneAdopt, events: UnboundedSender<PaneEvent>) -> Result<Pane> {
        NEXT_ID.fetch_max(spec.id.0.saturating_add(1), Ordering::Relaxed);
        let (cols, rows) = clamp_size(spec.cols, spec.rows);
        let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        let pty = Pty::start(
            spec.master,
            spec.pid,
            spec.input,
            spec.hung_up,
            spec.id,
            generation,
            events,
        )
        .context("taking up the pane's terminal")?;
        let mut pane = Pane::around(
            spec.id,
            generation,
            pty,
            (cols, rows),
            spec.scrollback,
            spec.extended_keys,
        );
        // What the replay sets is the program's own doing, so what the
        // emulator reports of it (the title) is taken as it would be.
        pane.process(spec.screen.as_bytes());
        pane.process(&spec.tail);
        Ok(pane)
    }

    /// A pane around a terminal: its emulator of the size given.
    fn around(
        id: PaneId,
        generation: u64,
        pty: Pty,
        (cols, rows): (u16, u16),
        scrollback: usize,
        extended_keys: bool,
    ) -> Pane {
        let queued = Arc::new(Mutex::new(Vec::new()));
        let config = Config {
            scrolling_history: scrollback,
            // Programs may put text on the clipboard (OSC 52), and one that
            // asks for it is answered, with nothing: the clipboard is
            // yours, not every program's.
            osc52: Osc52::CopyPaste,
            // The emulator keeps the kitty keyboard protocol's flags a
            // program sets, and answers its query for them, only when told to.
            kitty_keyboard: extended_keys,
            ..Config::default()
        };
        let size = Size {
            cols: cols.into(),
            rows: rows.into(),
        };
        let term = Term::new(config, &size, Listener(queued.clone()));
        Pane {
            id,
            generation,
            version: NEXT_VERSION.fetch_add(1, Ordering::Relaxed),
            term,
            parser: Processor::new(),
            events: queued,
            pty,
            cols,
            rows,
            title: String::new(),
            exited: None,
            anchor_side: Side::Left,
            graphics: None,
            other_keys: extended_keys.then(ModifyOtherKeys::default),
            cell: CELL,
            colors: Default::default(),
            recent: Vec::new(),
        }
    }

    /// What this pane hands to a new build: its terminal by descriptor, its
    /// program, what was still to be written to it, and what it shows. It
    /// is paused (`pause`) first, so nothing is read or written meanwhile.
    pub fn hand_over(&mut self) -> HandedPane {
        // A frame held back (synchronized output) is drawn into the screen
        // handed over, the rest of it following there; what the emulator
        // answers meanwhile waits with the input.
        let _ = self.flush_sync();
        let other_keys = self.other_keys.as_ref().map_or(0, ModifyOtherKeys::level);
        HandedPane {
            id: self.id,
            fd: self.pty.fd(),
            pid: self.pty.pid(),
            cols: self.cols,
            rows: self.rows,
            input: self.pty.pending_input(),
            hung_up: self.pty.hung_up(),
            screen: super::replay::replay(&mut self.term, &self.title, other_keys),
            tail: super::replay::unfinished(&self.recent).to_vec(),
        }
    }

    /// The terminal's master, by descriptor.
    pub fn fd(&self) -> std::os::fd::RawFd {
        self.pty.fd()
    }

    /// Tells the program its size again, as after a handover: the terminal
    /// is given it, and the program in front SIGWINCH, so a full-screen
    /// program draws itself again though its size is the same.
    pub fn remind_size(&self) {
        let _ = self.pty.resize(pty_size(self.cols, self.rows, self.cell));
        self.pty.remind_size();
    }

    pub fn id(&self) -> PaneId {
        self.id
    }

    /// Which program of the pane's this is: what its events carry.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// What `styled_lines` gives is the same while this is.
    pub fn version(&self) -> u64 {
        self.version
    }

    fn changed(&mut self) {
        self.version = NEXT_VERSION.fetch_add(1, Ordering::Relaxed);
    }

    /// Whether the program is still running (not yet reaped).
    pub fn running(&self) -> bool {
        self.exited.is_none() && !self.pty.reaped()
    }

    /// Whether kitty's graphics commands in the output are picked out for
    /// the terminal (`TermNotice::Graphics`); a command half read when it is
    /// turned off is dropped.
    pub fn pass_graphics(&mut self, on: bool) {
        match (on, self.graphics.is_some()) {
            (true, false) => self.graphics = Some(Default::default()),
            (false, true) => self.graphics = None,
            _ => {}
        }
    }

    /// Feeds what the program wrote to the emulator, writing its answers to
    /// the program's queries back to the PTY; gives what the server should
    /// know of (a title, the bell, the clipboard, graphics commands).
    ///
    /// A program drawing a frame inside a synchronized update (mode 2026,
    /// as Claude Code does) has it held back until the update ends, so a
    /// half-drawn frame is never shown; one that never ends is let through
    /// after 150ms, here when more output comes, else by `flush_sync`.
    pub fn process(&mut self, bytes: &[u8]) -> Vec<TermNotice> {
        self.changed();
        // The emulator drops what it doesn't draw, kitty's commands with it,
        // so they are picked out first.
        let mut notices: Vec<TermNotice> = match &mut self.graphics {
            Some(scanner) => scanner
                .feed(bytes)
                .into_iter()
                .map(TermNotice::Graphics)
                .collect(),
            None => Vec::new(),
        };
        if let Some(keys) = &mut self.other_keys {
            keys.feed(bytes);
        }
        if bytes.len() >= RECENT {
            self.recent.clear();
            self.recent
                .extend_from_slice(&bytes[bytes.len() - RECENT..]);
        } else {
            self.recent.extend_from_slice(bytes);
            let over = self.recent.len().saturating_sub(RECENT);
            self.recent.drain(..over);
        }
        if self.sync_deadline().is_some_and(|at| at <= Instant::now()) {
            self.parser.stop_sync(&mut self.term);
        }
        self.parser.advance(&mut self.term, bytes);
        notices.extend(self.handle_events());
        notices
    }

    /// When the synchronized update the program began, if any, has waited
    /// long enough to be drawn as it is: the server calls `flush_sync`
    /// then, unless more output came first.
    pub fn sync_deadline(&self) -> Option<Instant> {
        self.parser.sync_timeout().sync_timeout()
    }

    /// Draws what a synchronized update has held back, ended or not.
    pub fn flush_sync(&mut self) -> Vec<TermNotice> {
        self.changed();
        if self.sync_deadline().is_some() {
            self.parser.stop_sync(&mut self.term);
        }
        self.handle_events()
    }

    /// Answers what the program asked and gathers what the server should
    /// know of, from what the emulator queued while reading.
    fn handle_events(&mut self) -> Vec<TermNotice> {
        let events = std::mem::take(&mut *lock(&self.events));
        let mut notices = Vec::new();
        for event in events {
            match event {
                Event::PtyWrite(text) => self.reply(text),
                Event::Title(title) => self.set_title(Some(title), &mut notices),
                Event::ResetTitle => self.set_title(None, &mut notices),
                Event::Bell => notices.push(TermNotice::Bell),
                Event::ClipboardStore(_, text) => notices.push(TermNotice::Clipboard(text)),
                Event::ClipboardLoad(_, format) => self.reply(format("")),
                Event::ColorRequest(index, format) => {
                    // One the program set, else the terminal's own, as tmux
                    // answered from its client's.
                    let color = self.term.colors()[index.min(COLOR_COUNT - 1)]
                        .or_else(|| self.colors.get(index))
                        .unwrap_or_else(|| default_color(index));
                    self.reply(format(color));
                }
                Event::TextAreaSizeRequest(format) => {
                    let (cols, rows) = (self.term.columns(), self.term.screen_lines());
                    self.reply(format(WindowSize {
                        num_lines: rows as u16,
                        num_cols: cols as u16,
                        cell_width: self.cell.0,
                        cell_height: self.cell.1,
                    }));
                }
                _ => {}
            }
        }
        notices
    }

    /// A title the program set; an empty one is a reset. Told to the server
    /// only when it changes, since a shell sets the same one at every
    /// prompt.
    fn set_title(&mut self, title: Option<String>, notices: &mut Vec<TermNotice>) {
        let title = title.filter(|t| !t.is_empty());
        let now = title.as_deref().unwrap_or("");
        if now == self.title {
            return;
        }
        self.title = now.to_string();
        notices.push(TermNotice::Title(title));
    }

    fn reply(&mut self, text: String) {
        let _ = self.write(text.as_bytes());
    }

    /// Sends bytes to the program, as if typed. They are queued for the
    /// pane's writer thread, so this never waits on the program; it fails
    /// once the program has ended or its terminal can't be written to.
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.pty.write(bytes)
    }

    /// Resizes the PTY and the emulator; a no-op when the size is the same.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.changed();
        let (cols, rows) = clamp_size(cols, rows);
        if (cols, rows) == (self.cols, self.rows) {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.term.resize(Size {
            cols: cols.into(),
            rows: rows.into(),
        });
        // The program learns of it by SIGWINCH; a terminal that is gone
        // has no program left to tell.
        let _ = self.pty.resize(pty_size(cols, rows, self.cell));
    }

    /// The colours the terminal said it draws in, for the program's
    /// questions from now on.
    pub fn set_colors(&mut self, colors: super::terminal::Colors) {
        self.changed();
        self.colors = colors;
    }

    /// The cell's size in pixels, as the terminal said it: the PTY's
    /// window in pixels follows, which the program learns of by SIGWINCH,
    /// and `CSI 14 t` is answered with it. A no-op when it is the same.
    pub fn set_cell(&mut self, cell: (u16, u16)) {
        if cell == self.cell || cell.0 == 0 || cell.1 == 0 {
            return;
        }
        self.cell = cell;
        let _ = self.pty.resize(pty_size(self.cols, self.rows, self.cell));
    }

    /// (columns, rows).
    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// The modes the program has set, for encoding what is sent to it.
    pub fn modes(&self) -> PaneModes {
        let mode = *self.term.mode();
        let mouse = if mode.contains(TermMode::MOUSE_MOTION) {
            MouseReporting::Motion
        } else if mode.contains(TermMode::MOUSE_DRAG) {
            MouseReporting::Drag
        } else if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
            MouseReporting::Press
        } else {
            MouseReporting::Off
        };
        PaneModes {
            app_cursor: mode.contains(TermMode::APP_CURSOR),
            app_keypad: mode.contains(TermMode::APP_KEYPAD),
            bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
            focus_events: mode.contains(TermMode::FOCUS_IN_OUT),
            mouse,
            mouse_sgr: mode.contains(TermMode::SGR_MOUSE),
            mouse_utf8: mode.contains(TermMode::UTF8_MOUSE),
            alt_screen: mode.contains(TermMode::ALT_SCREEN),
            alternate_scroll: mode.contains(TermMode::ALTERNATE_SCROLL),
            kitty_keys: kitty_flags(mode),
            modify_other_keys: self.other_keys.as_ref().map_or(0, ModifyOtherKeys::level),
        }
    }

    /// The cursor's shape as the program set it (`CSI q`), for the client
    /// to show while this pane has the focus: Neovim's bar in Insert mode.
    /// A steady block, which is also what a program that never set one
    /// has, is your terminal's own cursor.
    pub fn cursor_style(&self) -> SetCursorStyle {
        let style = self.term.cursor_style();
        match (style.shape, style.blinking) {
            (CursorShape::Block | CursorShape::HollowBlock, false) => {
                SetCursorStyle::DefaultUserShape
            }
            (CursorShape::Block | CursorShape::HollowBlock, true) => SetCursorStyle::BlinkingBlock,
            (CursorShape::Underline, false) => SetCursorStyle::SteadyUnderScore,
            (CursorShape::Underline, true) => SetCursorStyle::BlinkingUnderScore,
            (CursorShape::Beam, false) => SetCursorStyle::SteadyBar,
            (CursorShape::Beam, true) => SetCursorStyle::BlinkingBar,
            (CursorShape::Hidden, _) => SetCursorStyle::DefaultUserShape,
        }
    }

    /// Draws the screen (scrolled back as far as `scroll` says) into `area`
    /// of `buf`, with the selection shown reversed; gives where the cursor
    /// is, in `buf`'s coordinates, when it should be shown.
    pub fn render(&self, buf: &mut Buffer, area: Rect, view: &PaneView) -> Option<(u16, u16)> {
        let visible = area.intersection(buf.area);
        let grid = self.term.grid();
        let (cols, rows) = (self.term.columns(), self.term.screen_lines());
        let offset = grid.display_offset();
        let colors = self.term.colors();
        let selection = self
            .term
            .selection
            .as_ref()
            .and_then(|s| s.to_range(&self.term));
        let mut symbol = String::new();

        for y in visible.top()..visible.bottom() {
            let row = usize::from(y - area.y);
            // Whether the cell just drawn was a wide character, whose second
            // half ratatui leaves blank for it.
            let mut wide_before = false;
            for x in visible.left()..visible.right() {
                let col = usize::from(x - area.x);
                let Some(out) = buf.cell_mut((x, y)) else {
                    continue;
                };
                out.reset();
                if row >= rows || col >= cols {
                    wide_before = false;
                    continue;
                }
                let point = GridPoint::new(Line(row as i32 - offset as i32), Column(col));
                let cell = &grid[point];

                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) && wide_before {
                    wide_before = false;
                    continue;
                }
                let wide = cell.flags.contains(Flags::WIDE_CHAR);
                // A wide character with no room for its second half here, or
                // the gap at a line's end where one didn't fit, is a blank.
                let fits = !wide || x + 1 < visible.right();
                let blank = !fits
                    || cell
                        .flags
                        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER);
                symbol.clear();
                if blank || cell.c == '\t' || cell.c.is_control() {
                    symbol.push(' ');
                } else {
                    symbol.push(cell.c);
                    if let Some(marks) = cell.zerowidth() {
                        symbol.extend(marks);
                    }
                }
                out.set_symbol(&symbol);
                out.fg = to_color(cell.fg, colors);
                out.bg = to_color(cell.bg, colors);
                let mut modifier = modifiers(cell);
                let selected = selection.is_some_and(|range| {
                    range.contains(point)
                        || (wide && range.contains(GridPoint::new(point.line, point.column + 1)))
                });
                if selected {
                    modifier.toggle(Modifier::REVERSED);
                }
                if view.dim {
                    modifier.insert(Modifier::DIM);
                }
                out.modifier = modifier;
                wide_before = wide && fits;
            }
        }

        let cursor = grid.cursor.point;
        let shown = view.focused
            && offset == 0
            && self.term.mode().contains(TermMode::SHOW_CURSOR)
            && self.term.cursor_style().shape != CursorShape::Hidden;
        if !shown {
            return None;
        }
        let mut col = cursor.column.0;
        // On a wide character's second half, it is drawn on the first.
        if col > 0
            && col < cols
            && grid[cursor.line][Column(col)]
                .flags
                .contains(Flags::WIDE_CHAR_SPACER)
        {
            col -= 1;
        }
        let x = area.x.checked_add(u16::try_from(col).ok()?)?;
        let y = area.y.checked_add(u16::try_from(cursor.line.0).ok()?)?;
        (x >= visible.left() && x < visible.right() && y >= visible.top() && y < visible.bottom())
            .then_some((x, y))
    }

    /// The hyperlinks (OSC 8) on the screen as `render` drew it into `area`
    /// of a frame (`frame_area`), each cell's put in `links`, so the frame
    /// sends them on to the terminal. An address that can't safely go to
    /// the terminal is left out (`Links::safe`).
    pub fn put_links(&self, area: Rect, frame_area: Rect, links: &mut Links) {
        let visible = area.intersection(frame_area);
        let grid = self.term.grid();
        let (cols, rows) = (self.term.columns(), self.term.screen_lines());
        let offset = grid.display_offset() as i32;
        // The address last seen, kept whole for the cells after it.
        let mut last: Option<(Hyperlink, Option<Arc<str>>)> = None;
        for y in visible.top()..visible.bottom() {
            let row = usize::from(y - area.y);
            if row >= rows {
                break;
            }
            for x in visible.left()..visible.right() {
                let col = usize::from(x - area.x);
                if col >= cols {
                    break;
                }
                let cell = &grid[GridPoint::new(Line(row as i32 - offset), Column(col))];
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                let Some(link) = cell.hyperlink() else {
                    continue;
                };
                let uri = match &last {
                    Some((seen, uri)) if *seen == link => uri.clone(),
                    _ => {
                        let uri = Links::safe(link.uri()).then(|| Arc::from(link.uri()));
                        last = Some((link, uri.clone()));
                        uri
                    }
                };
                if let Some(uri) = uri {
                    links.set(x, y, uri);
                }
            }
        }
    }

    /// Scrolls the view back (positive) or forward (negative) by lines.
    pub fn scroll(&mut self, lines: i32) {
        self.changed();
        self.term.scroll_display(Scroll::Delta(lines));
    }

    /// Back to the live screen.
    pub fn scroll_to_bottom(&mut self) {
        self.changed();
        self.term.scroll_display(Scroll::Bottom);
    }

    /// How many lines back the view is (0: the live screen).
    pub fn scrolled(&self) -> usize {
        self.term.grid().display_offset()
    }

    /// A point on the screen as now shown (scrolled back or not), as the
    /// emulator's grid has it, kept inside the screen.
    fn screen_point(&self, col: u16, row: u16) -> GridPoint {
        let cols = self.term.columns();
        let rows = self.term.screen_lines();
        let col = usize::from(col).min(cols.saturating_sub(1));
        let row = usize::from(row).min(rows.saturating_sub(1));
        let offset = self.term.grid().display_offset();
        GridPoint::new(Line(row as i32 - offset as i32), Column(col))
    }

    /// Starts a selection at a point on the screen as now shown (`col`,
    /// `row` from its top left); `block` for a rectangle. Nothing is
    /// selected until it is extended, so a click alone selects nothing.
    pub fn select_start(&mut self, col: u16, row: u16, block: bool) {
        self.changed();
        let point = self.screen_point(col, row);
        let ty = if block {
            SelectionType::Block
        } else {
            SelectionType::Simple
        };
        self.anchor_side = Side::Left;
        self.term.selection = Some(Selection::new(ty, point, Side::Left));
    }

    /// Moves the selection's end to a point on the screen as now shown.
    ///
    /// Both ends' cells are selected, as tmux selects, whichever way the
    /// mouse goes. alacritty's selection counts the side of a cell each end
    /// is on, so the fixed end (the anchor) is put on the side facing away
    /// from the other: its left when the end is after it, its right when
    /// before. The selection is built again from the anchor each time, and
    /// the anchor is read back from the selection the emulator holds rather
    /// than kept here, since the emulator moves a selection with its text
    /// as output scrolls the screen.
    pub fn select_extend(&mut self, col: u16, row: u16) {
        self.changed();
        let point = self.screen_point(col, row);
        let Some(selection) = self.term.selection.as_mut() else {
            return;
        };
        let ty = selection.ty;
        if matches!(ty, SelectionType::Semantic | SelectionType::Lines) {
            // These grow to whole words or lines at both ends whichever
            // side is given, so only the moving end changes.
            selection.update(point, Side::Right);
            return;
        }
        let Some(anchor) = self.selection_anchor() else {
            return;
        };
        let anchor_first = match ty {
            SelectionType::Block => {
                anchor.column < point.column
                    || (anchor.column == point.column && anchor.line <= point.line)
            }
            _ => anchor <= point,
        };
        let (anchor_side, end_side) = if anchor_first {
            (Side::Left, Side::Right)
        } else {
            (Side::Right, Side::Left)
        };
        let mut selection = Selection::new(ty, anchor, anchor_side);
        selection.update(point, end_side);
        self.anchor_side = anchor_side;
        self.term.selection = Some(selection);
    }

    /// Where the selection's fixed end is now. The selection keeps it as
    /// its first point, which only the emulator's scrolling moves; asking
    /// for its range with the other end put at the screen's last cell
    /// gives it back as the range's start (a cell on when it is on its
    /// cell's right side, which is taken off again).
    fn selection_anchor(&self) -> Option<GridPoint> {
        let selection = self.term.selection.as_ref()?;
        let last = GridPoint::new(self.term.bottommost_line(), self.term.last_column());
        let mut probe = selection.clone();
        probe.update(last, Side::Right);
        // Empty only when the anchor is that last cell itself, on its right.
        let Some(range) = probe.to_range(&self.term) else {
            return Some(last);
        };
        let mut anchor = range.start;
        if self.anchor_side == Side::Right {
            if anchor.column.0 > 0 {
                anchor.column -= 1;
            } else if selection.ty == SelectionType::Simple {
                anchor.line -= 1;
                anchor.column = self.term.last_column();
            }
        }
        Some(anchor)
    }

    /// Selects the word, or with `line` the line, at a point.
    pub fn select_semantic(&mut self, col: u16, row: u16, line: bool) {
        self.changed();
        let point = self.screen_point(col, row);
        let ty = if line {
            SelectionType::Lines
        } else {
            SelectionType::Semantic
        };
        self.anchor_side = Side::Left;
        self.term.selection = Some(Selection::new(ty, point, Side::Left));
    }

    pub fn select_clear(&mut self) {
        self.changed();
        self.term.selection = None;
    }

    /// The selected text, lines joined by newlines, wrapped lines joined.
    pub fn selection_text(&self) -> Option<String> {
        self.term
            .selection_to_string()
            .filter(|text| !text.is_empty())
    }

    // ── Copy mode's cursor ──────────────────────────────────────────────────

    /// Copy mode's cursor comes up, where the program's cursor is (at the
    /// top of the view when that is scrolled out of sight), as tmux's copy
    /// mode put its own cursor there. The emulator's vi mode is that cursor:
    /// it moves through the history, scrolling the view to keep in sight,
    /// and drags a selection's moving end with it.
    pub fn copy_begin(&mut self) {
        self.changed();
        if !self.copying() {
            self.term.toggle_vi_mode();
        }
    }

    /// Copy mode ends: its cursor and selection go, and the view goes back
    /// to the live screen.
    pub fn copy_end(&mut self) {
        self.changed();
        if self.copying() {
            self.term.toggle_vi_mode();
        }
        self.term.selection = None;
        self.term.scroll_display(Scroll::Bottom);
    }

    /// Whether copy mode's cursor is up.
    pub fn copying(&self) -> bool {
        self.term.mode().contains(TermMode::VI)
    }

    /// Moves copy mode's cursor, the view scrolling to keep it in sight.
    pub fn copy_move(&mut self, motion: Motion) {
        self.changed();
        if !self.copying() {
            return;
        }
        let vi = match motion {
            Motion::Left => ViMotion::Left,
            Motion::Right => ViMotion::Right,
            Motion::Up => ViMotion::Up,
            Motion::Down => ViMotion::Down,
            Motion::WordNext => ViMotion::SemanticRight,
            Motion::WordBack => ViMotion::SemanticLeft,
            Motion::WordEnd => ViMotion::SemanticRightEnd,
            Motion::BigWordNext => ViMotion::WordRight,
            Motion::BigWordBack => ViMotion::WordLeft,
            Motion::BigWordEnd => ViMotion::WordRightEnd,
            Motion::LineStart => ViMotion::First,
            Motion::LineEnd => ViMotion::Last,
            Motion::FirstChar => ViMotion::FirstOccupied,
            Motion::ScreenTop => ViMotion::High,
            Motion::ScreenMiddle => ViMotion::Middle,
            Motion::ScreenBottom => ViMotion::Low,
            Motion::Top => {
                let top = GridPoint::new(self.term.topmost_line(), Column(0));
                self.term.vi_goto_point(top);
                return;
            }
            Motion::Bottom => {
                let bottom = GridPoint::new(self.term.bottommost_line(), Column(0));
                self.term.vi_goto_point(bottom);
                return;
            }
            Motion::Scroll(lines) => {
                // The view and the cursor move together, so the cursor keeps
                // its row on the screen, as tmux's halfpage-up and page-up.
                let cursor = self.term.vi_mode_cursor.scroll(&self.term, lines);
                self.term.scroll_display(Scroll::Delta(lines));
                self.term.vi_goto_point(cursor.point);
                return;
            }
        };
        self.term.vi_motion(vi);
    }

    /// Starts a selection of `kind` at copy mode's cursor, which then drags
    /// its other end; the same kind again takes it away, and another kind
    /// changes it, as v, V and C-v do in tmux's copy-mode-vi.
    pub fn copy_select(&mut self, kind: SelectKind) {
        self.changed();
        if !self.copying() {
            return;
        }
        let ty = match kind {
            SelectKind::Chars => SelectionType::Simple,
            SelectKind::Lines => SelectionType::Lines,
            SelectKind::Block => SelectionType::Block,
        };
        match self.term.selection.as_mut() {
            Some(s) if s.ty == ty => self.term.selection = None,
            Some(s) => {
                s.ty = ty;
                s.include_all();
            }
            None => {
                let mut s = Selection::new(ty, self.term.vi_mode_cursor.point, Side::Left);
                s.include_all();
                self.anchor_side = Side::Left;
                self.term.selection = Some(s);
            }
        }
    }

    /// Where copy mode's cursor is on the screen as now shown (column, row
    /// from its top left), when it is up and in sight.
    pub fn copy_cursor(&self) -> Option<(u16, u16)> {
        if !self.copying() {
            return None;
        }
        let point = self.term.vi_mode_cursor.point;
        let row = point.line.0 + self.term.grid().display_offset() as i32;
        if row < 0 || row >= self.term.screen_lines() as i32 {
            return None;
        }
        Some((u16::try_from(point.column.0).ok()?, row as u16))
    }

    /// Moves copy mode's cursor to the next match of `pattern` after it
    /// (`forward`) or before it, round the history's ends when there is none
    /// that way, as tmux's search-forward and search-backward did. Smart
    /// case: a pattern with no capital matches either case. A pattern that
    /// isn't a regular expression is looked for as it is written. Gives
    /// whether there was a match.
    pub fn copy_search(&mut self, pattern: &str, forward: bool) -> bool {
        self.changed();
        if !self.copying() || pattern.is_empty() {
            return false;
        }
        let built =
            RegexSearch::new(pattern).or_else(|_| RegexSearch::new(&regex::escape(pattern)));
        let Ok(mut search) = built else {
            return false;
        };
        let cursor = self.term.vi_mode_cursor.point;
        let (origin, direction) = if forward {
            (cursor.add(&self.term, Boundary::None, 1), SearchWay::Right)
        } else {
            (cursor.sub(&self.term, Boundary::None, 1), SearchWay::Left)
        };
        match self
            .term
            .search_next(&mut search, origin, direction, Side::Left, None)
        {
            Some(found) => {
                self.term.vi_goto_point(*found.start());
                true
            }
            None => false,
        }
    }

    /// The last `lines` lines of the screen and scrollback as text, trailing
    /// blanks trimmed: what tmux's `capture-pane -p -S -<lines>` gave.
    ///
    /// As with tmux, that is the whole screen (the live one, however far
    /// the view is scrolled back) and up to `lines` lines of scrollback
    /// above it, with each line's trailing blanks trimmed, wrapped lines
    /// joined (its `-J`), and the empty lines at the end dropped, so a
    /// screen with a prompt at its top gives the prompt, not a page of
    /// blanks. Lines are joined by newlines, with none after the last.
    pub fn text(&self, lines: usize) -> String {
        self.text_and_links(lines).0
    }

    /// As `text`, with what is drawn dimmed made blanks: what a program
    /// shows greyed is a hint rather than something typed, as Claude Code
    /// shows its prompt's placeholder and suggested next prompt
    /// (`capture-pane -D`, for the relay's check for an unsent draft).
    pub fn typed_text(&self, lines: usize) -> String {
        self.lines_and_links(lines, false).0
    }

    /// As `text`, with the hyperlinks (OSC 8) in it: each run of text a
    /// program made a link, by its bytes in the text, and its address, as
    /// tmux's `capture-pane -e` kept them for link-view.sh to read.
    pub fn text_and_links(&self, lines: usize) -> (String, Vec<(Range<usize>, String)>) {
        self.lines_and_links(lines, true)
    }

    fn lines_and_links(&self, lines: usize, dimmed: bool) -> (String, Vec<(Range<usize>, String)>) {
        let grid = self.term.grid();
        let cols = self.term.columns();
        let history = grid.history_size().min(lines);
        let top = -(history as i32);
        let bottom = self.term.screen_lines() as i32;

        let mut out: Vec<(String, Linked)> = Vec::new();
        let mut current = String::new();
        let mut found: Linked = Vec::new();
        for line in top..bottom {
            let row = &grid[Line(line)];
            for col in 0..cols {
                let cell = &row[Column(col)];
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                let start = current.len();
                if !dimmed && cell.flags.contains(Flags::DIM) {
                    current.push(' ');
                    continue;
                }
                current.push(if cell.c == '\t' { ' ' } else { cell.c });
                if let Some(marks) = cell.zerowidth() {
                    current.extend(marks);
                }
                if let Some(link) = cell.hyperlink() {
                    match found.last_mut() {
                        Some((range, uri)) if range.end == start && uri == link.uri() => {
                            range.end = current.len();
                        }
                        _ => found.push((start..current.len(), link.uri().to_string())),
                    }
                }
            }
            if cols > 0 && row[Column(cols - 1)].flags.contains(Flags::WRAPLINE) {
                continue;
            }
            end_line(&mut current, &mut found, &mut out);
        }
        if !current.is_empty() {
            end_line(&mut current, &mut found, &mut out);
        }
        while out.last().is_some_and(|(line, _)| line.is_empty()) {
            out.pop();
        }
        let mut text = String::new();
        let mut links = Vec::new();
        for (i, (line, found)) in out.into_iter().enumerate() {
            if i > 0 {
                text.push('\n');
            }
            let at = text.len();
            links.extend(
                found
                    .into_iter()
                    .map(|(range, uri)| (range.start + at..range.end + at, uri)),
            );
            text.push_str(&line);
        }
        (text, links)
    }

    /// The screen's rows in the pane's own colors, as `render` draws them,
    /// each row's trailing blanks and the blank rows at the end left out,
    /// for UX 2.0's mission control cards. Read from the emulator in memory
    /// alone.
    pub fn styled_lines(&self) -> Vec<Vec<ratatui::text::Span<'static>>> {
        use ratatui::style::Style;
        use ratatui::text::Span;
        use unicode_width::UnicodeWidthStr;
        let (cols, rows) = (self.term.columns(), self.term.screen_lines());
        let (Ok(w), Ok(h)) = (u16::try_from(cols), u16::try_from(rows)) else {
            return Vec::new();
        };
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        self.render(&mut buf, area, &PaneView::default());
        let mut out: Vec<Vec<Span<'static>>> = (0..h)
            .map(|y| {
                let cells: Vec<_> = (0..w).filter_map(|x| buf.cell((x, y))).collect();
                let end = cells
                    .iter()
                    .rposition(|c| c.symbol() != " " || c.bg != Color::Reset)
                    .map_or(0, |i| i + 1);
                let mut row: Vec<Span<'static>> = Vec::new();
                // The blank ratatui leaves after a wide character's first half.
                let mut skip = false;
                for c in &cells[..end] {
                    if std::mem::take(&mut skip) {
                        continue;
                    }
                    skip = c.symbol().width() > 1;
                    let style = Style::new().fg(c.fg).bg(c.bg).add_modifier(c.modifier);
                    match row.last_mut() {
                        Some(last) if last.style == style => {
                            last.content.to_mut().push_str(c.symbol());
                        }
                        _ => row.push(Span::styled(c.symbol().to_string(), style)),
                    }
                }
                row
            })
            .collect();
        while out.last().is_some_and(Vec::is_empty) {
            out.pop();
        }
        out
    }

    /// The title the program set, or "".
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The pane's own program (its shell, usually).
    pub fn pid(&self) -> Option<u32> {
        self.pty.pid()
    }

    /// The process group in front on the PTY (the program you're using).
    pub fn foreground_pgid(&self) -> Option<u32> {
        self.pty.foreground_pgid()
    }

    /// The directory of the program in front, else of the pane's own:
    /// what tmux's `pane_current_path` was.
    pub fn cwd(&self) -> Option<PathBuf> {
        self.foreground_pgid()
            .and_then(super::procinfo::cwd_of)
            .or_else(|| self.pid().and_then(super::procinfo::cwd_of))
    }

    /// The name of the program in front (`zsh`, `claude`, `node`): what
    /// tmux's `pane_current_command` was.
    pub fn foreground_command(&self) -> Option<String> {
        self.foreground_pgid()
            .and_then(super::procinfo::command_of)
            .or_else(|| self.pid().and_then(super::procinfo::command_of))
    }

    /// The arguments of the program in front, its own name first.
    pub fn foreground_args(&self) -> Option<Vec<String>> {
        self.foreground_pgid()
            .and_then(super::procinfo::args_of)
            .or_else(|| self.pid().and_then(super::procinfo::args_of))
    }

    /// Every process in front on the PTY (the foreground process group), for
    /// finding which runs Claude (it has a session file by its pid).
    pub fn foreground_pids(&self) -> Vec<u32> {
        self.foreground_pgid()
            .map(super::procinfo::group_members)
            .unwrap_or_default()
    }

    /// The exit code once the program has ended (`Some(None)` for a signal).
    pub fn exited(&self) -> Option<Option<i32>> {
        self.exited
    }

    /// Marks the pane's program ended (from `PaneEvent::Exited`).
    pub fn set_exited(&mut self, code: Option<i32>) {
        self.exited = Some(code);
    }

    /// Ends the program: SIGHUP to its process group, as closing a terminal does.
    ///
    /// Closing a terminal hangs up both the shell's group and the group in
    /// front (a program the shell started, in a group of its own), so both
    /// get it here too; the shell passes it on to its other jobs.
    pub fn kill(&mut self) {
        if self.exited.is_some() {
            return;
        }
        self.pty.hang_up();
    }

    /// Stops reading the program's output and writing to it, between two
    /// reads and two writes, as a server does before handing its panes to
    /// a new build: what the program writes meanwhile waits in the
    /// terminal. `parked` says when it has stopped; `resume` reads again.
    pub fn pause(&self) {
        self.pty.pause();
    }

    /// Whether, paused, nothing more will be read or written.
    pub fn parked(&self) -> bool {
        self.pty.parked()
    }

    /// Reads and writes again after `pause`.
    pub fn resume(&self) {
        self.pty.resume();
    }
}

/// A pane let go of takes its program with it, as closing a tmux pane did:
/// the reader thread's copy of the terminal keeps it open, so dropping the
/// pane alone wouldn't hang it up.
impl Drop for Pane {
    fn drop(&mut self) {
        self.kill();
    }
}

/// How many colors the emulator keeps: the 256 of the palette, then the
/// default foreground, background and cursor, the dim colors, the bright
/// and dim foreground (alacritty's `term::color::COUNT`).
const COLOR_COUNT: usize = 269;

/// A cell's color for ratatui. The sixteen named colors and the palette
/// stay indexes, so they come out in your terminal's own colors, and the
/// default foreground and background are your terminal's default; a color
/// the program changed (OSC 4, 10, 11) is drawn as it set it.
fn to_color(color: AnsiColor, colors: &Colors) -> Color {
    let set = |index: usize| colors[index].map(|Rgb { r, g, b }| Color::Rgb(r, g, b));
    match color {
        AnsiColor::Spec(Rgb { r, g, b }) => Color::Rgb(r, g, b),
        AnsiColor::Indexed(index) => set(index.into()).unwrap_or(Color::Indexed(index)),
        AnsiColor::Named(name) => {
            let index = name as usize;
            match name {
                _ if index < 16 => set(index).unwrap_or(Color::Indexed(index as u8)),
                NamedColor::DimBlack
                | NamedColor::DimRed
                | NamedColor::DimGreen
                | NamedColor::DimYellow
                | NamedColor::DimBlue
                | NamedColor::DimMagenta
                | NamedColor::DimCyan
                | NamedColor::DimWhite => set(index).unwrap_or(Color::Indexed(
                    (index - NamedColor::DimBlack as usize) as u8,
                )),
                NamedColor::Background => set(index).unwrap_or(Color::Reset),
                NamedColor::Foreground
                | NamedColor::BrightForeground
                | NamedColor::DimForeground => {
                    set(NamedColor::Foreground as usize).unwrap_or(Color::Reset)
                }
                // The cursor's color, which no cell is drawn in.
                _ => Color::Reset,
            }
        }
    }
}

fn modifiers(cell: &Cell) -> Modifier {
    let flags = cell.flags;
    let mut modifier = Modifier::empty();
    for (flag, add) in [
        (Flags::BOLD, Modifier::BOLD),
        (Flags::DIM, Modifier::DIM),
        (Flags::ITALIC, Modifier::ITALIC),
        (Flags::ALL_UNDERLINES, Modifier::UNDERLINED),
        (Flags::INVERSE, Modifier::REVERSED),
        (Flags::HIDDEN, Modifier::HIDDEN),
        (Flags::STRIKEOUT, Modifier::CROSSED_OUT),
    ] {
        if flags.intersects(flag) {
            modifier.insert(add);
        }
    }
    modifier
}

/// The answer to a program asking what a color is, when it hasn't set it
/// itself: xterm's 256-color palette, and for the defaults a light grey
/// (xterm's white, #e5e5e5) on black, the usual dark terminal. The real
/// colors are your terminal's, which the server doesn't know; a program
/// asks mostly to tell a dark background from a light one, and dark is
/// the likelier.
fn default_color(index: usize) -> Rgb {
    const ANSI: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0xcd, 0x00, 0x00),
        (0x00, 0xcd, 0x00),
        (0xcd, 0xcd, 0x00),
        (0x00, 0x00, 0xee),
        (0xcd, 0x00, 0xcd),
        (0x00, 0xcd, 0xcd),
        (0xe5, 0xe5, 0xe5),
        (0x7f, 0x7f, 0x7f),
        (0xff, 0x00, 0x00),
        (0x00, 0xff, 0x00),
        (0xff, 0xff, 0x00),
        (0x5c, 0x5c, 0xff),
        (0xff, 0x00, 0xff),
        (0x00, 0xff, 0xff),
        (0xff, 0xff, 0xff),
    ];
    const FOREGROUND: Rgb = Rgb {
        r: 0xe5,
        g: 0xe5,
        b: 0xe5,
    };
    let rgb = |(r, g, b): (u8, u8, u8)| Rgb { r, g, b };
    // Two thirds as bright, as alacritty dims a color.
    let dim = |Rgb { r, g, b }: Rgb| Rgb {
        r: (u16::from(r) * 2 / 3) as u8,
        g: (u16::from(g) * 2 / 3) as u8,
        b: (u16::from(b) * 2 / 3) as u8,
    };
    match index {
        0..=15 => rgb(ANSI[index]),
        16..=231 => {
            let level = |n: usize| if n == 0 { 0 } else { (55 + 40 * n) as u8 };
            let n = index - 16;
            rgb((level(n / 36), level(n / 6 % 6), level(n % 6)))
        }
        232..=255 => {
            let grey = (8 + 10 * (index - 232)) as u8;
            rgb((grey, grey, grey))
        }
        256 | 258 => FOREGROUND,
        257 => rgb((0, 0, 0)),
        259..=266 => dim(rgb(ANSI[index - 259])),
        267 => rgb((0xff, 0xff, 0xff)),
        268 => dim(FOREGROUND),
        // Past the emulator's colors: none asks, but the foreground serves.
        _ => FOREGROUND,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    fn assert_send<T: Send>() {}

    #[test]
    fn a_pane_can_move_between_threads() {
        assert_send::<Pane>();
    }

    struct Running {
        home: PathBuf,
        pane: Pane,
        rx: UnboundedReceiver<PaneEvent>,
        notices: Vec<TermNotice>,
        exit: Option<Option<i32>>,
    }

    impl Drop for Running {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.home);
        }
    }

    /// A temporary directory of the test's own, so nothing reads or writes
    /// the real HOME: it is the panes' HOME too.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sb-pane-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir.canonicalize().expect("canonical scratch dir")
    }

    fn start(command: Option<&[&str]>, cols: u16, rows: u16, env: &[(&str, &str)]) -> Running {
        let home = scratch("home");
        let mut vars = vec![("HOME".to_string(), home.display().to_string())];
        vars.extend(env.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        let (tx, rx) = unbounded_channel();
        let pane = Pane::spawn(
            PaneSpawn {
                command: command.map(|c| c.iter().map(|s| s.to_string()).collect()),
                env: vars,
                cols,
                rows,
                scrollback: 1000,
                ..PaneSpawn::default()
            },
            tx,
        )
        .expect("spawn");
        Running {
            home,
            pane,
            rx,
            notices: Vec::new(),
            exit: None,
        }
    }

    impl Running {
        /// Feeds the program's output to the emulator until `done` holds
        /// (or five seconds pass, which fails the test).
        fn until(&mut self, what: &str, done: impl Fn(&Running) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if done(self) {
                    return;
                }
                match self.rx.try_recv() {
                    Ok(PaneEvent::Output(id, generation, bytes)) => {
                        assert_eq!(id, self.pane.id());
                        assert_eq!(generation, self.pane.generation());
                        let notices = self.pane.process(&bytes);
                        self.notices.extend(notices);
                    }
                    Ok(PaneEvent::Exited(id, generation, code)) => {
                        assert_eq!(id, self.pane.id());
                        assert_eq!(generation, self.pane.generation());
                        self.pane.set_exited(code);
                        self.exit = Some(code);
                    }
                    Err(_) => {
                        assert!(
                            Instant::now() < deadline,
                            "timed out waiting for {what}; screen:\n{}",
                            self.pane.text(100)
                        );
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
            }
        }

        fn until_text(&mut self, needle: &str) {
            self.until(needle, |r| r.pane.text(1000).contains(needle));
        }

        fn until_exit(&mut self) -> Option<i32> {
            self.until("the exit", |r| r.exit.is_some());
            self.exit.flatten()
        }
    }

    /// A pane whose program waits and prints nothing, for feeding the
    /// emulator by hand.
    fn idle(cols: u16, rows: u16) -> Running {
        start(Some(&["sleep", "30"]), cols, rows, &[])
    }

    fn render(pane: &Pane, view: PaneView) -> (Buffer, Option<(u16, u16)>) {
        let (cols, rows) = pane.size();
        let area = Rect::new(0, 0, cols, rows);
        let mut buf = Buffer::empty(area);
        let cursor = pane.render(&mut buf, area, &view);
        (buf, cursor)
    }

    fn row_text(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn output_and_a_title_reach_the_screen_and_the_server() {
        let mut run = start(
            Some(&["/bin/sh", "-c", "printf 'hello\\033]2;title\\007'"]),
            40,
            5,
            &[],
        );
        run.until_text("hello");
        run.until("the title", |r| r.pane.title() == "title");
        assert!(
            run.notices
                .contains(&TermNotice::Title(Some("title".to_string())))
        );
        assert_eq!(run.until_exit(), Some(0));
        assert_eq!(run.pane.exited(), Some(Some(0)));
    }

    #[test]
    fn the_exit_code_and_a_signal() {
        let mut run = start(Some(&["/bin/sh", "-c", "exit 3"]), 20, 5, &[]);
        assert_eq!(run.until_exit(), Some(3));

        let mut run = idle(20, 5);
        let pid = run.pane.pid().expect("pid");
        assert!(crate::util::is_alive(pid.into()));
        run.pane.kill();
        assert_eq!(run.until_exit(), None);
        assert_eq!(run.pane.exited(), Some(None));
    }

    #[test]
    fn the_program_gets_answers_to_its_queries() {
        // bash reads the answer to "where is the cursor?" as typed input.
        let script =
            r#"printf 'ab\033[6n'; IFS= read -rs -d R reply; printf '\ngot:%s\n' "${reply#*[}""#;
        let mut run = start(Some(&["bash", "-c", script]), 40, 5, &[]);
        run.until_text("got:1;3");
        assert_eq!(run.until_exit(), Some(0));
    }

    #[test]
    fn color_size_clipboard_and_attribute_queries_are_answered() {
        // The terminal echoes what is written to the program, so its
        // answers show on the screen (as ^[ for ESC).
        let mut run = idle(80, 24);
        run.until("the program to start", |r| {
            r.pane.foreground_command().as_deref() == Some("sleep")
        });
        run.pane.process(b"\x1b]11;?\x07");
        run.until_text("]11;rgb:0000/0000/0000");
        run.pane.process(b"\x1b]10;?\x1b\\");
        run.until_text("]10;rgb:e5e5/e5e5/e5e5");
        run.pane.process(b"\x1b]4;196;?\x07");
        run.until_text("]4;196;rgb:ffff/0000/0000");
        run.pane.process(b"\x1b[14t");
        run.until_text("[4;384;640t");
        run.pane.process(b"\x1b]52;c;?\x07");
        run.until_text("]52;c;^G");
        run.pane.process(b"\x1b[c");
        run.until_text("[?6c");
        // A color the program set is the one it is told.
        run.pane.process(b"\x1b]4;1;rgb:12/34/56\x07\x1b]4;1;?\x07");
        run.until_text("]4;1;rgb:1212/3434/5656");
        // One the terminal said is told in place of the stand-in.
        let mut said = crate::mux::terminal::Colors::default();
        said.take(b"\x1b]11;rgb:fa/f8/f0\x07");
        run.pane.set_colors(said);
        run.pane.process(b"\x1b]11;?\x07");
        run.until_text("]11;rgb:fafa/f8f8/f0f0");
    }

    #[test]
    fn the_bell_and_the_clipboard_are_told() {
        let mut run = idle(20, 5);
        let notices = run.pane.process(b"\x07\x1b]52;c;aGVsbG8=\x07");
        assert_eq!(
            notices,
            vec![TermNotice::Bell, TermNotice::Clipboard("hello".to_string())]
        );
        let notices = run.pane.process(b"\x1b]2;one\x07\x1b]2;one\x07\x1b]2;\x07");
        assert_eq!(
            notices,
            vec![
                TermNotice::Title(Some("one".to_string())),
                TermNotice::Title(None)
            ]
        );
        assert_eq!(run.pane.title(), "");
    }

    #[test]
    fn a_resize_reaches_the_program() {
        let mut run = start(
            Some(&["/bin/sh", "-c", "stty size; read x; stty size; read x"]),
            80,
            24,
            &[],
        );
        run.until_text("24 80");
        run.pane.resize(100, 30);
        assert_eq!(run.pane.size(), (100, 30));
        run.pane.write(b"\n").expect("write");
        run.until_text("30 100");
        // The same size again changes nothing; too small is made usable.
        run.pane.resize(100, 30);
        run.pane.resize(0, 0);
        assert_eq!(run.pane.size(), (2, 1));
    }

    #[test]
    fn the_environment_and_directory() {
        let dir = scratch("cwd");
        let script = r#"printf '%s|%s|%s|%s|%s|%s\n' "$TERM" "$COLORTERM" "$FOO" "${TMUX-none}" "$SWITCHBOARD_PANE" "$PWD""#;
        let (tx, rx) = unbounded_channel();
        let pane = Pane::spawn(
            PaneSpawn {
                command: Some(vec!["/bin/sh".into(), "-c".into(), script.into()]),
                cwd: Some(dir.clone()),
                env: vec![
                    ("FOO".into(), "bar".into()),
                    ("HOME".into(), dir.display().to_string()),
                ],
                cols: 200,
                rows: 5,
                ..PaneSpawn::default()
            },
            tx,
        )
        .expect("spawn");
        let mut run = Running {
            home: dir.clone(),
            pane,
            rx,
            notices: Vec::new(),
            exit: None,
        };
        let id = run.pane.id();
        run.until_text(&format!(
            "xterm-256color|truecolor|bar|none|{id}|{}",
            dir.display()
        ));
    }

    #[test]
    fn with_no_command_the_login_shell_runs_in_home() {
        let mut run = start(None, 80, 5, &[("SHELL", "/bin/sh")]);
        run.pane
            .write(b"echo \"[$0] [$PWD]\"; exit\n")
            .expect("write");
        let home = run.home.clone();
        run.until_text(&format!("[-sh] [{}]", home.display()));
        assert_eq!(run.until_exit(), Some(0));
    }

    #[test]
    fn the_program_in_front_its_directory_and_group() {
        // An interactive shell runs each command in a group of its own and
        // puts it in front; this one runs sleep elsewhere than itself.
        let mut run = start(Some(&["bash", "--norc", "--noprofile", "-i"]), 80, 5, &[]);
        let shell = run.pane.pid().expect("pid");
        // In front and named bash: on macOS the pane's process has its group
        // before it has run bash, while it is still a copy of this test.
        run.until("the shell in front", |r| {
            r.pane.foreground_pgid() == Some(shell)
                && r.pane.foreground_command().as_deref() == Some("bash")
        });
        assert_eq!(run.pane.cwd(), Some(run.home.clone()));
        let dir = scratch("front");
        run.pane
            .write(format!("(cd '{}' && exec sleep 30)\n", dir.display()).as_bytes())
            .expect("write");
        run.until("sleep in front", |r| {
            r.pane.foreground_command().as_deref() == Some("sleep")
        });
        let front = run.pane.foreground_pgid().expect("a group in front");
        assert_ne!(front, shell);
        assert_eq!(run.pane.cwd(), Some(dir.clone()));
        assert_eq!(run.pane.foreground_pids(), vec![front]);
        // Closing it hangs up both, as closing a terminal would.
        run.pane.kill();
        assert_eq!(run.until_exit(), None);
        run.until("sleep to end", |_| !crate::util::is_alive(front.into()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wide_characters_take_two_cells() {
        let mut run = idle(10, 3);
        run.pane.process("a漢b\r\n123456789漢".as_bytes());
        let (buf, _) = render(&run.pane, PaneView::default());
        assert_eq!(buf[(0, 0)].symbol(), "a");
        assert_eq!(buf[(1, 0)].symbol(), "漢");
        assert_eq!(buf[(2, 0)].symbol(), " ");
        assert_eq!(buf[(3, 0)].symbol(), "b");
        // No room at the end of a line: it went to the next, leaving a gap.
        assert_eq!(buf[(9, 1)].symbol(), " ");
        assert_eq!(run.pane.text(10), "a漢b\n123456789漢");
        // Cut by a narrower area, the half that doesn't fit is a blank.
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
        run.pane
            .render(&mut buf, Rect::new(0, 0, 2, 1), &PaneView::default());
        assert_eq!(buf[(0, 0)].symbol(), "a");
        assert_eq!(buf[(1, 0)].symbol(), " ");
    }

    #[test]
    fn its_version_changes_with_what_it_draws_and_not_with_reading_it() {
        let mut run = idle(10, 4);
        let at = run.pane.version();
        run.pane.styled_lines();
        assert_eq!(run.pane.version(), at, "reading changes nothing");
        run.pane.process(b"x");
        let after_output = run.pane.version();
        assert_ne!(after_output, at);
        run.pane.resize(12, 4);
        assert_ne!(run.pane.version(), after_output);
    }

    #[test]
    fn styled_lines_keep_the_cells_colors_and_drop_trailing_blanks() {
        let mut run = idle(10, 4);
        run.pane.process("a漢b\r\n\x1b[1;32mok\x1b[0m x".as_bytes());
        let lines = run.pane.styled_lines();
        assert_eq!(lines.len(), 2, "{lines:?}");
        let text = |l: &Vec<ratatui::text::Span>| {
            l.iter().map(|s| s.content.to_string()).collect::<String>()
        };
        assert_eq!(text(&lines[0]), "a漢b");
        assert_eq!(text(&lines[1]), "ok x");
        assert_eq!(lines[1][0].content, "ok");
        assert_eq!(lines[1][0].style.fg, Some(Color::Indexed(2)));
        assert!(lines[1][0].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn typed_text_leaves_what_is_dimmed_blank() {
        let mut run = idle(20, 2);
        run.pane
            .process("> \x1b[2mTry this\x1b[0m\r\n> fix it \x1b[2mhint\x1b[0m".as_bytes());
        assert_eq!(run.pane.text(2), "> Try this\n> fix it hint");
        assert_eq!(run.pane.typed_text(2), ">\n> fix it");
    }

    #[test]
    fn combining_marks_stay_with_their_character() {
        let mut run = idle(10, 2);
        run.pane.process("e\u{301}x".as_bytes());
        let (buf, _) = render(&run.pane, PaneView::default());
        assert_eq!(buf[(0, 0)].symbol(), "e\u{301}");
        assert_eq!(buf[(1, 0)].symbol(), "x");
        assert_eq!(run.pane.text(1), "e\u{301}x");
    }

    #[test]
    fn kittys_placeholder_cells_keep_their_marks_and_colour_through_a_frame() {
        // Two cells of an image whose id is 42: U+10EEEE, its row's and
        // column's marks, in the id's colour (src/notes/images.rs).
        let mut run = idle(10, 2);
        run.pane.process(
            "\x1b[38;2;0;0;42m\u{10EEEE}\u{305}\u{305}\u{10EEEE}\u{305}\u{30D}\x1b[0mz".as_bytes(),
        );
        let (buf, _) = render(&run.pane, PaneView::default());
        assert_eq!(buf[(0, 0)].symbol(), "\u{10EEEE}\u{305}\u{305}");
        assert_eq!(buf[(1, 0)].symbol(), "\u{10EEEE}\u{305}\u{30D}");
        assert_eq!(buf[(2, 0)].symbol(), "z", "each a cell wide");
        assert_eq!(buf[(1, 0)].fg, Color::Rgb(0, 0, 42));

        // Sent to the terminal as they are, colour first, and sent again
        // whole when only one changes.
        let mut screen = crate::mux::compose::Screen::default();
        let mut frame = crate::mux::compose::Frame::new(10, 2);
        run.pane
            .render(&mut frame.buf, Rect::new(0, 0, 10, 2), &PaneView::default());
        let sent = String::from_utf8(screen.render(&frame)).unwrap();
        // In one SGR with the background, as crossterm writes them.
        let colour = sent.find("\x1b[38;2;0;0;42;").expect("the id's colour");
        let cells = sent
            .find("\u{10EEEE}\u{305}\u{305}\u{10EEEE}\u{305}\u{30D}")
            .expect("both cells with their marks");
        assert!(colour < cells, "{sent:?}");
        run.pane
            .process("\x1b[1;2H\x1b[38;2;0;0;42m\u{10EEEE}\u{305}\u{30E}".as_bytes());
        run.pane
            .render(&mut frame.buf, Rect::new(0, 0, 10, 2), &PaneView::default());
        let sent = String::from_utf8(screen.render(&frame)).unwrap();
        assert!(sent.contains("\u{10EEEE}\u{305}\u{30E}"), "{sent:?}");
        assert!(!sent.contains("\u{30D}"), "only the cell that changed");
    }

    #[test]
    fn a_programs_hyperlinks_reach_the_terminal_around_their_text() {
        let mut run = idle(20, 2);
        run.pane.process(
            b"see \x1b]8;;https://example.com/a\x1b\\the page\x1b]8;;\x1b\\ now\r\n\
              \x1b]8;id=x;https://example.com/b\x07b\x1b]8;;\x07",
        );
        let mut screen = crate::mux::compose::Screen::default();
        let mut frame = crate::mux::compose::Frame::new(20, 2);
        let area = Rect::new(0, 0, 20, 2);
        run.pane.render(&mut frame.buf, area, &PaneView::default());
        run.pane.put_links(area, frame.area(), &mut frame.links);
        assert_eq!(
            frame.links.get(4, 0).map(|u| u.to_string()),
            Some("https://example.com/a".into())
        );
        assert_eq!(frame.links.get(3, 0), None, "the space before isn't");
        assert_eq!(frame.links.get(12, 0), None, "nor the one after");
        let sent = String::from_utf8(screen.render(&frame)).unwrap();
        let open = sent
            .find("\x1b]8;;https://example.com/a\x1b\\")
            .expect("the link opened");
        let text = sent.find("the page").expect("its text");
        let close = sent[text..].find("\x1b]8;;\x1b\\").expect("and closed") + text;
        assert!(open < text && close < sent.find("now").unwrap(), "{sent:?}");
        assert!(sent.contains("\x1b]8;;https://example.com/b\x1b\\"));

        // Drawn again with no link there, the same text is sent again
        // without one.
        run.pane.process(b"\x1b[1;5Hthe page");
        run.pane.render(&mut frame.buf, area, &PaneView::default());
        frame.links = crate::mux::compose::Links::default();
        run.pane.put_links(area, frame.area(), &mut frame.links);
        assert_eq!(frame.links.get(4, 0), None);
        let sent = String::from_utf8(screen.render(&frame)).unwrap();
        assert!(sent.contains("the page"), "{sent:?}");
        assert!(!sent.contains("example.com/a"), "{sent:?}");
    }

    #[test]
    fn a_panes_text_gives_its_hyperlinks_by_their_place_in_it() {
        let mut run = idle(12, 3);
        run.pane.process(
            b"one\r\nsee \x1b]8;;https://example.com/long\x1b\\the long page\x1b]8;;\x1b\\  \r\nend",
        );
        let (text, links) = run.pane.text_and_links(100);
        assert_eq!(text, "one\nsee the long page\nend", "a wrapped line joined");
        assert_eq!(links.len(), 1, "{links:?}");
        let (range, uri) = &links[0];
        assert_eq!(&text[range.clone()], "the long page");
        assert_eq!(uri, "https://example.com/long");
        assert_eq!(run.pane.text(100), text);
    }

    #[test]
    fn graphics_commands_are_picked_out_only_while_allowed() {
        let mut run = idle(10, 2);
        let command = b"\x1b_Ga=T,U=1,q=2,i=42;L3RtcC9hLnBuZw==\x1b\\";
        let mut out = b"ab".to_vec();
        out.extend_from_slice(command);
        assert_eq!(run.pane.process(&out), vec![]);
        run.pane.pass_graphics(true);
        assert_eq!(
            run.pane.process(&out),
            vec![TermNotice::Graphics(command.to_vec())]
        );
        // The emulator draws nothing of it.
        assert_eq!(run.pane.text(1), "abab");
        // Half a command when it is turned off is dropped.
        run.pane.process(&command[..10]);
        run.pane.pass_graphics(false);
        run.pane.pass_graphics(true);
        assert_eq!(run.pane.process(&command[10..]), vec![]);
    }

    #[test]
    fn colors_and_attributes_map_to_ratatuis() {
        let mut run = idle(20, 2);
        run.pane.process(
            b"\x1b[31;42mA\x1b[0m\x1b[38;5;200;48;5;17mB\x1b[0m\x1b[38;2;1;2;3;48;2;4;5;6mC\
              \x1b[0mD\x1b[1;2;3;4;7;8;9mE\x1b[0m\x1b[91mF\x1b[0m",
        );
        let (buf, _) = render(&run.pane, PaneView::default());
        let cell = |x: u16| buf[(x, 0)].clone();
        assert_eq!(
            (cell(0).fg, cell(0).bg),
            (Color::Indexed(1), Color::Indexed(2))
        );
        assert_eq!(
            (cell(1).fg, cell(1).bg),
            (Color::Indexed(200), Color::Indexed(17))
        );
        assert_eq!(
            (cell(2).fg, cell(2).bg),
            (Color::Rgb(1, 2, 3), Color::Rgb(4, 5, 6))
        );
        assert_eq!((cell(3).fg, cell(3).bg), (Color::Reset, Color::Reset));
        assert_eq!(cell(3).modifier, Modifier::empty());
        assert_eq!(
            cell(4).modifier,
            Modifier::BOLD
                | Modifier::DIM
                | Modifier::ITALIC
                | Modifier::UNDERLINED
                | Modifier::REVERSED
                | Modifier::HIDDEN
                | Modifier::CROSSED_OUT
        );
        assert_eq!(cell(5).fg, Color::Indexed(9));

        // Dimmed as an inactive pane.
        let (buf, _) = render(
            &run.pane,
            PaneView {
                dim: true,
                focused: false,
            },
        );
        assert!(buf[(3, 0)].modifier.contains(Modifier::DIM));

        // A color the program set for itself is drawn as it set it.
        run.pane
            .process(b"\x1b]4;1;rgb:ff/80/00\x07\x1b]11;rgb:10/20/30\x07");
        let (buf, _) = render(&run.pane, PaneView::default());
        assert_eq!(buf[(0, 0)].fg, Color::Rgb(0xff, 0x80, 0x00));
        assert_eq!(buf[(3, 0)].bg, Color::Rgb(0x10, 0x20, 0x30));
    }

    #[test]
    fn the_default_palette_is_xterms() {
        assert_eq!(
            default_color(1),
            Rgb {
                r: 0xcd,
                g: 0,
                b: 0
            }
        );
        assert_eq!(default_color(16), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(
            default_color(196),
            Rgb {
                r: 0xff,
                g: 0,
                b: 0
            }
        );
        assert_eq!(
            default_color(110),
            Rgb {
                r: 0x87,
                g: 0xaf,
                b: 0xd7
            }
        );
        assert_eq!(
            default_color(231),
            Rgb {
                r: 0xff,
                g: 0xff,
                b: 0xff
            }
        );
        assert_eq!(default_color(232), Rgb { r: 8, g: 8, b: 8 });
        assert_eq!(
            default_color(255),
            Rgb {
                r: 0xee,
                g: 0xee,
                b: 0xee
            }
        );
        assert_eq!(default_color(257), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(default_color(256), default_color(7));
        assert_eq!(
            default_color(260),
            Rgb {
                r: 0x88,
                g: 0,
                b: 0
            }
        );
        assert_eq!(default_color(9999), default_color(256));
    }

    #[test]
    fn the_cursor_is_given_only_when_it_should_show() {
        let mut run = idle(10, 3);
        run.pane.process(b"ab\r\nc");
        let focused = PaneView {
            focused: true,
            dim: false,
        };
        assert_eq!(render(&run.pane, focused).1, Some((1, 1)));
        assert_eq!(render(&run.pane, PaneView::default()).1, None);
        // Offset by the area.
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let at = run.pane.render(&mut buf, Rect::new(5, 2, 10, 3), &focused);
        assert_eq!(at, Some((6, 3)));
        // Not in the area: nothing drawn outside it, no cursor.
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let at = run.pane.render(&mut buf, Rect::new(5, 2, 1, 1), &focused);
        assert_eq!(at, None);
        assert_eq!(buf[(6, 2)].symbol(), " ");
        assert_eq!(buf[(5, 2)].symbol(), "a");
        // Hidden by the program.
        run.pane.process(b"\x1b[?25l");
        assert_eq!(render(&run.pane, focused).1, None);
        run.pane.process(b"\x1b[?25h\x1b[6 q");
        assert_eq!(render(&run.pane, focused).1, Some((1, 1)));
        assert_eq!(run.pane.cursor_style(), SetCursorStyle::SteadyBar);
        run.pane.process(b"\x1b[0 q");
        assert_eq!(run.pane.cursor_style(), SetCursorStyle::DefaultUserShape);
    }

    #[test]
    fn scrollback_scrolls_and_reads_back() {
        let mut run = idle(20, 5);
        for n in 0..50 {
            run.pane.process(format!("line {n}\r\n").as_bytes());
        }
        let focused = PaneView {
            focused: true,
            dim: false,
        };
        let (buf, cursor) = render(&run.pane, focused);
        assert_eq!(row_text(&buf, 0), "line 46");
        assert_eq!(cursor, Some((0, 4)));

        run.pane.scroll(10);
        assert_eq!(run.pane.scrolled(), 10);
        let (buf, cursor) = render(&run.pane, focused);
        assert_eq!(row_text(&buf, 0), "line 36");
        assert_eq!(cursor, None, "no cursor while scrolled back");
        // New output keeps the view on what it shows.
        run.pane.process(b"line 50\r\n");
        assert_eq!(run.pane.scrolled(), 11);
        assert_eq!(row_text(&render(&run.pane, focused).0, 0), "line 36");
        run.pane.scroll(-4);
        assert_eq!(run.pane.scrolled(), 7);
        run.pane.scroll(1000);
        assert_eq!(run.pane.scrolled(), 47);
        assert_eq!(row_text(&render(&run.pane, focused).0, 0), "line 0");
        run.pane.scroll_to_bottom();
        assert_eq!(run.pane.scrolled(), 0);

        // The screen and that much scrollback, as tmux captured it.
        assert_eq!(
            run.pane.text(2),
            "line 45\nline 46\nline 47\nline 48\nline 49\nline 50"
        );
        assert!(run.pane.text(1000).starts_with("line 0\nline 1\n"));
        assert_eq!(run.pane.text(0), "line 47\nline 48\nline 49\nline 50");
    }

    #[test]
    fn text_joins_wrapped_lines_and_trims() {
        let mut run = idle(5, 4);
        run.pane.process(b"abcdefgh  \r\nxy   \r\n\r\n");
        assert_eq!(run.pane.text(10), "abcdefgh\nxy");
        let mut run = idle(20, 4);
        assert_eq!(run.pane.text(10), "");
        // A tab is the blanks it moved over, as tmux captured it.
        run.pane.process(b"a\tb");
        assert_eq!(run.pane.text(10), "a       b");
    }

    #[test]
    fn the_alternate_screen() {
        let mut run = idle(20, 4);
        run.pane.process(b"main text\r\n");
        assert!(!run.pane.modes().alt_screen);
        run.pane.process(b"\x1b[?1049h\x1b[Hfull screen");
        assert!(run.pane.modes().alt_screen);
        assert_eq!(run.pane.text(100), "full screen");
        run.pane.process(b"\x1b[?1049l");
        assert!(!run.pane.modes().alt_screen);
        assert_eq!(run.pane.text(100), "main text");
    }

    #[test]
    fn modes_follow_the_program() {
        let mut run = idle(20, 4);
        let modes = run.pane.modes();
        assert_eq!(modes.mouse, MouseReporting::Off);
        assert!(!modes.app_cursor && !modes.bracketed_paste && !modes.focus_events);
        run.pane
            .process(b"\x1b[?1h\x1b=\x1b[?2004h\x1b[?1004h\x1b[?1000h\x1b[?1005h");
        let modes = run.pane.modes();
        assert!(modes.app_cursor && modes.app_keypad && modes.bracketed_paste);
        assert!(modes.focus_events && modes.mouse_utf8 && !modes.mouse_sgr);
        assert_eq!(modes.mouse, MouseReporting::Press);
        // SGR and UTF-8 coordinates are one or the other.
        run.pane.process(b"\x1b[?1006h");
        assert!(run.pane.modes().mouse_sgr && !run.pane.modes().mouse_utf8);
        run.pane.process(b"\x1b[?1002h");
        assert_eq!(run.pane.modes().mouse, MouseReporting::Drag);
        run.pane.process(b"\x1b[?1003h");
        assert_eq!(run.pane.modes().mouse, MouseReporting::Motion);
        run.pane.process(b"\x1b[?1003l\x1b[?1007l");
        assert_eq!(run.pane.modes().mouse, MouseReporting::Off);
        assert!(!run.pane.modes().alternate_scroll);
    }

    #[test]
    fn keys_told_apart_follow_what_the_program_asked() {
        let home = scratch("keys");
        let spawn = |extended_keys| {
            let (tx, _rx) = unbounded_channel();
            Pane::spawn(
                PaneSpawn {
                    command: Some(vec!["sleep".into(), "30".into()]),
                    env: vec![("HOME".into(), home.display().to_string())],
                    cols: 20,
                    rows: 4,
                    extended_keys,
                    ..PaneSpawn::default()
                },
                tx,
            )
            .expect("spawn")
        };
        use crate::mux::input::{Key, encode_key};
        let ctrl_shift_p = Key::parse("C-S-p").unwrap();
        let sent = |pane: &Pane| encode_key(&ctrl_shift_p, &pane.modes());

        let mut pane = spawn(true);
        assert_eq!(
            (pane.modes().kitty_keys, pane.modes().modify_other_keys),
            (0, 0)
        );
        assert_eq!(sent(&pane), b"\x10", "a shell gets Ctrl-P's byte");
        // Neovim's way and Notes' first: the kitty protocol, pushed and popped.
        pane.process(b"\x1b[>1u");
        assert_eq!(pane.modes().kitty_keys, 1);
        assert_eq!(sent(&pane), b"\x1b[112;6u");
        pane.process(b"\x1b[<u");
        assert_eq!(pane.modes().kitty_keys, 0);
        // Then modifyOtherKeys, which the emulator doesn't keep itself.
        pane.process(b"\x1b[>4;2m");
        assert_eq!(pane.modes().modify_other_keys, 2);
        assert_eq!(sent(&pane), b"\x1b[112;6u");
        pane.process(b"\x1b[>4;0m");
        assert_eq!(pane.modes().modify_other_keys, 0);
        assert_eq!(sent(&pane), b"\x10");
        pane.kill();

        // With the setting off, a program asking is a plain terminal's.
        let mut plain = spawn(false);
        plain.process(b"\x1b[>1u\x1b[>4;2m");
        assert_eq!(
            (plain.modes().kitty_keys, plain.modes().modify_other_keys),
            (0, 0)
        );
        assert_eq!(sent(&plain), b"\x10");
        plain.kill();
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn selections_select_both_ends_either_way() {
        let mut run = idle(20, 4);
        run.pane.process(b"hello world\r\nsecond line");

        run.pane.select_start(0, 0, false);
        assert_eq!(run.pane.selection_text(), None, "a click selects nothing");
        run.pane.select_extend(4, 0);
        assert_eq!(run.pane.selection_text().as_deref(), Some("hello"));
        run.pane.select_extend(10, 0);
        assert_eq!(run.pane.selection_text().as_deref(), Some("hello world"));

        run.pane.select_start(4, 0, false);
        run.pane.select_extend(0, 0);
        assert_eq!(run.pane.selection_text().as_deref(), Some("hello"));
        run.pane.select_extend(4, 0);
        assert_eq!(run.pane.selection_text().as_deref(), Some("o"));
        run.pane.select_extend(5, 1);
        assert_eq!(
            run.pane.selection_text().as_deref(),
            Some("o world\nsecond")
        );
        run.pane.select_extend(2, 0);
        assert_eq!(run.pane.selection_text().as_deref(), Some("llo"));

        // Shown reversed.
        let (buf, _) = render(&run.pane, PaneView::default());
        assert!(buf[(2, 0)].modifier.contains(Modifier::REVERSED));
        assert!(!buf[(5, 0)].modifier.contains(Modifier::REVERSED));

        run.pane.select_semantic(8, 0, false);
        assert_eq!(run.pane.selection_text().as_deref(), Some("world"));
        run.pane.select_semantic(1, 1, true);
        assert_eq!(run.pane.selection_text().as_deref(), Some("second line\n"));

        run.pane.select_start(8, 0, true);
        run.pane.select_extend(7, 1);
        assert_eq!(run.pane.selection_text().as_deref(), Some("or\nli"));
        run.pane.select_extend(9, 1);
        assert_eq!(run.pane.selection_text().as_deref(), Some("rl\nin"));
        run.pane.select_start(7, 1, true);
        run.pane.select_extend(8, 0);
        assert_eq!(run.pane.selection_text().as_deref(), Some("or\nli"));

        run.pane.select_clear();
        assert_eq!(run.pane.selection_text(), None);
        run.pane.select_extend(3, 0);
        assert_eq!(run.pane.selection_text(), None, "nothing to extend");
    }

    #[test]
    fn a_selection_keeps_to_its_text_as_output_scrolls() {
        let mut run = idle(20, 4);
        run.pane.process(b"alpha\r\nbravo\r\ncharlie\r\n");
        run.pane.select_start(4, 1, false);
        run.pane.select_extend(0, 1);
        assert_eq!(run.pane.selection_text().as_deref(), Some("bravo"));
        run.pane.process(b"delta\r\necho\r\n");
        assert_eq!(run.pane.selection_text().as_deref(), Some("bravo"));
        // Its fixed end, the "o" of bravo, moved with it.
        let (buf, _) = render(&run.pane, PaneView::default());
        assert_eq!(row_text(&buf, 0), "charlie");
        run.pane.select_extend(6, 0);
        assert_eq!(run.pane.selection_text().as_deref(), Some("o\ncharlie"));

        // A selection scrolled back reads the same, by the screen as shown.
        run.pane.scroll(1);
        run.pane.select_start(0, 0, false);
        run.pane.select_extend(4, 0);
        assert_eq!(run.pane.selection_text().as_deref(), Some("bravo"));
    }

    #[test]
    fn copy_modes_cursor_moves_selects_and_searches() {
        let mut run = idle(20, 4);
        run.pane.process(b"alpha beta\r\ngamma delta\r\nlast");
        assert_eq!(run.pane.copy_cursor(), None, "not before copy mode");
        run.pane.copy_begin();
        assert!(run.pane.copying());
        // It comes up where the program's cursor is.
        assert_eq!(run.pane.copy_cursor(), Some((4, 2)));
        run.pane.copy_move(Motion::Up);
        run.pane.copy_move(Motion::LineStart);
        assert_eq!(run.pane.copy_cursor(), Some((0, 1)));

        // v, then the cursor's moves stretch it.
        run.pane.copy_select(SelectKind::Chars);
        run.pane.copy_move(Motion::WordEnd);
        assert_eq!(run.pane.selection_text().as_deref(), Some("gamma"));
        run.pane.copy_move(Motion::LineEnd);
        assert_eq!(run.pane.selection_text().as_deref(), Some("gamma delta"));
        // V makes it whole lines, and V again takes it away.
        run.pane.copy_select(SelectKind::Lines);
        assert_eq!(run.pane.selection_text().as_deref(), Some("gamma delta\n"));
        run.pane.copy_select(SelectKind::Lines);
        assert_eq!(run.pane.selection_text(), None);

        // C-v: a rectangle.
        run.pane.copy_move(Motion::Top);
        assert_eq!(run.pane.copy_cursor(), Some((0, 0)));
        run.pane.copy_select(SelectKind::Block);
        run.pane.copy_move(Motion::Down);
        run.pane.copy_move(Motion::Right);
        assert_eq!(run.pane.selection_text().as_deref(), Some("al\nga"));
        // Shown reversed, the cursor's cell among them.
        let (buf, _) = render(&run.pane, PaneView::default());
        assert!(buf[(1, 1)].modifier.contains(Modifier::REVERSED));
        assert!(!buf[(2, 1)].modifier.contains(Modifier::REVERSED));
        run.pane.select_clear();

        // / and ?: the next match either way, round the ends; smart case.
        assert!(run.pane.copy_search("delta", true));
        assert_eq!(run.pane.copy_cursor(), Some((6, 1)));
        assert!(run.pane.copy_search("a", true));
        assert_eq!(run.pane.copy_cursor(), Some((10, 1)), "the next one on");
        assert!(run.pane.copy_search("ALPHA|beta", false));
        assert_eq!(run.pane.copy_cursor(), Some((6, 0)), "case counts");
        assert!(run.pane.copy_search("last", false), "round the top");
        assert_eq!(run.pane.copy_cursor(), Some((0, 2)));
        assert!(!run.pane.copy_search("nowhere", true));
        assert!(
            !run.pane.copy_search("(", true),
            "a bad pattern is looked for as written"
        );

        run.pane.copy_end();
        assert!(!run.pane.copying());
        assert_eq!(run.pane.copy_cursor(), None);
    }

    #[test]
    fn copy_mode_scrolls_the_view_with_its_cursor() {
        let mut run = idle(10, 4);
        for i in 0..20 {
            run.pane.process(format!("line {i}\r\n").as_bytes());
        }
        run.pane.copy_begin();
        let (_, row) = run.pane.copy_cursor().expect("in sight");
        run.pane.copy_move(Motion::Scroll(2));
        assert_eq!(run.pane.scrolled(), 2);
        assert_eq!(run.pane.copy_cursor().map(|c| c.1), Some(row));
        // Up past the top of the screen scrolls it too.
        run.pane.copy_move(Motion::ScreenTop);
        run.pane.copy_move(Motion::Up);
        assert_eq!(run.pane.scrolled(), 3);
        assert_eq!(run.pane.copy_cursor(), Some((0, 0)));
        run.pane.copy_move(Motion::Top);
        assert!(run.pane.scrolled() > 3, "the history's first line");
        run.pane.copy_end();
        assert_eq!(run.pane.scrolled(), 0);
    }

    #[test]
    fn a_selection_across_a_wrapped_line_is_one_line() {
        let mut run = idle(5, 4);
        run.pane.process(b"abcdefgh\r\nxy");
        run.pane.select_start(0, 0, false);
        run.pane.select_extend(1, 2);
        assert_eq!(run.pane.selection_text().as_deref(), Some("abcdefgh\nxy"));
    }

    #[test]
    fn a_synchronized_update_is_drawn_whole() {
        let mut run = idle(20, 4);
        run.pane.process(b"\x1b[?2026hhalf");
        assert_eq!(run.pane.text(10), "");
        assert!(run.pane.sync_deadline().is_some());
        run.pane.process(b" done\x1b[?2026l");
        assert_eq!(run.pane.text(10), "half done");
        assert!(run.pane.sync_deadline().is_none());

        // One that never ends is drawn when asked.
        run.pane.process(b"\r\n\x1b[?2026hstuck");
        assert_eq!(run.pane.text(10), "half done");
        run.pane.flush_sync();
        assert_eq!(run.pane.text(10), "half done\nstuck");
        assert!(run.pane.sync_deadline().is_none());
    }

    #[test]
    fn writes_after_the_program_ends_fail_rather_than_wait() {
        let mut run = start(Some(&["/bin/sh", "-c", "exit 0"]), 20, 4, &[]);
        run.until_exit();
        let err = run.pane.write(b"x").expect_err("the program has ended");
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
        // And killing an ended pane signals nothing.
        run.pane.kill();
    }

    #[test]
    fn a_pane_handed_over_is_taken_up_as_it_was() {
        let mut run = start(
            Some(&[
                "/bin/sh",
                "-c",
                "stty -echo; while read l; do echo \"got:$l\"; done",
            ]),
            30,
            6,
            &[],
        );
        run.pane
            .process(b"\x1b]2;the title\x07before\r\n\x1b[?2004h\x1b[?2026hheld back");
        assert!(run.pane.sync_deadline().is_some(), "a frame held back");
        // Paused, with a key still to be written and a sequence cut short.
        run.pane.pause();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !run.pane.parked() {
            assert!(Instant::now() < deadline, "the pane didn't stop");
            std::thread::sleep(Duration::from_millis(5));
        }
        run.pane.write(b"queued\r").unwrap();
        run.pane.process(b"\r\n\x1b[3");
        let handed = run.pane.hand_over();
        assert_eq!(handed.tail, b"\x1b[3");
        assert_eq!(handed.input, b"queued\r");

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let master = super::super::pty::duplicate(handed.fd).unwrap();
        let pane = Pane::adopt(
            PaneAdopt {
                id: PaneId(77),
                master,
                pid: handed.pid,
                cols: handed.cols,
                rows: handed.rows,
                scrollback: 1000,
                extended_keys: false,
                input: handed.input,
                hung_up: handed.hung_up,
                screen: handed.screen,
                tail: handed.tail,
            },
            tx,
        )
        .unwrap();
        let mut taken = Running {
            home: scratch("adopted"),
            pane,
            rx,
            notices: Vec::new(),
            exit: None,
        };
        assert_eq!(taken.pane.title(), "the title");
        assert!(taken.pane.modes().bracketed_paste);
        assert_eq!(taken.pane.size(), (30, 6));
        let text = taken.pane.text(100);
        assert!(
            text.contains("before") && text.contains("held back"),
            "{text}"
        );
        // The sequence finishes there, as it would have.
        taken.pane.process(b"1mred");
        let (buf, _) = render(&taken.pane, PaneView::default());
        let red = (0..30u16)
            .find(|x| buf[(*x, 2)].symbol() == "r")
            .map(|x| buf[(x, 2)].fg);
        assert_eq!(red, Some(Color::Indexed(1)), "{}", taken.pane.text(100));
        // The key queued is written by the pane that took it up.
        taken.until_text("got:queued");
        // The second ends the program, and the first, let go of, sees it
        // end too.
        taken.pane.kill();
        assert_eq!(taken.until_exit(), None);
        run.pane.resume();
        assert_eq!(run.until_exit(), None);
    }

    #[test]
    fn ids_count_up_past_one_restored() {
        let (tx, _rx) = unbounded_channel();
        let restored = Pane::spawn(
            PaneSpawn {
                id: Some(PaneId(5000)),
                command: Some(vec!["true".into()]),
                cwd: Some(std::env::temp_dir()),
                env: vec![("HOME".into(), std::env::temp_dir().display().to_string())],
                cols: 10,
                rows: 2,
                ..PaneSpawn::default()
            },
            tx.clone(),
        )
        .expect("spawn");
        assert_eq!(restored.id(), PaneId(5000));
        let next = Pane::spawn(
            PaneSpawn {
                command: Some(vec!["true".into()]),
                cwd: Some(std::env::temp_dir()),
                env: vec![("HOME".into(), std::env::temp_dir().display().to_string())],
                cols: 10,
                rows: 2,
                ..PaneSpawn::default()
            },
            tx,
        )
        .expect("spawn");
        assert!(next.id().0 > 5000);
    }

    #[test]
    fn a_program_that_isnt_there_is_an_error() {
        let (tx, _rx) = unbounded_channel();
        let spawned = Pane::spawn(
            PaneSpawn {
                command: Some(vec!["/nonexistent/program".into()]),
                cwd: Some(std::env::temp_dir()),
                env: vec![("HOME".into(), std::env::temp_dir().display().to_string())],
                cols: 10,
                rows: 2,
                ..PaneSpawn::default()
            },
            tx,
        );
        assert!(spawned.is_err());
    }
}
