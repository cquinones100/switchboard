//! How every picture Switchboard draws reaches a terminal: the one place
//! that decides it, and the one that writes kitty's graphics commands.
//!
//! A picture is one of two kinds. The server's own (src/ui/images.rs:
//! UX 2.0's image atoms, board 6's lane and chart, mission control's
//! diagrams and Slack pictures, the picture shown full size by
//! src/ui/picture.rs) are described into a frame: each writes kitty's
//! Unicode placeholder cells where it goes (U+10EEEE, marked with their
//! row and column and colored with its id) and adds a `Placed` to the
//! frame's `images`, saying how to raster it, or draws its text form where
//! the frame carries no `Graphics`. The panes' (Notes, the image view, the
//! PR and ticket views: src/notes/images.rs, src/views/) are given by a
//! pane's program as kitty's graphics commands and its placeholder cells,
//! which reach the server through the pane, only from the panes allowed to
//! send them (src/mux/passthrough.rs). Either way a picture's placeholders
//! ride in the frame as text, so it is placed, scrolled, cut off and
//! covered exactly as they are.
//!
//! Which way it reaches a terminal is chosen here, never by a caller, for
//! each terminal attached by what it says it is (`for_terminal`):
//!
//! - kitty (`Protocol::Kitty`), for Ghostty and kitty whatever is chosen:
//!   the panes' commands passed on as they came, and the server's images
//!   sent inline (`t=d`) by each client's `ImageCache`, placed by their
//!   placeholders, which the terminal draws them on, or, for the shadow
//!   under a box, directly under the text (`z=-1`);
//! - sixel (`Protocol::Sixel`), for any other terminal while Settings'
//!   "Image protocol" (`image_protocol`) is on `sixel`, as Windows Terminal
//!   and the VT340's other heirs want: the panes' commands kept rather than
//!   passed on, and every picture of either kind drawn by the server as
//!   sixel where its placeholders still show once everything is drawn,
//!   beside the frame's cells, the placeholders blanked (src/mux/sixel). A
//!   sixel is drawn over its cells and can't sit under text, so the shadow
//!   under a box is its text form there, half blocks along the box's edges;
//! - neither: each of the server's images is its text form (any other
//!   terminal while kitty is chosen, or Settings' `server_images` off), the
//!   panes' commands are still passed on, as a terminal drops a string it
//!   doesn't know, and the apps in panes draw their pictures as text
//!   (`apps_draw`).
//!
//! So a Ghostty and a Windows Terminal attached at once each get the
//! pictures their own way, and a reader of the screen that takes kitty's
//! (`switchboard headless`) still does with sixel chosen.
//!
//! Nothing else writes kitty's graphics commands or a sixel: this module
//! writes the one, src/mux/sixel/encode.rs the other, and
//! tests/it/graphics.rs fails on any other file in src/ whose source writes
//! either out (or any DCS, tmux's passthrough among them), as an escape in
//! a string or the byte in an array. The server's images are
//! always sent with their data inline, never a file's path, which would
//! have the terminal read any file of yours: only the pictures Notes and
//! the image view give by their PNG's path (`by_path`) name one, and only
//! their own panes may pass those on.
//!
//! Each client keeps what its terminal was sent (`ImageCache`, in its
//! `compose::Screen`), by a stable id its caller names, with a key made of
//! what the image is drawn from: one whose key is unchanged isn't sent
//! again, one that changed replaces the old under the same id, and one no
//! longer placed is deleted, so the terminal never collects images.
//! Anything that may have cleared the terminal (an attach, a resize,
//! `Screen::invalidate`, a new build after `switchboard upgrade`, whose
//! screen starts empty) sends them all again, as it does the sixels.
//!
//! New with the Rust port, with no tmux-version file behind it (tmux passed
//! the notes pane's commands on and the server drew no pictures of its
//! own): its code was gathered here from where each kind of picture chose
//! its own way, `ImageCache` and its commands from src/ui/images.rs, the
//! commands given by a path and tmux's wrapping from src/notes/images.rs
//! and src/views/image/mod.rs, and `apps_draw` from Notes' `terminal_can`.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::server::{Client, Server};
use crate::ui::images::{Placed, Spot, png};
use crate::util;

/// How pictures reach a terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Protocol {
    /// kitty's graphics protocol, placed by Unicode placeholders.
    Kitty,
    /// Sixel, drawn by the server over the placeholders' cells.
    Sixel,
}

/// How a frame's images reach its client: by which protocol, and the
/// cell's size in the pixels they are rastered at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Graphics {
    pub protocol: Protocol,
    pub cell: (u16, u16),
}

impl Graphics {
    pub fn kitty(cell: (u16, u16)) -> Graphics {
        Graphics {
            protocol: Protocol::Kitty,
            cell,
        }
    }

