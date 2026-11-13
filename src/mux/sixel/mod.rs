//! Pictures as sixel, for a terminal that draws sixel but not kitty's
//! graphics protocol: Windows Terminal (1.22 and later), and the rest of
//! the VT340's heirs (foot, WezTerm, xterm with sixel on, mlterm). Settings'
//! "Image protocol" (`image_protocol`) chooses, as src/mux/graphics.rs
//! reads it: by the terminal (`auto`, the default), one whose answer to
//! "what are you" (DA1) says it draws sixel gets it; `sixel` gives it to
//! every terminal that doesn't speak kitty's; `kitty` to none. For a
//! terminal that gets it, the server keeps the panes' graphics commands
//! here rather than passing them on (src/mux/passthrough.rs) and draws
//! every picture itself, the panes' and its own alike; any other is passed
//! the panes' commands and sent the server's own images by kitty's
//! protocol where it speaks it.
//!
//! The apps stay as they are. Notes and the image view give the terminal
//! each picture's PNG by its path as a kitty virtual placement, and the
//! Markdown views (the PR, ticket, link and file views) its data inline,
//! and both write its placeholder cells (src/notes/images.rs,
//! src/views/markdown.rs), which the panes' emulators keep as text; with
//! sixel chosen, `graphics::apps_draw` lets them draw inside the workspace
//! whatever the terminal is called. The server reads the commands from the
//! panes allowed to send them, as before, and keeps what each image is
//! (`kitty`: its path or its PNG's data, and its placement), forgetting it
//! as they delete it, never forwarding them to a terminal that gets sixel.
//! A picture given inline is taken in only where sixel may draw it, kept as
//! it was sent until its pane deletes it, and named by its transmission
//! (`Source::Data`), its data handed to the work that decodes and fits it
//! off the task beside the name (`Make`), so no cache's key keeps it
//! alive. The server's own images (src/ui/
//! images.rs: the image atoms, board 6's lane and chart, mission control's
//! diagrams and Slack pictures, the picture shown full size) write their
//! placeholder cells into the frame as they are drawn and add themselves
//! to its `images`, as they do for kitty. As each frame is composed, the
//! placeholder cells the panes drew are noted before anything is drawn
//! over them, and after everything is, every placeholder cell in the
//! frame is blanked, and those still as their pane drew them, and those
//! naming one of the frame's own images wherever they are, are gathered
//! into pieces of one picture each (`place`), and each piece is drawn as
//! sixel beside the frame's cells (`compose::Frame::pictures`, as the
//! hyperlinks ride beside them), which `compose::Screen` writes over its
//! cells, so a picture is placed, scrolled, cut off and covered exactly as
//! its placeholders are, and a copy of a pane's (UX 2.0's mission control
//! cards show each tab's last lines, a peek a tab's screen) is never
//! taken for the picture. The shadow under a box is the one image that
//! goes under the text, which a sixel can't, so with sixel the frame asks
//! for its text form instead (`images::shadow`).
//!
//! UX 2.0 (`ux2`) changes where panes are, not how: its header's rows are
//! above the panes, so a picture scrolled up out of a pane is cut at the
//! pane's top, under the header; and with no status line a pane reaches
//! the screen's last row, which no piece is drawn on (`place`), so a
//! picture there loses that row. Notes and the image view keep their own
//! last row for a message or footer, so theirs never reach it.
//!
//! Nothing on the drawing path waits: a picture's file is looked at (its
//! time and length) and read, fitted, quantised and encoded (`encode`) off
//! the server's task, the answer coming back as `SixelEvent`, and its cells
//! stay blank until then. One of the server's own images is rastered at
//! the cell its frame was drawn for and taken as it is, no fitting; one no
//! larger than `AT_ONCE_PIXELS` that is a drawing whose colours fit a
//! palette as they are (a switch, a meter, a chart) is rastered and encoded
//! on the task as the frame is drawn, as kitty's are sent, so one that
//! changes shows changed with no blank frame between; a photo (`Placed::
//! photo`: an avatar, a Slack picture), whose palette takes a while to
//! learn, and a larger one (a picture shown full size), are drawn off it,
//! and only a piece cut anew from one fitted before is encoded on it.
//! The fitted picture and each piece's sixel are kept, each cache held to
//! a size (`Lru`), the piece's under what it is drawn from (a pane's
//! file's path, time and length and its placement, or a server image's
//! key and size), the part of it shown and the cell's size, so a piece
//! shown again (a page turned back to, a box closed) is drawn at once, and
//! one cut anew (a scroll) only encoded. A
//! picture given again (Notes gives its pictures again as it gets the
//! focus) is drawn by what its file was when last looked at until it has
//! been looked at again, so it doesn't blink, and a file changed since is
//! drawn anew once it has.
//!
//! A cell's size in the terminal's sixel pixels is asked of each terminal
//! that gets sixel, as it attaches or answers DA1 (`CSI 16 t`, answered as `CSI 6 ;
//! <height> ; <width> t` among its keys, src/mux/terminal.rs). Windows
//! Terminal answers with the virtual cell its sixel works in, 10 by 20
//! pixels whatever its font (`SixelParser::CellSizeForLevel`, as the VT340
//! had; it scales each cell's part of a picture to the font's cell), so a
//! picture drawn at 10 by 20 a cell fills its cells there. Otherwise the
//! window's pixels the terminal said serve, else that same 10 by 20. The
//! panes are still given the cell its window's pixels make (or 8 by 16),
//! so the apps size and lay pictures out as before, the image view a page
//! as wide as the text around it; a picture is scaled to its cells either
//! way, its shape kept. The cell is one for the server, the last terminal
//! to answer's: two attached at once that answer differently get pictures
//! sized for that one, as the terminal's colours are one for the server.
//!
//! What a pane's program gave is part of what `switchboard upgrade` hands
//! the new build (src/mux/upgrade.rs), since a program gives a picture
//! once; the caches are made again, and the server's own images are
//! described anew with every frame.

pub mod encode;
pub mod kitty;
pub mod place;

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use ratatui::layout::Rect;

use super::compose::{Frame, Placed};
use super::graphics::{self, Protocol};
use super::server::{Event, Server};
use super::{PaneId, TabId};
use crate::ui::images::{Draw, Spot};
use encode::Fitted;
use kitty::Look;

