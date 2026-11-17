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
//! (crates/ui/src/theme.rs). A program of Switchboard's own (the diff, Notes)
//! asks the same as it starts (`ask`), of the pane it runs in, which
//! answers from here, or of the terminal itself outside the workspace.

pub use switchboard_ui::terminal::{Colors, ask, queries};

use alacritty_terminal::vte::ansi::Rgb;

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
    /// sets while it sets them (crates/ui/src/theme.rs).
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
    use crate::mux::input::Parser;

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
}
