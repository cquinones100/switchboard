//! What the terminal a client attached from says of itself, which the
//! panes' programs are told when they ask, as tmux told them its
//! `client_cell_width` and `client_cell_height` and answered their colour
//! queries from the client's terminal.
//!
//! Its cell's size in pixels: the client sends its window's size in pixels
//! with its size in cells (TIOCGWINSZ's `ws_xpixel` and `ws_ypixel`), as it
//! attaches and on every resize, and each pane's PTY is given the cell it
//! makes (`Pane::set_cell`), so a program that reads TIOCGWINSZ or asks
//! `CSI 14 t` finds the real cell, and Notes and the image view draw a
//! picture at its own size. A terminal that doesn't say its pixels leaves
//! the stand-in cell (`pane::CELL`). While pictures are drawn as sixel
//! (src/mux/sixel), each terminal is also asked its cell with `CSI 16 t`
//! as it attaches, and its answer kept apart (`asked_cell`) as the size
//! the server draws them at: Windows Terminal, whose window's pixels may
//! not reach a program at all under WSL, answers with its sixel's virtual
//! cell. The panes keep the cell the window's pixels make, so the apps lay
//! pictures out as before.
//!
//! Its colours: the text's, the background's and the sixteen of its
//! palette, asked with OSC 10, 11 and 4 as it attaches (`queries`) and
//! read from its answers among the keys it sends (`Server::terminal_reply`),
//! so a program in a pane asking the same is told the terminal's own
//! (`Pane::set_colors`) rather than a stand-in: the diff mixes its tints
//! over the real background and follows the `system` theme by it. A colour
//! the terminal doesn't say is answered as before.
//!
//! The latest terminal to say wins, as with the `system` theme
//! (src/status/theme.rs). A program of Switchboard's own (the diff, Notes)
//! asks the same as it starts (`ask`), of the pane it runs in, which
//! answers from here, or of the terminal itself outside the workspace.

use std::io::Write;
use std::time::{Duration, Instant};

use alacritty_terminal::vte::ansi::Rgb;

use super::input::Parser;
use super::server::Server;

/// What the server keeps of the terminal attached last.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Terminal {
    /// A cell's width and height in pixels, once a terminal has said.
    pub cell: Option<(u16, u16)>,
    /// The same as a terminal answered `CSI 16 t`, asked while pictures
    /// are drawn as sixel (src/mux/sixel): in Windows Terminal its sixel's
    /// virtual cell, whatever its font.
    pub asked_cell: Option<(u16, u16)>,
    pub colors: Colors,
}

/// The colours a terminal said it draws in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Colors {
    /// The text's (OSC 10) and the background's (OSC 11).
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
    /// The sixteen of its palette (OSC 4), the ANSI colours.
    pub ansi: [Option<Rgb>; 16],
}

impl Colors {
    /// Whether the background said is light: a perceived brightness over
    /// half, as `theme::read_reply` judges it.
    pub fn light(&self) -> Option<bool> {
        let Rgb { r, g, b } = self.bg?;
        let lum = 0.299 * f64::from(r) + 0.587 * f64::from(g) + 0.114 * f64::from(b);
        Some(lum > 127.5)
    }

    /// A colour by the emulator's numbering (0 to 255 the palette, 256 the
    /// text, 257 the background, 258 the cursor, taken as the text's), when
    /// the terminal said it.
    pub fn get(&self, index: usize) -> Option<Rgb> {
        match index {
            0..=15 => self.ansi[index],
            256 | 258 => self.fg,
            257 => self.bg,
            _ => None,
        }
    }

