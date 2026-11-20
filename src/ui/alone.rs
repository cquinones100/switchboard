//! A box of `switchboard-ui`'s (a picker, a menu) run in a terminal of its
//! own, with no server: what a command typed in a shell asks in, as `wt`
//! with no argument asks which work to start. It draws on the terminal's
//! alternate screen, reads keys, the mouse and pastes from `/dev/tty` with
//! the parser every box is given input by, and gives back how the box
//! ended, the screen as it was before. No tmux-version file is behind it.
//!
//! It reads the terminal only while the box is up, polling rather than
//! from a thread of its own, so nothing is left reading it once the box is
//! gone, when the program goes on to start Claude in the same terminal.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use switchboard_ui::input::{ESC_TIMEOUT, Input, Parser};
use switchboard_ui::screen::Screen;

use super::{Look, Outcome, Overlay, Picked, Picker, Step};

/// The terminal set up for a box: raw, on its alternate screen, the mouse
/// and bracketed paste on, each undone as it is dropped.
struct Term {
    tty: File,
}

impl Term {
    fn open() -> Result<Term, String> {
        let tty = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .map_err(|_| "this needs a terminal to ask in".to_string())?;
        crossterm::terminal::enable_raw_mode().map_err(|e| e.to_string())?;
        let mut term = Term { tty };
        term.write(b"\x1b[?1049h\x1b[?25l\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[?2004h");
        Ok(term)
    }

    fn write(&mut self, bytes: &[u8]) {
        let _ = self.tty.write_all(bytes);
        let _ = self.tty.flush();
    }

    /// What the terminal sent within `wait`, empty when nothing came.
    fn read(&mut self, wait: Duration) -> Option<Vec<u8>> {
        let mut fds = libc::pollfd {
            fd: self.tty.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = i32::try_from(wait.as_millis()).unwrap_or(i32::MAX);
        // SAFETY: one pollfd, owned here, for the length given.
        let ready = unsafe { libc::poll(&mut fds, 1, millis) };
        if ready < 0 {
            return None;
        }
        if ready == 0 {
            return Some(Vec::new());
        }
        let mut buf = [0u8; 4096];
        match self.tty.read(&mut buf) {
            Ok(0) | Err(_) => None,
            Ok(n) => Some(buf[..n].to_vec()),
        }
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        self.write(b"\x1b[?2004l\x1b[?1006l\x1b[?1002l\x1b[?1000l\x1b[?25h\x1b[?1049l");
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// Runs `overlay` until it closes, and says how. A terminal that closes, or
/// none to ask in, is the box dismissed, as a closed input is always no.
pub fn run(mut overlay: Box<dyn Overlay>) -> Result<Outcome, String> {
    let mut term = Term::open()?;
    let look = Look::read();
    let mut parser = Parser::new();
    let mut screen = Screen::default();
    let mut size = (0, 0);
    let mut dirty = true;
    loop {
        let now = crossterm::terminal::size().unwrap_or((80, 24));
        if now != size {
            size = now;
            screen.invalidate();
            dirty = true;
        }
        if dirty {
            let mut buf = Buffer::empty(Rect::new(0, 0, size.0, size.1));
            overlay.draw(&mut buf, &look);
            let bytes = screen.render(&buf, overlay.cursor());
            term.write(&bytes);
            dirty = false;
        }
        let wait = if parser.pending() {
            ESC_TIMEOUT
        } else {
            Duration::from_millis(100)
        };
        let Some(bytes) = term.read(wait) else {
            return Ok(Outcome::Dismissed);
        };
        let events = if bytes.is_empty() {
            if !parser.pending() {
                continue;
            }
            parser.flush()
        } else {
            parser.feed(&bytes)
        };
        for event in events {
            let step = match event.input {
                Input::Key(key) => overlay.key(key, &look),
                Input::Mouse(mouse) => overlay.mouse(mouse, &look),
                Input::Paste(text) => overlay.paste(&text, &look),
                _ => Step::Same,
            };
            match step {
                Step::Same | Step::Through => {}
                Step::Redraw => dirty = true,
                Step::Close(outcome) => return Ok(outcome),
                Step::Pass => return Ok(Outcome::Dismissed),
            }
        }
    }
}

/// A picker's item, or `None` when it was closed with nothing picked.
pub fn pick(picker: Picker) -> Result<Option<Picked>, String> {
    Ok(match run(Box::new(picker))? {
        Outcome::Picked(p) => Some(p),
        _ => None,
    })
}

/// A menu's option, by its place, or `None` when it was closed.
pub fn choose(menu: super::Menu) -> Result<Option<usize>, String> {
    Ok(match run(Box::new(menu))? {
        Outcome::Chose(at) => Some(at),
        _ => None,
    })
}
