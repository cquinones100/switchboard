//! A box over what a program draws: what the workspace's server shows over
//! a client's screen, and what an app that draws its whole screen (Notes)
//! shows over its own. Taken from `switchboard`'s src/ui/mod.rs with the
//! boxes themselves (`confirm`, `menu`, `message`, `picker`) when Notes
//! became a crate of its own, so the server and an app ask and answer in
//! one box.
//!
//! An overlay draws itself, handles a key or a mouse event, and says what
//! came of it (`Step`): nothing to draw, something to draw again, or that
//! it is done, with an `Outcome`. Whoever opened it is told the outcome.
//!
//! An overlay draws into cells (`draw`). The server's frame carries more
//! than cells (the images it places, src/ui/images.rs there), so one that
//! places images is handed the frame whole (`draw_frame`, a `Canvas`) and
//! takes it for the server's own (`Canvas::whole`), drawing into its cells
//! alone when it is anything else; an app's frame is the buffer itself.

use std::any::Any;
use std::time::Instant;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};

use crate::input::{Key, Mouse};
use crate::picker::Picked;
use crate::pointer::{self, BoxPointer};
use crate::{Look, Theme, draw, kit, placeholder, theme};

/// What an overlay is drawn into: cells, and whatever else its program's
/// frame carries.
pub trait Canvas {
    fn cells(&mut self) -> &mut Buffer;

    /// The frame itself, for an overlay that knows its program's.
    fn whole(&mut self) -> &mut dyn Any;
}

impl Canvas for Buffer {
    fn cells(&mut self) -> &mut Buffer {
        self
    }

    fn whole(&mut self) -> &mut dyn Any {
        self
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
    fn draw(&mut self, buf: &mut Buffer, look: &Look);

    /// Draws itself into the whole frame, for one that places images
    /// (src/ui/images.rs); the rest draw into its buffer alone.
    fn draw_frame(&mut self, frame: &mut dyn Canvas, look: &Look) {
        self.draw(frame.cells(), look);
    }

    /// Where it last drew its box, for one that stands out over the
    /// workspace as a box waiting on you does (`dim_behind`); `None` for a
    /// tip, a hover's peek or a view that fills the screen.
    fn stands_out(&self) -> Option<Rect> {
        None
    }

    /// Whether it fills the screen (mission control), so a box over it goes
    /// in the screen's middle rather than beside a narrow pane it hides;
    /// one drawn over a pane alone (the history, a picture) leaves the
    /// columns beside it empty.
    fn covers_screen(&self) -> bool {
        false
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

    /// Where it last drew its box, for the pointer's shape over it.
    fn drawn(&self) -> Option<&draw::Drawn> {
        None
    }

    /// The mouse pointer's shape over a cell (`pointer`), or `None` where
    /// what is under it decides, for one that hands a click there on (only
    /// mission control, over the sidebar). A box's own from where it drew;
    /// the arrow outside it, since a click there only closes it.
    fn pointer(&self, col: u16, row: u16, look: &Look) -> Option<pointer::Pointer> {
        let shape = self.drawn().and_then(|d| d.pointer(col, row, look.mouse));
        Some(shape.unwrap_or(pointer::Pointer::Arrow))
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
/// the text where the terminal draws one there, by kitty's protocol, and
/// half blocks where it doesn't, sixel among them; `images::shadow_cells`
/// in the notes app, which draws into a buffer of its own).
///
/// A color is dimmed by mixing it toward the background. The terminal's
/// own colors (its default and the 16 its theme names) can't be mixed by
/// their pixels, so a default or named text color is drawn faint instead,
/// and a default background stays. An image's placeholder cells keep their
/// color, which is the image's id.
pub fn dim_behind(buf: &mut Buffer, outer: Rect, theme: &Theme) {
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
        let [r, g, b] = theme::rgb(color);
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
            if cell.symbol().starts_with(placeholder::PLACEHOLDER) {
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