    /// Keeps a terminal's answer to one of `queries`; whether it was one.
    pub fn take(&mut self, raw: &[u8]) -> bool {
        let Some(text) = std::str::from_utf8(raw).ok() else {
            return false;
        };
        let Some(body) = text
            .strip_prefix("\x1b]")
            .and_then(|b| b.strip_suffix('\x07').or_else(|| b.strip_suffix("\x1b\\")))
        else {
            return false;
        };
        let mut parts = body.splitn(3, ';');
        let (slot, rest) = match (parts.next(), parts.next(), parts.next()) {
            (Some("10"), Some(c), None) => (Slot::Fg, c),
            (Some("11"), Some(c), None) => (Slot::Bg, c),
            (Some("4"), Some(n), Some(c)) => match n.parse::<usize>() {
                Ok(n) if n < 16 => (Slot::Ansi(n), c),
                _ => return false,
            },
            _ => return false,
        };
        let Some(rgb) = rgb(rest) else {
            return false;
        };
        match slot {
            Slot::Fg => self.fg = Some(rgb),
            Slot::Bg => self.bg = Some(rgb),
            Slot::Ansi(n) => self.ansi[n] = Some(rgb),
        }
        true
    }
}

enum Slot {
    Fg,
    Bg,
    Ansi(usize),
}

/// A colour in xterm's `rgb:` form (or kitty's `rgba:`), each channel one
/// to four hex digits scaled by its own width, or `#rrggbb`.
fn rgb(text: &str) -> Option<Rgb> {
    if let Some(hex) = text.strip_prefix('#').filter(|h| h.len() == 6) {
        let n = u32::from_str_radix(hex, 16).ok()?;
        return Some(Rgb {
            r: (n >> 16) as u8,
            g: (n >> 8) as u8,
            b: n as u8,
        });
    }
    let channels = text
        .strip_prefix("rgb:")
        .or_else(|| text.strip_prefix("rgba:"))?;
    let mut parts = channels.split('/').map(|hex| {
        if hex.is_empty() || hex.len() > 4 {
            return None;
        }
        let value = u32::from_str_radix(hex, 16).ok()?;
        let max = (1u32 << (4 * hex.len() as u32)) - 1;
        Some(((value * 255 + max / 2) / max) as u8)
    });
    Some(Rgb {
        r: parts.next()??,
        g: parts.next()??,
        b: parts.next()??,
    })
}

/// What is asked of a terminal for its colours: the text's, the
/// background's, and each of the sixteen ANSI colours'.
pub fn queries() -> Vec<u8> {
    let mut out = b"\x1b]10;?\x1b\\".to_vec();
    out.extend_from_slice(crate::status::theme::QUERY);
    for n in 0..16 {
        out.extend_from_slice(format!("\x1b]4;{n};?\x1b\\").as_bytes());
    }
    out
}

/// A terminal's answer to `CSI 16 t`, `CSI 6 ; <height> ; <width> t`, as
/// a cell's width and height in pixels.
pub fn cell_answer(raw: &[u8]) -> Option<(u16, u16)> {
    let body = raw.strip_prefix(b"\x1b[6;")?.strip_suffix(b"t")?;
    let (h, w) = std::str::from_utf8(body).ok()?.split_once(';')?;
    let (h, w) = (h.parse::<u16>().ok()?, w.parse::<u16>().ok()?);
    (h > 0 && w > 0).then_some((w, h))
}

/// A cell's width and height in pixels from a window's size in cells and
/// in pixels; `None` when the terminal didn't say its pixels.
pub fn cell_of(cols: u16, rows: u16, xpixel: u16, ypixel: u16) -> Option<(u16, u16)> {
    (cols > 0 && rows > 0 && xpixel >= cols && ypixel >= rows)
        .then(|| (xpixel / cols, ypixel / rows))
}

/// What a terminal's answer to "what are you" (DA1, `CSI ? <class> ; <attr>
/// … c`) says of sixel: whether it lists attribute 4, which a terminal
/// that draws sixel does (Windows Terminal from 1.22, foot, WezTerm, xterm
/// with sixel, mlterm); `None` for bytes that aren't such an answer.
pub fn draws_sixel(raw: &[u8]) -> Option<bool> {
    let body = raw.strip_prefix(b"\x1b[?")?.strip_suffix(b"c")?;
    let body = std::str::from_utf8(body).ok()?;
    if !body.bytes().all(|b| b.is_ascii_digit() || b == b';') {
        return None;
    }
    Some(body.split(';').skip(1).any(|attr| attr == "4"))
}

/// Whether bytes are a terminal's answer to "what are you" (DA1,
/// `CSI ? … c`), which every terminal gives.
fn is_attributes(raw: &[u8]) -> bool {
    raw.starts_with(b"\x1b[?") && raw.ends_with(b"c")
}