    pub fn sixel(cell: (u16, u16)) -> Graphics {
        Graphics {
            protocol: Protocol::Sixel,
            cell,
        }
    }

    /// Whether a picture can sit under the frame's text (the shadow under
    /// a box): kitty places one there; a sixel is drawn over its cells.
    pub fn under_text(&self) -> bool {
        self.protocol == Protocol::Kitty
    }
}

/// Whether pictures are drawn as sixel (Settings' `image_protocol`), from
/// the settings as the server last read them.
pub fn sixel_chosen() -> bool {
    crate::settings::cached("image_protocol").is_some_and(|v| v == "sixel")
}

/// The same from the settings file as it is now, for a program that reads
/// them once (Notes, the views).
pub fn sixel_chosen_now() -> bool {
    crate::settings::setting("image_protocol").is_some_and(|v| v == "sixel")
}

/// Whether a terminal, by its `TERM` or `TERM_PROGRAM`, speaks kitty's
/// graphics protocol with placeholders: Ghostty and kitty do.
pub fn speaks_kitty(term: &str) -> bool {
    let term = term.to_lowercase();
    term.contains("ghostty") || term.contains("kitty")
}

/// How pictures reach a terminal that says it is `term` (its `TERM`): by
/// kitty's protocol where it speaks it, whatever is chosen; else as sixel
/// while that is chosen; else neither.
pub fn for_terminal(term: &str) -> Option<Protocol> {
    way(speaks_kitty(term), sixel_chosen())
}

/// The way for a terminal that speaks kitty's protocol or not, with sixel
/// chosen or not.
fn way(kitty: bool, sixel: bool) -> Option<Protocol> {
    if kitty {
        Some(Protocol::Kitty)
    } else if sixel {
        Some(Protocol::Sixel)
    } else {
        None
    }
}

/// How the pictures of the panes allowed to send them reach a terminal:
/// their commands passed on (to one that draws neither protocol too, which
/// drops them), or, for one that gets sixel, kept and drawn by the server.
pub fn for_panes(term: &str) -> Protocol {
    match for_terminal(term) {
        Some(Protocol::Sixel) => Protocol::Sixel,
        _ => Protocol::Kitty,
    }
}

/// How the server's own images reach a terminal: as `for_terminal` says,
/// with Settings' `server_images` on; `None`, their text forms, otherwise.
pub fn for_server(term: &str) -> Option<Protocol> {
    if crate::settings::is_on("server_images") {
        for_terminal(term)
    } else {
        None
    }
}

/// Whether an app in a pane (Notes, the views) draws its pictures: the
/// terminal it runs in speaks kitty's protocol (Ghostty or kitty, by
/// `$TERM` or `$TERM_PROGRAM`; the multiplexer's panes keep the terminal
/// the workspace started in as `TERM_PROGRAM`), or, inside the workspace
/// with sixel chosen, the server draws them as sixel where their
/// placeholders are, whatever the terminal is called.
pub fn apps_draw() -> bool {
    speaks_kitty(&format!(
        "{} {}",
        std::env::var("TERM").unwrap_or_default(),
        std::env::var("TERM_PROGRAM").unwrap_or_default()
    )) || (super::socket::in_pane() && sixel_chosen_now())
}

impl Server {
    /// How the server's images reach a client, and the cell they are
    /// rastered at: for kitty the cell the terminal's window makes (or the
    /// panes' stand-in), for sixel the cell its sixel works in
    /// (`picture_cell`). `None` draws their text forms.
    pub fn graphics_of(&self, c: &Client) -> Option<Graphics> {
        Some(match for_server(&c.term)? {
            Protocol::Kitty => Graphics::kitty(self.terminal.cell.unwrap_or(super::pane::CELL)),
            Protocol::Sixel => Graphics::sixel(self.picture_cell()),
        })
    }
}

// ── kitty's graphics commands ───────────────────────────────────────────────

/// One kitty graphics command carrying `data` base64'd in chunks of 4096
/// bytes, `keys` on the first; with no data, the keys alone.
fn command(keys: &str, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    if data.is_empty() {
        out.extend_from_slice(format!("\x1b_G{keys}\x1b\\").as_bytes());
        return out;
    }
    let encoded = util::base64(data);
    let chunks: Vec<&[u8]> = encoded.as_bytes().chunks(4096).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        out.extend_from_slice(b"\x1b_G");
        if i == 0 {
            out.extend_from_slice(format!("{keys},m={more};").as_bytes());
        } else {
            out.extend_from_slice(format!("m={more};").as_bytes());
        }
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\x1b\\");
    }
    out
}

fn delete(id: u32) -> Vec<u8> {
    command(&format!("a=d,d=I,i={id},q=2"), &[])
}

