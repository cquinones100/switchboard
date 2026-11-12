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
//! - `draw`: text measured in columns, and the box everything is drawn in.
//! - `images`: the image atoms (avatar, picture, meter, series, shadow)
//!   drawn with kitty graphics, or as text where the terminal can't.
//! - `kit`: UX 2.0's atoms (tones, marks, chips, cards, keys), which every
//!   part draws with.
//! - `picture`: a picture shown full size over its pane, in place of
//!   Quick Look.
//! - `message`: say.sh's box in the middle, which any key closes.
//! - `tip`: tip.sh's box in the top-right corner, once each.
//! - `confirm`: a yes/no question, Enter yes ([Y/n]).
//! - `menu`: menu.sh's and mission control's menus.
//! - `picker`: a filterable list, what fzf was, with nucleo-matcher.
//! - `palette`: M-p, every command with a description (palette.sh).
//! - `keylist`: the list of keys.
//! - `when`: when something came, as a date and a clock time for a row's
//!   end (the Notifications view and window).
//! - `headless`: a client with no terminal, for `switchboard headless`,
//!   reading the whole screen through an emulator of its own.
//! - `serve`: the server's side: opening them, input, timers, commands.
//!
//! The look follows the settings as they are when an overlay opens
//! (`Look`): clicks with menu_mouse, the mouse lighting what a click would
//! press with hover_highlight, easy mode's × with easy_keys and menu_mouse,
//! and the theme's shades of grey.

use std::any::Any;
use std::collections::BTreeSet;
use std::time::Instant;

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use crate::mux::compose::Frame;
use crate::mux::input::{Key, Mouse};
use crate::mux::server::{ClientId, Server};
use crate::settings;

pub mod confirm;
pub mod draw;
pub mod headless;
pub mod images;
pub mod keylist;
pub mod kit;
pub mod menu;
pub mod message;
pub mod palette;
pub mod picker;
pub mod picture;
mod serve;
pub mod tip;
pub mod when;

pub use confirm::Confirm;
pub use menu::{Menu, MenuOption};
pub use message::Message;
pub use picker::{PickItem, Picked, Picker};
pub use tip::Tip;

/// The shades of grey the boxes are drawn in (lib/sidebar/text.js's
/// `PALETTES`), for a dark terminal or a light one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub text: Color,
    pub dim: Color,
    pub faint: Color,
    pub bright: Color,
    /// The chosen option's background.
    pub highlight: Color,
    /// A row under the mouse, fainter than the chosen one.
    pub panel: Color,
}

impl Theme {
    pub const DARK: Theme = Theme {
        text: Color::Indexed(250),
        dim: Color::Indexed(244),
        faint: Color::Indexed(240),
        bright: Color::Indexed(255),
        highlight: Color::Indexed(238),
        panel: Color::Indexed(236),
    };
    /// The dark shades as the UX 2.0 canvas's tokens give them, for a
    /// terminal that draws truecolor (src/status/theme.rs's `truecolor`).
    pub const DARK_TRUE: Theme = Theme {
        text: Color::Rgb(0xc9, 0xcd, 0xd9),
        dim: Color::Rgb(0x8d, 0x93, 0xa8),
        faint: Color::Rgb(0x56, 0x5b, 0x6e),
        bright: Color::Rgb(0xff, 0xff, 0xff),
        highlight: Color::Rgb(0x26, 0x2b, 0x40),
        panel: Color::Rgb(0x1b, 0x1e, 0x29),
    };
    pub const LIGHT: Theme = Theme {
        text: Color::Indexed(236),
        dim: Color::Indexed(242),
        faint: Color::Indexed(248),
        bright: Color::Indexed(232),
        highlight: Color::Indexed(252),
        panel: Color::Indexed(254),
    };
}

/// The settings the boxes follow, read as an overlay opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    /// A click takes an option, Yes or No, and closes a box from outside it
    /// (menu_mouse).
    pub mouse: bool,
    /// What a click would press lights under the mouse (hover_highlight,
    /// with menu_mouse).
    pub hover: bool,
    /// Easy mode's × in a box's top edge (easy_keys with menu_mouse), as
    /// render.js's `closable`.
    pub close: bool,
    /// Tab and Shift-Tab move focus as on a webpage (tab_focus). A box's
    /// own options take Tab whatever it says.
    pub tab_focus: bool,
    pub theme: Theme,
}

impl Default for Look {
    /// The settings' defaults: clicks and hover on, easy mode off, dark.
    fn default() -> Look {
        Look {
            mouse: true,
            hover: true,
            close: false,
            tab_focus: true,
            theme: Theme::DARK,
        }
    }
}

impl Look {
    /// The look the settings give now. The theme is light when the theme
    /// setting says light, or says system on a terminal whose background is
    /// light (src/status/theme.rs).
    /// The dark theme in the canvas's colors for a terminal that draws
    /// truecolor (`truecolor`), else as `read`.
    pub fn read_for(truecolor: bool) -> Look {
        let mut look = Look::read();
        if truecolor && look.theme == Theme::DARK {
            look.theme = Theme::DARK_TRUE;
        }
        look
    }

