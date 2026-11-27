//! What the server shows over the workspace: the boxes tmux showed as
//! popups (`display-popup` running say.sh, tip.sh, menu.sh, confirm.sh, fzf
//! with palette.sh), drawn by the server into a client's frame. Each client
//! has a stack of overlays; the one on top takes the keys, the mouse and
//! pastes while it is up, and each is drawn over the one under it, so a tip
//! can come up over the palette where tmux, showing one popup at a time,
//! would have dropped it.
//!
//! An overlay draws itself, handles a key or a mouse event, and says what
//! came of it (`Step`): nothing to draw, something to draw again, or that it
//! is done, with an `Outcome`. Whoever opened it is told the outcome
//! through a callback (`Then`) holding the server, so a menu's choice can do
//! anything the server can (the server's `ask`, `choose` and `pick` give it
//! as a yes, an option or an item). An overlay that closes on any key (a
//! tip) can hand that key on (`Step::Pass`), and it goes to the overlay
//! under it, or to the tab, as if the box hadn't been there.
//!
//! The trait an overlay is (`Overlay`), what comes of a key given to one
//! and the boxes with nothing of the server in them (`draw`, `field`,
//! `form`, `kit`, `message`, `confirm`, `menu`, `picker`, `pointer`) are
//! `switchboard-ui`'s (crates/ui), given here at the paths they had, so an
//! app that draws its own screen (Notes) asks in the same boxes.
//!
//! - `draw`: text measured in columns, and the box everything is drawn in.
//! - `field`: the text field every box that takes typing shares.
//! - `form`: a box's parts, its focus ring, hit boxes and what is wrong.
//! - `images`: the image atoms (avatar, picture, meter, series, shadow)
//!   drawn with kitty graphics, or as text where the terminal can't.
//! - `kit`: UX 2.0's atoms (tones, marks, chips, cards, keys), which every
//!   part draws with.
//! - `layout`: the narrowest screen on which no view's panes stack
//!   (`SIDE_BY_SIDE`), `switchboard headless`'s size.
//! - `picture`: a picture shown full size over its pane, in place of
//!   Quick Look.
//! - `message`: say.sh's box in the middle, which any key closes (but `j` and `k`, which scroll one taller than the screen).
//! - `tip`: tip.sh's box in the top-right corner, once each.
//! - `confirm`: a yes/no question, Enter yes ([Y/n]).
//! - `menu`: menu.sh's and mission control's menus.
//! - `picker`: a filterable list, what fzf was, with nucleo-matcher.
//! - `palette`: M-p, every command with a description (palette.sh).
//! - `keylist`: the list of keys.
//! - `pointer`: the mouse pointer's shape, a hand over what a click
//!   presses (`OSC 22`).
//! - `alone`: a picker or a menu run in a terminal of its own, with no
//!   server, for a command typed in a shell (`wt` with no argument).
//! - `headless`: a client with no terminal, for `switchboard headless`,
//!   reading the whole screen through an emulator of its own.
//! - `serve`: the server's side: opening them, input, timers, commands.
//!
//! A box a command chosen in M-p opens remembers where it came from
//! (`Came`): Esc closing it opens the palette again with its search and
//! chosen row, and a box opened by a pick in such a box goes back to that
//! box the same way, one step at a time down to M-p. A click outside and
//! the × still close it to the pane, and a box opened any other way has
//! nowhere to go back to.
//!
//! The look follows the settings as they are when an overlay opens
//! (`Look`): clicks with menu_mouse, the mouse lighting what a click would
//! press with hover_highlight, easy mode's × with easy_keys and menu_mouse,
//! and the theme's shades of grey.

use std::collections::BTreeSet;
use std::time::Instant;

use ratatui::layout::Rect;
use ratatui::text::Span;

use crate::mux::compose::Frame;
use crate::mux::server::{ClientId, Server};

