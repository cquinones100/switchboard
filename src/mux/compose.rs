//! Drawing a client's screen. Each frame is built whole in a cell buffer (the
//! tab's panes, the lines between them, its header, anything over them),
//! then only the cells that changed since the client's last frame are sent,
//! inside a synchronized update so the terminal never shows half a frame.
//!
//! The look is tmux.conf's: lines between panes in colour238, the focused
//! pane's in the current tab's color (update-colors.sh's
//! `pane-active-border-style`), panes without the focus on a colour236
//! background (its `window-style`) and the focused one on the terminal's
//! own; theme.sh's light shades in place of the dark ones with the theme
//! light (crates/ui/src/theme.rs).
//!
//! A cell a pane's program made part of a hyperlink (OSC 8) carries it in
//! the frame's `links`, beside the cells, since a ratatui cell can't, and
//! is sent inside the same OSC 8, so a link in a pane is one in your
//! terminal too, as tmux passed them on. Anything drawn over such a cell
//! (a box, mission control) takes its link away (`Frame::cover_links`), so
//! no box is clickable as the link under it.
//!
//! Pictures ride beside the cells too, in the form src/mux/graphics.rs
//! chose for the client (`Frame::graphics`): the server's images placed
//! by kitty's protocol are sent by the client's `ImageCache` before the
//! cells, and those drawn as sixel (src/mux/sixel, the panes' and the
//! server's own alike) are `Frame::pictures`, each written after the
//! frame's text at its top left: again whenever any of its cells is sent,
//! since text written over a sixel takes it away, and when it is new or
//! moved. The cells of one shown before and not now are erased (`ECH`,
//! which a terminal's picture goes with) and sent again, so nothing of it
//! stays behind. No picture reaches the last row, where a sixel would
//! scroll the screen. A sixel's bytes come from the pipeline alone: a
//! `Placed` can be made only in src/mux.

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use crossterm::{cursor, queue};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use unicode_width::UnicodeWidthStr;

use super::layout::Placement;

/// tmux.conf's colours, by the 256-colour palette's numbers: the dark
/// theme's (crates/ui/src/theme.rs has both).
pub mod colors {
    use ratatui::style::Color;
    pub const ACCENT: Color = Color::Indexed(81);
    pub const BORDER: Color = Color::Indexed(238);
    pub const INACTIVE_BG: Color = Color::Indexed(236);
}

/// The hyperlinks on a frame's cells, by position (sparse, since most
/// cells have none), each the address a pane's program gave it with OSC 8.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Links(HashMap<(u16, u16), Arc<str>>);

impl Links {
    pub fn set(&mut self, x: u16, y: u16, uri: Arc<str>) {
        self.0.insert((x, y), uri);
    }

    pub fn get(&self, x: u16, y: u16) -> Option<&Arc<str>> {
        self.0.get(&(x, y))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// No links in `area` (something drawn over it).
    pub fn clear_area(&mut self, area: Rect) {
        if !self.0.is_empty() {
            self.0.retain(|&(x, y), _| {
                x < area.left() || x >= area.right() || y < area.top() || y >= area.bottom()
            });
        }
    }

    /// An address as it may go to the terminal inside OSC 8: printable
    /// ASCII alone, so nothing a program wrote can end the sequence and
    /// write escapes of its own, and not so long a terminal drops it.
    pub fn safe(uri: &str) -> bool {
        !uri.is_empty() && uri.len() <= 2048 && uri.bytes().all(|b| (0x21..0x7f).contains(&b))
    }
}

/// A symbol nothing draws, put in each linked cell while something is drawn
/// over the frame, so a cell still holding it afterwards was left alone.
const UNDER: &str = "\u{10fffe}";

/// A symbol no cell holds, put in the last frame's cells that must be sent
/// again whatever this one holds.
const UNSENT: &str = "\u{10fffd}";

/// The most a frame's pictures add to it, well inside the largest frame a
/// client reads (`proto::MAX_FRAME`).
const PICTURES_MOST: usize = 40 << 20;

/// A picture drawn over cells as sixel: where, which piece of which
/// picture (`key`, to tell one from another), and the string that draws
/// it from its top left, which only the pipeline makes (src/mux/sixel).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placed {
    pub(in crate::mux) area: Rect,
    pub(in crate::mux) key: u64,
    pub(in crate::mux) sixel: Arc<[u8]>,
}