    pub fn read() -> Look {
        let all = settings::read_settings();
        let on = |key: &str| {
            all.iter()
                .find(|s| s.key == key)
                .is_none_or(|s| s.value != serde_json::Value::Bool(false))
        };
        let light = all
            .iter()
            .any(|s| s.key == "theme" && crate::status::theme::light_for(&s.value));
        let mouse = on("menu_mouse");
        Look {
            mouse,
            hover: mouse && on("hover_highlight"),
            close: mouse && on("easy_keys"),
            tab_focus: on("tab_focus"),
            theme: if light { Theme::LIGHT } else { Theme::DARK },
        }
    }

    pub fn text(&self) -> Style {
        Style::new().fg(self.theme.text)
    }

    pub fn dim(&self) -> Style {
        Style::new().fg(self.theme.dim)
    }

    pub fn faint(&self) -> Style {
        Style::new().fg(self.theme.faint)
    }

    pub fn bright(&self) -> Style {
        Style::new().fg(self.theme.bright)
    }

    pub fn bold_bright(&self) -> Style {
        self.bright().add_modifier(Modifier::BOLD)
    }

    /// The yellow bold `›` beside the chosen option.
    pub fn pointer(&self) -> Span<'static> {
        Span::styled(
            "›",
            Style::new().fg(draw::YELLOW).add_modifier(Modifier::BOLD),
        )
    }
}

/// How an overlay ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Closed with nothing chosen: Esc, a click outside it, its time ran
    /// out, or the server closed it.
    Dismissed,
    /// A question answered yes.
    Yes,
    /// A question answered no.
    No,
    /// A menu's option, by its place in the menu.
    Chose(usize),
    /// A picker's item.
    Picked(Picked),
    /// A picker's item taken with its second key (`Picker::also`), for
    /// what the list does besides what Enter does.
    PickedAlso(Picked),
    /// A list's items marked with Tab and taken together (`Picker::multi`),
    /// in the list's order.
    PickedMany(Vec<Picked>),
}

/// What came of a key or a mouse event given to an overlay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing changed on the screen (the mouse moved over nothing new).
    Same,
    /// It changed: draw the client again.
    Redraw,
    /// It is done.
    Close(Outcome),
    /// It is done, and the key or click that closed it goes on to what is
    /// under it, as if it hadn't been up (a tip's any key).
    Pass,
    /// The key goes on to the tab and it stays up: a box that takes no
    /// keys (the restart's steps, src/mux/build_row.rs).
    Through,
}

/// Told how an overlay ended, with the server, so it can act on it.
pub type Then = Box<dyn FnOnce(&mut Server, ClientId, Outcome) + Send>;

/// Lets the server find an overlay of a type on the stack, to change what it
/// shows while it is up.
pub trait AsAny: Any {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Any> AsAny for T {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// A box over the workspace.
pub trait Overlay: AsAny + Send {
    /// What kind it is ("message", "tip", "confirm", "menu", "picker",
    /// "palette", "keys"), for finding it and for the tests.
    fn kind(&self) -> &'static str;

    /// Draws itself over what is in the buffer, remembering where its parts
    /// went for the mouse.
    fn draw(&mut self, buf: &mut ratatui::buffer::Buffer, look: &Look);

    /// Draws itself into the whole frame, for one that places images
    /// (src/ui/images.rs); the rest draw into its buffer alone.
    fn draw_frame(&mut self, frame: &mut Frame, look: &Look) {
        self.draw(&mut frame.buf, look);
    }

    /// Where it last drew its box, for one that stands out over the
    /// workspace as a box waiting on you does (`dim_behind`); `None` for a
    /// tip, a hover's peek or a view that fills the screen.
    fn stands_out(&self) -> Option<Rect> {
        None
    }

    /// Opens beside a click's cell rather than where it would (`Place::Near`),
    /// for a menu or a question a click opened; the rest stay put.
    fn open_near(&mut self, _at: (u16, u16)) {}

    fn key(&mut self, key: Key, look: &Look) -> Step;

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step;

    /// Text pasted while it is up.
    fn paste(&mut self, _text: &str, _look: &Look) -> Step {
        Step::Same
    }

    /// When it closes by itself, if it does.
    fn deadline(&self) -> Option<Instant> {
        None
    }

    /// Where the terminal's cursor goes while it is on top (a field being
    /// typed in); hidden when `None`.
    fn cursor(&self) -> Option<(u16, u16)> {
        None
    }