pub mod alone;
pub mod headless;
pub mod images;
pub mod keylist;
pub mod layout;
pub mod palette;
pub mod picture;
pub mod pictures;
mod serve;
pub mod tip;

pub use switchboard_ui::{
    AsAny, Canvas, Confirm, Kept, Look, Menu, MenuOption, Message, Outcome, PickItem, Picked,
    Picker, Step, Theme, confirm, dim_behind, draw, field, form, kit, menu, message, picker,
    pointer, when,
};

pub use serve::Handed;
/// A box over the workspace: an overlay (`switchboard-ui`'s) of the
/// server's frame.
pub use switchboard_ui::Overlay;
pub use tip::Tip;

/// Told how an overlay ended, with the server, so it can act on it.
pub type Then = Box<dyn FnOnce(&mut Server, ClientId, Outcome) + Send>;

/// One overlay on a client's stack, and who is told how it ends.
pub(crate) struct Layer {
    pub overlay: Box<dyn Overlay>,
    pub then: Option<Then>,
    /// For a box about the tab in front (its links, renaming it, answering
    /// its Claude, ...), the tab as its header names it ("tab 1 ·
    /// planning"): such a box alone goes beside a narrow lone pane, as a
    /// pane (`Server::about_tab`).
    pub about: Option<String>,
    /// The box Esc goes back to, and how this one opens again.
    pub came: Came,
}

/// Opens a box again: its opener run once more.
pub(crate) type Reopen = std::sync::Arc<dyn Fn(&mut Server, ClientId) + Send + Sync>;

/// A box Esc goes back to: how to open it again, what it showed as it
/// closed, and the box it goes back to in turn.
pub(crate) struct Back {
    pub reopen: Reopen,
    pub kept: Option<Kept>,
    pub before: Option<Box<Back>>,
}

/// Where a box came from: the box Esc in it goes back to, and how it opens
/// again itself, for a box opened from it to go back to it. Neither for a
/// box opened by a key, a sidebar menu or mission control.
#[derive(Default)]
pub(crate) struct Came {
    pub back: Option<Back>,
    pub reopen: Option<Reopen>,
}

/// What the next box opened on a client takes: where it came from, and
/// the search and row it had when Esc is opening it again. Kept until a
/// box takes it, input reaches the workspace (a key, a press, a paste) or
/// `COMING_FOR` passes, so a box a lookup opens a moment after the pick
/// takes it too.
pub(crate) struct Coming {
    pub came: Came,
    pub kept: Option<Kept>,
    pub at: Instant,
}

/// A client's overlays, the top one taking its input.
#[derive(Default)]
pub struct Stack {
    pub(crate) layers: Vec<Layer>,
    pub(crate) look: Look,
    /// The tab the next box opened is about, while `Server::about_tab`
    /// opens it.
    pub(crate) about_next: Option<String>,
    /// Where the next box opened comes from (`Coming`).
    pub(crate) coming: Option<Coming>,
    /// Whether the client's terminal was asked for every mouse movement
    /// (1003), for hover.
    pub(crate) motion: bool,
}

impl Stack {
    /// Whether no overlay is up.
    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// Whether every overlay up lets input through to the tab (none up
    /// included), so its pane is drawn focused, its cursor shown.
    pub fn lets_through(&self) -> bool {
        self.layers.iter().all(|l| l.overlay.lets_through())
    }

    pub fn len(&self) -> usize {
        self.layers.len()
    }

