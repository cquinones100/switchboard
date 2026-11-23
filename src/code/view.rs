//! `switchboard code <file>[:<line>]`: a code file read in a pane, which
//! the server runs in the side panel beside a narrow lone pane (in the
//! pane's place where there is no room), as the PR view and the diff open.
//! New, with no tmux-version file behind it: Neovim read a file in a split
//! of its own.
//!
//! The file is shown read-only, never written: line numbers in a gutter,
//! the diff's syntax colors by the file's name (`switchboard_highlight`,
//! worked out off the screen's thread, the text uncolored until then), a
//! line too long for the pane wrapped onto the rows under it, opening at
//! the line given, centered. The keys are the shared Vim's
//! (`switchboard_vim`), read-only: every motion and text object, counts,
//! `/` and `?` with `n` and `N`, marks, `gg`, `G`, `<count>G`, `zz`,
//! Ctrl-D, Ctrl-U, Ctrl-F and Ctrl-B, Visual mode, and yanking, which copies
//! to the clipboard (`platform::copy`); a key that would change the text
//! says Vim's E21. Ctrl-O and Ctrl-I (Tab) go back and forward through the
//! jumps, as Vim's jump list does.
//!
//! The file is read again when it changes on disk, the cursor kept on its
//! line, so it follows Claude's edits. `q` or Esc in Normal mode closes it;
//! the wheel scrolls, a click puts the cursor and a drag selects.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant, SystemTime};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use switchboard_highlight::Hl;
use switchboard_ui::input::{
    ESC_TIMEOUT, Input, Key, KeyCode, Mouse, MouseButton, MouseKind, Parser,
};
use switchboard_ui::screen::Screen;
use switchboard_vim::{Editor, Effect, Mode, Text};
use unicode_width::UnicodeWidthChar;

/// The columns a tab takes, to the next stop.
const TAB: usize = 4;
/// How often the file is looked at for a change on disk.
const LOOK_EVERY: Duration = Duration::from_millis(300);
/// The lines the wheel scrolls a notch.
const WHEEL: usize = 3;

/// What reaches the loop.
enum Msg {
    Input(Vec<u8>),
    /// The colors worked out for the text of a generation.
    Colors(u64, Option<Vec<Vec<Hl>>>),
}

/// One cell of a line laid out: the byte it shows and its symbol.
#[derive(Clone, Debug)]
struct Cell {
    byte: usize,
    symbol: String,
    width: usize,
}

pub struct Viewer {
    path: PathBuf,
    /// The path as the header shows it.
    name: String,
    pub editor: Editor,
    colors: Vec<Vec<Hl>>,
    /// The text the colors are for, counted up as it is read again.
    generation: u64,
    /// `/`, `?` or `:` and what is typed after it.
    prompt: Option<(char, String)>,
    message: Option<String>,
    /// The first line shown.
    top: usize,
    width: u16,
    height: u16,
    /// Ctrl-O's and Ctrl-I's places, and where in them the cursor is (the
    /// end while it is at none).
    jumps: Vec<(usize, usize)>,
    jump_at: usize,
    pub quit: bool,
    /// How a yank reaches the clipboard.
    copy: fn(&str),
    /// When the file was last read.
    modified: Option<SystemTime>,
    looked: Instant,
    /// Where a drag started, in the text.
    drag_from: Option<(usize, usize)>,
    /// Why the file can't be shown: said until any key closes the viewer.
    failed: Option<String>,
}

/// The file's text, a binary one said rather than shown.
fn read_text(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Couldn't read it: {e}"))?;
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return Err("A binary file, not shown".into());
    }
    // A Windows line ending is a line's end, not a character on it.
    Ok(String::from_utf8_lossy(&bytes).replace("\r\n", "\n"))
}