/// The images one terminal holds, by id, each with its key: a client's
/// (in its `compose::Screen`), or a view's in its pane (src/views/pictures.rs).
#[derive(Default, Debug)]
pub struct ImageCache {
    sent: HashMap<u32, u64>,
}

impl ImageCache {
    /// The commands that bring the terminal from what it holds to what the
    /// frame places: an id new, or placed with another key, is sent (the
    /// old under it deleted first), and one no longer placed is deleted.
    pub fn commands(&mut self, placed: &[Placed]) -> Vec<u8> {
        let mut out = Vec::new();
        let now: HashMap<u32, &Placed> = placed.iter().map(|p| (p.id, p)).collect();
        let mut gone: Vec<u32> = self
            .sent
            .keys()
            .filter(|id| !now.contains_key(id))
            .copied()
            .collect();
        gone.sort_unstable();
        for id in gone {
            out.extend(delete(id));
            self.sent.remove(&id);
        }
        let mut done = HashSet::new();
        for p in placed {
            if !done.insert(p.id) || self.sent.get(&p.id) == Some(&p.key) {
                continue;
            }
            if self.sent.contains_key(&p.id) {
                out.extend(delete(p.id));
            }
            let data = png(&(p.draw)());
            match p.spot {
                Spot::Placeholders => out.extend(command(
                    &format!("a=T,U=1,f=100,t=d,q=2,i={},c={},r={}", p.id, p.cols, p.rows),
                    &data,
                )),
                Spot::Under { x, y, dx, dy } => {
                    out.extend_from_slice(format!("\x1b7\x1b[{};{}H", y + 1, x + 1).as_bytes());
                    out.extend(command(
                        &format!("a=T,f=100,t=d,q=2,i={},X={dx},Y={dy},z=-1,C=1", p.id),
                        &data,
                    ));
                    out.extend_from_slice(b"\x1b8");
                }
            }
            self.sent.insert(p.id, p.key);
        }
        out
    }

    /// Every image deleted, and nothing taken as held, so the next frame
    /// sends them all again (the terminal may have lost them).
    pub fn forget(&mut self) -> Vec<u8> {
        let mut ids: Vec<u32> = self.sent.drain().map(|(id, _)| id).collect();
        ids.sort_unstable();
        ids.into_iter().flat_map(delete).collect()
    }
}

/// What Notes and the image view give the terminal to show a PNG by its
/// path: a virtual placement `id`, `cols` by `rows` cells, drawn where its
/// placeholders are. Only their panes may pass it on (a command naming a
/// file has the terminal read it), and with sixel chosen the server keeps
/// it and draws the picture itself (src/mux/sixel/kitty.rs reads it).
pub fn by_path(id: u32, cols: u16, rows: u16, png: &Path) -> String {
    for_tmux(&given_by_path(id, cols, rows, png), in_tmux())
}

fn given_by_path(id: u32, cols: u16, rows: u16, png: &Path) -> String {
    format!(
        "\x1b_Ga=T,U=1,f=100,t=f,q=2,i={id},c={cols},r={rows};{}\x1b\\",
        util::base64(png.to_string_lossy().as_bytes())
    )
}

/// What takes back a picture an app gave by `by_path`.
pub fn take_back(id: u32) -> String {
    for_tmux(&taken_back(id), in_tmux())
}

fn taken_back(id: u32) -> String {
    format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\")
}

/// Whether the app runs inside tmux, which passes a command on to the
/// terminal only wrapped in its passthrough.
fn in_tmux() -> bool {
    std::env::var_os("TMUX").is_some_and(|t| !t.is_empty())
}

