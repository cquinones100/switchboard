//! Pictures as sixel, for a terminal that draws sixel but not kitty's
//! graphics protocol: Windows Terminal (1.22 and later), and the rest of
//! the VT340's heirs (foot, WezTerm, xterm with sixel on, mlterm). Settings'
//! "Image protocol" (`image_protocol`) chooses: `kitty`, the default, passes
//! a pane's graphics commands on to the terminal as before
//! (src/mux/passthrough.rs); `sixel` keeps them here and draws each
//! picture itself.
//!
//! The apps stay as they are. Notes and the image view give the terminal
//! each picture's PNG by its path as a kitty virtual placement and write
//! its placeholder cells (src/notes/images.rs), which the panes' emulators
//! keep as text; with sixel chosen, `images::terminal_can` lets them draw
//! inside the workspace whatever the terminal is called. The server reads
//! the commands from the panes allowed to send them, as before, and keeps
//! what each image is (`kitty`: its path and placement), forgetting it as
//! they delete it, never forwarding them. As each frame is composed, the
//! placeholder cells those panes drew are noted before anything is drawn
//! over them, and after everything is, every placeholder cell in the
//! frame is blanked and those still as their pane drew them are gathered
//! into pieces of one picture each (`place`), and each piece is drawn as
//! sixel beside the frame's cells (`compose::Frame::pictures`, as the
//! hyperlinks ride beside them), which `compose::Screen` writes over its
//! cells, so a picture is placed, scrolled, cut off and covered exactly as
//! its placeholders are, and a copy of them (UX 2.0's mission control
//! cards show each tab's last lines, a peek a tab's screen) is never
//! taken for the picture.
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
//! stay blank until then. The fitted picture and each piece's sixel are
//! kept, each cache held to a size (`Lru`), the piece's under its file's
//! path, time and length, its placement, the part of it shown and the
//! cell's size, so a piece shown again (a page turned back to, a box
//! closed) is drawn at once, and one cut anew (a scroll) only encoded. A
//! picture given again (Notes gives its pictures again as it gets the
//! focus) is drawn by what its file was when last looked at until it has
//! been looked at again, so it doesn't blink, and a file changed since is
//! drawn anew once it has.
//!
//! A cell's size in the terminal's sixel pixels is asked of each terminal
//! as it attaches while sixel is chosen (`CSI 16 t`, answered as `CSI 6 ;
//! <height> ; <width> t` among its keys, src/mux/terminal.rs). Windows
//! Terminal answers with the virtual cell its sixel works in, 10 by 20
//! pixels whatever its font (`SixelParser::CellSizeForLevel`, as the VT340
//! had; it scales each cell's part of a picture to the font's cell), so a
//! picture drawn at 10 by 20 a cell fills its cells there. Otherwise the
//! window's pixels the terminal said serve, else that same 10 by 20. The
//! panes are still given the cell its window's pixels make (or 8 by 16),
//! so the apps size and lay pictures out as before, the image view a page
//! as wide as the text around it; a picture is scaled to its cells either
//! way, its shape kept.
//!
//! What a pane's program gave is part of what `switchboard upgrade` hands
//! the new build (src/mux/upgrade.rs), since a program gives a picture
//! once; the caches are made again.

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
use super::server::{Event, Server};
use super::{PaneId, TabId};
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

/// Whether pictures are drawn as sixel (Settings' `image_protocol`), from
/// the settings as the server last read them.
pub fn chosen() -> bool {
    crate::settings::cached("image_protocol").is_some_and(|v| v == "sixel")
}

/// The same from the settings file as it is now, for a program that reads
/// them once (Notes, the image view).
pub fn chosen_now() -> bool {
    crate::settings::setting("image_protocol").is_some_and(|v| v == "sixel")
}

/// A fitted picture's name: its file as looked at, its placement's columns
/// and rows as given, and the cell's size in pixels.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Fit {
    pub path: PathBuf,
    pub look: Look,
    pub given: (Option<u16>, Option<u16>),
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