/// What asks a terminal for its cell's size in pixels.
pub const CELL_QUERY: &[u8] = b"\x1b[16t";

/// The cell taken when a terminal says nothing of its pixels: the VT340's,
/// which Windows Terminal's sixel keeps whatever its font.
pub const STAND_IN: (u16, u16) = (10, 20);

/// How much the sixel strings kept, and the fitted pictures, may take.
const ENCODED_BYTES: usize = 32 << 20;
const FITTED_BYTES: usize = 96 << 20;
/// How many pictures are drawn off the task at once; the rest wait for the
/// next frame, by when a scroll may have made them needless.
const AT_ONCE: usize = 4;
/// The longest sixel string a piece may be (a photo of every colour, the
/// whole screen wide, could be tens of megabytes): one longer isn't drawn.
const MOST_BYTES: usize = 8 << 20;
/// The most of the pictures given inline a handover carries to the new
/// build, which has no other way to have them again (a megabyte in the
/// unit tests, which fill it).
const HANDED_DATA: usize = if cfg!(test) { 1 << 20 } else { 64 << 20 };

/// The server's own images no larger than this many pixels whole, and of
/// no more colours than a palette holds, are drawn on the server's task as
/// the frame is (a switch, a meter, a chart), so one that changes is drawn
/// changed in the same frame, as kitty's are sent; others off it.
const AT_ONCE_PIXELS: u32 = 160_000;

/// What a picture is drawn from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    /// A pane's: its file as looked at, and its placement's columns and
    /// rows as given.
    File {
        path: PathBuf,
        look: Look,
        given: (Option<u16>, Option<u16>),
    },
    /// A pane's given inline (the Markdown views'): its transmission (the
    /// server's count, `kitty::Image::serial`), which names it, and its
    /// placement's columns and rows as given. Its data goes beside the name
    /// (`Make::Data`), so a cache's keys don't keep it alive.
    Data {
        serial: u64,
        given: (Option<u16>, Option<u16>),
    },
    /// One of the server's own images (src/ui/images.rs): its key, made of
    /// everything it is drawn from, and its size in cells.
    Server { key: u64, cols: u16, rows: u16 },
}

/// What a picture not yet fitted is made from, beyond what names it: one
/// of the server's images rastered, or a PNG a pane gave inline, base64 as
/// it was sent, decoded. A picture by its file needs none.
#[derive(Clone)]
pub enum Make {
    Raster(Draw),
    Data(Arc<[u8]>),
}

/// A fitted picture's name: what it is drawn from, and the cell's size in
/// pixels.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Fit {
    pub source: Source,
    pub cell: (u16, u16),
}

/// A piece's sixel's name: its picture, and the part shown (its first
/// column and row of the placement, and its width and height, in cells).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub fit: Fit,
    pub crop: (u16, u16, u16, u16),
}

impl Key {
    /// A number for it, by which a screen tells one piece from another.
    pub fn number(&self) -> u64 {
        let mut h = std::hash::DefaultHasher::new();
        self.hash(&mut h);
        h.finish()
    }
}

/// Draws a piece, off the server's task or, for a small one of the
/// server's own, on it: its picture read and fitted (a pane's) or rastered
/// (`raster`, the server's), unless `fitted` has it, and its part encoded,
/// a whole number of sixel bands tall (encode.rs says why). Its height is
/// cut down to that, so a piece loses up to 4 pixel rows at its foot (with
/// Windows Terminal's 20-pixel cell, 2 for a piece one row tall, 4 for
/// two, none for three): a picture whole loses a sliver of its bottom
/// edge, and one a box cuts into pieces a row tall a sliver under each
/// row, less than a band reaching into the row below would cover.
pub fn draw(key: &Key, fitted: Option<Arc<Fitted>>, make: Option<&Make>) -> Made {
    let cell = (key.fit.cell.0.max(1) as u32, key.fit.cell.1.max(1) as u32);
    let fitted = match fitted {
        Some(f) => f,
        None => Arc::new(fit(&key.fit.source, cell, make)?),
    };
    let (col, row, cols, rows) = key.crop;
    let height = rows as u32 * cell.1;
    let height = if height >= 6 { height / 6 * 6 } else { height };
    let crop = (
        col as u32 * cell.0,
        row as u32 * cell.1,
        cols as u32 * cell.0,
        height,
    );
    if crop.2 > encode::MOST || crop.3 > encode::MOST {
        return Err(Failure::Piece("a piece too large to draw".into()));
    }
    let sixel = encode::encode(&fitted, crop);
    if sixel.len() > MOST_BYTES {
        return Err(Failure::Piece(format!(
            "a piece of {} bytes, more than a terminal should be sent at once",
            sixel.len()
        )));
    }
    Ok((sixel.into(), fitted))
}

/// A picture made ready to cut pieces from: a pane's read from its file or
/// its data and fitted into its placement, or one of the server's rastered
/// at its cells and taken as it is.
fn fit(source: &Source, cell: (u32, u32), make: Option<&Make>) -> Result<Fitted, Failure> {
    let read = |image: Result<image::RgbaImage, String>, given| {
        let image = image.map_err(Failure::Picture)?;
        let cells = encode::cells_of(given, image.dimensions(), cell);
        encode::prepare(&image, cells, cell).map_err(Failure::Picture)
    };
    match source {
        Source::File { path, given, .. } => read(encode::read_png(path), *given),
        Source::Data { given, .. } => match make {
            Some(Make::Data(data)) => match crate::util::unbase64(data) {
                Some(png) => read(encode::read_png_bytes(&png), *given),
                None => Err(Failure::Picture("data that isn't base64".into())),
            },
            _ => Err(Failure::Picture("no data to read".into())),
        },
        Source::Server { cols, rows, .. } => {
            let Some(Make::Raster(raster)) = make else {
                return Err(Failure::Picture("nothing to raster".into()));
            };
            let size = (*cols as u32 * cell.0, *rows as u32 * cell.1);
            if size.0 > encode::MOST || size.1 > encode::MOST {
                return Err(Failure::Picture(format!(
                    "an image of {}x{} pixels is too large",
                    size.0, size.1
                )));
            }
            Ok(Fitted {
                picture: encode::quantize(&crate::ui::images::rgba(&raster())),
                at: (0, 0),
                size,
            })
        }
    }
}