/// One client's frame: what to show, and where the cursor goes (hidden when
/// `None`).
pub struct Frame {
    pub buf: Buffer,
    pub cursor: Option<(u16, u16)>,
    /// The cursor's shape, as the focused pane's program asked (a bar in
    /// Neovim's Insert mode).
    pub cursor_style: Option<cursor::SetCursorStyle>,
    /// The cells that are part of a hyperlink.
    pub links: Links,
    /// How the server's images (src/ui/images.rs) reach the client's
    /// terminal, and the cell's size in the pixels they are rastered at
    /// (src/mux/graphics.rs); `None` where they are text.
    pub graphics: Option<super::graphics::Graphics>,
    /// The images the frame places.
    pub images: Vec<crate::ui::images::Placed>,
    /// The pictures drawn over its cells as sixel.
    pub pictures: Vec<Placed>,
}

impl crate::ui::Canvas for Frame {
    fn cells(&mut self) -> &mut Buffer {
        &mut self.buf
    }

    fn whole(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl Frame {
    /// The server's frame an overlay was handed to draw into whole
    /// (`Overlay::draw_frame`); `None` for any other program's.
    pub fn of(canvas: &mut dyn crate::ui::Canvas) -> Option<&mut Frame> {
        canvas.whole().downcast_mut()
    }
}

impl Frame {
    pub fn new(cols: u16, rows: u16) -> Frame {
        Frame {
            buf: Buffer::empty(Rect::new(0, 0, cols, rows)),
            cursor: None,
            cursor_style: None,
            links: Links::default(),
            graphics: None,
            images: Vec::new(),
            pictures: Vec::new(),
        }
    }

    pub fn area(&self) -> Rect {
        self.buf.area
    }

    /// Draws something over the frame (`draw`), each linked cell it draws
    /// on losing its link; a style alone changed keeps it, since the text
    /// is still there.
    pub fn cover_links<R>(&mut self, draw: impl FnOnce(&mut Frame) -> R) -> R {
        if self.links.is_empty() {
            return draw(self);
        }
        let mut under: Vec<((u16, u16), String)> = Vec::with_capacity(self.links.0.len());
        for &(x, y) in self.links.0.keys() {
            if let Some(cell) = self.buf.cell_mut((x, y)) {
                under.push(((x, y), cell.symbol().to_string()));
                cell.set_symbol(UNDER);
            }
        }
        let drawn = draw(self);
        for ((x, y), symbol) in under {
            match self.buf.cell_mut((x, y)) {
                Some(cell) if cell.symbol() == UNDER => {
                    cell.set_symbol(&symbol);
                }
                _ => {
                    self.links.0.remove(&(x, y));
                }
            }
        }
        drawn
    }
}

/// What a client was last sent, to send only what changes.
#[derive(Default)]
pub struct Screen {
    prev: Option<Buffer>,
    prev_links: Links,
    /// The pictures last sent: where, and which.
    prev_pictures: Vec<(Rect, u64)>,
    style: Option<cursor::SetCursorStyle>,
    /// The images the terminal holds by kitty's protocol.
    images: super::graphics::ImageCache,
}

impl Screen {
    /// The next frame draws everything again (after an attach, a resize, or
    /// anything that may have left the terminal showing something else).
    pub fn invalidate(&mut self) {
        // The images go with the screen: `render` sends them all again.
        self.prev = None;
        self.prev_links = Links::default();
        self.prev_pictures.clear();
        self.style = None;
    }

    /// The bytes that turn the last frame sent into this one.
    pub fn render(&mut self, frame: &Frame) -> Vec<u8> {
        let mut out = Vec::with_capacity(4096);
        // Synchronized output: the terminal shows the frame once it is whole.
        out.extend_from_slice(b"\x1b[?2026h\x1b[?25l");
        let full = self.prev.as_ref().is_none_or(|p| p.area != frame.buf.area);
        let mut prev = match self.prev.take() {
            Some(p) if !full => p,
            _ => {
                out.extend_from_slice(b"\x1b[0m\x1b[H\x1b[2J");
                self.prev_links = Links::default();
                self.prev_pictures.clear();
                Buffer::empty(frame.buf.area)
            }
        };
        // Drawn again whole, the terminal may have lost the images: all
        // sent again, after the screen is cleared, so a placement under the
        // text isn't cleared with it. Those placed by kitty's protocol only;
        // with any other, any it holds are deleted.
        if full {
            out.extend(self.images.forget());
        }
        let by_kitty = frame
            .graphics
            .is_some_and(|g| g.protocol == super::graphics::Protocol::Kitty);
        out.extend(
            self.images
                .commands(if by_kitty { &frame.images } else { &[] }),
        );
        // A picture shown before and not now, the same in the same place:
        // its cells erased, with whatever of it the terminal shows, and
        // sent again.
        let stale: Vec<Rect> = self
            .prev_pictures
            .iter()
            .filter(|(area, key)| {
                !frame
                    .pictures
                    .iter()
                    .any(|p| p.area == *area && p.key == *key)
            })
            .map(|(area, _)| area.intersection(prev.area))
            .collect();
        if !stale.is_empty() {
            out.extend_from_slice(b"\x1b[0m");
            for area in &stale {
                for y in area.top()..area.bottom() {
                    let _ = write!(out, "\x1b[{};{}H\x1b[{}X", y + 1, area.x + 1, area.width);
                    for x in area.left()..area.right() {
                        prev[(x, y)].set_symbol(UNSENT);
                    }
                }
            }
        }
        let mut diff = prev.diff(&frame.buf);
        let linked = !(frame.links.is_empty() && self.prev_links.is_empty());
        if linked {
            // A cell whose link came or went is sent again though its text
            // is the same.
            let sent: std::collections::HashSet<(u16, u16)> =
                diff.iter().map(|(x, y, _)| (*x, *y)).collect();
            let mut changed: Vec<(u16, u16)> = frame
                .links
                .0
                .iter()
                .filter(|(at, uri)| self.prev_links.0.get(*at) != Some(*uri))
                .map(|(at, _)| *at)
                .chain(
                    self.prev_links
                        .0
                        .keys()
                        .filter(|at| !frame.links.0.contains_key(*at))
                        .copied(),
                )
                .filter(|at| !sent.contains(at))
                .collect();
            changed.sort_unstable();
            changed.dedup();
            for (x, y) in changed {
                if let Some(cell) = frame.buf.cell((x, y)) {
                    diff.push((x, y, cell));
                }
            }
            diff.sort_by_key(|(x, y, _)| (*y, *x));
        }
        // A picture any of whose cells is sent is drawn again over them.
        let touched: Vec<bool> = frame
            .pictures
            .iter()
            .map(|p| {
                diff.iter()
                    .any(|(x, y, _)| p.area.contains(Position::new(*x, *y)))
            })
            .collect();
        if !linked {
            let mut backend = CrosstermBackend::new(&mut out);
            // Writing into a Vec can't fail.
            let _ = backend.draw(diff.into_iter());
        } else {
            // Each run of cells with one link is sent inside its OSC 8.
            let mut rest = diff.as_slice();
            while let Some(&(x, y, _)) = rest.first() {
                let link = frame.links.get(x, y);
                let run = rest
                    .iter()
                    .take_while(|(x, y, _)| frame.links.get(*x, *y) == link)
                    .count();
                if let Some(uri) = link {
                    let _ = write!(out, "\x1b]8;;{uri}\x1b\\");
                }
                {
                    let mut backend = CrosstermBackend::new(&mut out);
                    let _ = backend.draw(rest[..run].iter().copied());
                }
                if link.is_some() {
                    out.extend_from_slice(b"\x1b]8;;\x1b\\");
                }
                rest = &rest[run..];
            }
        }
        self.prev_links = frame.links.clone();
        // The pictures, after the text: one new, moved or under cells just
        // sent; never one reaching the last row, where it would scroll.
        let rows = frame.buf.area.bottom();
        let mut shown = Vec::with_capacity(frame.pictures.len());
        for (p, touched) in frame.pictures.iter().zip(touched) {
            if p.area.bottom() >= rows || p.area.is_empty() {
                continue;
            }
            let same = self
                .prev_pictures
                .iter()
                .any(|(area, key)| *area == p.area && *key == p.key);
            if touched || !same {
                // One more would make the frame larger than a client
                // takes: left for the next frame.
                if out.len() + p.sixel.len() > PICTURES_MOST {
                    continue;
                }
                let _ = write!(out, "\x1b[{};{}H", p.area.y + 1, p.area.x + 1);
                out.extend_from_slice(&p.sixel);
            }
            shown.push((p.area, p.key));
        }
        self.prev_pictures = shown;
        if let Some((x, y)) = frame.cursor {
            let style = frame
                .cursor_style
                .unwrap_or(cursor::SetCursorStyle::DefaultUserShape);
            if self.style != Some(style) {
                let _ = queue!(out, style);
                self.style = Some(style);
            }
            let _ = queue!(out, cursor::MoveTo(x, y), cursor::Show);
        }
        let _ = out.write_all(b"\x1b[?2026l");
        self.prev = Some(frame.buf.clone());
        out
    }
}

/// The columns of `whole` a capped lone pane at `inner` leaves over
/// (`layout::capped`), as UX 2.0 board 62 draws them: the sunken surface,
/// with a rule down each of the pane's edges that `whole` holds (its right).
pub fn draw_spare(buf: &mut Buffer, whole: Rect, inner: Rect, theme: &crate::ui::Theme) {
    let whole = whole.intersection(buf.area);
    let sunken = crate::ui::kit::Surface::Sunken.color(theme);
    let line = crate::ui::kit::border(theme);
    let edges = [inner.x.checked_sub(1), Some(inner.right())];
    for y in whole.top()..whole.bottom() {
        for x in whole.left()..whole.right() {
            if x >= inner.left() && x < inner.right() {
                continue;
            }
            let cell = &mut buf[(x, y)];
            cell.reset();
            cell.set_bg(sunken);
            if edges.contains(&Some(x)) {
                cell.set_symbol("│").set_fg(line);
            }
        }
    }
}

/// Lines between panes, with their crossings joined, in `line`, the ones
/// beside the focused pane in `focus` (the current tab's color).
pub fn draw_borders(
    buf: &mut Buffer,
    placement: &Placement,
    focused: Option<Rect>,
    focus: Color,
    line: Color,
) {
    let area = buf.area;
    let is_border = |x: u16, y: u16| placement.borders.iter().any(|b| on_border(b, x, y));
    let cells: Vec<(u16, u16)> = placement
        .borders
        .iter()
        .flat_map(|b| {
            (0..b.len).map(move |i| {
                if b.vertical {
                    (b.x, b.y + i)
                } else {
                    (b.x + i, b.y)
                }
            })
        })
        .collect();
    for (x, y) in cells {
        if x >= area.right() || y >= area.bottom() {
            continue;
        }
        let up = y > 0 && is_border(x, y - 1);
        let down = is_border(x, y + 1);
        let left = x > 0 && is_border(x - 1, y);
        let right = is_border(x + 1, y);
        let symbol = match (up, down, left, right) {
            (true, true, true, true) => "┼",
            (true, true, true, false) => "┤",
            (true, true, false, true) => "├",
            (false, true, true, true) => "┬",
            (true, false, true, true) => "┴",
            (true, true, false, false)
            | (true, false, false, false)
            | (false, true, false, false) => "│",
            _ => "─",
        };
        let beside_focus = focused.is_some_and(|r| touches(r, x, y));
        let color = if beside_focus { focus } else { line };
        buf[(x, y)]
            .set_symbol(symbol)
            .set_style(Style::new().fg(color).bg(Color::Reset));
    }
}

fn on_border(b: &super::layout::Border, x: u16, y: u16) -> bool {
    if b.vertical {
        x == b.x && y >= b.y && y < b.y + b.len
    } else {
        y == b.y && x >= b.x && x < b.x + b.len
    }
}

/// Whether a cell is on the edge just outside a rectangle (a border of it).
fn touches(r: Rect, x: u16, y: u16) -> bool {
    let (x, y) = (x as i32, y as i32);
    let (l, t, rr, b) = (r.x as i32, r.y as i32, r.right() as i32, r.bottom() as i32);
    let beside_x = (x == l - 1 || x == rr) && y >= t - 1 && y <= b;
    let beside_y = (y == t - 1 || y == b) && x >= l - 1 && x <= rr;
    beside_x || beside_y
}

/// Gives an unfocused pane tmux's `window-style` background: cells left on
/// the terminal's default background get `bg` (colour236, or colour254
/// light).
pub fn tint_inactive(buf: &mut Buffer, area: Rect, bg: Color) {
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            if cell.bg == Color::Reset {
                cell.bg = bg;
            }
        }
    }
}

