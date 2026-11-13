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
//! - sixel (`Protocol::Sixel`), for any other terminal that says it draws
//!   sixel when asked as it attaches (its DA1 answer, `Chosen::Auto`, the
//!   default), as Windows Terminal from 1.22 and the VT340's other heirs
//!   do, or whatever it says with Settings' "Image protocol"
//!   (`image_protocol`) on `sixel`: the panes' commands kept rather than
//!   passed on, and every picture of either kind drawn by the server as
//!   sixel where its placeholders still show once everything is drawn,
//!   beside the frame's cells, the placeholders blanked (src/mux/sixel). A
//!   sixel is drawn over its cells and can't sit under text, so the shadow
//!   under a box is its text form there, half blocks along the box's edges;
//! - neither: each of the server's images is its text form (any other
//!   terminal, or Settings' `server_images` off), a picture's cells blank
//!   rather than the placeholder's box where the terminal is known never to
//!   draw kitty's (`pictures_blank`), the
//!   panes' commands are still passed on, as a terminal drops a string it
//!   doesn't know, and the apps in panes draw their pictures as text
//!   (`apps_draw`).
//!
//! So a Ghostty and a Windows Terminal attached at once each get the
//! pictures their own way, and a reader of the screen that takes kitty's
//! (`switchboard headless`) still does with sixel chosen.
//!
//! Everything else drawn one way for one terminal and another for the rest
//! is decided here too, so a terminal without a feature gets a stand-in
//! rather than whatever a caller assumed: the rounded ends of kit's objects
//! (a chip, a keycap, a cell, an input), the half circles U+E0B6 and
//! U+E0B4, as the glyphs where the terminal draws them itself (Ghostty),
//! as sixel pictures of them for a terminal that gets sixel (the frame's
//! glyphs read back and drawn as pictures once it is drawn, as its images
//! are, src/mux/sixel), and as a space of the object's fill elsewhere
//! (`ends`); the mouse pointer's shapes, sent only to a terminal that takes
//! them (`pointer_shapes`); and the canvas's truecolor, for a terminal that
//! draws it, Windows Terminal among them by the word its client adds when
//! `WT_SESSION` is set, else the 256-color palette (`truecolor`). Callers
//! ask these and never read a terminal's name; tests/it/graphics.rs fails
//! on any file that does, outside those that say what a terminal is (the
//! client, the headless reader) or set one up.
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

/// What Settings' "Image protocol" (`image_protocol`) chooses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Chosen {
    /// By the terminal: kitty's protocol where it speaks it, sixel where
    /// its answer to "what are you" (DA1) says it draws sixel (Windows
    /// Terminal from 1.22, foot, WezTerm, xterm with sixel, mlterm),
    /// neither elsewhere.
    #[default]
    Auto,
    /// Kitty's protocol where the terminal speaks it, neither elsewhere.
    Kitty,
    /// Sixel for every terminal that doesn't speak kitty's protocol.
    Sixel,
}

fn chosen_of(value: Option<serde_json::Value>) -> Chosen {
    match value.as_ref().and_then(|v| v.as_str()) {
        Some("kitty") => Chosen::Kitty,
        Some("sixel") => Chosen::Sixel,
        _ => Chosen::Auto,
    }
}

/// What is chosen, from the settings as the server last read them.
pub fn chosen() -> Chosen {
    chosen_of(crate::settings::cached("image_protocol"))
}

/// The same from the settings file as it is now, for a program that reads
/// them once (Notes, the views).
pub fn chosen_now() -> Chosen {
    chosen_of(crate::settings::setting("image_protocol"))
}

/// The word the server adds to what a client attached as once its
/// terminal's answer to "what are you" (DA1) says it draws sixel
/// (`Server::terminal_answered`), asked as it attaches.
pub const ANSWERS_SIXEL: &str = "answers-sixel";

/// The word added once that answer says it doesn't.
pub const ANSWERS_NO_SIXEL: &str = "answers-no-sixel";

/// Whether a terminal that doesn't speak kitty's protocol gets sixel: with
/// sixel chosen, or, chosen by the terminal, where its DA1 answer said it
/// draws sixel.
fn wants_sixel(term: &str, chosen: Chosen) -> bool {
    match chosen {
        Chosen::Sixel => true,
        Chosen::Auto => term.split_whitespace().any(|w| w == ANSWERS_SIXEL),
        Chosen::Kitty => false,
    }
}