/// A piece of one of the server's own small drawings rastered and drawn on
/// the task, when its colours fit a palette as they are (a switch, a
/// meter, a chart: a few colours and their edges' transparency); `None`
/// when they don't, since learning a palette takes a while.
fn draw_at_once(key: &Key, raster: &Draw) -> Option<Made> {
    let Source::Server { cols, rows, .. } = key.fit.source else {
        return None;
    };
    let picture = encode::own_colours(&crate::ui::images::rgba(&raster()))?;
    let size = (
        cols as u32 * key.fit.cell.0 as u32,
        rows as u32 * key.fit.cell.1 as u32,
    );
    let fitted = Fitted {
        picture,
        at: (0, 0),
        size,
    };
    Some(draw(key, Some(Arc::new(fitted)), None))
}

/// The pixels a rounded end's color is in the terminal that draws it: one
/// of its sixteen and its default text color as it said them when asked
/// (src/mux/terminal.rs), or as `terminal_colors` set them, so a half
/// circle a program of yours colored with
/// them (a powerline prompt's) matches the rest of what it drew, else
/// xterm's (`images::rgb`), as Switchboard's own, in 256 colors or
/// truecolor, are anyway.
fn end_color(color: ratatui::style::Color, said: &super::terminal::Colors) -> [u8; 3] {
    use ratatui::style::Color;
    let index = match color {
        Color::Indexed(n) if n < 16 => Some(n as usize),
        Color::Black => Some(0),
        Color::Red => Some(1),
        Color::Green => Some(2),
        Color::Yellow => Some(3),
        Color::Blue => Some(4),
        Color::Magenta => Some(5),
        Color::Cyan => Some(6),
        Color::Gray => Some(7),
        Color::DarkGray => Some(8),
        Color::LightRed => Some(9),
        Color::LightGreen => Some(10),
        Color::LightYellow => Some(11),
        Color::LightBlue => Some(12),
        Color::LightMagenta => Some(13),
        Color::LightCyan => Some(14),
        Color::White => Some(15),
        _ => None,
    };
    let own = match color {
        Color::Reset => said.fg,
        _ => index.and_then(|i| said.ansi[i]),
    };
    own.map_or_else(|| crate::ui::images::rgb(color), |c| [c.r, c.g, c.b])
}

/// A rounded end of kit's objects as drawn: the left one or not, its
/// color, and the cell's size in pixels.
type End = (bool, [u8; 3], (u16, u16), bool);

/// One rounded end as sixel, a cell of `cell` pixels, the rest of the cell
/// clear. Its height is rounded up to a whole number of bands, the rows
/// past the cell clear, where a picture's is cut down (`draw`): cut, the
/// half circle would lose its foot beside the object's body, whose cells
/// reach the cell's bottom, while the clear rows lie over the top of the
/// row below and leave it as it is; not `whole`, on the row above the
/// screen's last, which no sixel reaches, it is cut down as a picture is.
fn end_sixel(left: bool, color: [u8; 3], cell: (u16, u16), whole: bool) -> Option<Arc<[u8]>> {
    let pm = crate::ui::images::raster_end(left, color, cell);
    let picture = encode::own_colours(&crate::ui::images::rgba(&pm))?;
    let (w, h) = (cell.0.max(1) as u32, cell.1.max(1) as u32);
    let fitted = Fitted {
        picture,
        at: (0, 0),
        size: (w, h),
    };
    let height = if whole || h < 6 {
        h.div_ceil(6) * 6
    } else {
        h / 6 * 6
    };
    Some(encode::encode(&fitted, (0, 0, w, height)).into())
}

/// Whether a picture is small enough to draw on the server's task: one of
/// the server's own of no more than `AT_ONCE_PIXELS`.
fn small(fit: &Fit) -> bool {
    match fit.source {
        Source::Server { cols, rows, .. } => {
            (cols as u32 * fit.cell.0 as u32).saturating_mul(rows as u32 * fit.cell.1 as u32)
                <= AT_ONCE_PIXELS
        }
        Source::File { .. } | Source::Data { .. } => false,
    }
}

/// A piece drawn, its sixel and the picture fitted for it, or why it
/// couldn't be.
pub type Made = Result<(Arc<[u8]>, Arc<Fitted>), Failure>;

/// Why a piece couldn't be drawn: its picture (not a PNG, gone), which no
/// other piece of it can be drawn from either, or the piece alone (too
/// large to send), when a smaller piece of the same picture still can be.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    Picture(String),
    Piece(String),
}

/// What the work off the server's task found.
#[derive(Debug)]
pub enum SixelEvent {
    /// An image's file looked at, for its transmission `serial`.
    Looked {
        pane: PaneId,
        id: u32,
        serial: u64,
        path: PathBuf,
        look: Look,
    },
    /// A piece drawn, or why it couldn't be.
    Drawn { key: Key, made: Made },
}

/// A cache held to a number of bytes, the entry used longest ago going
/// first.
struct Lru<K, V> {
    entries: HashMap<K, (V, usize, u64)>,
    bytes: usize,
    limit: usize,
    clock: u64,
}

impl<K: Hash + Eq + Clone, V: Clone> Lru<K, V> {
    fn new(limit: usize) -> Self {
        Lru {
            entries: HashMap::new(),
            bytes: 0,
            limit,
            clock: 0,
        }
    }

    fn get(&mut self, key: &K) -> Option<V> {
        self.clock += 1;
        let entry = self.entries.get_mut(key)?;
        entry.2 = self.clock;
        Some(entry.0.clone())
    }

    fn insert(&mut self, key: K, value: V, bytes: usize) {
        self.clock += 1;
        if let Some((_, old, _)) = self.entries.insert(key, (value, bytes, self.clock)) {
            self.bytes -= old;
        }
        self.bytes += bytes;
        while self.bytes > self.limit && self.entries.len() > 1 {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, _, used))| *used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some((_, gone, _)) = self.entries.remove(&oldest) {
                self.bytes -= gone;
            }
        }
    }
}

