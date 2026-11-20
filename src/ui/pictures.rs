//! Pictures shown beside a narrow lone pane (`switchboard look <file>...`,
//! what Claude and the skills run to show you an image for feedback), in
//! place of the desktop's viewer opening over the workspace: a pane in the
//! empty columns at the pane's right, drawn as a box about the tab is
//! there (UX 2.0 board 111: a header naming the file, or "Before and
//! after" for two, and the tab, the rule at the pane's edge its only
//! border), each picture Images' picture atom (src/ui/images.rs) scaled to
//! fit, two or more stacked top to bottom with each file's name dim above
//! it, sharing the height. More than fit at `LEAST_ROWS` rows each are
//! paged a picture at a time, by the wheel over them and M-PgDn and
//! M-PgUp.
//!
//! They go under any box already up, so a menu or a question keeps its
//! input. The pane keeps its keys and its mouse: this takes only M-Esc, which
//! closes it, the paging keys while there is more, and the mouse over its
//! own columns, where a click closes it. The server keeps `area` on the
//! spare columns before each draw (`Server::looks_before_draw`): hidden
//! while another tab or mission control is in front, closed once the
//! columns go (a split, a wider pane, the tab closed). The pictures are
//! decoded off the server's task, as `picture.rs` decodes one, and each
//! one's key made once.
//!
//! New with the Rust port, with no tmux-version file behind it.

use std::path::PathBuf;
use std::sync::Arc;

use image::RgbaImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use super::draw::{self, BoxSpec, Drawn, Gesture, Hit};
use super::images;
use super::pointer::{BoxPointer, Pointer};
use super::{Look, Outcome, Overlay, Step};
use crate::mux::TabId;
use crate::mux::compose::Frame;
use crate::mux::input::{Key, KeyCode, Mods, Mouse};

/// The narrowest the spare columns are for pictures to go there, as a box
/// about the tab needs.
pub const LEAST_COLS: u16 = 40;
/// The shortest they are: the header, a row clear, a picture at its
/// least, a row clear and the keys.
pub const LEAST_HEIGHT: u16 = LEAST_ROWS + 4;
/// The fewest rows a picture is drawn in; more pictures than fit at that
/// are paged.
const LEAST_ROWS: u16 = 8;
/// The most pictures shown there at once; more open in the desktop's
/// viewer, as they did.
pub const MOST: usize = 6;

/// A picture read for showing: its file, its name and its pixels.
#[derive(Debug)]
pub struct Shown {
    pub file: PathBuf,
    pub title: String,
    pub img: RgbaImage,
}

struct Item {
    title: String,
    img: Arc<RgbaImage>,
    key: u64,
}

pub struct Pictures {
    pub tab: TabId,
    /// The spare columns beside the tab's pane, as the server last found
    /// them; none while the tab isn't the one in front.
    pub area: Option<Rect>,
    /// The tab as the header names it ("tab 1 · planning").
    about: String,
    items: Vec<Item>,
    /// Whether `look --before-after` named the two a before and after.
    before_after: bool,
    /// The first picture shown, while more are up than fit.
    first: usize,
    /// How many fit when last drawn.
    fit: usize,
    drawn: Option<Drawn>,
    close_lit: bool,
}

impl Pictures {
    pub fn new(tab: TabId, about: &str, shown: Vec<Shown>, before_after: bool) -> Pictures {
        let items: Vec<Item> = shown
            .into_iter()
            .map(|s| Item {
                title: s.title,
                key: images::picture_key(&s.img),
                img: Arc::new(s.img),
            })
            .collect();
        Pictures {
            tab,
            area: None,
            about: about.to_string(),
            fit: items.len(),
            items,
            before_after,
            first: 0,
            drawn: None,
            close_lit: false,
        }
    }

    /// What the header says: the one file's name, "Before and after" for
    /// the two `look --before-after` names so, how many otherwise.
    pub fn title(&self) -> String {
        match self.items.as_slice() {
            [one] => one.title.clone(),
            [_, _] if self.before_after => "Before and after".to_string(),
            many => format!("{} pictures", many.len()),
        }
    }

    fn pages(&self) -> bool {
        self.fit < self.items.len()
    }

    /// Moves a picture on or back, staying within them.
    fn page(&mut self, on: bool) -> Step {
        if !self.pages() {
            return Step::Same;
        }
        let last = self.items.len() - self.fit;
        let first = if on {
            (self.first + 1).min(last)
        } else {
            self.first.saturating_sub(1)
        };
        if first == self.first {
            return Step::Same;
        }
        self.first = first;
        Step::Redraw
    }