/// The guess an app in a pane falls back on when the server is too old to
/// say what the session's terminals get (`workspace_draws`), since it can't
/// see their answers: chosen by the terminal, Windows Terminal (by the
/// `WT_SESSION` the panes keep) is taken to draw sixel. Also the server's
/// own count of a Windows Terminal that hasn't answered yet
/// (`may_get_sixel`).
fn apps_want_sixel(term: &str, chosen: Chosen) -> bool {
    wants_sixel(term, chosen)
        || (chosen == Chosen::Auto
            && term
                .to_lowercase()
                .contains(&WINDOWS_TERMINAL.to_lowercase()))
}

/// What the terminal an app runs in says it is, as a client attaching from
/// it would (src/mux/client.rs's `attaching_as`); in a pane, the workspace's
/// terminal's `TERM_PROGRAM` and `WT_SESSION`, which the panes keep.
fn this_terminal() -> String {
    super::client::attaching_as(|name| std::env::var(name).ok())
}

/// Whether a terminal, by its `TERM` or `TERM_PROGRAM`, speaks kitty's
/// graphics protocol with placeholders: Ghostty and kitty do.
pub fn speaks_kitty(term: &str) -> bool {
    let term = term.to_lowercase();
    term.contains("ghostty") || term.contains("kitty")
}

/// Whether a terminal gets sixel or may once it answers DA1: one that does,
/// or, chosen by the terminal, Windows Terminal (`apps_want_sixel`), whose
/// answer comes a moment after it attaches, by when a view may already have
/// given a picture again, which would otherwise be dropped.
pub fn may_get_sixel(term: &str) -> bool {
    for_panes(term) == Protocol::Sixel
        || (!speaks_kitty(term)
            && apps_want_sixel(term, chosen())
            && !term.split_whitespace().any(|w| w == ANSWERS_NO_SIXEL))
}

/// Whether a terminal's picture cells are blanked rather than sent as the
/// placeholder's box: one that gets neither way. The apps in panes draw
/// pictures only where the server said a terminal showing their session
/// gets them (`show @graphics`), so cells left for one that doesn't are
/// for another, or for one that has gone since they asked, and a program
/// of yours never has its pictures passed on.
pub fn pictures_blank(term: &str) -> bool {
    for_terminal(term).is_none()
}

/// How pictures reach a terminal that says it is `term` (what it attached
/// as): by kitty's protocol where it speaks it, whatever is chosen; else as
/// sixel where that is chosen (`Chosen::Sixel`) or the terminal is Windows
/// Terminal and it is chosen by the terminal (`Chosen::Auto`, the
/// default); else neither.
pub fn for_terminal(term: &str) -> Option<Protocol> {
    way(speaks_kitty(term), wants_sixel(term, chosen()))
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

/// What the workspace an app runs in draws for the terminals showing its
/// pane's session, as the server says (`show @graphics`,
/// `Server::graphics_for_pane`): the words `kitty`, `sixel`, `round`, or
/// `none`. `None` outside the workspace, or from a server too old to say.
fn workspace_draws() -> Option<String> {
    if !super::socket::in_pane() {
        return None;
    }
    let said = super::socket::ask_ok(&["show", "@graphics"]).ok()?;
    let said = said.trim();
    (!said.is_empty()).then(|| said.to_string())
}

/// Whether an app (Notes, the views) draws its pictures: in the workspace,
/// where a terminal showing its pane's session gets them, kitty's or sixel,
/// as the server says, since only it has the terminals' answers; outside
/// it, or with a server too old to say, where the terminal it runs in
/// speaks kitty's protocol (Ghostty or kitty, by `$TERM` or
/// `$TERM_PROGRAM`, which the panes keep as the workspace's), or, in a
/// pane, gets sixel by its guess (`apps_want_sixel`).
pub fn apps_draw() -> bool {
    match workspace_draws() {
        Some(said) => said
            .split_whitespace()
            .any(|w| w == "kitty" || w == "sixel"),
        None => {
            let term = this_terminal();
            speaks_kitty(&term)
                || (super::socket::in_pane() && apps_want_sixel(&term, chosen_now()))
        }
    }
}

/// The word a client adds to what it attaches as when it runs in Windows
/// Terminal (`WT_SESSION` set), whose `TERM` says only `xterm-256color`.
pub const WINDOWS_TERMINAL: &str = "WindowsTerminal";

/// Whether a terminal draws truecolor, so the dark theme is drawn in the
/// UX 2.0 canvas's colors for it (src/status/theme.rs): it says so
/// (`COLORTERM`'s `truecolor` or `24bit`), or it is Ghostty, kitty or
/// Windows Terminal, which always do; else the 256-color palette.
pub fn truecolor(term: &str) -> bool {
    let term = term.to_lowercase();
    let windows_terminal = WINDOWS_TERMINAL.to_lowercase();
    ["truecolor", "24bit", "ghostty", "kitty", &windows_terminal]
        .iter()
        .any(|w| term.contains(w))
}

/// Whether a terminal takes the mouse pointer's shapes (`OSC 22 ; <name>
/// ST`, src/ui/pointer.rs): Ghostty and kitty do; any other is never sent
/// one, its pointer left as it is.
pub fn pointer_shapes(term: &str) -> bool {
    let term = term.to_lowercase();
    term.contains("ghostty") || term.contains("kitty")
}

/// How the rounded ends of kit's objects (src/ui/kit.rs: a chip, a keycap,
/// a cell, an input), the half circles U+E0B6 and U+E0B4, reach a terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ends {
    /// The glyphs, which the terminal draws itself, never from a font that
    /// may not have them: Ghostty.
    Glyphs,
    /// Sixel pictures of the half circles, drawn by the server where the
    /// frame's glyphs are once it is drawn (src/mux/sixel): a terminal that
    /// gets sixel.
    Pictures,
    /// A space of the object's fill, square, the object keeping its width:
    /// any other.
    Spaces,
}