    /// Whether it lets input through to the tab (`Step::Through`), so the
    /// pane keeps its focus and its cursor while it is up.
    fn lets_through(&self) -> bool {
        false
    }
}

/// One overlay on a client's stack, and who is told how it ends.
pub(crate) struct Layer {
    pub overlay: Box<dyn Overlay>,
    pub then: Option<Then>,
}

/// A client's overlays, the top one taking its input, and what the status
/// line says in place of a message box when message_boxes is off.
#[derive(Default)]
pub struct Stack {
    pub(crate) layers: Vec<Layer>,
    pub(crate) look: Look,
    /// The status line's message (tmux's `display-message`) and when it goes.
    pub(crate) note: Option<(String, Instant)>,
    /// Whether the client's terminal was asked for every mouse movement
    /// (1003), for hover.
    pub(crate) motion: bool,
}

impl Stack {
    /// Whether no overlay is up (the status line's message doesn't count).
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

    /// The soonest an overlay closes by itself, or the status line's
    /// message goes.
    pub fn deadline(&self) -> Option<Instant> {
        self.layers
            .iter()
            .filter_map(|l| l.overlay.deadline())
            .chain(self.note.as_ref().map(|(_, at)| *at))
            .min()
    }

    /// Draws the overlays over the frame, bottom first, and the status
    /// line's message over the status line.
    pub fn draw(&mut self, frame: &mut Frame) {
        // A message kept from before the status line went (UX 2.0 turned on
        // while one was up) has no row to go in, and is left undrawn.
        if let Some((text, _)) = &self.note
            && crate::status::lines() > 0
            && frame.area().height > 0
        {
            let area = frame.area();
            let row = Rect::new(
                0,
                area.height.saturating_sub(crate::status::lines()),
                area.width,
                1,
            );
            frame.buf.set_style(
                row,
                Style::new()
                    .bg(crate::status::theme::palette().status_bg)
                    .fg(crate::mux::compose::colors::ACCENT),
            );
            for x in row.left()..row.right() {
                frame.buf[(x, row.y)].set_symbol(" ");
            }
            draw::put_line(
                &mut frame.buf,
                0,
                row.y,
                row.width,
                &ratatui::text::Line::from(Span::raw(text.clone())),
            );
        }
        if self.layers.is_empty() {
            return;
        }
        let look = self.look;
        // Where the shadow of the topmost box so far is in the frame's images.
        let mut shadow: Option<usize> = None;
        for layer in &mut self.layers {
            layer.overlay.draw_frame(frame, &look);
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

/// How much of a color is kept behind a box that stands out, the rest
/// taken from the terminal's background.
const KEPT_BEHIND: f32 = 0.45;

/// Makes the box at `outer` stand out over what is behind it: everything
/// else in the buffer dimmed, the sidebar and the status line too, and the
/// box's own cells on the surface one step from the base (`Surface::Sunken`,
/// which stays apart from the shades of a row under the mouse and the
/// chosen one, so both still show on it) where it drew no background of
/// its own. Its shadow is drawn after (`images::shadow`, a soft image under
/// the text where the terminal draws them and half blocks where it doesn't;
/// `images::shadow_cells` in the notes app, which draws into a buffer of
/// its own).
///
/// A color is dimmed by mixing it toward the background. The terminal's
/// own colors (its default and the 16 its theme names) can't be mixed by
/// their pixels, so a default or named text color is drawn faint instead,
/// and a default background stays. An image's placeholder cells keep their
/// color, which is the image's id.
pub fn dim_behind(buf: &mut ratatui::buffer::Buffer, outer: Rect, theme: &Theme) {
    let back: [u8; 3] = if *theme == Theme::LIGHT {
        [255, 255, 255]
    } else {
        [0, 0, 0]
    };
    let themed = |color: Color| match color {
        Color::Indexed(n) => n < 16,
        Color::Rgb(..) => false,
        _ => true,
    };
    let mix = |color: Color| -> Color {
        let [r, g, b] = images::rgb(color);
        let m = |c: u8, to: u8| (c as f32 * KEPT_BEHIND + to as f32 * (1.0 - KEPT_BEHIND)) as u8;
        Color::Rgb(m(r, back[0]), m(g, back[1]), m(b, back[2]))
    };
    let area = buf.area;
    let surface = kit::Surface::Sunken.color(theme);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let inside =
                x >= outer.left() && x < outer.right() && y >= outer.top() && y < outer.bottom();
            let cell = &mut buf[(x, y)];
            if inside {
                if cell.bg == Color::Reset {
                    cell.bg = surface;
                }
                continue;
            }
            if cell.symbol().starts_with(images::PLACEHOLDER) {
                continue;
            }
            if themed(cell.fg) {
                cell.modifier.insert(Modifier::DIM);
            } else {
                cell.fg = mix(cell.fg);
            }
            if cell.bg != Color::Reset {
                cell.bg = mix(cell.bg);
            }
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
        f.graphics = Some((8, 16));
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
            });
        }
        let mut f = frame_behind(60, 12);
        f.graphics = Some((8, 16));
        stack.draw(&mut f);
        assert_eq!(f.images.len(), 1);
    }
}
