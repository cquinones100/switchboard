//! The mouse pointer's shape, which says what a click would do: a hand over
//! anything a click presses, a resizing arrow over a line between panes, an
//! arrow over the rest of Switchboard's own drawing, and the terminal's text
//! beam over a pane. Ghostty and kitty take `OSC 22 ; <name> ST` with a CSS
//! cursor name; other terminals don't, and are never sent it. New with the
//! port: the tmux version left the pointer as the terminal had it.
//!
//! Each part says what is under the mouse from where it last drew (a box's
//! `Drawn`, mission control's hits, the sidebar's rows); the server sends a
//! shape only when it differs from the one its client last got
//! (`Server::sync_pointer`), and the client puts the beam back as it
//! detaches (src/mux/client.rs).

use super::draw::{Drawn, Hit};

/// A shape, by its CSS cursor name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pointer {
    /// The text beam, the terminal's own over a pane.
    Text,
    /// The arrow, over Switchboard's drawing that a click does nothing on.
    Arrow,
    /// The hand, over what a click presses.
    Hand,
    /// Over a line between panes side by side, which a drag moves.
    ColResize,
    /// Over a line between panes one above the other.
    RowResize,
}

impl Pointer {
    pub fn name(self) -> &'static str {
        match self {
            Pointer::Text => "text",
            Pointer::Arrow => "default",
            Pointer::Hand => "pointer",
            Pointer::ColResize => "ew-resize",
            Pointer::RowResize => "ns-resize",
        }
    }

    /// What sets the terminal's pointer to it.
    pub fn osc(self) -> Vec<u8> {
        format!("\x1b]22;{}\x1b\\", self.name()).into_bytes()
    }

    /// Over a line between panes, by whether it runs down.
    pub fn border(vertical: bool) -> Pointer {
        if vertical {
            Pointer::ColResize
        } else {
            Pointer::RowResize
        }
    }
}

/// Whether a terminal, by what it attached as (`TERM`, `TERM_PROGRAM` and
/// `COLORTERM`), takes `OSC 22`.
pub fn takes(term: &str) -> bool {
    let term = term.to_lowercase();
    term.contains("ghostty") || term.contains("kitty")
}

impl Drawn {
    /// The shape over a cell of a box: a hand on its close button and on a
    /// line a click means something on (while clicks are on), the arrow
    /// elsewhere inside it, and nothing outside it.
    pub fn pointer(&self, col: u16, row: u16, clicks: bool) -> Option<Pointer> {
        let hand = |on: bool| {
            if on && clicks {
                Pointer::Hand
            } else {
                Pointer::Arrow
            }
        };
        match self.hit(col, row) {
            Hit::Outside => None,
            Hit::Close => Some(hand(true)),
            Hit::Line(..) => Some(hand(self.clickable.contains(&row))),
            Hit::Inside => Some(Pointer::Arrow),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn names_are_css_cursors() {
        assert_eq!(Pointer::Hand.osc(), b"\x1b]22;pointer\x1b\\");
        assert_eq!(Pointer::border(true).name(), "ew-resize");
        assert_eq!(Pointer::border(false).name(), "ns-resize");
        assert_eq!(Pointer::Arrow.name(), "default");
    }

    #[test]
    fn only_ghostty_and_kitty_take_it() {
        assert!(takes("xterm-ghostty"));
        assert!(takes("xterm-256color ghostty truecolor"));
        assert!(takes("xterm-kitty"));
        assert!(!takes("xterm-256color Apple_Terminal"));
        assert!(!takes("screen-256color"));
    }

    #[test]
    fn a_hand_over_a_box_s_clickable_lines_and_close_button() {
        let drawn = Drawn {
            outer: Rect::new(10, 5, 20, 6),
            inner: Rect::new(11, 6, 18, 4),
            rows: vec![(6, 0), (7, 1), (8, 2)],
            clickable: vec![7, 8],
            close: Some(Rect::new(26, 5, 3, 1)),
        };
        assert_eq!(drawn.pointer(12, 7, true), Some(Pointer::Hand));
        assert_eq!(drawn.pointer(12, 6, true), Some(Pointer::Arrow));
        assert_eq!(drawn.pointer(27, 5, true), Some(Pointer::Hand));
        assert_eq!(drawn.pointer(12, 9, true), Some(Pointer::Arrow));
        assert_eq!(drawn.pointer(2, 2, true), None);
        // With clicks off in menus, nothing in a box is pressed.
        assert_eq!(drawn.pointer(12, 7, false), Some(Pointer::Arrow));
    }
}