/// Asks the terminal on stdin and stdout for its colours, as a program of
/// Switchboard's own starts with its terminal raw: the queries, then "what
/// are you" (DA1), which every terminal answers after the colours it knows,
/// so they are all in once its answer comes, or `wait` has passed. Gives
/// the colours and whatever else was read meanwhile (a key typed), for the
/// program to read as input. Read from the descriptor itself, so nothing is
/// left in a buffer another reader can't see.
pub fn ask(wait: Duration) -> (Colors, Vec<u8>) {
    let mut colors = Colors::default();
    let mut rest = Vec::new();
    let mut out = std::io::stdout();
    let mut asked = queries();
    asked.extend_from_slice(b"\x1b[c");
    if out.write_all(&asked).and_then(|_| out.flush()).is_err() {
        return (colors, rest);
    }
    let deadline = Instant::now() + wait;
    let mut parser = Parser::new();
    let mut buf = [0u8; 4096];
    'reading: loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let mut fds = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        let ms = libc::c_int::try_from(left.as_millis()).unwrap_or(libc::c_int::MAX);
        // SAFETY: one pollfd, owned here, for the length of the call.
        if unsafe { libc::poll(&mut fds, 1, ms.max(1)) } <= 0 {
            break;
        }
        // SAFETY: reads into a buffer owned here, at most its length.
        let n = unsafe { libc::read(libc::STDIN_FILENO, buf.as_mut_ptr().cast(), buf.len()) };
        let Ok(n) = usize::try_from(n) else {
            break;
        };
        if n == 0 {
            break;
        }
        for parsed in parser.feed(&buf[..n]) {
            if is_attributes(&parsed.raw) {
                break 'reading;
            }
            if !colors.take(&parsed.raw) {
                rest.extend_from_slice(&parsed.raw);
            }
        }
    }
    for parsed in parser.flush() {
        rest.extend_from_slice(&parsed.raw);
    }
    (colors, rest)
}

impl Server {
    /// A cell's size as a terminal said it: kept for the panes to come and
    /// given to every pane's PTY now, when it changed.
    pub fn terminal_cell(&mut self, cell: (u16, u16)) {
        if self.terminal.cell == Some(cell) {
            return;
        }
        self.terminal.cell = Some(cell);
        for slot in self.panes.values_mut() {
            slot.pane.set_cell(cell);
        }
    }

    /// A terminal's answer about one of its colours, or its cell's size in
    /// pixels, among its keys: kept, and every pane told of a colour, every
    /// client drawn again for a cell (the size pictures are drawn at,
    /// src/mux/sixel). Gives whether the bytes were such an answer, which
    /// no pane is given as typed.
    pub fn terminal_reply(&mut self, raw: &[u8]) -> bool {
        if let Some(cell) = cell_answer(raw) {
            if self.terminal.asked_cell != Some(cell) {
                self.terminal.asked_cell = Some(cell);
                for c in self.clients.values_mut() {
                    c.dirty = true;
                }
            }
            return true;
        }
        let mut colors = self.terminal.colors;
        if !colors.take(raw) {
            return false;
        }
        if colors != self.terminal.colors {
            self.terminal.colors = colors;
            let colors = self.pane_colors();
            for slot in self.panes.values_mut() {
                slot.pane.set_colors(colors);
            }
        }
        true
    }

