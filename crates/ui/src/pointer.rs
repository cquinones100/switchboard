//! The mouse pointer's shape, which says what a click would do: a hand over
//! anything a click presses, a resizing arrow over a line between panes, an
//! arrow over the rest of Switchboard's own drawing, and the terminal's text
//! beam over a pane, as `OSC 22 ; <name> ST` with a CSS cursor name, sent
//! only to a terminal src/mux/graphics.rs says takes it
//! (`pointer_shapes`, which is `takes_shapes` here: Ghostty and kitty); any
//! other is never sent one, its pointer left as it is. New with the port:
//! the tmux version left the pointer as the terminal had it.
//!
//! Each part says what is under the mouse from where it last drew (a box's
//! `Drawn`, mission control's hits, the sidebar's rows); the server sends a
//! shape only when it differs from the one its client last got
//! (`Server::sync_pointer`), and the client puts the beam back as it
//! detaches (src/mux/client.rs).
//!
//! An app in a pane says the shape over its own drawing the same way, from
//! where it last drew: in the workspace it names the shape for its pane
//! through the plugin API (`Action::Pointer`, kept as the pane's
//! `@pointer` option, `PANE_OPTION`), which the server shows over that
//! pane in the beam's place; alone in a terminal it sends the shape itself,
//! to a terminal that takes it (`here`), and puts the beam back as it
//! quits. The diff is the first to.

use crate::draw::{Drawn, Hit};

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

    /// The shape a CSS cursor name is, as an app in a pane names one;
    /// `None` for any other word.
    pub fn named(name: &str) -> Option<Pointer> {
        [
            Pointer::Text,
            Pointer::Arrow,
            Pointer::Hand,
            Pointer::ColResize,
            Pointer::RowResize,
        ]
        .into_iter()
        .find(|shape| shape.name() == name)
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

/// The pane option an app's shape over its own pane is kept in, by its CSS
/// cursor name; unset, the pane has the beam.
pub const PANE_OPTION: &str = "@pointer";

/// Whether a terminal, by what it says it is (`TERM`, `TERM_PROGRAM`),
/// takes the pointer's shapes: Ghostty and kitty do. The rule
/// src/mux/graphics.rs's `pointer_shapes` gives the server, here so an app
/// alone in a terminal follows the same one.
pub fn takes_shapes(term: &str) -> bool {
    let term = term.to_lowercase();
    term.contains("ghostty") || term.contains("kitty")
}

/// Whether the terminal this program runs in takes the pointer's shapes,
/// for an app alone in it, with no server to say.
pub fn here() -> bool {
    let var = |name: &str| std::env::var(name).unwrap_or_default();
    takes_shapes(&format!("{} {}", var("TERM"), var("TERM_PROGRAM")))
}

/// The pointer over a box drawn (`draw::Drawn`, which `switchboard-ui`
/// holds, so its method is a trait's here).
pub trait BoxPointer {
    fn pointer(&self, col: u16, row: u16, clicks: bool) -> Option<Pointer>;
}

impl BoxPointer for Drawn {
    /// The shape over a cell of a box: a hand on its close button and on a
    /// line a click means something on (while clicks are on), the arrow
    /// elsewhere inside it, and nothing outside it.
    fn pointer(&self, col: u16, row: u16, clicks: bool) -> Option<Pointer> {
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
    fn a_shape_is_read_back_from_its_name_and_ghostty_and_kitty_take_them() {
        for shape in [
            Pointer::Text,
            Pointer::Arrow,
            Pointer::Hand,
            Pointer::ColResize,
            Pointer::RowResize,
        ] {
            assert_eq!(Pointer::named(shape.name()), Some(shape));
        }
        assert_eq!(Pointer::named("wait"), None);
        assert_eq!(Pointer::named(""), None);
        assert!(takes_shapes("xterm-ghostty ghostty"));
        assert!(takes_shapes("xterm-kitty"));
        assert!(!takes_shapes("xterm-256color Apple_Terminal"));
    }

    #[test]
    fn a_hand_over_a_box_s_clickable_lines_and_close_button() {
        let drawn = Drawn {
            outer: Rect::new(10, 5, 20, 6),
            inner: Rect::new(11, 6, 18, 4),
            rows: vec![(6, 0), (7, 1), (8, 2)],
            clickable: vec![7, 8],
            close: Some(Rect::new(26, 5, 3, 1)),
            header_end: None,
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
