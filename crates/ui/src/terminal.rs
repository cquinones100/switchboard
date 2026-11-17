//! A terminal asked for the colours it draws in: the text's, the
//! background's and the sixteen of its palette (OSC 10, 11 and 4), as the
//! multiplexer asks each terminal that attaches and as a program of
//! Switchboard's own asks as it starts (`ask`), of the pane it runs in or of
//! the terminal itself outside the workspace: the diff mixes its tints over
//! the real background and follows the `system` theme by it. Taken from
//! `switchboard`'s src/mux/terminal.rs, which keeps what the server does
//! with the answers.

use std::io::Write;
use std::time::{Duration, Instant};

pub use vte::ansi::Rgb;

use crate::input::Parser;

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
    out.extend_from_slice(crate::theme::QUERY);
    for n in 0..16 {
        out.extend_from_slice(format!("\x1b]4;{n};?\x1b\\").as_bytes());
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

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