/// What the server keeps for drawing pictures as sixel.
pub struct Sixel {
    /// What each allowed pane's program gave the terminal, by image id.
    pub images: HashMap<PaneId, kitty::Images>,
    encoded: Lru<Key, Arc<[u8]>>,
    fitted: Lru<Fit, Arc<Fitted>>,
    /// The pieces being drawn, and the pictures being fitted for them.
    drawing: HashSet<Key>,
    fitting: HashSet<Fit>,
    /// The files being looked at, by pane, image and transmission.
    looking: HashSet<(PaneId, u32, u64)>,
    /// What each file was when last looked at, which a picture given
    /// again (a page turned back to, Notes' pictures as it gets the focus)
    /// is drawn by until its own look comes, so it doesn't blink.
    looks: HashMap<PathBuf, Look>,
    /// Pictures that couldn't be drawn (not a PNG, gone), not tried again
    /// until their file changes (`Fit` holds its look).
    failed: HashSet<Fit>,
    /// Pieces too large to draw, not tried again while their picture and
    /// crop stay the same; the picture's other pieces still are.
    failed_pieces: HashSet<Key>,
    /// The server's drawings found to have more colours than a palette
    /// holds, drawn off the task from then on rather than rastered on it.
    many_colours: HashSet<Fit>,
    /// Each rounded end drawn (which, its color and the cell), as sixel.
    ends: HashMap<End, Arc<[u8]>>,
    /// Transmissions counted, newest highest.
    serial: u64,
    /// What "Image protocol" chose when last looked, so a change draws
    /// every screen again whole.
    pub chosen: graphics::Chosen,
}

impl Default for Sixel {
    fn default() -> Self {
        Sixel {
            images: HashMap::new(),
            encoded: Lru::new(ENCODED_BYTES),
            fitted: Lru::new(FITTED_BYTES),
            drawing: HashSet::new(),
            fitting: HashSet::new(),
            looking: HashSet::new(),
            looks: HashMap::new(),
            failed: HashSet::new(),
            failed_pieces: HashSet::new(),
            many_colours: HashSet::new(),
            ends: HashMap::new(),
            serial: 0,
            chosen: graphics::Chosen::Auto,
        }
    }
}

impl Sixel {
    /// As a server starts, with the setting as it is.
    pub fn new() -> Sixel {
        Sixel {
            chosen: graphics::chosen(),
            ..Sixel::default()
        }
    }

    /// What a handover carries: each pane's images, those given inline
    /// while `HANDED_DATA` lasts, the newest first across every pane.
    pub fn hand_over(&self) -> Vec<(PaneId, Vec<kitty::Handed>)> {
        let mut inline: Vec<(u64, usize, PaneId, u32)> = self
            .images
            .iter()
            .flat_map(|(pane, images)| {
                images
                    .inline()
                    .map(move |(id, serial, len)| (serial, len, *pane, id))
            })
            .collect();
        inline.sort_by_key(|(serial, ..)| std::cmp::Reverse(*serial));
        let mut budget = HANDED_DATA;
        let mut carried: HashSet<(PaneId, u32)> = HashSet::new();
        for (_, len, pane, id) in inline {
            if len <= budget {
                budget -= len;
                carried.insert((pane, id));
            }
        }
        let mut out: Vec<(PaneId, Vec<kitty::Handed>)> = self
            .images
            .iter()
            .map(|(pane, images)| {
                let handed = images.hand_over(|id| carried.contains(&(*pane, id)));
                (*pane, handed)
            })
            .filter(|(_, images)| !images.is_empty())
            .collect();
        out.sort_by_key(|(pane, _)| *pane);
        out
    }

    /// The images a handover carried, for the panes taken up.
    pub fn take_up(&mut self, handed: Vec<(PaneId, Vec<kitty::Handed>)>, panes: &HashSet<PaneId>) {
        for (pane, images) in handed {
            if panes.contains(&pane) {
                self.serial = self
                    .images
                    .entry(pane)
                    .or_default()
                    .take_up(images, self.serial);
            }
        }
    }
}

impl Server {
    /// A cell's size in the pixels pictures are drawn in: the terminal's
    /// answer when asked, else what its window's pixels make, else the
    /// VT340's.
    pub fn picture_cell(&self) -> (u16, u16) {
        self.terminal
            .asked_cell
            .or(self.terminal.cell)
            .unwrap_or(STAND_IN)
    }

    /// Graphics commands from a pane allowed to send them: what they give
    /// kept, and the commands passed on to the terminals that take them
    /// (src/mux/graphics.rs), the server drawing the pictures itself as
    /// sixel for the rest.
    pub(super) fn take_graphics(&mut self, pane: PaneId, tab: TabId, commands: Vec<Vec<u8>>) {
        if commands.is_empty() {
            return;
        }
        // A picture given inline is kept only where sixel may draw it:
        // with sixel chosen, or a terminal attached that gets it or may
        // (Windows Terminal before its answer to DA1 is in).
        let inline = graphics::chosen() == graphics::Chosen::Sixel
            || self
                .clients
                .values()
                .any(|c| graphics::may_get_sixel(&c.term));
        for command in &commands {
            let Some(c) = kitty::parse(command) else {
                continue;
            };
            self.sixel.serial += 1;
            let serial = self.sixel.serial;
            let images = self.sixel.images.entry(pane).or_default();
            images.apply(&c, serial, inline);
            if images.is_empty() {
                self.sixel.images.remove(&pane);
            }
        }
        self.pass_graphics(tab, commands);
    }

    /// A pane's program may no longer give pictures, or has gone: what it
    /// gave is forgotten.
    pub(super) fn forget_images(&mut self, pane: PaneId) {
        self.sixel.images.remove(&pane);
    }