    /// The colors a pane's program is told the terminal draws in: the
    /// terminal's own, with the text and background `terminal_colors`
    /// sets while it sets them (src/status/theme.rs).
    pub fn pane_colors(&self) -> Colors {
        let mut colors = self.terminal.colors;
        if crate::status::theme::sets_terminal_colors() {
            colors.fg = Some(Rgb {
                r: 0xc9,
                g: 0xcd,
                b: 0xd9,
            });
            colors.bg = Some(Rgb {
                r: 0x15,
                g: 0x16,
                b: 0x1e,
            });
        }
        colors
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_terminal_s_answer_says_whether_it_draws_sixel() {
        // Windows Terminal 1.22, and one from before it.
        let wt = b"\x1b[?61;4;6;7;14;21;22;23;24;28;32;42c";
        assert_eq!(draws_sixel(wt), Some(true));
        assert_eq!(draws_sixel(b"\x1b[?61;6;7;22;23;24;28;32;42c"), Some(false));
        assert_eq!(draws_sixel(b"\x1b[?62;4;22c"), Some(true), "foot");
        assert_eq!(draws_sixel(b"\x1b[?62;22c"), Some(false), "Ghostty");
        // The class is not an attribute: a VT132 is no sixel terminal.
        assert_eq!(draws_sixel(b"\x1b[?4;6c"), Some(false));
        assert_eq!(draws_sixel(b"\x1b[?1;2c"), Some(false));
        assert_eq!(draws_sixel(b"\x1b[>1;4000;0c"), None, "DA2");
        assert_eq!(draws_sixel(b"\x1b[12;40R"), None);
    }

    #[test]
    fn a_cell_is_the_window_s_pixels_over_its_cells() {
        assert_eq!(cell_of(211, 58, 3376, 2088), Some((16, 36)));
        // A terminal that doesn't say its pixels gives none.
        assert_eq!(cell_of(80, 24, 0, 0), None);
        assert_eq!(cell_of(0, 0, 100, 100), None);
    }

    #[test]
    fn a_cell_asked_for_is_read_from_the_answer() {
        assert_eq!(cell_answer(b"\x1b[6;20;10t"), Some((10, 20)));
        assert_eq!(cell_answer(b"\x1b[6;36;16t"), Some((16, 36)));
        assert_eq!(cell_answer(b"\x1b[4;480;800t"), None, "the window's");
        assert_eq!(cell_answer(b"\x1b[6;0;10t"), None);
        assert_eq!(cell_answer(b"\x1b[6;20t"), None);
        // As the parser reads it from what the terminal sent: whole, and
        // not a key.
        let mut parser = Parser::new();
        let read = parser.feed(b"\x1b[6;20;10t");
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].input, super::super::input::Input::Unknown);
        assert_eq!(cell_answer(&read[0].raw), Some((10, 20)));
    }

    #[test]
    fn a_terminal_s_colours_are_read_from_its_answers() {
        let mut c = Colors::default();
        assert!(c.take(b"\x1b]11;rgb:1d1d/1f1f/2121\x1b\\"));
        assert!(c.take(b"\x1b]10;rgb:c5/c8/c6\x07"));
        assert!(c.take(b"\x1b]4;4;rgb:8181/a2a2/bebe\x07"));
        assert!(c.take(b"\x1b]4;1;#cc6666\x07"));
        let rgb = |r, g, b| Some(Rgb { r, g, b });
        assert_eq!(c.bg, rgb(0x1d, 0x1f, 0x21));
        assert_eq!(c.fg, rgb(0xc5, 0xc8, 0xc6));
        assert_eq!(c.get(4), rgb(0x81, 0xa2, 0xbe));
        assert_eq!(c.get(1), rgb(0xcc, 0x66, 0x66));
        assert_eq!(c.get(257), c.bg);
        assert_eq!(c.get(2), None, "not said");
        // Not answers: a question, a colour past the sixteen, a key.
        for raw in [
            &b"\x1b]11;?\x07"[..],
            b"\x1b]4;200;rgb:00/00/00\x07",
            b"\x1b]12;rgb:00/00/00\x07",
            b"x",
        ] {
            assert!(!c.take(raw), "{raw:?}");
        }
    }

    #[test]
    fn the_queries_ask_for_the_text_the_background_and_sixteen_colours() {
        let asked = String::from_utf8(queries()).unwrap();
        assert!(asked.starts_with("\x1b]10;?\x1b\\\x1b]11;?\x1b\\"));
        assert_eq!(asked.matches("\x1b]4;").count(), 16);
        let mut parser = Parser::new();
        let answers = parser.feed(b"\x1b]4;3;rgb:f0/c6/74\x1b\\\x1b[?62;22c");
        assert!(Colors::default().take(&answers[0].raw));
        assert!(is_attributes(&answers[1].raw));
    }
}