/// How the rounded ends reach a terminal that says it is `term`.
pub fn ends(term: &str) -> Ends {
    ends_way(
        term.to_lowercase().contains("ghostty"),
        for_panes(term) == Protocol::Sixel,
    )
}

/// The way for a terminal that draws the half circles itself or not, and
/// that gets sixel or not.
fn ends_way(draws_them: bool, sixel: bool) -> Ends {
    if draws_them {
        Ends::Glyphs
    } else if sixel {
        Ends::Pictures
    } else {
        Ends::Spaces
    }
}

/// Whether the drawing for a terminal writes the half circles (kit's
/// `caps`): where it draws them itself, and where they become pictures;
/// elsewhere kit writes a space of the fill.
pub fn round_ends(term: &str) -> bool {
    ends(term) != Ends::Spaces
}

/// The same for an app in a pane (Notes, the views), which draws into a
/// terminal of its own: in the workspace, where a terminal showing its
/// pane's session has round ends, as the server says; else by the terminal
/// it runs in (`$TERM` and `$TERM_PROGRAM`), or, in a pane, by its guess
/// at sixel.
pub fn apps_round_ends() -> bool {
    if let Some(said) = workspace_draws() {
        return said.split_whitespace().any(|w| w == "round");
    }
    let term = this_terminal();
    ends_way(
        term.to_lowercase().contains("ghostty"),
        super::socket::in_pane() && !speaks_kitty(&term) && apps_want_sixel(&term, chosen_now()),
    ) != Ends::Spaces
}

impl Server {
    /// What the terminals showing a pane's session get, for an app in the
    /// pane to draw by (`show @graphics`): `kitty` where one gets kitty's
    /// pictures, `sixel` where one gets sixel or may once it answers DA1,
    /// `round` where one's pill ends are round, `none` for none of them.
    /// Nothing (unknown) while no terminal shows the session, so an app
    /// started then goes by the terminal it runs in.
    pub(super) fn graphics_for_pane(&self, pane: super::PaneId) -> String {
        let session = self
            .panes
            .get(&pane)
            .and_then(|slot| self.session_of_tab(slot.tab))
            .map(|s| s.name.clone());
        let (mut kitty, mut sixel, mut round, mut any) = (false, false, false, false);
        for c in self
            .clients
            .values()
            .filter(|c| Some(&c.session) == session.as_ref())
        {
            any = true;
            let may = may_get_sixel(&c.term);
            match for_terminal(&c.term) {
                Some(Protocol::Kitty) => kitty = true,
                Some(Protocol::Sixel) => sixel = true,
                None => sixel |= may,
            }
            round |= ends(&c.term) != Ends::Spaces || may;
        }
        if !any {
            return String::new();
        }
        let words: Vec<&str> = [(kitty, "kitty"), (sixel, "sixel"), (round, "round")]
            .into_iter()
            .filter_map(|(on, w)| on.then_some(w))
            .collect();
        if words.is_empty() {
            "none".into()
        } else {
            words.join(" ")
        }
    }