/// Writes text from column `x`, clipped to the area; gives the column after it.
pub fn put(buf: &mut Buffer, x: u16, area: Rect, text: &str, style: Style) -> u16 {
    if x >= area.right() {
        return x;
    }
    let room = (area.right() - x) as usize;
    let (end, _) = buf.set_stringn(x, area.y, text, room, style);
    end.min(area.right())
        .max(x + UnicodeWidthStr::width(text).min(room) as u16)
}

/// A box with a rounded edge in the accent colour and a title, cleared
/// inside: the frame every popup and menu is drawn in (tmux's
/// `popup-border-lines rounded`). Gives the area inside it.
pub fn draw_box(buf: &mut Buffer, area: Rect, title: &str) -> Rect {
    let area = area.intersection(buf.area);
    if area.width < 2 || area.height < 2 {
        return Rect::default();
    }
    let edge = Style::new().fg(colors::ACCENT);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            buf[(x, y)].reset();
        }
    }
    let (l, r, t, b) = (area.left(), area.right() - 1, area.top(), area.bottom() - 1);
    for x in l..=r {
        buf[(x, t)].set_symbol("─").set_style(edge);
        buf[(x, b)].set_symbol("─").set_style(edge);
    }
    for y in t..=b {
        buf[(l, y)].set_symbol("│").set_style(edge);
        buf[(r, y)].set_symbol("│").set_style(edge);
    }
    buf[(l, t)].set_symbol("╭");
    buf[(r, t)].set_symbol("╮");
    buf[(l, b)].set_symbol("╰");
    buf[(r, b)].set_symbol("╯");
    if !title.is_empty() && area.width > 4 {
        put(
            buf,
            l + 2,
            Rect::new(l + 2, t, area.width - 4, 1),
            &format!(" {title} "),
            edge.add_modifier(Modifier::BOLD),
        );
    }
    Rect::new(l + 1, t + 1, area.width - 2, area.height - 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect()
    }

    #[test]
    fn a_meter_reaches_a_ghostty_client_as_placeholders_and_one_image_and_others_as_text() {
        use crate::mux::graphics::Graphics;
        use crate::ui::images;
        let draw = |graphics: Option<Graphics>| {
            let mut frame = Frame::new(20, 2);
            frame.graphics = graphics;
            images::meter(
                &mut frame,
                "m",
                Rect::new(0, 0, 10, 1),
                0.5,
                Color::Indexed(41),
                None,
            );
            frame
        };
        let kitty = Some(Graphics::kitty((8, 16)));
        let mut screen = Screen::default();
        let out = String::from_utf8_lossy(&screen.render(&draw(kitty))).into_owned();
        assert_eq!(out.matches("\x1b_Ga=T").count(), 1);
        assert!(out.contains(images::PLACEHOLDER));
        let again = String::from_utf8_lossy(&screen.render(&draw(kitty))).into_owned();
        assert!(!again.contains("\x1b_G"), "{again:?}");
        // Drawn whole again, the image is sent again.
        screen.invalidate();
        let whole = String::from_utf8_lossy(&screen.render(&draw(kitty))).into_owned();
        assert_eq!(whole.matches("\x1b_Ga=T").count(), 1);
        // Turned to sixel, no kitty command but the one deleting what the
        // terminal held: the frame's sixels are the pipeline's to add.
        screen.invalidate();
        let sixel = String::from_utf8_lossy(&screen.render(&draw(Some(Graphics::sixel((10, 20))))))
            .into_owned();
        assert_eq!(sixel.matches("\x1b_G").count(), 1, "{sixel:?}");
        assert!(sixel.contains("\x1b_Ga=d,d=I,"), "{sixel:?}");

        let mut plain = Screen::default();
        let out = String::from_utf8_lossy(&plain.render(&draw(None))).into_owned();
        assert!(!out.contains("\x1b_G") && !out.contains(images::PLACEHOLDER));
        assert!(out.contains('━'));
    }

    #[test]
    fn a_second_frame_sends_only_what_changed() {
        let mut screen = Screen::default();
        let mut frame = Frame::new(10, 2);
        frame.buf.set_string(0, 0, "hello", Style::new());
        let first = screen.render(&frame);
        assert!(String::from_utf8_lossy(&first).contains("hello"));
        frame.buf.set_string(0, 1, "x", Style::new());
        let second = String::from_utf8_lossy(&screen.render(&frame)).into_owned();
        assert!(
            second.contains('x') && !second.contains("hello"),
            "{second:?}"
        );
    }

    /// A picture over cells, its sixel standing in as `SIXEL<n>`.
    fn picture(area: Rect, key: u64) -> Placed {
        Placed {
            area,
            key,
            sixel: Arc::from(format!("\x1bPSIXEL{key}\x1b\\").into_bytes()),
        }
    }

    fn count(haystack: &str, needle: &str) -> usize {
        haystack.matches(needle).count()
    }

    #[test]
    fn a_picture_is_drawn_after_its_text_and_again_only_when_its_cells_are() {
        let mut screen = Screen::default();
        let mut frame = Frame::new(20, 6);
        frame.buf.set_string(0, 0, "above", Style::new());
        frame.pictures.push(picture(Rect::new(2, 1, 4, 2), 1));
        let first = String::from_utf8(screen.render(&frame)).unwrap();
        let text = first.find("above").unwrap();
        let drawn = first
            .find("\x1b[2;3H\x1bPSIXEL1\x1b\\")
            .expect("at its top left");
        assert!(drawn > text, "after the text: {first:?}");
        assert!(
            first.find("\x1b[?2026l").unwrap() > drawn,
            "inside the frame's update"
        );
        // Nothing changed: not sent again.
        let same = String::from_utf8(screen.render(&frame)).unwrap();
        assert_eq!(count(&same, "SIXEL"), 0, "{same:?}");
        // A cell beside it sent: still not.
        frame.buf.set_string(10, 1, "x", Style::new());
        let beside = String::from_utf8(screen.render(&frame)).unwrap();
        assert_eq!(count(&beside, "SIXEL"), 0, "{beside:?}");
        // One of its cells sent (text over it takes it away): drawn again.
        frame.buf[(3, 2)].set_bg(Color::Indexed(236));
        let under = String::from_utf8(screen.render(&frame)).unwrap();
        assert_eq!(count(&under, "SIXEL1"), 1, "{under:?}");
        assert!(under.find("SIXEL1").unwrap() > under.find("\x1b[3;4H").unwrap());
        // Everything drawn again: so is it.
        screen.invalidate();
        let all = String::from_utf8(screen.render(&frame)).unwrap();
        assert_eq!(count(&all, "SIXEL1"), 1);
    }

    #[test]
    fn a_picture_gone_or_moved_has_its_cells_erased_and_sent_again() {
        let mut screen = Screen::default();
        let mut frame = Frame::new(20, 6);
        frame.pictures.push(picture(Rect::new(2, 1, 4, 2), 1));
        screen.render(&frame);
        // Scrolled up a row: the old place erased row by row, its blank
        // cells sent again, and the picture drawn at its new place.
        frame.pictures = vec![picture(Rect::new(2, 0, 4, 2), 2)];
        let moved = String::from_utf8(screen.render(&frame)).unwrap();
        let erase = moved
            .find("\x1b[2;3H\x1b[4X")
            .expect("its first row erased");
        assert!(moved.contains("\x1b[3;3H\x1b[4X"), "and its second");
        let drawn = moved.find("\x1b[1;3H\x1bPSIXEL2").expect("drawn anew");
        assert!(erase < drawn, "{moved:?}");
        assert!(
            moved.contains("\x1b[3;3H    "),
            "the cells sent again: {moved:?}"
        );
        // Gone: erased, and nothing drawn.
        frame.pictures.clear();
        let gone = String::from_utf8(screen.render(&frame)).unwrap();
        assert!(gone.contains("\x1b[1;3H\x1b[4X") && gone.contains("\x1b[2;3H\x1b[4X"));
        assert_eq!(count(&gone, "SIXEL"), 0);
        // Erased once only.
        let after = String::from_utf8(screen.render(&frame)).unwrap();
        assert!(!after.contains('X'), "{after:?}");
    }

    #[test]
    fn no_picture_reaches_the_last_row() {
        let mut screen = Screen::default();
        let mut frame = Frame::new(20, 6);
        frame.pictures.push(picture(Rect::new(0, 4, 4, 2), 1));
        frame.pictures.push(picture(Rect::new(0, 3, 4, 2), 2));
        let sent = String::from_utf8(screen.render(&frame)).unwrap();
        assert_eq!(count(&sent, "SIXEL1"), 0, "it would scroll the screen");
        assert_eq!(count(&sent, "SIXEL2"), 1, "ending just above it is drawn");
    }

    #[test]
    fn with_no_pictures_a_frame_is_sent_as_it_always_was() {
        // What a screen sent before pictures could ride beside the cells:
        // the frame's update, the screen cleared the first time, the cells
        // that changed as ratatui's backend writes them, the cursor.
        fn as_before(prev: Option<&Buffer>, frame: &Frame) -> Vec<u8> {
            let mut out = b"\x1b[?2026h\x1b[?25l".to_vec();
            let empty = Buffer::empty(frame.buf.area);
            let prev = match prev {
                Some(p) => p,
                None => {
                    out.extend_from_slice(b"\x1b[0m\x1b[H\x1b[2J");
                    &empty
                }
            };
            let _ = CrosstermBackend::new(&mut out).draw(prev.diff(&frame.buf).into_iter());
            if let Some((x, y)) = frame.cursor {
                let _ = queue!(
                    out,
                    cursor::SetCursorStyle::DefaultUserShape,
                    cursor::MoveTo(x, y),
                    cursor::Show
                );
            }
            out.extend_from_slice(b"\x1b[?2026l");
            out
        }
        let mut screen = Screen::default();
        let mut frame = Frame::new(30, 4);
        frame
            .buf
            .set_string(0, 0, "a pane", Style::new().fg(Color::Red));
        frame.cursor = Some((3, 1));
        assert_eq!(screen.render(&frame), as_before(None, &frame));
        let first = frame.buf.clone();
        frame.buf.set_string(0, 2, "more", Style::new());
        frame.cursor = None;
        assert_eq!(screen.render(&frame), as_before(Some(&first), &frame));
    }

    #[test]
    fn a_box_drawn_over_a_link_takes_it_away_and_a_style_alone_does_not() {
        let mut frame = Frame::new(10, 2);
        frame.buf.set_string(0, 0, "link", Style::new());
        frame.buf.set_string(0, 1, "more", Style::new());
        let uri: Arc<str> = Arc::from("https://example.com/");
        for x in 0..4 {
            frame.links.set(x, 0, uri.clone());
            frame.links.set(x, 1, uri.clone());
        }
        frame.cover_links(|f| {
            f.buf.set_string(0, 0, "bo", Style::new());
            f.buf
                .set_style(Rect::new(0, 1, 10, 1), Style::new().fg(Color::Red));
        });
        assert_eq!(row(&frame.buf, 0).trim_end(), "bonk", "the text put back");
        assert_eq!(frame.links.get(0, 0), None, "under the box");
        assert_eq!(frame.links.get(2, 0), Some(&uri), "beside it");
        assert_eq!(frame.links.get(0, 1), Some(&uri), "a style alone");
        assert_eq!(row(&frame.buf, 1).trim_end(), "more");
        let sent = String::from_utf8(Screen::default().render(&frame)).unwrap();
        assert!(
            sent.contains("\x1b]8;;https://example.com/\x1b\\"),
            "{sent:?}"
        );

        assert!(Links::safe("https://example.com/a?b=c;d#e"));
        assert!(!Links::safe("https://example.com/\x1b]0;x"));
        assert!(!Links::safe("https://example.com/a b"));
        assert!(!Links::safe(""));
    }

    #[test]
    fn crossing_lines_join() {
        use super::super::layout::Border;
        let mut frame = Frame::new(5, 5);
        let placement = Placement {
            panes: vec![],
            borders: vec![
                Border {
                    x: 2,
                    y: 0,
                    len: 5,
                    vertical: true,
                },
                Border {
                    x: 0,
                    y: 2,
                    len: 5,
                    vertical: false,
                },
            ],
        };
        draw_borders(
            &mut frame.buf,
            &placement,
            None,
            colors::ACCENT,
            colors::BORDER,
        );
        assert_eq!(frame.buf[(2, 2)].symbol(), "┼");
        assert_eq!(frame.buf[(2, 0)].symbol(), "│");
        assert_eq!(frame.buf[(0, 2)].symbol(), "─");
    }
}
