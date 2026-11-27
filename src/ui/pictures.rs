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
//! own columns. With Settings' `pictures_menu` on (UX 2.0 boards 170 and
//! 171), each picture is in the kit's card, the one under the mouse lit,
//! and a click on one chooses it and gives the pictures the keys, the
//! tab's pane greyed as any pane without them: `j`/`k`, the arrows and
//! Tab move the choice, paging to keep it in view, Enter, Space or a right
//! click ask the server for its menu (`take_asked`), and Esc gives the
//! keys back with the pictures still up; off, a click closes them. The
//! server keeps `area` on the
//! spare columns before each draw (`Server::looks_before_draw`): hidden
//! while another tab or mission control is in front, closed once the
//! columns go (a split, a wider pane, the tab closed). The pictures are
//! decoded off the server's task, as `picture.rs` decodes one, and each
//! one's key made once.
//!
//! New with the Rust port, with no tmux-version file behind it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::RgbaImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::draw::{self, BoxSpec, Drawn, Gesture, Hit};
use super::images;
use super::kit::{self, Surface, Tone};
use super::pointer::{BoxPointer, Pointer};
use super::{Look, Outcome, Overlay, Step};
use crate::mux::TabId;
use crate::mux::compose::Frame;
use crate::mux::input::{Key, KeyCode, Mods, Mouse, MouseButton, MouseKind};

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
    file: PathBuf,
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
    /// Settings' `pictures_menu` as they opened: a click chooses a picture
    /// rather than closing them.
    menu: bool,
    /// The chosen picture, while the pictures have the keys.
    chosen: Option<usize>,
    /// The picture under the mouse.
    lit: Option<usize>,
    /// The chosen picture's menu, asked for by a key or a right click, for
    /// the server to open (`take_asked`).
    asked: bool,
    /// The tab's pane, greyed while the pictures have the keys.
    pub pane: Option<Rect>,
    /// Where each picture shown was drawn last (its card), for the mouse.
    cards: Vec<(usize, Rect)>,
}