    /// The kinds of the overlays up, bottom first.
    pub fn kinds(&self) -> Vec<&'static str> {
        self.layers.iter().map(|l| l.overlay.kind()).collect()
    }

    /// The kind of the one on top.
    pub fn top(&self) -> Option<&'static str> {
        self.layers.last().map(|l| l.overlay.kind())
    }

    /// The topmost overlay of a type, to read what it shows.
    pub fn find<T: Overlay>(&self) -> Option<&T> {
        self.layers
            .iter()
            .rev()
            .find_map(|l| l.overlay.as_ref().as_any().downcast_ref::<T>())
    }

    /// The topmost overlay of a type, to change what it shows; the server
    /// draws the client again after.
    pub fn find_mut<T: Overlay>(&mut self) -> Option<&mut T> {
        self.layers
            .iter_mut()
            .rev()
            .find_map(|l| l.overlay.as_mut().as_any_mut().downcast_mut::<T>())
    }

    /// The look its overlays are drawn in.
    pub fn look(&self) -> &Look {
        &self.look
    }

    /// The soonest an overlay closes by itself.
    pub fn deadline(&self) -> Option<Instant> {
        self.layers
            .iter()
            .filter_map(|l| l.overlay.deadline())
            .min()
    }

    /// Draws the overlays over the frame, bottom first. A box about the tab that goes
    /// in the middle is drawn as a pane in `beside` instead, the empty
    /// columns beside a narrow lone pane, when it fits there and nothing
    /// under it fills the screen; every other box stays in the middle of
    /// the screen (UX 2.0 boards 111 and 112).
    pub fn draw(&mut self, frame: &mut Frame, beside: Option<Rect>) {
        if self.layers.is_empty() {
            return;
        }
        let mut look = self.look;
        let mut beside = beside;
        // Where the shadow of the topmost box so far is in the frame's images.
        let mut shadow: Option<usize> = None;
        for layer in &mut self.layers {
            if layer.overlay.covers_screen() {
                beside = None;
            }
            look.middle = beside.filter(|_| layer.about.is_some());
            layer.overlay.draw_frame(frame, &look);
            let header = layer.overlay.drawn().and_then(|d| d.header_end);
            if let (Some((from, to)), Some(tab)) = (header, &layer.about) {
                // A pane beside the lone pane: no shadow, nothing dimmed,
                // the tab it is about named after its title.
                let words = format!("   {tab}");
                let y = layer.overlay.drawn().map_or(0, |d| d.outer.y);
                let style = ratatui::style::Style::new().fg(look.theme.dim);
                draw::put_line(
                    &mut frame.buf,
                    from,
                    y,
                    to.saturating_sub(from),
                    &ratatui::text::Line::from(Span::styled(words, style)),
                );
                continue;
            }
            if let Some(outer) = layer.overlay.stands_out() {
                dim_behind(&mut frame.buf, outer, &look.theme);
                // Only the topmost box casts a shadow: one under a box over
                // it would show through that box's surface, since a shadow
                // is placed over the backgrounds.
                if let Some(at) = shadow.take() {
                    frame.images.remove(at);
                }
                let before = frame.images.len();
                images::shadow(frame, "box/shadow", outer);
                shadow = (frame.images.len() > before).then_some(before);
            }
        }
        if !self.lets_through() {
            frame.cursor = self.layers.last().and_then(|l| l.overlay.cursor());
        }
    }
}

/// What the server is told about overlays by its timers.
#[derive(Debug)]
pub enum UiEvent {
    /// Time for a client's overlays that close by themselves to look at the
    /// clock.
    Due(ClientId),
    /// A tip to show now: the palette's "next time, press" a moment after a
    /// pick, once the command has run.
    Tip {
        name: String,
        key: Option<String>,
        client: ClientId,
    },
    /// A picture read off the server's task for `switchboard picture`, to
    /// show full size over the pane, or why it couldn't be.
    Picture {
        client: ClientId,
        pane: crate::mux::PaneId,
        title: String,
        read: Result<image::RgbaImage, String>,
    },
    /// Something a task of the overlays' has to say (a file `switchboard
    /// look` couldn't open), on a client or the one used last.
    Said {
        client: Option<ClientId>,
        text: String,
    },
    /// Pictures read off the server's task for `switchboard look`, to show
    /// beside the tab's narrow lone pane.
    Look {
        client: ClientId,
        tab: crate::mux::TabId,
        shown: Vec<pictures::Shown>,
        before_after: bool,
    },
}