    /// Once the panes are drawn (`panes`, each drawn and its area on the
    /// screen) and before anything is drawn over them, for a terminal that
    /// gets the panes' pictures as sixel (`way`): the placeholder cells
    /// drawn by the panes allowed to send pictures, as they drew them
    /// (`place::Drawn`).
    pub(super) fn pictures_drawn(
        &self,
        frame: &Frame,
        panes: &[(PaneId, Rect)],
        way: Protocol,
        ends: graphics::Ends,
    ) -> Option<place::Drawn> {
        let any_allowed = panes.iter().any(|(pane, _)| {
            self.panes
                .get(pane)
                .is_some_and(|s| super::passthrough::graphics_allowed(&s.opts))
        });
        if way != Protocol::Sixel && !(ends == graphics::Ends::Spaces && any_allowed) {
            return None;
        }
        type Panes = Vec<(PaneId, Rect)>;
        let (allowed, others): (Panes, Panes) = panes.iter().partition(|(pane, _)| {
            self.panes
                .get(pane)
                .is_some_and(|s| super::passthrough::graphics_allowed(&s.opts))
        });
        let others: Vec<Rect> = others.into_iter().map(|(_, area)| area).collect();
        Some(place::Drawn::of(&frame.buf, allowed).with_others(&frame.buf, &others))
    }

    /// The rounded ends of kit's objects in a frame as `ends` has them
    /// reach its terminal (src/mux/graphics.rs), once everything is drawn:
    /// as sixel pictures (`draw_ends`) where it gets sixel; as spaces of
    /// their color where it gets spaces, for those Switchboard's apps drew
    /// in their panes, for another terminal or one that has gone since
    /// they asked (the server's own drawing writes spaces itself). A half
    /// circle a program of yours drew in its pane is left to the terminal
    /// either way.
    pub(super) fn fix_ends(
        &mut self,
        frame: &mut Frame,
        drawn: &place::Drawn,
        ends: graphics::Ends,
    ) {
        match ends {
            graphics::Ends::Pictures => self.draw_ends(frame, drawn),
            graphics::Ends::Spaces => {
                let area = frame.buf.area;
                for y in area.top()..area.bottom() {
                    for x in area.left()..area.right() {
                        let c = &mut frame.buf[(x, y)];
                        if place::is_end(c) && drawn.end_by(x, y, c) == Some(true) {
                            let fill = c.fg;
                            c.set_symbol(" ");
                            c.fg = ratatui::style::Color::Reset;
                            c.bg = fill;
                        }
                    }
                }
            }
            graphics::Ends::Glyphs => {}
        }
    }

    /// The pictures a frame shows, for a terminal that gets sixel, once everything
    /// is drawn: every placeholder cell in it blanked, and each piece of a
    /// picture still showing put beside the frame's cells when ready: a
    /// pane's where the pane drew it (`drawn`, from `pictures_drawn`), and
    /// one of the server's own images (the frame's `images`, which with
    /// sixel are drawn here rather than sent) wherever its cells still
    /// show. One not ready yet is looked at or drawn off the task, or, one
    /// of the server's small enough, drawn now.
    pub(super) fn place_pictures(&mut self, frame: &mut Frame, drawn: Option<place::Drawn>) {
        let Some(drawn) = drawn else {
            return;
        };
        // The server's images, by id, where this frame draws them as sixel.
        let by_sixel = frame
            .graphics
            .filter(|g| g.protocol == Protocol::Sixel)
            .map(|g| g.cell);
        let mut own: HashMap<u32, crate::ui::images::Placed> = HashMap::new();
        if by_sixel.is_some() {
            for image in std::mem::take(&mut frame.images) {
                if image.spot == Spot::Placeholders {
                    own.entry(image.id).or_insert(image);
                }
            }
        }
        let ids: HashSet<u32> = own.keys().copied().collect();
        let bottom = frame.area().bottom().saturating_sub(1);
        let pieces = place::find(&mut frame.buf, &drawn, &ids, bottom);
        if pieces.is_empty() {
            return;
        }
        let cell = self.picture_cell();
        for piece in pieces {
            let crop = (piece.col, piece.row, piece.area.width, piece.area.height);
            let (key, make, photo) = match piece.pane {
                Some(pane) => match self.pane_fit(pane, piece.id, cell) {
                    Some((fit, make)) => (Key { fit, crop }, make, true),
                    None => continue,
                },
                None => {
                    let (Some(image), Some(cell)) = (own.get(&piece.id), by_sixel) else {
                        continue;
                    };
                    let source = Source::Server {
                        key: image.key,
                        cols: image.cols,
                        rows: image.rows,
                    };
                    let fit = Fit { source, cell };
                    let make = Make::Raster(image.draw.clone());
                    (Key { fit, crop }, Some(make), image.photo)
                }
            };
            if let Some(sixel) = self.sixel.encoded.get(&key) {
                frame.pictures.push(Placed {
                    area: piece.area,
                    key: key.number(),
                    sixel,
                });
                continue;
            }
            match make {
                Some(Make::Raster(raster)) if small(&key.fit) => {
                    if self.sixel.failed.contains(&key.fit)
                        || self.sixel.failed_pieces.contains(&key)
                        || self.sixel.drawing.contains(&key)
                        || self.sixel.fitting.contains(&key.fit)
                    {
                        continue;
                    }
                    // Encoded now from the picture fitted before; else, a
                    // drawing of a few colours rastered now; a photo, or a
                    // drawing found to have more colours than a palette
                    // holds, whose palette takes a while to learn, off the
                    // task, never rastered here first.
                    let at_once = match self.sixel.fitted.get(&key.fit) {
                        Some(fitted) => Some(draw(&key, Some(fitted), None)),
                        None if photo || self.sixel.many_colours.contains(&key.fit) => None,
                        None => {
                            let drawn = draw_at_once(&key, &raster);
                            if drawn.is_none() {
                                if self.sixel.many_colours.len() >= 256 {
                                    self.sixel.many_colours.clear();
                                }
                                self.sixel.many_colours.insert(key.fit.clone());
                            }
                            drawn
                        }
                    };
                    match at_once {
                        Some(made) => {
                            if let Ok((sixel, _)) = &made {
                                frame.pictures.push(Placed {
                                    area: piece.area,
                                    key: key.number(),
                                    sixel: sixel.clone(),
                                });
                            }
                            self.keep_drawn(key, made);
                        }
                        None => self.draw_piece(key, Some(Make::Raster(raster))),
                    }
                }
                make => self.draw_piece(key, make),
            }
        }
    }