/// Draws a piece, off the server's task: its picture read and fitted,
/// unless `fitted` has it, and its part encoded, a whole number of sixel
/// bands tall.
pub fn draw(key: &Key, fitted: Option<Arc<Fitted>>) -> Result<(Arc<[u8]>, Arc<Fitted>), String> {
    let cell = (key.fit.cell.0.max(1) as u32, key.fit.cell.1.max(1) as u32);
    let fitted = match fitted {
        Some(f) => f,
        None => {
            let image = encode::read_png(&key.fit.path)?;
            let cells = encode::cells_of(key.fit.given, image.dimensions(), cell);
            Arc::new(encode::prepare(&image, cells, cell)?)
        }
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
        return Err("a piece too large to draw".into());
    }
    let sixel = encode::encode(&fitted, crop);
    if sixel.len() > MOST_BYTES {
        return Err(format!(
            "a piece of {} bytes, more than a terminal should be sent at once",
            sixel.len()
        ));
    }
    Ok((sixel.into(), fitted))
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
    Drawn {
        key: Key,
        made: Result<(Arc<[u8]>, Arc<Fitted>), String>,
    },
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
    /// until given again.
    failed: HashSet<Fit>,
    /// Transmissions counted, newest highest.
    serial: u64,
    /// Whether sixel was chosen when last looked, so a change draws every
    /// screen again whole.
    pub on: bool,
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
            serial: 0,
            on: false,
        }
    }
}

impl Sixel {
    /// As a server starts, with the setting as it is.
    pub fn new() -> Sixel {
        Sixel {
            on: chosen(),
            ..Sixel::default()
        }
    }