    fn inside(&self, col: u16, row: u16) -> bool {
        self.area
            .is_some_and(|a| col >= a.x && col < a.right() && row >= a.y && row < a.bottom())
    }
}

/// How many of `count` pictures fit in `rows` rows at `LEAST_ROWS` each,
/// a name's row over each of several and a row clear between them; one at
/// least.
fn fits(rows: u16, count: usize) -> usize {
    let named = usize::from(count > 1);
    (1..=count)
        .rev()
        .find(|n| n * (named + LEAST_ROWS as usize) + (n - 1) <= rows as usize)
        .unwrap_or(1)
}

/// Where the pictures from `first` go in `body`, top to bottom, sharing
/// its height: each one's place among them all, the row of its name (none
/// for a picture alone) and its own rows.
fn places(body: Rect, count: usize, first: usize) -> Vec<(usize, Option<u16>, Rect)> {
    if count == 0 || body.width == 0 {
        return Vec::new();
    }
    let n = fits(body.height, count);
    let first = first.min(count - n);
    let named = u16::from(count > 1);
    let each = (body.height.saturating_sub(n as u16 - 1) / n as u16).saturating_sub(named);
    if each == 0 {
        return Vec::new();
    }
    (0..n)
        .map(|i| {
            let top = body.y + i as u16 * (named + each + 1);
            (
                first + i,
                (named == 1).then_some(top),
                Rect::new(body.x, top + named, body.width, each),
            )
        })
        .collect()
}