/// What the server keeps for the overlays across clients: the tips already
/// shown, read from their file the first time one is asked for.
#[derive(Default)]
pub struct Ui {
    pub(crate) tips_seen: Option<BTreeSet<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Modifier};

    fn frame_behind(cols: u16, rows: u16) -> Frame {
        let mut f = Frame::new(cols, rows);
        for y in 0..rows {
            for x in 0..cols {
                f.buf[(x, y)]
                    .set_symbol("x")
                    .set_fg(Color::Indexed(81))
                    .set_bg(Color::Indexed(236));
            }
        }
        f.buf[(0, 0)].set_fg(Color::Reset).set_bg(Color::Reset);
        f
    }

    #[test]
    fn a_box_that_stands_out_dims_what_is_behind_it_and_sits_on_a_surface() {
        let mut f = frame_behind(20, 8);
        let outer = Rect::new(5, 2, 8, 3);
        for y in outer.top()..outer.bottom() {
            for x in outer.left()..outer.right() {
                f.buf[(x, y)].reset();
            }
        }
        f.buf[(6, 3)].set_bg(Theme::DARK.highlight);
        dim_behind(&mut f.buf, outer, &Theme::DARK);
        images::shadow(&mut f, "box/shadow", outer);
        // Behind it: a color mixed toward the background, the terminal's
        // default faint and its default background kept.
        let behind = &f.buf[(2, 6)];
        let [r, ..] = images::rgb(Color::Indexed(81));
        assert!(matches!(behind.fg, Color::Rgb(v, ..) if v < r));
        assert!(matches!(behind.bg, Color::Rgb(..)));
        assert!(f.buf[(0, 0)].modifier.contains(Modifier::DIM));
        assert_eq!(f.buf[(0, 0)].bg, Color::Reset);
        // The box on the surface next to the base, a background of its own
        // (the chosen row's) kept.
        assert_eq!(f.buf[(5, 2)].bg, kit::Surface::Sunken.color(&Theme::DARK));
        assert_eq!(f.buf[(6, 3)].bg, Theme::DARK.highlight);
        // With no graphics, half blocks down its right and along its bottom.
        assert_eq!(f.buf[(13, 3)].symbol(), "▌");
        assert_eq!(f.buf[(7, 5)].symbol(), "▀");
        assert_eq!(f.buf[(13, 5)].symbol(), "▘");
        assert!(f.images.is_empty());
    }

    #[test]
    fn with_graphics_a_box_s_shadow_is_an_image_and_placeholders_keep_their_id() {
        let mut f = frame_behind(20, 8);
        f.graphics = Some(crate::mux::graphics::Graphics::kitty((8, 16)));
        f.buf[(1, 6)].set_symbol(&images::PLACEHOLDER.to_string());
        let id = f.buf[(1, 6)].fg;
        let outer = Rect::new(5, 2, 8, 3);
        dim_behind(&mut f.buf, outer, &Theme::DARK);
        images::shadow(&mut f, "box/shadow", outer);
        assert_eq!(f.images.len(), 1);
        assert_eq!(f.buf[(13, 3)].symbol(), "x");
        assert_eq!(f.buf[(1, 6)].fg, id);
    }

    #[test]
    fn only_the_topmost_of_stacked_boxes_casts_an_image_shadow() {
        let mut stack = Stack::default();
        for text in ["under", "a longer message over it"] {
            stack.layers.push(Layer {
                overlay: Box::new(Message::new(text, 5)),
                then: None,
                about: None,
                came: Came::default(),
            });
        }
        let mut f = frame_behind(60, 12);
        f.graphics = Some(crate::mux::graphics::Graphics::kitty((8, 16)));
        stack.draw(&mut f, None);
        assert_eq!(f.images.len(), 1);
    }
}