impl Pictures {
    pub fn new(
        tab: TabId,
        about: &str,
        shown: Vec<Shown>,
        before_after: bool,
        menu: bool,
    ) -> Pictures {
        let items: Vec<Item> = shown
            .into_iter()
            .map(|s| Item {
                file: s.file,
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
            menu,
            chosen: None,
            lit: None,
            asked: false,
            pane: None,
            cards: Vec::new(),
        }
    }

    /// The chosen picture's file and name, once its menu was asked for,
    /// the ask taken.
    pub fn take_asked(&mut self) -> Option<(PathBuf, String)> {
        if !std::mem::take(&mut self.asked) {
            return None;
        }
        let item = &self.items[self.chosen?];
        Some((item.file.clone(), item.title.clone()))
    }

    /// The chosen picture's file, while the pictures have the keys.
    pub fn chosen_file(&self) -> Option<&Path> {
        self.chosen.map(|i| self.items[i].file.as_path())
    }

    /// Chooses the first picture shown, giving the pictures the keys, as a
    /// click on it does (the palette's "Choose a picture shown beside the
    /// pane"); false while they are hidden or the menu is off.
    pub fn choose_first(&mut self) -> bool {
        if !self.menu || self.area.is_none() {
            return false;
        }
        self.choose(self.first);
        true
    }

    /// Gives the keys back to the tab, the pictures staying up (the sidebar
    /// or the coordinator taking them).
    pub fn release(&mut self) {
        self.chosen = None;
    }

    /// Chooses picture `i`, paging so it shows.
    fn choose(&mut self, i: usize) {
        self.chosen = Some(i);
        if i < self.first {
            self.first = i;
        } else if i >= self.first + self.fit {
            self.first = i + 1 - self.fit;
        }
    }

    /// Moves the choice a picture on or back: `wrap` past either end to
    /// the other (Tab), else staying at the end.
    fn step_choice(&mut self, on: bool, wrap: bool) -> Step {
        let Some(at) = self.chosen else {
            return Step::Same;
        };
        let last = self.items.len() - 1;
        let to = match (on, wrap) {
            (true, _) if at < last => at + 1,
            (false, _) if at > 0 => at - 1,
            (true, true) => 0,
            (false, true) => last,
            _ => return Step::Same,
        };
        self.choose(to);
        Step::Redraw
    }

    /// The picture whose card is under a cell.
    fn card_at(&self, col: u16, row: u16) -> Option<usize> {
        self.cards
            .iter()
            .find(|(_, r)| col >= r.x && col < r.right() && row >= r.y && row < r.bottom())
            .map(|(i, _)| *i)
    }

    /// The keys while the pictures have them: moving the choice, its menu,
    /// Esc giving them back, paging and closing; the rest go on to the
    /// workspace's bindings alone.
    fn chosen_key(&mut self, key: Key) -> Step {
        let plain = key.mods == Mods::NONE;
        let shift = key.mods == Mods::SHIFT;
        match key.code {
            KeyCode::Esc if plain => {
                self.chosen = None;
                Step::Redraw
            }
            KeyCode::Char('j') | KeyCode::Down if plain => self.step_choice(true, false),
            KeyCode::Char('k') | KeyCode::Up if plain => self.step_choice(false, false),
            KeyCode::Tab if plain => self.step_choice(true, true),
            KeyCode::BackTab if plain || shift => self.step_choice(false, true),
            KeyCode::Enter | KeyCode::Char(' ') if plain => {
                self.asked = true;
                Step::Redraw
            }
            // C-w would hand the pane a key of its own (`pane_nav`), so the
            // pictures keep it.
            KeyCode::Char('w') if key.mods == Mods::CTRL => Step::Same,
            // S-F10 opens what a right click on the chosen one would.
            KeyCode::F(10) if shift => {
                self.asked = true;
                Step::Redraw
            }
            // The rest go on to the workspace's bindings (M-p, M-1 to M-9,
            // the prefix and the key after it), never to the greyed pane
            // (`Server::key`).
            _ => Step::Through,
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
        // The choice stays among those shown.
        if let Some(at) = self.chosen {
            self.chosen = Some(at.clamp(first, first + self.fit - 1));
        }
        Step::Redraw
    }

    fn inside(&self, col: u16, row: u16) -> bool {
        self.area
            .is_some_and(|a| col >= a.x && col < a.right() && row >= a.y && row < a.bottom())
    }
}

/// The rows each picture takes besides its own: a name's row over each of
/// several, or a card's top and bottom edges around every one.
fn framing(count: usize, carded: bool) -> u16 {
    if carded { 2 } else { u16::from(count > 1) }
}

/// How many of `count` pictures fit in `rows` rows at `LEAST_ROWS` each,
/// framed (`framing`) and a row clear between them; one at least.
fn fits(rows: u16, count: usize, carded: bool) -> usize {
    let framed = framing(count, carded) as usize;
    (1..=count)
        .rev()
        .find(|n| n * (framed + LEAST_ROWS as usize) + (n - 1) <= rows as usize)
        .unwrap_or(1)
}

/// Where a picture goes in `body`: its place among them all, the row of
/// its name (none for a picture alone or in a card), its card (with the
/// column of the chosen one's bar left of it) and its own rows.
#[derive(Debug, PartialEq)]
struct Slot {
    index: usize,
    name: Option<u16>,
    card: Option<Rect>,
    picture: Rect,
}

/// Where the pictures from `first` go in `body`, top to bottom, sharing
/// its height, each in a card when `carded`.
fn places(body: Rect, count: usize, first: usize, carded: bool) -> Vec<Slot> {
    if count == 0 || body.width < 8 {
        return Vec::new();
    }
    let n = fits(body.height, count, carded);
    let first = first.min(count - n);
    let framed = framing(count, carded);
    let each = (body.height.saturating_sub(n as u16 - 1) / n as u16).saturating_sub(framed);
    if each == 0 {
        return Vec::new();
    }
    (0..n)
        .map(|i| {
            let top = body.y + i as u16 * (framed + each + 1);
            if carded {
                let card = Rect::new(body.x + 2, top, body.width - 2, each + 2);
                return Slot {
                    index: first + i,
                    name: None,
                    card: Some(card),
                    picture: Rect::new(card.x + 2, top + 1, card.width - 4, each),
                };
            }
            Slot {
                index: first + i,
                name: (framed == 1).then_some(top),
                card: None,
                picture: Rect::new(body.x, top + framed, body.width, each),
            }
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
        // With the keys, the tab's pane is greyed as any pane without them.
        if self.chosen.is_some()
            && let Some(pane) = self.pane.map(|p| p.intersection(frame.buf.area))
        {
            kit::unfocused(&mut frame.buf, pane, &look.theme);
        }
        // The header and the keys take a row each, with a row clear under
        // the one and over the other.
        self.fit = fits(area.height - 4, self.items.len(), self.menu);
        self.first = self.first.min(self.items.len() - self.fit);
        let page = format!(
            "{}–{}/{}",
            self.first + 1,
            self.first + self.fit,
            self.items.len()
        );
        let keys = match (self.chosen.is_some(), self.pages()) {
            (true, true) => {
                "j/k ↑↓ Tab choose  ↵ Space its menu  Esc keys back to the tab  \
                             M-PgUp/PgDn page  M-Esc close"
            }
            (true, false) => {
                "j/k ↑↓ Tab choose  ↵ Space its menu  Esc keys back to the tab  M-Esc close"
            }
            (false, true) => "M-PgUp/PgDn more  M-Esc close",
            (false, false) => "M-Esc close",
        };
        // The page goes at the keys' right end, the keys cut before it.
        let page = if self.pages() { page } else { String::new() };
        let page_room = if page.is_empty() {
            0
        } else {
            draw::width(&page) + 2
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
        let keys = draw::cut(keys, room.saturating_sub(5 + page_room));
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
        if !page.is_empty() && drawn.inner.height > 0 {
            let cols = draw::width(&page) as u16;
            draw::put_line(
                &mut frame.buf,
                drawn.inner.right().saturating_sub(cols + 1),
                drawn.inner.bottom() - 1,
                cols,
                &Line::from(Span::styled(page, look.dim())),
            );
        }
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
        self.cards.clear();
        let count = self.items.len();
        for slot in places(body, count, self.first, self.menu) {
            let i = slot.index;
            let item = &self.items[i];
            if let Some(y) = slot.name {
                let line = Line::from(Span::styled(
                    draw::cut(&item.title, body.width as usize),
                    look.dim(),
                ));
                draw::put_line(&mut frame.buf, body.x, y, body.width, &line);
            }
            if let Some(card) = slot.card {
                let chosen = self.chosen == Some(i);
                let lit = !chosen && look.hover && self.lit == Some(i);
                draw_card(
                    &mut frame.buf,
                    card,
                    &item.title,
                    &format!("{} of {count}", i + 1),
                    chosen,
                    lit,
                    look,
                );
                self.cards.push((i, card));
            }
            images::picture_shared(
                frame,
                &format!("look:{}:{i}", self.tab),
                slot.picture,
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
    /// closes them, and the paging keys while there are more than fit;
    /// while a picture is chosen they are the pictures' (`chosen_key`).
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
        if self.chosen.is_some() {
            return self.chosen_key(key);
        }
        Step::Through
    }

    /// The pane keeps its focus and its cursor but while a picture is
    /// chosen.
    fn lets_through(&self) -> bool {
        self.chosen.is_none() || self.area.is_none()
    }

    /// A paste is the tab's, but while a picture is chosen, when it goes
    /// nowhere rather than into the greyed pane.
    fn paste(&mut self, _text: &str, _look: &Look) -> Step {
        if self.chosen.is_some() && self.area.is_some() {
            return Step::Same;
        }
        Step::Through
    }

    /// Over its own columns the wheel pages and a click closes it, or,
    /// with `pictures_menu` on, chooses the picture under it (its × still
    /// closing it), a right click opening that one's menu too; the mouse
    /// anywhere else is the tab's, a click there giving the tab its keys
    /// back.
    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        if !look.mouse || !self.inside(mouse.col, mouse.row) {
            self.close_lit = false;
            let pressed = mouse.kind == MouseKind::Press
                && matches!(mouse.button, MouseButton::Left | MouseButton::Right);
            if pressed {
                self.chosen = None;
            }
            if self.lit.take().is_some() && !pressed && self.area.is_some() {
                return Step::Redraw;
            }
            return Step::Through;
        }
        let on_close = self
            .drawn
            .as_ref()
            .is_some_and(|d| d.hit(mouse.col, mouse.row) == Hit::Close);
        let under = self.card_at(mouse.col, mouse.row);
        if self.menu && mouse.kind == MouseKind::Press && mouse.button == MouseButton::Right {
            let Some(i) = under else {
                return Step::Same;
            };
            self.choose(i);
            self.asked = true;
            return Step::Redraw;
        }
        match draw::gesture(&mouse) {
            Gesture::Click if !self.menu || on_close => Step::Close(Outcome::Dismissed),
            Gesture::Click => match under {
                Some(i) => {
                    self.choose(i);
                    Step::Redraw
                }
                None => Step::Same,
            },
            Gesture::WheelDown => self.page(true),
            Gesture::WheelUp => self.page(false),
            Gesture::Move => {
                let lit = if self.menu { under } else { None };
                if on_close == self.close_lit && lit == self.lit {
                    return Step::Same;
                }
                self.close_lit = on_close;
                self.lit = lit;
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

/// A picture's card (board 170): the kit's card with its file's name on
/// the top edge and its place at the right, the chosen one's edge and name
/// in the accent with the kit's `▌` bar two columns left of it, the one
/// under the mouse on the chosen surface with its edge in the text's tone.
fn draw_card(
    buf: &mut Buffer,
    card: Rect,
    title: &str,
    place: &str,
    chosen: bool,
    lit: bool,
    look: &Look,
) {
    let theme = &look.theme;
    let name = if chosen {
        Tone::Accent.bold(theme)
    } else {
        Style::new().fg(Tone::Text.fg(theme))
    };
    let edge = kit::card_edge(chosen, lit.then(|| Tone::Text.fg(theme)), theme);
    let lines = kit::card(
        card.width as usize,
        card.height as usize,
        vec![Span::styled(title.to_string(), name)],
        vec![Span::styled(place.to_string(), look.dim())],
        &[],
        edge,
    );
    let surface = lit.then(|| Surface::Chosen.color(theme));
    for (j, spans) in lines.into_iter().enumerate() {
        let spans = spans
            .into_iter()
            .map(|s| match surface {
                Some(bg) => Span::styled(s.content, s.style.bg(bg)),
                None => s,
            })
            .collect::<Vec<_>>();
        draw::put_line(
            buf,
            card.x,
            card.y + j as u16,
            card.width,
            &Line::from(spans),
        );
    }
    if chosen {
        let bar = Line::from(Span::styled("▌", Tone::Accent.style(theme)));
        for y in card.y + 1..card.bottom().saturating_sub(1) {
            draw::put_line(buf, card.x.saturating_sub(2), y, 1, &bar);
        }
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
        let mut p = Pictures::new(
            TabId(1),
            "tab 1 · planning",
            shown(&["shot.png"]),
            false,
            false,
        );
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
        let mut p = Pictures::new(
            TabId(1),
            "tab 1",
            shown(&["one.png", "two.png"]),
            false,
            false,
        );
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
        let mut p = Pictures::new(
            TabId(1),
            "tab 1",
            shown(&["old.png", "new.png"]),
            true,
            false,
        );
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
        let mut p = Pictures::new(TabId(1), "tab 1", shown(&names), false, false);
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
        let mut p = Pictures::new(TabId(1), "tab 1", shown(&["shot.png"]), false, false);
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
        let mut p = Pictures::new(TabId(1), "tab 1", shown(&[long]), false, false);
        let frame = drawn(&mut p, Rect::new(100, 0, 40, 20));
        assert!(row(&frame, 0)[100..].contains('…'), "{}", row(&frame, 0));
        assert_eq!(frame.images.len(), 1);
        assert_eq!(frame.buf[(60, 10)].symbol(), " ", "no box in the middle");
    }

    fn press(button: crate::mux::input::MouseButton, col: u16, row: u16) -> Mouse {
        Mouse {
            kind: MouseKind::Press,
            button,
            col,
            row,
            mods: Mods::NONE,
        }
    }

    /// Three pictures with the menu on, two fitting, drawn beside a pane.
    fn chooseable() -> (Pictures, Rect) {
        let names = ["a.png", "b.png", "c.png"];
        let mut p = Pictures::new(TabId(1), "tab 1", shown(&names), false, true);
        let area = Rect::new(100, 0, 110, 30);
        p.pane = Some(Rect::new(0, 0, 99, 30));
        drawn(&mut p, area);
        (p, area)
    }

    /// Where a picture's card was drawn, by its place.
    fn card_of(p: &Pictures, i: usize) -> Rect {
        p.cards.iter().find(|(at, _)| *at == i).expect("shown").1
    }

    #[test]
    fn a_click_on_a_picture_chooses_it_and_takes_the_keys() {
        let (mut p, area) = chooseable();
        let look = Look::default();
        assert!(
            p.lets_through(),
            "nothing chosen yet: the pane has the keys"
        );
        let second = card_of(&p, 1);
        let step = p.mouse(press(MouseButton::Left, second.x + 5, second.y + 3), &look);
        assert_eq!(step, Step::Redraw, "chosen rather than closed");
        assert_eq!(p.chosen_file(), Some(Path::new("/tmp/b.png")));
        assert!(!p.lets_through(), "the pictures have the keys");
        assert_eq!(
            p.key(Key::plain(KeyCode::Char('x')), &look),
            Step::Through,
            "on to the workspace's bindings, which hold it from the pane"
        );
        let frame = drawn(&mut p, area);
        let bottom = row(&frame, 29);
        assert!(bottom.contains("Esc keys back to the tab"), "{bottom}");
        let second = card_of(&p, 1);
        assert_eq!(
            frame.buf[(second.x - 2, second.y + 2)].symbol(),
            "▌",
            "the chosen one's bar"
        );
        assert_eq!(
            frame.buf[(10, 10)].bg,
            Look::default().theme.panel,
            "the tab's pane greyed"
        );
    }

    #[test]
    fn j_moves_the_choice_on_and_pages_to_keep_it_shown() {
        let (mut p, area) = chooseable();
        let look = Look::default();
        let first = card_of(&p, 0);
        p.mouse(press(MouseButton::Left, first.x + 5, first.y + 3), &look);
        assert_eq!(p.key(Key::plain(KeyCode::Char('j')), &look), Step::Redraw);
        assert_eq!(p.chosen_file(), Some(Path::new("/tmp/b.png")));
        assert_eq!(p.key(Key::plain(KeyCode::Char('j')), &look), Step::Redraw);
        assert_eq!(p.chosen_file(), Some(Path::new("/tmp/c.png")));
        let frame = drawn(&mut p, area);
        assert!(row(&frame, 29).contains("2–3/3"), "{}", row(&frame, 29));
        assert_eq!(
            p.key(Key::plain(KeyCode::Char('j')), &look),
            Step::Same,
            "the last stays chosen"
        );
        assert_eq!(p.key(Key::plain(KeyCode::Tab), &look), Step::Redraw);
        assert_eq!(p.chosen_file(), Some(Path::new("/tmp/a.png")), "Tab wraps");
    }

    #[test]
    fn enter_or_a_right_click_asks_for_the_chosen_pictures_menu() {
        let (mut p, _) = chooseable();
        let look = Look::default();
        let first = card_of(&p, 0);
        p.mouse(press(MouseButton::Left, first.x + 5, first.y + 3), &look);
        assert_eq!(p.take_asked(), None);
        assert_eq!(p.key(Key::plain(KeyCode::Enter), &look), Step::Redraw);
        assert_eq!(
            p.take_asked(),
            Some((PathBuf::from("/tmp/a.png"), "a.png".to_string()))
        );
        assert_eq!(p.take_asked(), None, "asked once");
        assert_eq!(
            p.key(Key::new(KeyCode::F(10), Mods::SHIFT), &look),
            Step::Redraw
        );
        assert!(p.take_asked().is_some(), "S-F10 asks as a right click does");
        assert_eq!(
            p.key(Key::new(KeyCode::Char('p'), Mods::ALT), &look),
            Step::Through,
            "the workspace's keys still reach it"
        );
        let second = card_of(&p, 1);
        p.mouse(press(MouseButton::Right, second.x + 5, second.y + 3), &look);
        assert_eq!(
            p.take_asked(),
            Some((PathBuf::from("/tmp/b.png"), "b.png".to_string())),
            "a right click chooses the picture under it and asks for its menu"
        );
    }

    #[test]
    fn esc_gives_the_keys_back_with_the_pictures_still_shown() {
        let (mut p, area) = chooseable();
        let look = Look::default();
        let first = card_of(&p, 0);
        p.mouse(press(MouseButton::Left, first.x + 5, first.y + 3), &look);
        assert_eq!(
            p.paste("typed", &look),
            Step::Same,
            "not into the greyed pane"
        );
        assert_eq!(p.key(Key::plain(KeyCode::Esc), &look), Step::Redraw);
        assert_eq!(p.paste("typed", &look), Step::Through);
        assert!(p.lets_through());
        assert_eq!(p.key(Key::plain(KeyCode::Char('j')), &look), Step::Through);
        let frame = drawn(&mut p, area);
        assert_eq!(frame.images.len(), 2, "still shown");
        assert_eq!(
            p.key(Key::new(KeyCode::Esc, Mods::ALT), &look),
            Step::Close(Outcome::Dismissed)
        );
    }

    #[test]
    fn the_picture_under_the_mouse_lights() {
        let (mut p, area) = chooseable();
        let look = Look::default();
        let second = card_of(&p, 1);
        let moved = Mouse {
            kind: MouseKind::Move,
            ..press(MouseButton::Left, second.x + 5, second.y + 3)
        };
        assert_eq!(p.mouse(moved, &look), Step::Redraw);
        let frame = drawn(&mut p, area);
        let second = card_of(&p, 1);
        assert_eq!(
            frame.buf[(second.x + 1, second.y + 1)].bg,
            Surface::Chosen.color(&look.theme)
        );
    }

    #[test]
    fn choosing_the_first_from_the_palette_gives_the_pictures_the_keys() {
        let (mut p, _) = chooseable();
        assert!(p.choose_first());
        assert_eq!(p.chosen_file(), Some(Path::new("/tmp/a.png")));
        assert!(!p.lets_through());
        let mut off = Pictures::new(TabId(1), "tab 1", shown(&["a.png"]), false, false);
        drawn(&mut off, Rect::new(100, 0, 60, 30));
        assert!(!off.choose_first(), "not with the menu off");
    }
}