/// A command as it reaches the terminal: wrapped in tmux's passthrough,
/// each of its ESCs doubled, inside tmux.
fn for_tmux(seq: &str, inside: bool) -> String {
    if inside {
        format!("\x1bPtmux;{}\x1b\\", seq.replace('\x1b', "\x1b\x1b"))
    } else {
        seq.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::compose::Frame;
    use crate::ui::images::{id_of, meter, picture, shadow};
    use image::RgbaImage;
    use ratatui::layout::Rect;
    use ratatui::style::Color;

    fn graphics_frame(cols: u16, rows: u16) -> Frame {
        let mut f = Frame::new(cols, rows);
        f.graphics = Some(Graphics::kitty((8, 16)));
        f
    }

    fn text(out: &[u8]) -> String {
        String::from_utf8_lossy(out).into_owned()
    }

    fn red() -> Color {
        Color::Rgb(220, 50, 50)
    }

    #[test]
    fn a_kitty_or_ghostty_terminal_speaks_the_protocol_and_others_do_not() {
        assert!(speaks_kitty("xterm-ghostty"));
        assert!(speaks_kitty("xterm-kitty"));
        assert!(speaks_kitty("ghostty"));
        assert!(!speaks_kitty("xterm-256color"));
        assert!(!speaks_kitty("tmux-256color"));
        assert!(!speaks_kitty(""));
    }

    #[test]
    fn a_kitty_terminal_keeps_kitty_s_way_and_another_gets_sixel_only_when_chosen() {
        assert_eq!(way(true, false), Some(Protocol::Kitty));
        assert_eq!(
            way(true, true),
            Some(Protocol::Kitty),
            "Ghostty with sixel chosen"
        );
        assert_eq!(way(false, true), Some(Protocol::Sixel), "Windows Terminal");
        assert_eq!(way(false, false), None);
    }

    #[test]
    fn only_kitty_places_a_picture_under_the_text() {
        assert!(Graphics::kitty((8, 16)).under_text());
        assert!(!Graphics::sixel((10, 20)).under_text());
    }

    #[test]
    fn the_cache_sends_once_replaces_a_change_and_deletes_what_goes() {
        let mut cache = ImageCache::default();
        let mut f = graphics_frame(20, 3);
        meter(&mut f, "m", Rect::new(0, 0, 10, 1), 0.4, red(), None);
        let id = id_of("m");
        let first = text(&cache.commands(&f.images));
        assert!(first.starts_with(&format!("\x1b_Ga=T,U=1,f=100,t=d,q=2,i={id},c=10,r=1,m=")));
        assert_eq!(first.matches("a=T").count(), 1);

        let mut same = graphics_frame(20, 3);
        meter(&mut same, "m", Rect::new(0, 0, 10, 1), 0.4, red(), None);
        assert!(cache.commands(&same.images).is_empty());

        let mut changed = graphics_frame(20, 3);
        meter(&mut changed, "m", Rect::new(0, 0, 10, 1), 0.9, red(), None);
        let again = text(&cache.commands(&changed.images));
        assert!(again.starts_with(&format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\\x1b_Ga=T")));

        let empty = graphics_frame(20, 3);
        assert_eq!(
            text(&cache.commands(&empty.images)),
            format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\")
        );
        assert!(cache.commands(&empty.images).is_empty());

        cache.commands(&f.images);
        assert!(!cache.forget().is_empty());
        assert!(cache.commands(&f.images).windows(3).any(|w| w == b"a=T"));
    }

    #[test]
    fn a_large_image_is_sent_in_chunks_and_every_command_carries_its_data_inline() {
        let mut cache = ImageCache::default();
        let mut f = graphics_frame(60, 20);
        let noise = RgbaImage::from_fn(200, 200, |x, y| {
            image::Rgba([
                (x * 7 % 256) as u8,
                (y * 13 % 256) as u8,
                ((x ^ y) % 256) as u8,
                255,
            ])
        });
        picture(&mut f, "p", Rect::new(0, 0, 40, 15), "shot.png", &noise);
        shadow(&mut f, "box", Rect::new(5, 5, 10, 4));
        let out = text(&cache.commands(&f.images));
        let commands: Vec<&str> = out.split("\x1b_G").skip(1).collect();
        assert!(commands.len() > 2);
        assert!(out.contains(",m=1;"));
        assert!(commands.last().unwrap().starts_with("m=0;"));
        for c in &commands {
            let keys = c.split(';').next().unwrap();
            assert!(!keys.contains("t=f") && !keys.contains("t=t") && !keys.contains("t=s"));
        }
        assert!(out.contains("z=-1,C=1"));
    }

    #[test]
    fn an_app_gives_a_picture_by_its_path_and_takes_it_back_by_its_id() {
        let given = given_by_path(7, 4, 2, Path::new("/tmp/n/a.png"));
        let c = crate::mux::sixel::kitty::parse(given.as_bytes()).expect("one whole command");
        assert_eq!(
            (c.action, c.id, c.medium, c.format, c.cols, c.rows),
            (b'T', Some(7), b'f', 100, Some(4), Some(2))
        );
        assert!(c.virtual_placement);
        assert_eq!(util::unbase64(&c.payload).unwrap(), b"/tmp/n/a.png");
        let back = crate::mux::sixel::kitty::parse(taken_back(7).as_bytes()).unwrap();
        assert_eq!((back.action, back.delete, back.id), (b'd', b'I', Some(7)));
        // Inside tmux, wrapped in its passthrough, each ESC doubled.
        assert_eq!(for_tmux(&given, false), given);
        assert_eq!(
            for_tmux("\x1b_Gi=1\x1b\\", true),
            "\x1bPtmux;\x1b\x1b_Gi=1\x1b\x1b\\\x1b\\"
        );
    }
}
