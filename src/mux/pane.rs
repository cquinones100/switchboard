//! One pane: a program on a PTY (portable-pty) and the terminal emulator
//! that keeps its screen (alacritty_terminal), drawn into a cell buffer.
//! A thread per pane reads the PTY and sends what it reads to the server
//! (`PaneEvent`), which hands it to `process` on its own task, so the
//! emulator is only ever touched there. The emulator's answers to the
//! program's queries (cursor position, device attributes, colors) go
//! straight back to the PTY.
//!
//! STUB: the signatures are the contract; step 2 fills them in.

use std::io;
use std::path::PathBuf;

use anyhow::Result;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use tokio::sync::mpsc::UnboundedSender;

use super::{PaneEvent, PaneId, PaneModes, TermNotice};

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

pub struct Pane {
    id: PaneId,
}

impl Pane {
    /// Starts the program on a new PTY of the given size, and a thread that
    /// sends its output (and its end) to `events`.
    pub fn spawn(spec: PaneSpawn, events: UnboundedSender<PaneEvent>) -> Result<Pane> {
        let _ = (spec, events);
        todo!("pane::Pane::spawn")
    }

    pub fn id(&self) -> PaneId {
        self.id
    }

    /// Feeds what the program wrote to the emulator, writing its answers to
    /// the program's queries back to the PTY; gives what the server should
    /// know of (a title, the bell, the clipboard).
    pub fn process(&mut self, bytes: &[u8]) -> Vec<TermNotice> {
        let _ = bytes;
        todo!("pane::Pane::process")
    }

    /// Sends bytes to the program, as if typed.
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        let _ = bytes;
        todo!("pane::Pane::write")
    }

    /// Resizes the PTY and the emulator; a no-op when the size is the same.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let _ = (cols, rows);
        todo!("pane::Pane::resize")
    }

    /// (columns, rows).
    pub fn size(&self) -> (u16, u16) {
        todo!("pane::Pane::size")
    }

    /// The modes the program has set, for encoding what is sent to it.
    pub fn modes(&self) -> PaneModes {
        todo!("pane::Pane::modes")
    }

    /// Draws the screen (scrolled back as far as `scroll` says) into `area`
    /// of `buf`, with the selection shown reversed; gives where the cursor
    /// is, in `buf`'s coordinates, when it should be shown.
    pub fn render(&self, buf: &mut Buffer, area: Rect, view: &PaneView) -> Option<(u16, u16)> {
        let _ = (buf, area, view);
        todo!("pane::Pane::render")
    }

    /// Scrolls the view back (positive) or forward (negative) by lines.
    pub fn scroll(&mut self, lines: i32) {
        let _ = lines;
        todo!("pane::Pane::scroll")
    }

    /// Back to the live screen.
    pub fn scroll_to_bottom(&mut self) {
        todo!("pane::Pane::scroll_to_bottom")
    }

    /// How many lines back the view is (0: the live screen).
    pub fn scrolled(&self) -> usize {
        todo!("pane::Pane::scrolled")
    }

    /// Starts a selection at a point on the screen as now shown (`col`,
    /// `row` from its top left); `block` for a rectangle.
    pub fn select_start(&mut self, col: u16, row: u16, block: bool) {
        let _ = (col, row, block);
        todo!("pane::Pane::select_start")
    }

    /// Moves the selection's end to a point on the screen as now shown.
    pub fn select_extend(&mut self, col: u16, row: u16) {
        let _ = (col, row);
        todo!("pane::Pane::select_extend")
    }

    /// Selects the word, or with `line` the line, at a point.
    pub fn select_semantic(&mut self, col: u16, row: u16, line: bool) {
        let _ = (col, row, line);
        todo!("pane::Pane::select_semantic")
    }

    pub fn select_clear(&mut self) {
        todo!("pane::Pane::select_clear")
    }

    /// The selected text, lines joined by newlines, wrapped lines joined.
    pub fn selection_text(&self) -> Option<String> {
        todo!("pane::Pane::selection_text")
    }

    /// The last `lines` lines of the screen and scrollback as text, trailing
    /// blanks trimmed: what tmux's `capture-pane -p -S -<lines>` gave.
    pub fn text(&self, lines: usize) -> String {
        let _ = lines;
        todo!("pane::Pane::text")
    }

    /// The title the program set, or "".
    pub fn title(&self) -> &str {
        todo!("pane::Pane::title")
    }

    /// The pane's own program (its shell, usually).
    pub fn pid(&self) -> Option<u32> {
        todo!("pane::Pane::pid")
    }

    /// The process group in front on the PTY (the program you're using).
    pub fn foreground_pgid(&self) -> Option<u32> {
        todo!("pane::Pane::foreground_pgid")
    }

    /// The directory of the program in front, else of the pane's own:
    /// what tmux's `pane_current_path` was.
    pub fn cwd(&self) -> Option<PathBuf> {
        todo!("pane::Pane::cwd")
    }

    /// The name of the program in front (`zsh`, `claude`, `node`): what
    /// tmux's `pane_current_command` was.
    pub fn foreground_command(&self) -> Option<String> {
        todo!("pane::Pane::foreground_command")
    }

    /// Every process in front on the PTY (the foreground process group), for
    /// finding which runs Claude (it has a session file by its pid).
    pub fn foreground_pids(&self) -> Vec<u32> {
        todo!("pane::Pane::foreground_pids")
    }

    /// The exit code once the program has ended (`Some(None)` for a signal).
    pub fn exited(&self) -> Option<Option<i32>> {
        todo!("pane::Pane::exited")
    }

    /// Marks the pane's program ended (from `PaneEvent::Exited`).
    pub fn set_exited(&mut self, code: Option<i32>) {
        let _ = code;
        todo!("pane::Pane::set_exited")
    }

    /// Ends the program: SIGHUP to its process group, as closing a terminal does.
    pub fn kill(&mut self) {
        todo!("pane::Pane::kill")
    }
}
