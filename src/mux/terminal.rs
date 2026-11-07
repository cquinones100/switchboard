//! What the terminal a client attached from says of itself, which the
//! panes' programs are told when they ask, as tmux told them its
//! `client_cell_width` and `client_cell_height`.
//!
//! Its cell's size in pixels: the client sends its window's size in pixels
//! with its size in cells (TIOCGWINSZ's `ws_xpixel` and `ws_ypixel`), as it
//! attaches and on every resize, and each pane's PTY is given the cell it
//! makes (`Pane::set_cell`), so a program that reads TIOCGWINSZ or asks
//! `CSI 14 t` finds the real cell, and Notes and the image view draw a
//! picture at its own size. A terminal that doesn't say its pixels leaves
//! the stand-in cell (`pane::CELL`). The latest terminal to say wins, as
//! with the `system` theme (src/status/theme.rs).

use super::server::Server;

/// What the server keeps of the terminal attached last.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Terminal {
    /// A cell's width and height in pixels, once a terminal has said.
    pub cell: Option<(u16, u16)>,
}

/// A cell's width and height in pixels from a window's size in cells and
/// in pixels; `None` when the terminal didn't say its pixels.
pub fn cell_of(cols: u16, rows: u16, xpixel: u16, ypixel: u16) -> Option<(u16, u16)> {
    (cols > 0 && rows > 0 && xpixel >= cols && ypixel >= rows)
        .then(|| (xpixel / cols, ypixel / rows))
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cell_is_the_window_s_pixels_over_its_cells() {
        assert_eq!(cell_of(211, 58, 3376, 2088), Some((16, 36)));
        // A terminal that doesn't say its pixels gives none.
        assert_eq!(cell_of(80, 24, 0, 0), None);
        assert_eq!(cell_of(0, 0, 100, 100), None);
    }
}