    /// What a handover carries: each pane's images.
    pub fn hand_over(&self) -> Vec<(PaneId, Vec<kitty::Handed>)> {
        let mut out: Vec<(PaneId, Vec<kitty::Handed>)> = self
            .images
            .iter()
            .map(|(pane, images)| (*pane, images.hand_over()))
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
    /// kept, and the commands passed on to the terminal unless sixel is
    /// chosen, when the server draws the pictures itself.
    pub(super) fn take_graphics(&mut self, pane: PaneId, tab: TabId, commands: Vec<Vec<u8>>) {
        if commands.is_empty() {
            return;
        }
        for command in &commands {
            let Some(c) = kitty::parse(command) else {
                continue;
            };
            self.sixel.serial += 1;
            let serial = self.sixel.serial;
            let images = self.sixel.images.entry(pane).or_default();
            images.apply(&c, serial);
            if images.is_empty() {
                self.sixel.images.remove(&pane);
            }
        }
        if !chosen() {
            self.pass_graphics(tab, commands);
        }
    }

    /// A pane's program may no longer give pictures, or has gone: what it
    /// gave is forgotten.
    pub(super) fn forget_images(&mut self, pane: PaneId) {
        self.sixel.images.remove(&pane);
    }

    /// Once the panes are drawn (`panes`, each drawn and its area on the
    /// screen) and before anything is drawn over them, while sixel is
    /// chosen: the placeholder cells drawn by the panes allowed to send
    /// pictures, as they drew them (`place::Drawn`).
    pub(super) fn pictures_drawn(
        &self,
        frame: &Frame,
        panes: &[(PaneId, Rect)],
    ) -> Option<place::Drawn> {
        if !chosen() {
            return None;
        }
        let allowed = panes
            .iter()
            .filter(|(pane, _)| {
                self.panes
                    .get(pane)
                    .is_some_and(|s| super::passthrough::graphics_allowed(&s.opts))
            })
            .copied()
            .collect();
        Some(place::Drawn::of(&frame.buf, allowed))
    }

    /// The pictures a frame shows, while sixel is chosen, once everything
    /// is drawn: every placeholder cell in it blanked, and each piece of a
    /// picture still showing in its pane as the pane drew it (`drawn`,
    /// from `pictures_drawn`) put beside the frame's cells when ready; one
    /// not ready yet is looked at or drawn off the task.
    pub(super) fn place_pictures(&mut self, frame: &mut Frame, drawn: Option<place::Drawn>) {
        let Some(drawn) = drawn else {
            return;
        };
        let bottom = frame.area().bottom().saturating_sub(1);
        let pieces = place::find(&mut frame.buf, &drawn, bottom);
        if pieces.is_empty() {
            return;
        }
        let cell = self.picture_cell();
        for piece in pieces {
            let Some(image) = self
                .sixel
                .images
                .get(&piece.pane)
                .and_then(|m| m.get(piece.id))
                .filter(|i| i.placed)
            else {
                continue;
            };
            let (path, serial, given, look) = (
                image.path.clone(),
                image.serial,
                (image.cols, image.rows),
                image.look.clone(),
            );
            let look = match look {
                // Not looked at since it was given: looked at now, and
                // drawn meanwhile as its file was when last looked at.
                Look::Unknown => {
                    let before = self.sixel.looks.get(&path).cloned();
                    self.look_at(piece.pane, piece.id, serial, path.clone());
                    before.unwrap_or(Look::Unknown)
                }
                look => look,
            };
            let fit = Fit {
                path,
                look,
                given,
                cell,
            };
            if !matches!(fit.look, Look::File { .. }) {
                continue;
            }
            let key = Key {
                fit,
                crop: (piece.col, piece.row, piece.area.width, piece.area.height),
            };
            match self.sixel.encoded.get(&key) {
                Some(sixel) => frame.pictures.push(Placed {
                    area: piece.area,
                    key: key.number(),
                    sixel,
                }),
                None => self.draw_piece(key),
            }
        }
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
    fn draw_piece(&mut self, key: Key) {
        if tokio::runtime::Handle::try_current().is_err()
            || self.sixel.drawing.len() >= AT_ONCE
            || self.sixel.drawing.contains(&key)
            || self.sixel.failed.contains(&key.fit)
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
            let made = draw(&key, fitted);
            let _ = tx.send(Event::Sixel(SixelEvent::Drawn { key, made }));
        });
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
                match made {
                    Ok((sixel, fitted)) => {
                        let bytes = fitted.bytes();
                        self.sixel.fitted.insert(key.fit.clone(), fitted, bytes);
                        let bytes = sixel.len();
                        self.sixel.encoded.insert(key, sixel, bytes);
                    }
                    Err(e) => {
                        eprintln!("switchboard server: a picture couldn't be drawn as sixel: {e}");
                        if self.sixel.failed.len() >= 256 {
                            self.sixel.failed.clear();
                        }
                        self.sixel.failed.insert(key.fit);
                    }
                }
            }
        }
        for c in self.clients.values_mut() {
            c.dirty = true;
        }
    }

    /// Each tick: the images of panes gone forgotten, and, when the
    /// setting has changed, every screen drawn again whole (taking away
    /// the pictures of the protocol before) and, for sixel, each terminal
    /// asked for its cell.
    pub(super) fn sixel_tick(&mut self) {
        let panes = &self.panes;
        self.sixel.images.retain(|pane, _| panes.contains_key(pane));
        let on = chosen();
        if on == self.sixel.on {
            return;
        }
        self.sixel.on = on;
        for c in self.clients.values_mut() {
            c.screen.invalidate();
            c.dirty = true;
            if on {
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
                path: png.clone(),
                look: Look::File {
                    modified: 1,
                    len: 1,
                },
                given: (Some(4), Some(2)),
                cell: (10, 20),
            },
            crop: (1, 1, 2, 1),
        };
        let (sixel, fitted) = draw(&key, None).unwrap();
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
        let (sixel, _) = draw(&other, Some(fitted)).unwrap();
        assert_eq!(encode::tests::decode(&sixel).0, 40);
        assert!(draw(&key, None).is_err(), "the file gone, none to read");
        assert_ne!(key.number(), other.number());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