    /// The rounded ends of kit's objects in a frame for a terminal that
    /// gets sixel (src/mux/graphics.rs's `Ends::Pictures`), wherever
    /// Switchboard drew them (the server's drawing, an app's in its pane,
    /// a copy of either), never one a program of yours drew in its pane
    /// (`drawn`), which is left to the terminal: each half circle's cell
    /// left a space on its own background and the half circle drawn over it
    /// as sixel in its color, which, a few colors and a cell big, is drawn
    /// on the task and kept; one on the frame's last row, where a sixel
    /// would scroll the screen, a space of its color.
    fn draw_ends(&mut self, frame: &mut Frame, drawn: &place::Drawn) {
        use crate::ui::kit::{CAP_LEFT, CAP_RIGHT};
        let cell = self.picture_cell();
        // The colors the panes' programs draw in: the terminal's own, with
        // the ones `terminal_colors` sets in their place.
        let said = self.pane_colors();
        let area = frame.buf.area;
        let last = area.bottom().saturating_sub(1);
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                let c = &mut frame.buf[(x, y)];
                if drawn.end_by(x, y, c) == Some(false) {
                    continue;
                }
                let left = match c.symbol() {
                    s if s == CAP_LEFT => true,
                    s if s == CAP_RIGHT => false,
                    _ => continue,
                };
                let fill = c.fg;
                c.set_symbol(" ");
                c.fg = ratatui::style::Color::Reset;
                if y == last {
                    c.bg = fill;
                    continue;
                }
                let color = end_color(fill, &said);
                // On the row above the last, where clear rows reaching
                // into the last would have the screen scroll as a picture
                // there would, cut to whole bands as a picture is.
                let whole = y + 1 < last;
                let at = (left, color, cell, whole);
                let sixel = match self.sixel.ends.get(&at) {
                    Some(sixel) => sixel.clone(),
                    None => {
                        let Some(sixel) = end_sixel(left, color, cell, whole) else {
                            continue;
                        };
                        if self.sixel.ends.len() >= 256 {
                            self.sixel.ends.clear();
                        }
                        self.sixel.ends.insert(at, sixel.clone());
                        sixel
                    }
                };
                let mut h = std::hash::DefaultHasher::new();
                ("end", at).hash(&mut h);
                frame.pictures.push(Placed {
                    area: Rect::new(x, y, 1, 1),
                    key: h.finish(),
                    sixel,
                });
            }
        }
    }

    /// What a pane's picture is drawn from, and its data when it was given
    /// inline, while its image is placed: one given by its file while that
    /// is known to be one, looked at when it hasn't been since it was
    /// given, and drawn meanwhile as its file was when last looked at.
    fn pane_fit(&mut self, pane: PaneId, id: u32, cell: (u16, u16)) -> Option<(Fit, Option<Make>)> {
        let image = self
            .sixel
            .images
            .get(&pane)
            .and_then(|m| m.get(id))
            .filter(|i| i.placed)?;
        let given = (image.cols, image.rows);
        let path = match &image.given {
            kitty::Given::Data(data) => {
                let source = Source::Data {
                    serial: image.serial,
                    given,
                };
                return Some((Fit { source, cell }, Some(Make::Data(data.clone()))));
            }
            kitty::Given::Path(path) => path.clone(),
        };
        let (serial, look) = (image.serial, image.look.clone());
        let look = match look {
            Look::Unknown => {
                let before = self.sixel.looks.get(&path).cloned();
                self.look_at(pane, id, serial, path.clone());
                before.unwrap_or(Look::Unknown)
            }
            look => look,
        };
        let fit = Fit {
            source: Source::File { path, look, given },
            cell,
        };
        matches!(
            fit.source,
            Source::File {
                look: Look::File { .. },
                ..
            }
        )
        .then_some((fit, None))
    }

    /// Looks at an image's file off the task: its time and length, or that
    /// it isn't a file.
    fn look_at(&mut self, pane: PaneId, id: u32, serial: u64, path: PathBuf) {
        if tokio::runtime::Handle::try_current().is_err()
            || !self.sixel.looking.insert((pane, id, serial))
        {
            return;
        }
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let look = match std::fs::metadata(&path) {
                Ok(m) if m.is_file() => Look::File {
                    modified: m
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_nanos()),
                    len: m.len(),
                },
                _ => Look::Missing,
            };
            let _ = tx.send(Event::Sixel(SixelEvent::Looked {
                pane,
                id,
                serial,
                path,
                look,
            }));
        });
    }

    /// Draws a piece off the task, unless it is being drawn, its picture
    /// failed, or as many as are drawn at once are under way; a picture
    /// being fitted for another piece is waited for, and fitted once.
    /// `make` makes a picture not given by its file.
    fn draw_piece(&mut self, key: Key, make: Option<Make>) {
        if tokio::runtime::Handle::try_current().is_err()
            || self.sixel.drawing.len() >= AT_ONCE
            || self.sixel.drawing.contains(&key)
            || self.sixel.failed.contains(&key.fit)
            || self.sixel.failed_pieces.contains(&key)
        {
            return;
        }
        let fitted = self.sixel.fitted.get(&key.fit);
        if fitted.is_none() && !self.sixel.fitting.insert(key.fit.clone()) {
            return;
        }
        self.sixel.drawing.insert(key.clone());
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let made = draw(&key, fitted, make.as_ref());
            let _ = tx.send(Event::Sixel(SixelEvent::Drawn { key, made }));
        });
    }

    /// A piece drawn kept, or why it couldn't be, so it isn't tried again.
    fn keep_drawn(&mut self, key: Key, made: Made) {
        match made {
            Ok((sixel, fitted)) => {
                let bytes = fitted.bytes();
                self.sixel.fitted.insert(key.fit.clone(), fitted, bytes);
                let bytes = sixel.len();
                self.sixel.encoded.insert(key, sixel, bytes);
            }
            Err(Failure::Picture(e)) => {
                eprintln!("switchboard server: a picture couldn't be drawn as sixel: {e}");
                if self.sixel.failed.len() >= 256 {
                    self.sixel.failed.clear();
                }
                self.sixel.failed.insert(key.fit);
            }
            Err(Failure::Piece(e)) => {
                eprintln!("switchboard server: a picture couldn't be drawn as sixel: {e}");
                if self.sixel.failed_pieces.len() >= 256 {
                    self.sixel.failed_pieces.clear();
                }
                self.sixel.failed_pieces.insert(key);
            }
        }
    }

    pub fn sixel_event(&mut self, event: SixelEvent) {
        match event {
            SixelEvent::Looked {
                pane,
                id,
                serial,
                path,
                look,
            } => {
                self.sixel.looking.remove(&(pane, id, serial));
                if self.sixel.looks.len() >= 1024 {
                    self.sixel.looks.clear();
                }
                self.sixel.looks.insert(path, look.clone());
                let Some(image) = self
                    .sixel
                    .images
                    .get_mut(&pane)
                    .and_then(|m| m.get_mut(id))
                    .filter(|i| i.serial == serial)
                else {
                    return;
                };
                image.look = look;
            }
            SixelEvent::Drawn { key, made } => {
                self.sixel.drawing.remove(&key);
                self.sixel.fitting.remove(&key.fit);
                self.keep_drawn(key, made);
            }
        }
        for c in self.clients.values_mut() {
            c.dirty = true;
        }
    }

    /// Each tick: the images of panes gone forgotten, and, when the
    /// setting has changed, every screen drawn again whole (taking away
    /// the pictures of the protocol before) and, for sixel, each terminal
    /// that gets it asked for its cell.
    pub(super) fn sixel_tick(&mut self) {
        let panes = &self.panes;
        self.sixel.images.retain(|pane, _| panes.contains_key(pane));
        let chosen = graphics::chosen();
        if chosen == self.sixel.chosen {
            return;
        }
        self.sixel.chosen = chosen;
        for c in self.clients.values_mut() {
            c.screen.invalidate();
            c.dirty = true;
            if graphics::for_panes(&c.term) == Protocol::Sixel {
                let _ =
                    c.tx.send(super::proto::ServerMsg::Output(CELL_QUERY.to_vec()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cache_keeps_to_its_size_and_lets_go_of_what_was_used_longest_ago() {
        let mut lru: Lru<u32, &str> = Lru::new(10);
        lru.insert(1, "a", 4);
        lru.insert(2, "b", 4);
        assert_eq!(lru.get(&1), Some("a"), "1 used since 2");
        lru.insert(3, "c", 4);
        assert_eq!(lru.get(&2), None, "2 went, used longest ago");
        assert_eq!((lru.get(&1), lru.get(&3)), (Some("a"), Some("c")));
        assert_eq!(lru.bytes, 8);
        lru.insert(3, "c2", 2);
        assert_eq!(lru.bytes, 6, "a value replaced counts once");
        lru.insert(4, "huge", 50);
        assert_eq!(
            lru.entries.len(),
            1,
            "one larger than the whole is kept alone"
        );
    }

    #[test]
    fn a_piece_is_drawn_from_its_file_a_whole_number_of_bands_tall() {
        let dir = std::env::temp_dir().join(format!("sb-sixel-draw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("page.png");
        image::RgbaImage::from_pixel(40, 40, image::Rgba([0, 0, 255, 255]))
            .save(&png)
            .unwrap();
        let key = Key {
            fit: Fit {
                source: Source::File {
                    path: png.clone(),
                    look: Look::File {
                        modified: 1,
                        len: 1,
                    },
                    given: (Some(4), Some(2)),
                },
                cell: (10, 20),
            },
            crop: (1, 1, 2, 1),
        };
        let (sixel, fitted) = draw(&key, None, None).unwrap();
        let (w, h, pixels) = encode::tests::decode(&sixel);
        assert_eq!(
            (w, h),
            (20, 18),
            "two cells wide, a cell's rows to the band"
        );
        assert!(pixels.iter().all(|p| *p == Some([0, 0, 255])));
        // The fitted picture serves the next piece without the file.
        std::fs::remove_file(&png).unwrap();
        let other = Key {
            crop: (0, 0, 4, 2),
            ..key.clone()
        };
        let (sixel, _) = draw(&other, Some(fitted.clone()), None).unwrap();
        assert_eq!(encode::tests::decode(&sixel).0, 40);
        let too_wide = Key {
            crop: (0, 0, 2000, 1),
            ..key.clone()
        };
        assert!(
            matches!(
                draw(&too_wide, Some(fitted.clone()), None),
                Err(Failure::Piece(_))
            ),
            "the piece alone, not its picture"
        );
        assert!(
            matches!(draw(&key, None, None), Err(Failure::Picture(_))),
            "the file gone, none to read"
        );
        assert_ne!(key.number(), other.number());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rounded_end_takes_the_terminal_s_own_palette_where_it_said_it() {
        use alacritty_terminal::vte::ansi::Rgb;
        use ratatui::style::Color;
        let mut said = crate::mux::terminal::Colors::default();
        said.ansi[2] = Some(Rgb {
            r: 19,
            g: 161,
            b: 14,
        });
        said.fg = Some(Rgb {
            r: 204,
            g: 204,
            b: 204,
        });
        assert_eq!(end_color(Color::Indexed(2), &said), [19, 161, 14]);
        assert_eq!(end_color(Color::Green, &said), [19, 161, 14]);
        assert_eq!(end_color(Color::Reset, &said), [204, 204, 204]);
        assert_eq!(
            end_color(Color::Indexed(3), &said),
            [205, 205, 0],
            "not said"
        );
        assert_eq!(end_color(Color::Rgb(1, 2, 3), &said), [1, 2, 3]);
        assert_eq!(
            end_color(Color::Indexed(52), &said),
            crate::ui::images::rgb(Color::Indexed(52))
        );
    }

    #[test]
    fn a_rounded_end_is_one_cell_of_sixel_in_its_color() {
        let sixel = end_sixel(true, [0, 0, 255], (10, 20), true).unwrap();
        let (w, h, pixels) = encode::tests::decode(&sixel);
        assert_eq!((w, h), (10, 24), "a cell, rounded up to whole bands");
        assert!(pixels.iter().flatten().all(|p| *p == [0, 0, 255]));
        assert_eq!(pixels[0], None, "its far corner clear");
        assert_eq!(
            pixels[9 * 10 + 9],
            Some([0, 0, 255]),
            "solid at its flat side"
        );
        assert_eq!(
            pixels[19 * 10 + 9],
            Some([0, 0, 255]),
            "its foot, on the cell's last row"
        );
        assert!(
            pixels[20 * 10..].iter().all(Option::is_none),
            "clear past the cell"
        );
        // Above the screen's last row, kept inside its cell.
        let cut = end_sixel(true, [0, 0, 255], (10, 20), false).unwrap();
        assert_eq!(encode::tests::decode(&cut).1, 18);
    }

    #[test]
    fn a_handover_carries_the_newest_pictures_given_inline_across_every_pane() {
        let mut sixel = Sixel::default();
        let give = |sixel: &mut Sixel, pane: u32, id: u32, len: usize, serial: u64| {
            let c = format!(
                "\x1b_Ga=T,U=1,f=100,t=d,q=2,i={id},c=1,r=1;{}\x1b\\",
                "A".repeat(len)
            );
            let images = sixel.images.entry(PaneId(pane)).or_default();
            images.apply(&kitty::parse(c.as_bytes()).unwrap(), serial, true);
        };
        // The pane opened first holds two older pictures, the one in front
        // a newer one; together more than the handover carries.
        give(&mut sixel, 1, 10, 400 << 10, 1);
        give(&mut sixel, 1, 11, 400 << 10, 2);
        give(&mut sixel, 7, 70, 300 << 10, 3);
        let handed = sixel.hand_over();
        let ids: Vec<(PaneId, Vec<u32>)> = handed
            .iter()
            .map(|(pane, images)| (*pane, images.iter().map(|h| h.id).collect()))
            .collect();
        assert_eq!(
            ids,
            vec![(PaneId(1), vec![11]), (PaneId(7), vec![70])],
            "the oldest left out, whatever its pane"
        );
    }

    #[test]
    fn a_picture_given_inline_is_drawn_from_its_data_named_by_its_transmission() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(40, 40, image::Rgba([255, 0, 0, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let key = Key {
            fit: Fit {
                source: Source::Data {
                    serial: 3,
                    given: (Some(2), Some(1)),
                },
                cell: (10, 20),
            },
            crop: (0, 0, 2, 1),
        };
        let make = Make::Data(crate::util::base64(&png).into_bytes().into());
        let (sixel, _) = draw(&key, None, Some(&make)).unwrap();
        let (w, _, pixels) = encode::tests::decode(&sixel);
        assert_eq!(w, 20);
        assert!(pixels.iter().flatten().all(|p| *p == [255, 0, 0]));
        assert!(
            matches!(draw(&key, None, None), Err(Failure::Picture(_))),
            "no data, nothing drawn"
        );
        for not_png in [&b"bm8gcGljdHVyZQ=="[..], b"not base64 at all!"] {
            assert!(matches!(
                draw(&key, None, Some(&Make::Data(Arc::from(not_png)))),
                Err(Failure::Picture(_))
            ));
        }
    }

    #[test]
    fn one_of_the_server_s_images_is_drawn_from_its_raster_at_its_cells() {
        // A switch, on: four cells by one, its track in the done tone on
        // the right, the rest of its row clear.
        let tones = crate::ui::images::Tones::now();
        let raster: Draw =
            Arc::new(move || crate::ui::images::raster_switch(true, (10, 20), &tones));
        let fit = Fit {
            source: Source::Server {
                key: 7,
                cols: 4,
                rows: 1,
            },
            cell: (10, 20),
        };
        assert!(small(&fit), "a switch is drawn as the frame is");
        let whole = Key {
            fit: fit.clone(),
            crop: (0, 0, 4, 1),
        };
        let (sixel, fitted) = draw(&whole, None, Some(&Make::Raster(raster.clone()))).unwrap();
        let (w, h, pixels) = encode::tests::decode(&sixel);
        assert_eq!((w, h), (40, 18), "its cells, made whole bands");
        // Sixel gives colors in percent, so they come back near, not same.
        let done = crate::ui::images::rgb(tones.done);
        let near = |p: &[u8; 3]| (0..3).all(|i| p[i].abs_diff(done[i]) <= 3);
        assert!(pixels.iter().flatten().any(near), "its track drawn");
        assert_eq!(pixels[0], None, "its corner clear");
        // Its right half alone, from the picture fitted once.
        let half = Key {
            fit: fit.clone(),
            crop: (2, 0, 2, 1),
        };
        let (sixel, _) = draw(&half, Some(fitted), None).unwrap();
        assert_eq!(encode::tests::decode(&sixel).0, 20);
        // Nothing to raster it with, nothing drawn.
        assert!(matches!(draw(&whole, None, None), Err(Failure::Picture(_))));
        // A picture larger than drawing at once allows, as the picture
        // shown full size is.
        let large = Fit {
            source: Source::Server {
                key: 8,
                cols: 200,
                rows: 50,
            },
            cell: (10, 20),
        };
        assert!(!small(&large));
    }

    #[test]
    fn a_small_image_of_few_colours_is_drawn_at_once_and_a_photo_is_not() {
        let fit = |key| Fit {
            source: Source::Server {
                key,
                cols: 4,
                rows: 2,
            },
            cell: (10, 20),
        };
        let whole = |key| Key {
            fit: fit(key),
            crop: (0, 0, 4, 2),
        };
        let tones = crate::ui::images::Tones::now();
        let switch: Draw =
            Arc::new(move || crate::ui::images::raster_switch(true, (10, 20), &tones));
        let (sixel, fitted) = draw_at_once(&whole(1), &switch)
            .expect("drawn at once")
            .unwrap();
        assert!(encode::tests::decode(&sixel).0 == 40);
        // A photo's thousands of colours wait for a palette off the task.
        let photo: Draw = Arc::new(|| {
            let mut pm = tiny_skia::Pixmap::new(40, 40).unwrap();
            for (i, p) in pm.pixels_mut().iter_mut().enumerate() {
                let (x, y) = ((i % 40) as u8, (i / 40) as u8);
                *p = tiny_skia::ColorU8::from_rgba(x * 6, y * 6, x ^ y, 255).premultiply();
            }
            pm
        });
        assert!(draw_at_once(&whole(2), &photo).is_none());
        // Once fitted, a piece of it is only encoded.
        let half = Key {
            fit: fit(1),
            crop: (0, 0, 2, 1),
        };
        let (sixel, _) = draw(&half, Some(fitted), None).unwrap();
        assert_eq!(encode::tests::decode(&sixel).0, 20);
    }
}