    /// A client's terminal answered "what are you" (DA1), asked as it
    /// attached: whether it draws sixel kept with what it attached as
    /// (`ANSWERS_SIXEL`), and, where that changes how pictures reach it,
    /// its screen drawn again whole and, for sixel, its cell asked for.
    pub(super) fn terminal_answered(&mut self, id: super::server::ClientId, sixel: bool) {
        let Some(c) = self.clients.get_mut(&id) else {
            return;
        };
        let before = for_panes(&c.term);
        let mut words: Vec<&str> = c
            .term
            .split_whitespace()
            .filter(|w| *w != ANSWERS_SIXEL && *w != ANSWERS_NO_SIXEL)
            .collect();
        words.push(if sixel {
            ANSWERS_SIXEL
        } else {
            ANSWERS_NO_SIXEL
        });
        c.term = words.join(" ");
        let now = for_panes(&c.term);
        if now != before {
            c.screen.invalidate();
            c.dirty = true;
            if now == Protocol::Sixel {
                let query = super::sixel::CELL_QUERY.to_vec();
                let _ = c.tx.send(super::proto::ServerMsg::Output(query));
            }
        }
    }

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
    fn truecolor_pointer_shapes_and_round_ends_follow_the_terminal() {
        for term in [
            "xterm-256color  truecolor",
            "xterm-256color  24bit",
            "xterm-ghostty",
            "xterm-kitty",
            "xterm-256color  WindowsTerminal",
        ] {
            assert!(truecolor(term), "{term}");
        }
        assert!(!truecolor("xterm-256color Apple_Terminal"));
        assert!(!truecolor(""));
        for term in [
            "xterm-ghostty",
            "xterm-256color ghostty truecolor",
            "xterm-kitty",
        ] {
            assert!(pointer_shapes(term), "{term}");
        }
        for term in [
            "xterm-256color WindowsTerminal",
            "xterm-256color Apple_Terminal",
            "screen-256color",
        ] {
            assert!(!pointer_shapes(term), "{term}");
        }
        assert_eq!(ends_way(true, false), Ends::Glyphs);
        assert_eq!(ends_way(true, true), Ends::Glyphs, "Ghostty with sixel");
        assert_eq!(ends_way(false, true), Ends::Pictures, "Windows Terminal");
        assert_eq!(ends_way(false, false), Ends::Spaces);
    }

    #[test]
    fn chosen_by_the_terminal_one_that_answers_sixel_gets_it() {
        let wt = "xterm-256color WindowsTerminal";
        let answered = "xterm-256color WindowsTerminal answers-sixel";
        assert!(wants_sixel(answered, Chosen::Auto));
        assert!(
            !wants_sixel(wt, Chosen::Auto),
            "an older one, until it says"
        );
        assert!(!wants_sixel("xterm-256color", Chosen::Auto));
        assert!(!wants_sixel(answered, Chosen::Kitty));
        assert!(wants_sixel("xterm-256color", Chosen::Sixel));
        let no = "xterm-256color WindowsTerminal answers-no-sixel";
        assert!(!wants_sixel(no, Chosen::Auto));
        // An app can't see the answer: Windows Terminal is its guess.
        assert!(apps_want_sixel(wt, Chosen::Auto));
        assert!(!apps_want_sixel(wt, Chosen::Kitty));
        assert!(!apps_want_sixel("xterm-256color", Chosen::Auto));
        assert!(!wants_sixel("", Chosen::Kitty));
        assert_eq!(chosen_of(None), Chosen::Auto);
        assert_eq!(chosen_of(Some("auto".into())), Chosen::Auto);
        assert_eq!(chosen_of(Some("kitty".into())), Chosen::Kitty);
        assert_eq!(chosen_of(Some("sixel".into())), Chosen::Sixel);
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