impl Overlay for Pictures {
    fn kind(&self) -> &'static str {
        "pictures"
    }

    fn draw(&mut self, _buf: &mut Buffer, _look: &Look) {}

    fn draw_frame(&mut self, canvas: &mut dyn crate::ui::Canvas, look: &Look) {
        self.drawn = None;
        let Some(frame) = Frame::of(canvas) else {
            return;
        };
        let Some(area) = self.area.map(|a| a.intersection(frame.buf.area)) else {
            return;
        };
        if area.width < LEAST_COLS || area.height < LEAST_HEIGHT {
            return;
        }
        frame.links.clear_area(area);
        // The header and the keys take a row each, with a row clear under
        // the one and over the other.
        self.fit = fits(area.height - 4, self.items.len());
        self.first = self.first.min(self.items.len() - self.fit);
        let keys = if self.pages() {
            format!(
                "M-PgUp/PgDn more  M-Esc close  {}–{}/{}",
                self.first + 1,
                self.first + self.fit,
                self.items.len()
            )
        } else {
            "M-Esc close".to_string()
        };
        // Cut to the columns, so the box is a pane there whatever a file
        // is called.
        let close = if look.close {
            draw::width(draw::CLOSE)
        } else {
            0
        };
        let room = area.width as usize;
        let title = draw::cut(&self.title(), room.saturating_sub(8 + close));
        let keys = draw::cut(&keys, room.saturating_sub(5));
        let mut spec = BoxSpec::new(&title, &[]);
        spec.keys = Some(&keys);
        spec.close_lit = self.close_lit;
        let mut beside = *look;
        beside.middle = Some(area);
        let drawn = draw::draw_box(&mut frame.buf, &spec, &beside);
        let Some((from, to)) = drawn.header_end else {
            return;
        };
        draw::put_line(
            &mut frame.buf,
            from,
            drawn.outer.y,
            to.saturating_sub(from),
            &Line::from(Span::styled(format!("   {}", self.about), look.dim())),
        );
        let body = Rect {
            height: drawn.inner.height.saturating_sub(2),
            ..drawn.inner
        };
        if frame.graphics.is_none() {
            let say = Line::from(Span::styled(
                "This terminal doesn't draw pictures",
                look.dim(),
            ));
            draw::put_line(&mut frame.buf, body.x, body.y, body.width, &say);
            self.drawn = Some(drawn);
            return;
        }
        for (i, name, at) in places(body, self.items.len(), self.first) {
            let item = &self.items[i];
            if let Some(y) = name {
                let line = Line::from(Span::styled(
                    draw::cut(&item.title, body.width as usize),
                    look.dim(),
                ));
                draw::put_line(&mut frame.buf, body.x, y, body.width, &line);
            }
            images::picture_shared(
                frame,
                &format!("look:{}:{i}", self.tab),
                at,
                &item.title,
                &item.img,
                item.key,
            );
        }
        self.drawn = Some(drawn);
    }

    fn drawn(&self) -> Option<&Drawn> {
        self.drawn.as_ref()
    }

    /// Keys go on to the tab, the pictures staying up, but M-Esc, which
    /// closes them, and the paging keys while there are more than fit.
    fn key(&mut self, key: Key, _look: &Look) -> Step {
        if self.area.is_none() {
            return Step::Through;
        }
        let alt = |code| Key::new(code, Mods::ALT);
        if key == alt(KeyCode::Esc) {
            return Step::Close(Outcome::Dismissed);
        }
        if self.pages() && (key == alt(KeyCode::PageDown) || key == alt(KeyCode::PageUp)) {
            // At either end the key is still its own, never the pane's.
            return self.page(key == alt(KeyCode::PageDown));
        }
        Step::Through
    }

    fn lets_through(&self) -> bool {
        true
    }

    fn paste(&mut self, _text: &str, _look: &Look) -> Step {
        Step::Through
    }

    /// Over its own columns a click closes it and the wheel pages; the
    /// mouse anywhere else is the tab's.
    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        if !look.mouse || !self.inside(mouse.col, mouse.row) {
            self.close_lit = false;
            return Step::Through;
        }
        match draw::gesture(&mouse) {
            Gesture::Click => Step::Close(Outcome::Dismissed),
            Gesture::WheelDown => self.page(true),
            Gesture::WheelUp => self.page(false),
            Gesture::Move => {
                let lit = self
                    .drawn
                    .as_ref()
                    .is_some_and(|d| d.hit(mouse.col, mouse.row) == Hit::Close);
                if lit == self.close_lit {
                    return Step::Same;
                }
                self.close_lit = lit;
                Step::Redraw
            }
            Gesture::Other => Step::Same,
        }
    }

    /// The hand over its close button, the arrow over the rest of it, and
    /// whatever is under the mouse anywhere else.
    fn pointer(&self, col: u16, row: u16, look: &Look) -> Option<Pointer> {
        if !self.inside(col, row) {
            return None;
        }
        let shape = self
            .drawn
            .as_ref()
            .and_then(|d| d.pointer(col, row, look.mouse));
        Some(shape.unwrap_or(Pointer::Arrow))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(names: &[&str]) -> Vec<Shown> {
        names
            .iter()
            .map(|n| Shown {
                file: PathBuf::from(format!("/tmp/{n}")),
                title: n.to_string(),
                img: RgbaImage::new(40, 20),
            })
            .collect()
    }

    fn drawn(p: &mut Pictures, area: Rect) -> Frame {
        p.area = Some(area);
        let mut frame = Frame::new(area.right(), area.bottom());
        frame.graphics = Some(crate::mux::graphics::Graphics::kitty((8, 16)));
        p.draw_frame(&mut frame, &Look::default());
        frame
    }

    fn row(frame: &Frame, y: u16) -> String {
        (0..frame.buf.area.width)
            .map(|x| frame.buf[(x, y)].symbol().to_string())
            .collect()
    }

    #[test]
    fn one_picture_fills_the_columns_under_a_header_naming_it_and_the_tab() {
        let mut p = Pictures::new(TabId(1), "tab 1 · planning", shown(&["shot.png"]), false);
        let frame = drawn(&mut p, Rect::new(100, 0, 60, 30));
        let header = row(&frame, 0);
        assert!(header.contains("shot.png   tab 1 · planning"), "{header}");
        assert_eq!(frame.images.len(), 1);
        let placed = &frame.images[0];
        assert_eq!(
            (placed.cols, placed.rows),
            (58, 26),
            "a column clear each side, the header, the keys and a row clear by each"
        );
        assert!(
            row(&frame, 29).contains("M-Esc close"),
            "{}",
            row(&frame, 29)
        );
        assert_eq!(frame.buf[(99, 5)].symbol(), " ", "nothing left of it");
    }

    #[test]
    fn two_are_stacked_with_their_names_sharing_the_height() {
        let mut p = Pictures::new(TabId(1), "tab 1", shown(&["one.png", "two.png"]), false);
        let frame = drawn(&mut p, Rect::new(100, 0, 60, 30));
        assert!(
            row(&frame, 0).contains("2 pictures   tab 1"),
            "{}",
            row(&frame, 0)
        );
        assert!(row(&frame, 2).contains("one.png"), "{}", row(&frame, 2));
        assert!(row(&frame, 15).contains("two.png"), "{}", row(&frame, 15));
        let sizes: Vec<(u16, u16)> = frame.images.iter().map(|i| (i.cols, i.rows)).collect();
        assert_eq!(sizes, [(58, 11), (58, 11)]);
    }

    #[test]
    fn two_named_a_before_and_after_are_headed_so() {
        let mut p = Pictures::new(TabId(1), "tab 1", shown(&["old.png", "new.png"]), true);
        let frame = drawn(&mut p, Rect::new(100, 0, 60, 30));
        assert!(
            row(&frame, 0).contains("Before and after   tab 1"),
            "{}",
            row(&frame, 0)
        );
    }

    #[test]
    fn more_than_fit_are_paged_by_the_wheel_and_the_paging_keys() {
        let names = ["a.png", "b.png", "c.png", "d.png"];
        let mut p = Pictures::new(TabId(1), "tab 1", shown(&names), false);
        let area = Rect::new(100, 0, 60, 24);
        let frame = drawn(&mut p, area);
        assert_eq!(frame.images.len(), 2, "two fit at their least");
        assert!(row(&frame, 23).contains("1–2/4"), "{}", row(&frame, 23));
        let look = Look::default();
        let wheel = |down: bool| Mouse {
            kind: crate::mux::input::MouseKind::Press,
            button: if down {
                crate::mux::input::MouseButton::WheelDown
            } else {
                crate::mux::input::MouseButton::WheelUp
            },
            col: 120,
            row: 10,
            mods: Mods::NONE,
        };
        assert_eq!(p.mouse(wheel(true), &look), Step::Redraw);
        let frame = drawn(&mut p, area);
        assert!(row(&frame, 2).contains("b.png"), "{}", row(&frame, 2));
        assert_eq!(
            p.key(Key::new(KeyCode::PageDown, Mods::ALT), &look),
            Step::Redraw
        );
        assert_eq!(
            p.key(Key::new(KeyCode::PageDown, Mods::ALT), &look),
            Step::Same,
            "the last two are showing"
        );
        assert_eq!(p.mouse(wheel(false), &look), Step::Redraw);
        let frame = drawn(&mut p, area);
        assert!(row(&frame, 23).contains("2–3/4"), "{}", row(&frame, 23));
    }

    #[test]
    fn keys_reach_the_tab_but_m_esc_and_the_mouse_elsewhere_does_too() {
        let mut p = Pictures::new(TabId(1), "tab 1", shown(&["shot.png"]), false);
        drawn(&mut p, Rect::new(100, 0, 60, 30));
        let look = Look::default();
        assert_eq!(
            p.key(Key::new(KeyCode::Char('j'), Mods::NONE), &look),
            Step::Through
        );
        assert_eq!(
            p.key(Key::new(KeyCode::PageDown, Mods::ALT), &look),
            Step::Through,
            "nothing to page"
        );
        assert_eq!(
            p.key(Key::new(KeyCode::Esc, Mods::ALT), &look),
            Step::Close(Outcome::Dismissed)
        );
        let click = |col| Mouse {
            kind: crate::mux::input::MouseKind::Press,
            button: crate::mux::input::MouseButton::Left,
            col,
            row: 10,
            mods: Mods::NONE,
        };
        assert_eq!(p.mouse(click(20), &look), Step::Through);
        assert_eq!(p.mouse(click(120), &look), Step::Close(Outcome::Dismissed));
        assert_eq!(p.pointer(20, 10, &look), None);
        assert_eq!(p.pointer(120, 10, &look), Some(Pointer::Arrow));
        // Hidden behind another tab, every key is the tab's.
        p.area = None;
        assert_eq!(
            p.key(Key::new(KeyCode::Esc, Mods::ALT), &look),
            Step::Through
        );
    }

    #[test]
    fn a_long_name_is_cut_so_it_stays_a_pane() {
        let long = "a-very-long-name-for-a-screenshot-of-the-settings-page-at-its-widest.png";
        let mut p = Pictures::new(TabId(1), "tab 1", shown(&[long]), false);
        let frame = drawn(&mut p, Rect::new(100, 0, 40, 20));
        assert!(row(&frame, 0)[100..].contains('…'), "{}", row(&frame, 0));
        assert_eq!(frame.images.len(), 1);
        assert_eq!(frame.buf[(60, 10)].symbol(), " ", "no box in the middle");
    }
}