fn modified_of(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// A line laid out in cells, a tab to its next stop and a control
/// character as `?`, wrapped into rows `width` columns wide; an empty line
/// is one empty row.
fn layout(line: &str, width: usize) -> Vec<Vec<Cell>> {
    let width = width.max(1);
    let mut rows: Vec<Vec<Cell>> = vec![Vec::new()];
    let mut col = 0;
    let mut at = 0;
    for (byte, c) in line.char_indices() {
        let cells: Vec<(String, usize)> = if c == '\t' {
            let n = TAB - (at % TAB);
            (0..n).map(|_| (" ".to_string(), 1)).collect()
        } else if c.is_control() {
            vec![("?".into(), 1)]
        } else {
            let w = c.width().unwrap_or(0);
            if w == 0 {
                // Joined onto the cell before it, as the terminal draws it.
                if let Some(last) = rows.last_mut().and_then(|r| r.last_mut()) {
                    last.symbol.push(c);
                }
                continue;
            }
            vec![(c.to_string(), w)]
        };
        for (symbol, w) in cells {
            if col + w > width {
                rows.push(Vec::new());
                col = 0;
            }
            rows.last_mut().expect("a row").push(Cell {
                byte,
                symbol,
                width: w,
            });
            col += w;
            at += w;
        }
    }
    rows
}

impl Viewer {
    pub fn new(path: PathBuf, name: String, text: String, copy: fn(&str)) -> Viewer {
        let mut editor = Editor::new(Text::from_file(&text));
        editor.read_only = true;
        editor.clipboard = true;
        Viewer {
            modified: modified_of(&path),
            path,
            name,
            editor,
            colors: Vec::new(),
            generation: 0,
            prompt: None,
            message: None,
            top: 0,
            width: 80,
            height: 24,
            jumps: Vec::new(),
            jump_at: 0,
            quit: false,
            copy,
            looked: Instant::now(),
            drag_from: None,
            failed: None,
        }
    }

    /// The rows the text has, between the header and the status line.
    fn body_rows(&self) -> usize {
        usize::from(self.height.saturating_sub(2)).max(1)
    }

    /// The gutter's columns: the widest line number and a space.
    fn gutter(&self) -> usize {
        self.editor.doc.text.len().to_string().len() + 1
    }

    fn text_width(&self) -> usize {
        usize::from(self.width).saturating_sub(self.gutter()).max(1)
    }

    fn rows_of(&self, row: usize) -> usize {
        layout(&self.editor.doc.line(row), self.text_width()).len()
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.editor.height = self.body_rows();
        self.scroll_to_cursor();
    }

    /// The cursor on `line` (from 1), centered, as `<line>G` and `zz`.
    pub fn open_at(&mut self, line: usize) {
        let last = self.editor.doc.text.len();
        let row = line.clamp(1, last) - 1;
        self.editor.go_to(row, 0);
        self.center();
    }

    /// The cursor's line in the middle of the rows, as `zz` puts it.
    fn center(&mut self) {
        let row = self.editor.row();
        let half = self.body_rows().saturating_sub(self.rows_of(row)) / 2;
        let (mut top, mut above) = (row, 0);
        while top > 0 && above + self.rows_of(top - 1) <= half {
            top -= 1;
            above += self.rows_of(top);
        }
        self.top = top;
        self.editor.top = top;
    }

    /// The top moved so the cursor's line is shown whole: a line more than
    /// a screen above it can't be on the screen with it, so only the lines
    /// within a screen of it are laid out.
    fn scroll_to_cursor(&mut self) {
        let row = self.editor.row();
        let rows = self.body_rows();
        if row < self.top {
            self.top = row;
        }
        self.top = self.top.max(row.saturating_sub(rows));
        let mut used: usize = (self.top..=row).map(|r| self.rows_of(r)).sum();
        while self.top < row && used > rows {
            used -= self.rows_of(self.top);
            self.top += 1;
        }
        self.editor.top = self.top;
    }

    /// A key, read-only Vim's unless a prompt is open or it closes the
    /// viewer.
    pub fn key(&mut self, key: Key) {
        if self.failed.is_some() {
            self.quit = true;
            return;
        }
        self.message = None;
        if self.prompt.is_some() {
            return self.prompt_key(key);
        }
        let ctrl = key.mods.ctrl && !key.mods.alt;
        let idle = self.editor.mode == Mode::Normal && self.editor.no_keys();
        match key.code {
            KeyCode::Char('q') if idle && !ctrl => {
                self.quit = true;
                return;
            }
            KeyCode::Esc if idle => {
                self.quit = true;
                return;
            }
            KeyCode::Char('o') if idle && ctrl => return self.jump(true),
            KeyCode::Char('i') | KeyCode::Tab if idle && (ctrl || key.code == KeyCode::Tab) => {
                return self.jump(false);
            }
            _ => {}
        }
        self.editor.top = self.top;
        self.editor.begin();
        self.editor.key(key);
        self.editor.end();
        self.after_key();
    }

    fn after_key(&mut self) {
        for effect in self.editor.effects() {
            match effect {
                Effect::Copy(text) => {
                    (self.copy)(&text);
                    let lines = text.trim_end_matches('\n').lines().count().max(1);
                    self.message = Some(if lines == 1 {
                        "Copied".into()
                    } else {
                        format!("Copied {lines} lines")
                    });
                }
                Effect::Message(text) => self.message = Some(text),
                Effect::Prompt(c) => self.prompt = Some((c, String::new())),
                Effect::Quit | Effect::SaveQuit => self.quit = true,
                Effect::Jump { from } => {
                    self.jumps.truncate(self.jump_at);
                    self.jumps.retain(|j| j.0 != from.0);
                    self.jumps.push(from);
                    self.jump_at = self.jumps.len();
                }
                Effect::Save | Effect::Window(_) | Effect::WorkOnItem | Effect::AddItem => {}
            }
        }
        if std::mem::take(&mut self.editor.center) {
            self.center();
        } else {
            self.top = self.editor.top;
            self.scroll_to_cursor();
        }
    }

    /// Ctrl-O (`back`) and Ctrl-I: through the jumps.
    fn jump(&mut self, back: bool) {
        let here = (self.editor.row(), self.editor.col());
        if back {
            if self.jump_at == 0 {
                return;
            }
            if self.jump_at == self.jumps.len() {
                self.jumps.push(here);
            }
            self.jump_at -= 1;
        } else {
            if self.jump_at + 1 >= self.jumps.len() {
                return;
            }
            self.jump_at += 1;
        }
        let (row, col) = self.jumps[self.jump_at];
        let last = self.editor.doc.text.len() - 1;
        self.editor.go_to(row.min(last), col);
        self.scroll_to_cursor();
    }

    fn prompt_key(&mut self, key: Key) {
        let Some((kind, typed)) = self.prompt.as_mut() else {
            return;
        };
        let kind = *kind;
        match key.code {
            KeyCode::Esc => self.prompt = None,
            KeyCode::Char('c') if key.mods.ctrl => self.prompt = None,
            KeyCode::Backspace if typed.is_empty() => self.prompt = None,
            KeyCode::Backspace => {
                typed.pop();
            }
            KeyCode::Char('u') if key.mods.ctrl => typed.clear(),
            KeyCode::Char(c) if !key.mods.ctrl && !key.mods.alt => typed.push(c),
            KeyCode::Enter => {
                let typed = std::mem::take(typed);
                self.prompt = None;
                match kind {
                    ':' => self.editor.ex(&typed),
                    _ => self.editor.search_for(&typed, kind == '/'),
                }
                self.editor.prompt_done();
                self.after_key();
            }
            _ => {}
        }
    }

    /// Where a cell of the screen is in the text, counted from the body's
    /// top left: the row and the byte, the end of a row past its text.
    fn place_at(&self, x: u16, y: u16) -> Option<(usize, usize)> {
        let gutter = self.gutter();
        let mut y = usize::from(y.checked_sub(1)?);
        let x = usize::from(x).saturating_sub(gutter);
        let mut row = self.top;
        while row < self.editor.doc.text.len() {
            let line = self.editor.doc.line(row);
            let rows = layout(&line, self.text_width());
            if y < rows.len() {
                let mut col = 0;
                for cell in &rows[y] {
                    if x < col + cell.width {
                        return Some((row, cell.byte));
                    }
                    col += cell.width;
                }
                let end = rows
                    .get(y + 1)
                    .and_then(|r| r.first())
                    .map_or(line.len(), |c| c.byte);
                return Some((row, end));
            }
            y -= rows.len();
            row += 1;
        }
        None
    }

    pub fn mouse(&mut self, m: Mouse) {
        match (m.kind, m.button) {
            (MouseKind::Press, MouseButton::WheelDown) => self.scroll(WHEEL as isize),
            (MouseKind::Press, MouseButton::WheelUp) => self.scroll(-(WHEEL as isize)),
            (MouseKind::Press, MouseButton::Left) => {
                self.prompt = None;
                if let Some(at) = self.place_at(m.col, m.row) {
                    if matches!(self.editor.mode, Mode::Visual { .. }) {
                        self.editor.leave_visual();
                    }
                    self.editor.go_to(at.0, at.1);
                    self.drag_from = Some(at);
                    self.scroll_to_cursor();
                }
            }
            (MouseKind::Drag, MouseButton::Left) => {
                if let (Some(from), Some(to)) = (self.drag_from, self.place_at(m.col, m.row))
                    && from != to
                {
                    self.editor.select(from, to);
                    self.scroll_to_cursor();
                }
            }
            (MouseKind::Release, _) => self.drag_from = None,
            _ => {}
        }
    }

    /// The text scrolled `by` lines, the cursor kept on a line shown.
    fn scroll(&mut self, by: isize) {
        let last = self.editor.doc.text.len() - 1;
        self.top = self.top.saturating_add_signed(by).min(last);
        let row = self.editor.row();
        if row < self.top {
            self.editor.go_to(self.top, 0);
        } else {
            let mut bottom = self.top;
            let mut used = self.rows_of(bottom);
            while bottom < last && used + self.rows_of(bottom + 1) <= self.body_rows() {
                bottom += 1;
                used += self.rows_of(bottom);
            }
            if row > bottom {
                self.editor.go_to(bottom, 0);
            }
        }
        self.editor.top = self.top;
    }

    /// The file read again when it changed on disk since it was last read,
    /// the cursor kept on its line; whether it was.
    pub fn look(&mut self, now: Instant) -> bool {
        if self.failed.is_some() || now.duration_since(self.looked) < LOOK_EVERY {
            return false;
        }
        self.looked = now;
        let modified = modified_of(&self.path);
        if modified == self.modified {
            return false;
        }
        self.modified = modified;
        let Ok(text) = read_text(&self.path) else {
            return false;
        };
        self.take_text(&text)
    }

    /// New text for the file, the cursor kept on its line; whether it
    /// changed.
    pub fn take_text(&mut self, text: &str) -> bool {
        let text = Text::from_file(text);
        if text == self.editor.doc.text {
            return false;
        }
        let (row, col) = (self.editor.row(), self.editor.col());
        self.editor.change_outside(|doc| doc.text = text);
        let last = self.editor.doc.text.len() - 1;
        if matches!(self.editor.mode, Mode::Visual { .. }) {
            self.editor.leave_visual();
        }
        self.editor.go_to(row.min(last), col);
        self.generation += 1;
        self.colors.clear();
        self.scroll_to_cursor();
        true
    }

    /// The colors for the text as it is, worked out on a thread of its own
    /// and sent back.
    fn color(&self, tx: &Sender<Msg>) {
        let (generation, name, lines) = (
            self.generation,
            self.path.to_string_lossy().into_owned(),
            self.editor.doc.text.lines(),
        );
        let tx = tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Colors(
                generation,
                switchboard_highlight::highlight(&name, &lines),
            ));
        });
    }

    /// The screen into `buf`; the cursor's cell.
    pub fn draw(&self, buf: &mut Buffer) -> Option<(u16, u16)> {
        let area = buf.area;
        let dim = Style::default().fg(Color::DarkGray);
        let header = format!(" {}", self.name);
        buf.set_stringn(
            0,
            0,
            &header,
            usize::from(area.width),
            Style::default().add_modifier(Modifier::BOLD),
        );
        let tag = " read-only ";
        if usize::from(area.width) > header.len() + tag.len() {
            let x = area.width - tag.len() as u16;
            buf.set_string(x, 0, tag, dim);
        }
        if let Some(why) = &self.failed {
            buf.set_stringn(
                1,
                2,
                why,
                usize::from(area.width).saturating_sub(2),
                Style::default(),
            );
            let any = "Any key closes it";
            buf.set_stringn(1, 4, any, usize::from(area.width).saturating_sub(2), dim);
            return None;
        }
        let gutter = self.gutter();
        let width = self.text_width();
        let selection = self.editor.selection();
        let (crow, ccol) = (self.editor.row(), self.editor.col());
        let mut cursor = None;
        let mut y = 1u16;
        let bottom = area.height.saturating_sub(1);
        let mut row = self.top;
        while y < bottom && row < self.editor.doc.text.len() {
            let line = self.editor.doc.line(row);
            let number = format!("{:>w$} ", row + 1, w = gutter - 1);
            let number_style = if row == crow {
                Style::default().fg(Color::Yellow)
            } else {
                dim
            };
            buf.set_string(0, y, &number, number_style);
            let runs = self.colors.get(row);
            let span = selection.and_then(|s| s.span(row, &line));
            let rows = layout(&line, width);
            for (i, cells) in rows.iter().enumerate() {
                if y >= bottom {
                    break;
                }
                let mut x = gutter;
                for cell in cells {
                    let mut style = Style::default();
                    if let Some(hl) =
                        runs.and_then(|r| r.iter().find(|h| h.range.contains(&cell.byte)))
                    {
                        style = style.fg(hl.color);
                    }
                    if span.is_some_and(|(s, e)| cell.byte >= s && cell.byte < e) {
                        style = style.add_modifier(Modifier::REVERSED);
                    }
                    if let Some(c) = buf.cell_mut((x as u16, y)) {
                        c.set_symbol(&cell.symbol);
                        c.set_style(style);
                    }
                    if row == crow && cell.byte == ccol && cursor.is_none() {
                        cursor = Some((x as u16, y));
                    }
                    x += cell.width;
                }
                let last_row = i + 1 == rows.len();
                if row == crow && cursor.is_none() && last_row {
                    cursor = Some((x.min(usize::from(area.width) - 1) as u16, y));
                }
                y += 1;
            }
            row += 1;
        }
        while y < bottom {
            buf.set_string(0, y, "~", dim);
            y += 1;
        }
        let status = match (&self.prompt, &self.message, self.editor.mode) {
            (Some((c, typed)), _, _) => {
                let text = format!("{c}{typed}");
                buf.set_stringn(0, bottom, &text, usize::from(area.width), Style::default());
                return Some(((text.chars().count() as u16).min(area.width - 1), bottom));
            }
            (None, Some(m), _) => m.clone(),
            (None, None, Mode::Visual { line: true, .. }) => "-- VISUAL LINE --".into(),
            (None, None, Mode::Visual { block: true, .. }) => "-- VISUAL BLOCK --".into(),
            (None, None, Mode::Visual { .. }) => "-- VISUAL --".into(),
            _ => String::new(),
        };
        buf.set_stringn(
            0,
            bottom,
            &status,
            usize::from(area.width),
            Style::default(),
        );
        let place = format!(
            "{}:{}  q close ",
            crow + 1,
            self.editor.line()[..ccol.min(self.editor.line().len())]
                .chars()
                .count()
                + 1
        );
        if usize::from(area.width) > status.chars().count() + place.len() + 1 {
            buf.set_string(area.width - place.len() as u16, bottom, &place, dim);
        }
        cursor
    }
}

/// The terminal as the viewer wants it while it runs, put back however it
/// ends: raw, on the alternate screen, clicks, drags and the wheel
/// reported.
struct Term;

impl Term {
    fn enter() -> std::io::Result<Term> {
        crossterm::terminal::enable_raw_mode()?;
        let mut out = std::io::stdout();
        out.write_all(b"\x1b[?1049h\x1b[?25l\x1b[?1000h\x1b[?1002h\x1b[?1006h")?;
        out.flush()?;
        Ok(Term)
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        let mut out = std::io::stdout();
        let _ = out.write_all(b"\x1b[?1006l\x1b[?1002l\x1b[?1000l\x1b[0m\x1b[?25h\x1b[?1049l");
        let _ = out.flush();
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// Set by SIGHUP or SIGTERM: the viewer ends at the next turn of its loop.
static ENDED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    ENDED.store(true, Ordering::Relaxed);
}

/// `<file>[:<line>]`: the file and the line to open at.
pub fn parse_arg(arg: &str) -> (PathBuf, Option<usize>) {
    if let Some((file, line)) = arg.rsplit_once(':')
        && let Ok(line) = line.parse()
        && !file.is_empty()
    {
        return (PathBuf::from(file), Some(line));
    }
    (PathBuf::from(arg), None)
}

/// Runs the viewer on this terminal until it is closed.
pub fn run(path: PathBuf, line: Option<usize>) -> ExitCode {
    // A file that can't be shown says why in the viewer, until a key
    // closes it, rather than on a pane that goes as it ends.
    let (text, failed) = match read_text(&path) {
        Ok(text) => (text, None),
        Err(e) => (String::new(), Some(e)),
    };
    let name = crate::paths::tilde(&path);
    let copy: fn(&str) = |text| {
        let _ = crate::platform::copy(text);
    };
    let mut viewer = Viewer::new(path, name, text, copy);
    viewer.failed = failed;
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    viewer.resize(cols, rows);
    if let Some(line) = line {
        viewer.open_at(line);
    }
    for signal in [libc::SIGHUP, libc::SIGTERM] {
        // SAFETY: a handler that only stores to an atomic.
        unsafe {
            libc::signal(signal, on_signal as *const () as libc::sighandler_t);
        }
    }
    let term = match Term::enter() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("switchboard code: can't take the terminal: {e}");
            return ExitCode::FAILURE;
        }
    };
    let (tx, rx): (Sender<Msg>, Receiver<Msg>) = channel();
    viewer.color(&tx);
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin();
            let mut buf = [0u8; 4096];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(Msg::Input(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }
    let mut parser = Parser::new();
    let mut last_input = Instant::now();
    let mut screen = Screen::default();
    let mut size = (cols, rows);
    let mut dirty = true;
    while !viewer.quit && !ENDED.load(Ordering::Relaxed) {
        let mut inputs = Vec::new();
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(msg) => {
                let mut handle = |msg: Msg, inputs: &mut Vec<Input>| match msg {
                    Msg::Input(bytes) => {
                        last_input = Instant::now();
                        inputs.extend(parser.feed(&bytes).into_iter().map(|p| p.input));
                    }
                    Msg::Colors(generation, colors) => {
                        if generation == viewer.generation {
                            viewer.colors = colors.unwrap_or_default();
                        }
                    }
                };
                handle(msg, &mut inputs);
                while let Ok(msg) = rx.try_recv() {
                    handle(msg, &mut inputs);
                }
                dirty = true;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        let now = Instant::now();
        if parser.pending() && now.duration_since(last_input) >= ESC_TIMEOUT {
            inputs.extend(parser.flush().into_iter().map(|p| p.input));
            dirty = true;
        }
        for input in inputs {
            match input {
                Input::Key(key) => viewer.key(key),
                Input::Mouse(m) => viewer.mouse(m),
                _ => {}
            }
        }
        if viewer.look(now) {
            viewer.color(&tx);
            dirty = true;
        }
        let now_size = crossterm::terminal::size().unwrap_or(size);
        if now_size != size {
            size = now_size;
            viewer.resize(size.0, size.1);
            screen.invalidate();
            dirty = true;
        }
        if dirty && !viewer.quit {
            let mut buf = Buffer::empty(Rect::new(0, 0, size.0, size.1));
            let cursor = viewer.draw(&mut buf);
            let bytes =
                screen.render_shaped(&buf, cursor, crossterm::cursor::SetCursorStyle::SteadyBlock);
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(&bytes);
            let _ = out.flush();
            dirty = false;
        }
    }
    drop(term);
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use switchboard_ui::input::Mods;

    static COPIED: Mutex<Vec<String>> = Mutex::new(Vec::new());

    fn copied() -> Vec<String> {
        std::mem::take(&mut *COPIED.lock().unwrap())
    }

    const CODE: &str = "fn main() {\n    let total = add(1, 2);\n    println!(\"{total}\");\n}\n\nfn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n";

    fn viewer() -> Viewer {
        let copy: fn(&str) = |t| COPIED.lock().unwrap().push(t.to_string());
        let mut v = Viewer::new(
            PathBuf::from("/nowhere/main.rs"),
            "main.rs".into(),
            CODE.into(),
            copy,
        );
        v.resize(40, 12);
        v
    }

    fn press(v: &mut Viewer, keys: &str) {
        for c in keys.chars() {
            let key = match c {
                '\n' => Key::new(KeyCode::Enter, Mods::NONE),
                '\x1b' => Key::new(KeyCode::Esc, Mods::NONE),
                c => Key::new(KeyCode::Char(c), Mods::NONE),
            };
            v.key(key);
        }
    }

    #[test]
    fn it_moves_searches_and_copies_as_neovim_and_changes_nothing() {
        let mut v = viewer();
        press(&mut v, "w");
        assert_eq!((v.editor.row(), v.editor.col()), (0, 3));
        press(&mut v, "/fn\n");
        assert_eq!(v.editor.row(), 5, "/fn<Enter> to the next fn");
        press(&mut v, "n");
        assert_eq!(v.editor.row(), 0, "n wraps to the first");
        press(&mut v, "5j");
        assert_eq!(v.editor.row(), 5);
        press(&mut v, "G");
        assert_eq!(v.editor.row(), 7);
        press(&mut v, "gg");
        assert_eq!(v.editor.row(), 0);
        copied();
        press(&mut v, "Vjy");
        assert_eq!(copied(), ["fn main() {\n    let total = add(1, 2);\n"]);
        for keys in ["i", "x", "dd"] {
            press(&mut v, keys);
            assert_eq!(v.editor.mode, Mode::Normal, "{keys} left it in Normal mode");
            assert_eq!(
                v.editor.doc.text.file_text(),
                CODE,
                "{keys} changed nothing"
            );
            assert_eq!(
                v.message.as_deref(),
                Some(switchboard_vim::editor::READ_ONLY)
            );
        }
        assert!(copied().is_empty(), "nothing refused reached the clipboard");
    }

    #[test]
    fn it_opens_centered_on_its_line_and_q_or_esc_closes_it() {
        let lines: String = (1..=100).map(|n| format!("line {n}\n")).collect();
        let mut v = Viewer::new(
            PathBuf::from("/nowhere/a.txt"),
            "a.txt".into(),
            lines,
            |_| {},
        );
        v.resize(40, 12);
        v.open_at(50);
        assert_eq!(v.editor.row(), 49);
        assert_eq!(v.top, 49 - 4, "the line in the middle of the ten rows");
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 12));
        let cursor = v.draw(&mut buf);
        assert_eq!(cursor, Some((4, 5)), "on the line's first character");
        press(&mut v, "q");
        assert!(v.quit);
        let mut v = viewer();
        press(&mut v, "\x1b");
        assert!(v.quit, "Esc in Normal mode closes it");
    }

    #[test]
    fn a_change_on_disk_keeps_the_cursor_on_its_line() {
        let mut v = viewer();
        press(&mut v, "6G");
        assert_eq!(v.editor.row(), 5);
        let edited = CODE.replace("a + b", "a + b + 0");
        assert!(v.take_text(&edited));
        assert_eq!(v.editor.row(), 5, "the same line");
        assert!(v.editor.doc.text.file_text().contains("a + b + 0"));
    }

    #[test]
    fn a_long_line_wraps_and_a_click_puts_the_cursor() {
        let rows = layout(&"x".repeat(25), 10);
        assert_eq!(rows.iter().map(Vec::len).collect::<Vec<_>>(), [10, 10, 5]);
        let tabbed = layout("\tab", 10);
        assert_eq!(tabbed[0].len(), 6, "a tab to its stop");
        let mut v = viewer();
        // The gutter is two columns wide; row 2 of the screen is line 2.
        v.mouse(Mouse {
            kind: MouseKind::Press,
            button: MouseButton::Left,
            col: 6,
            row: 2,
            mods: Mods::NONE,
        });
        assert_eq!((v.editor.row(), v.editor.col()), (1, 4));
    }

    #[test]
    fn a_file_that_cant_be_shown_says_why_until_a_key_closes_it() {
        let mut v = viewer();
        v.failed = Some("A binary file, not shown".into());
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 12));
        v.draw(&mut buf);
        let row: String = (0..40).map(|x| buf[(x, 2)].symbol().to_string()).collect();
        assert!(row.contains("A binary file"), "{row}");
        press(&mut v, "j");
        assert!(v.quit);
    }

    #[test]
    fn a_far_jump_scrolls_straight_to_its_line() {
        let lines: String = (1..=50_000).map(|n| format!("line {n}\n")).collect();
        let mut v = Viewer::new(
            PathBuf::from("/nowhere/a.txt"),
            "a.txt".into(),
            lines,
            |_| {},
        );
        v.resize(40, 12);
        v.open_at(40_000);
        press(&mut v, "gg");
        let started = Instant::now();
        v.key(Key::new(KeyCode::Char('o'), Mods::CTRL));
        assert_eq!(v.editor.row(), 39_999, "back where it jumped from");
        assert!(v.top + 10 > 39_999 && v.top <= 39_999);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn its_argument_is_a_file_and_a_line() {
        assert_eq!(
            parse_arg("src/main.rs:12"),
            (PathBuf::from("src/main.rs"), Some(12))
        );
        assert_eq!(
            parse_arg("src/main.rs"),
            (PathBuf::from("src/main.rs"), None)
        );
    }
}
