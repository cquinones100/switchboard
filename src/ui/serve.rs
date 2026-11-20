//! The server's side of the overlays: opening one on a client, handing it
//! the client's input, telling its opener how it ended, closing the ones
//! whose time is up, and asking the client's terminal for every mouse
//! movement while one is up and hover_highlight is on (the server's own
//! 1003, which the client turns off again as it leaves). On top of that,
//! what Switchboard's scripts did with popups: `say` (say.sh), `tip`
//! (tip.sh), asking before M-w closes a busy pane (confirm.sh), M-p's
//! palette (palette.sh) and the list of keys, and the `say` and `tip`
//! commands.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use ratatui::layout::Rect;

use super::draw;
use super::palette::{self, Entry};
use super::picker::Order;
use super::picture::{self, Picture};
use super::pictures::{self, Pictures};
use super::{
    Confirm, Layer, Look, Menu, Message, Outcome, Overlay, PickItem, Picked, Picker, Step, Then,
    Tip, Ui, UiEvent, keylist, message, tip,
};
use crate::mux::input::{Input, Mouse, MouseButton, MouseKind, Parsed};
use crate::mux::proto::ServerMsg;
use crate::mux::server::{ClientId, Event, Reply, Server};
use crate::mux::{PaneId, TabId};
use crate::{paths, settings, util};

/// Where pictures shown beside a tab's pane go on a client
/// (`Server::look_spare`).
enum Spare {
    /// Nowhere any more: no such columns, tab or client.
    Gone,
    /// The columns are behind another tab or mission control for now.
    Hidden,
    At(Rect),
}

/// What `switchboard look` does with a file, by its name's ending.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// One the server may draw itself.
    Picture,
    /// A picture it doesn't decode, a video or a PDF: the desktop's viewer.
    Viewer,
}

impl Kind {
    fn of(file: &std::path::Path) -> Option<Kind> {
        let ending = file.extension()?.to_str()?.to_ascii_lowercase();
        match ending.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" => Some(Kind::Picture),
            // No SVG: one may carry a script, and a browser may be what
            // the desktop opens it with.
            "heic" | "avif" | "pdf" | "mp4" | "mov" | "m4v" | "webm" => Some(Kind::Viewer),
            _ => None,
        }
    }
}

/// What reaches the server's task from one of its own.
type ToServer = tokio::sync::mpsc::UnboundedSender<Event>;

/// Opens a file with what the desktop opens it with, from a task of its
/// own; one it couldn't open is said on the client, since whoever asked
/// was already told it would show.
fn open_file(file: &std::path::Path, tx: &ToServer, client: Option<ClientId>) {
    if let Err(why) = crate::platform::open(&file.to_string_lossy()) {
        let _ = tx.send(Event::Ui(UiEvent::Said {
            client,
            text: format!("{why:#}"),
        }));
    }
}

/// Whether a file is in a worktree holding someone else's PR
/// (`trust::is_untrusted`, which runs git, so from a task of its own),
/// said on the client when it is: nothing of such a worktree is decoded
/// or handed to the desktop.
fn untrusted(file: &std::path::Path, tx: &ToServer, client: Option<ClientId>) -> bool {
    let held = file.parent().is_some_and(crate::trust::is_untrusted);
    if held {
        let _ = tx.send(Event::Ui(UiEvent::Said {
            client,
            text: format!("look: {} is in a PR's untrusted worktree", file.display()),
        }));
    }
    held
}

/// Opens files with what the desktop opens them with, in order, off the
/// server's task; `vetted` for files a task already found in no untrusted
/// worktree.
fn open_outside(
    files: Vec<std::path::PathBuf>,
    tx: ToServer,
    client: Option<ClientId>,
    vetted: bool,
) {
    tokio::task::spawn_blocking(move || {
        for file in files {
            if vetted || !untrusted(&file, &tx, client) {
                open_file(&file, &tx, client);
            }
        }
    });
}

/// What became of input handed to a client's overlays.
pub enum Handed {
    /// None of them took it: it is the tab's.
    Left(Parsed),
    /// One took it; `only_itself` while all it changed was that box (a key
    /// typed in it), not anything a box closing hands its opener.
    Took { only_itself: bool },
}

/// How long the status line shows a message with message_boxes off: tmux's
/// `display-time`, whose default tmux.conf kept.
const DISPLAY_TIME: Duration = Duration::from_millis(750);
/// How long after a pick from the palette its key's tip comes, so it shows
/// over what the command opened rather than before it (tip.sh's `-d 1`).
const KEY_TIP_DELAY: Duration = Duration::from_secs(1);

/// Whether an on/off option is on, read from the file now: these are read
/// on rare events (a box opening, a pane closing), so a change made by hand
/// applies at once.
fn setting_on(key: &str) -> bool {
    settings::setting(key) != Some(serde_json::Value::Bool(false))
}

impl Ui {
    /// The tips shown, read from their file the first time.
    fn seen(&mut self) -> &mut BTreeSet<String> {
        self.tips_seen.get_or_insert_with(tip::read_seen)
    }
}

impl Server {
    // ── Overlays ─────────────────────────────────────────────────────────────

    /// Puts an overlay over a client's screen, above any already up; `then`
    /// is told how it ends.
    /// One a click opens (a menu, a question) opens beside the click, with
    /// Settings' `boxes_near_click` on.
    pub fn open(&mut self, client: ClientId, mut overlay: Box<dyn Overlay>, then: Option<Then>) {
        let Some(c) = self.clients.get_mut(&client) else {
            return;
        };
        if let Some(at) = c.pressed_at
            && setting_on("boxes_near_click")
        {
            overlay.open_near(at);
        }
        c.overlays.look = Look::read_for(c.truecolor());
        let about = c.overlays.about_next.take();
        c.overlays.layers.push(Layer {
            overlay,
            then,
            about,
        });
        self.overlays_changed(client);
    }

    /// Opens a box about a tab with `open` (its links, renaming it,
    /// answering its Claude, which note is its, its menu and full title,
    /// sending its Claude's last reply on, Work on this item), marked so that it goes beside a narrow lone pane as a pane
    /// when the tab is the one in front of the client (UX 2.0 board 111);
    /// every other box goes in the middle.
    pub fn about_tab(&mut self, client: ClientId, tab: TabId, open: impl FnOnce(&mut Server)) {
        let in_front = self
            .clients
            .get(&client)
            .and_then(|c| self.session(&c.session))
            .is_some_and(|s| s.current == tab);
        let header =
            in_front.then(|| format!("tab {} · {}", self.tab_number(tab), self.tab_label(tab)));
        if let Some(c) = self.clients.get_mut(&client) {
            c.overlays.about_next = header;
        }
        open(self);
        // A box `open` didn't open leaves nothing for the next one.
        if let Some(c) = self.clients.get_mut(&client) {
            c.overlays.about_next = None;
        }
    }

    /// Asks a question; `then` is told whether it was answered yes.
    pub fn ask(
        &mut self,
        client: ClientId,
        question: Confirm,
        then: impl FnOnce(&mut Server, ClientId, bool) + Send + 'static,
    ) {
        let then: Then = Box::new(move |s, c, o| then(s, c, o == Outcome::Yes));
        self.open(client, Box::new(question), Some(then));
    }

    /// Shows a menu; `then` is told the option chosen, by its place, or
    /// `None` when it was closed without one.
    pub fn choose(
        &mut self,
        client: ClientId,
        menu: Menu,
        then: impl FnOnce(&mut Server, ClientId, Option<usize>) + Send + 'static,
    ) {
        let then: Then = Box::new(move |s, c, o| {
            let chose = match o {
                Outcome::Chose(i) => Some(i),
                _ => None,
            };
            then(s, c, chose)
        });
        self.open(client, Box::new(menu), Some(then));
    }

    /// Shows a list to pick from; `then` is told the item picked, or `None`.
    pub fn pick(
        &mut self,
        client: ClientId,
        picker: Picker,
        then: impl FnOnce(&mut Server, ClientId, Option<Picked>) + Send + 'static,
    ) {
        let then: Then = Box::new(move |s, c, o| {
            let picked = match o {
                Outcome::Picked(p) => Some(p),
                _ => None,
            };
            then(s, c, picked)
        });
        self.open(client, Box::new(picker), Some(then));
    }

    /// As `pick`, for a list whose rows can be marked with Tab and taken
    /// together (`Picker::multi`): `then` is told the rows marked, or the
    /// one chosen when none is, or nothing.
    pub fn pick_many(
        &mut self,
        client: ClientId,
        picker: Picker,
        then: impl FnOnce(&mut Server, ClientId, Vec<Picked>) + Send + 'static,
    ) {
        let then: Then = Box::new(move |s, c, o| {
            let picked = match o {
                Outcome::Picked(p) => vec![p],
                Outcome::PickedMany(many) => many,
                _ => Vec::new(),
            };
            then(s, c, picked)
        });
        self.open(client, Box::new(picker), Some(then));
    }

    /// As `pick`, for a list that does a second thing with a row
    /// (`Picker::also`): `then` is told the item, and whether it was taken
    /// with Ctrl-O for that.
    pub fn pick_also(
        &mut self,
        client: ClientId,
        picker: Picker,
        then: impl FnOnce(&mut Server, ClientId, Option<(Picked, bool)>) + Send + 'static,
    ) {
        let then: Then = Box::new(move |s, c, o| {
            let picked = match o {
                Outcome::Picked(p) => Some((p, false)),
                Outcome::PickedAlso(p) => Some((p, true)),
                _ => None,
            };
            then(s, c, picked)
        });
        self.open(client, Box::new(picker), Some(then));
    }

    /// Closes every overlay of a kind on a client, each opener told it was
    /// dismissed.
    pub fn close_overlays(&mut self, client: ClientId, kind: &str) {
        let Some(c) = self.clients.get_mut(&client) else {
            return;
        };
        // A message on the status line (message_boxes off) goes too.
        if kind == "message" {
            c.overlays.note = None;
        }
        let (gone, kept): (Vec<Layer>, Vec<Layer>) = std::mem::take(&mut c.overlays.layers)
            .into_iter()
            .partition(|l| l.overlay.kind() == kind);
        c.overlays.layers = kept;
        for layer in gone {
            self.finished(client, layer, Outcome::Dismissed);
        }
        self.overlays_changed(client);
    }

    /// Hands a client's input to its overlays: the top one takes keys, the
    /// mouse and pastes, and one that closes on a key can pass it on, to the
    /// one under it or, past the last, to the tab. Gives back the input
    /// nothing took, for the tab, or whether what took it changed only the
    /// box itself (a key typed in it), which nothing behind it can show.
    pub fn overlay_input(&mut self, id: ClientId, parsed: Parsed) -> Handed {
        if !matches!(
            parsed.input,
            Input::Key(_) | Input::Mouse(_) | Input::Paste(_)
        ) {
            return Handed::Left(parsed);
        }
        let mut closed = false;
        loop {
            let Some(c) = self.clients.get_mut(&id) else {
                return Handed::Took { only_itself: false };
            };
            let look = c.overlays.look;
            let Some(top) = c.overlays.layers.last_mut() else {
                return Handed::Left(parsed);
            };
            let step = match &parsed.input {
                Input::Key(k) => top.overlay.key(*k, &look),
                // A right press on a box does nothing inside it and outside
                // what a left press there does (closing it), so it never
                // picks an option nor reaches what is under the box; with
                // Settings' "Click in menus" off, nothing anywhere.
                Input::Mouse(m)
                    if m.kind == MouseKind::Press
                        && m.button == MouseButton::Right
                        && let Some(area) = top.overlay.stands_out() =>
                {
                    let inside = m.col >= area.x
                        && m.col < area.right()
                        && m.row >= area.y
                        && m.row < area.bottom();
                    if inside || !look.mouse {
                        Step::Same
                    } else {
                        let left = Mouse {
                            button: MouseButton::Left,
                            ..*m
                        };
                        top.overlay.mouse(left, &look)
                    }
                }
                Input::Mouse(m) => top.overlay.mouse(*m, &look),
                Input::Paste(text) => top.overlay.paste(text, &look),
                _ => Step::Same,
            };
            // A box's own keys change nothing but the box (they reach no
            // more than it), until it closes and its opener acts.
            let only_itself = !closed;
            match step {
                Step::Same => return Handed::Took { only_itself },
                Step::Redraw => {
                    c.dirty = true;
                    return Handed::Took { only_itself };
                }
                Step::Close(outcome) => {
                    self.finish_top(id, outcome);
                    return Handed::Took { only_itself: false };
                }
                Step::Pass => {
                    closed = true;
                    self.finish_top(id, Outcome::Dismissed);
                }
                Step::Through => return Handed::Left(parsed),
            }
        }
    }

    fn finish_top(&mut self, id: ClientId, outcome: Outcome) {
        if let Some(layer) = self
            .clients
            .get_mut(&id)
            .and_then(|c| c.overlays.layers.pop())
        {
            self.finished(id, layer, outcome);
        }
        self.overlays_changed(id);
    }

    /// An overlay taken off the stack: its opener is told how it ended.
    fn finished(&mut self, id: ClientId, layer: Layer, outcome: Outcome) {
        if let Some(then) = layer.then {
            then(self, id, outcome);
        }
    }

    /// After overlays open or close: the client is drawn again, its terminal
    /// reports every mouse movement while one is up (for hover), and the
    /// soonest that closes by itself is timed.
    fn overlays_changed(&mut self, id: ClientId) {
        let Some(c) = self.clients.get_mut(&id) else {
            return;
        };
        c.dirty = true;
        let deadline = c.overlays.deadline();
        self.sync_motion(id);
        if let Some(at) = deadline {
            let after = at.saturating_duration_since(Instant::now()) + Duration::from_millis(10);
            self.send_later(after, Event::Ui(UiEvent::Due(id)));
        }
    }

    /// Asks a client's terminal for every mouse movement while something
    /// lights what is under the mouse (an overlay with hover_highlight, or
    /// the sidebar's rows) or the pointer follows it (`Client::takes_pointer`),
    /// and stops asking when nothing does. Turning
    /// 1003 off leaves a terminal reporting no mouse at all, so the presses
    /// and drags the client asked for (1002) are asked for again with it.
    pub fn sync_motion(&mut self, id: ClientId) {
        let sidebar = self.sidebar_wants_motion(id);
        let Some(c) = self.clients.get_mut(&id) else {
            return;
        };
        let header = !c.header_keys.is_empty() && crate::settings::is_on("hover_highlight");
        let motion = (!c.overlays.layers.is_empty() && c.overlays.look.hover)
            || sidebar
            || header
            || c.takes_pointer();
        // A pointer that stops following the mouse (hover_highlight turned
        // off) is put back to the beam now, as no movement may come again.
        if !c.takes_pointer() && c.pointer.take().is_some() {
            let _ =
                c.tx.send(ServerMsg::Output(super::pointer::Pointer::Text.osc()));
        }
        if motion != c.overlays.motion {
            c.overlays.motion = motion;
            let mode: &[u8] = if motion {
                b"\x1b[?1003h"
            } else {
                b"\x1b[?1003l\x1b[?1002h"
            };
            let _ = c.tx.send(ServerMsg::Output(mode.to_vec()));
        }
    }

    /// What the overlays' timers say.
    pub fn ui_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::Due(id) => self.overlays_due(id),
            // Only on the client the pick was made on: one that has gone
            // (the pick was Detach) gets none, and nobody else does.
            UiEvent::Tip { name, key, client } => {
                if self.clients.contains_key(&client) {
                    self.show_tip(&name, key.as_deref(), Some(client));
                }
            }
            UiEvent::Picture {
                client,
                pane,
                title,
                read,
            } => {
                if !self.clients.contains_key(&client) || !self.panes.contains_key(&pane) {
                    return;
                }
                match read {
                    Ok(img) => {
                        self.close_overlays(client, "picture");
                        let shown = Picture::new(pane, &title, img);
                        self.open(client, Box::new(shown), None);
                    }
                    Err(why) => self.say(Some(client), &why),
                }
            }
            UiEvent::Said { client, text } => self.say(client, &text),
            UiEvent::Look { client, tab, shown } => {
                if shown.is_empty() {
                    return;
                }
                // The columns may have gone while the pictures were read.
                if !matches!(self.look_spare(client, tab), Spare::At(_)) {
                    let files = shown.into_iter().map(|s| s.file).collect();
                    open_outside(files, self.tx.clone(), Some(client), true);
                    return;
                }
                self.close_overlays(client, "pictures");
                let about = format!("tab {} · {}", self.tab_number(tab), self.tab_label(tab));
                let pictures = Pictures::new(tab, &about, shown);
                // Under every box already up: they pass keys on to the tab,
                // which would take them from a menu or a question open
                // when Claude shows them, were they on top of it.
                if let Some(c) = self.clients.get_mut(&client) {
                    c.overlays.look = Look::read_for(c.truecolor());
                    c.overlays.layers.insert(
                        0,
                        Layer {
                            overlay: Box::new(pictures),
                            then: None,
                            about: None,
                        },
                    );
                }
                self.overlays_changed(client);
            }
        }
    }

    /// `look [-t pane] <file>...`: pictures shown to you beside the pane's
    /// tab, in the empty columns at the right of its narrow lone pane, read
    /// off the server's task (Settings' `images_beside_pane`). Where there
    /// are no such columns in front of a terminal that draws the server's
    /// images (a split, a wide pane, another tab in front, mission control
    /// up), with more than `pictures::MOST` of them, or with the setting
    /// off, each opens with what the desktop opens it with, as a video or
    /// a PDF always does, and a picture this can't decode. Anything else is
    /// refused, by the ending of the file a name leads to with links
    /// followed, and so is a file in a PR's untrusted worktree, since Claude
    /// may run this without asking and the desktop runs some kinds of file
    /// it is handed.
    pub fn cmd_look(
        &mut self,
        args: &[String],
        cwd: &std::path::Path,
        caller: Option<PaneId>,
    ) -> Reply {
        let (target, rest) = match args {
            [flag, t, rest @ ..] if flag == "-t" => (Some(t.as_str()), rest),
            rest => (None, rest),
        };
        if rest.is_empty() {
            return Reply::fail("usage: switchboard look [-t pane] <file>...");
        }
        let pane = match target {
            Some(t) => match t.strip_prefix('%').and_then(|p| p.parse::<PaneId>().ok()) {
                Some(p) if self.panes.contains_key(&p) => Some(p),
                _ => return Reply::fail(format!("no pane {t}")),
            },
            None => caller,
        };
        let mut files = Vec::new();
        for name in rest {
            let named = crate::mux::commands::resolve(cwd, name);
            // The file a name leads to, links followed, is what is read
            // or opened, so its own ending decides its kind: a link named
            // as a picture may lead to a file the desktop would run.
            let file = match named.canonicalize() {
                Ok(real) if real.is_file() => real,
                _ => return Reply::fail(format!("look: no file {}", named.display())),
            };
            // A PR checked out untrusted is its author's: nothing of it is
            // handed to the desktop. One marked so elsewhere is found off
            // this task, before its file is read or opened (`untrusted`).
            if crate::trust::in_review_dir(&file) {
                return Reply::fail(format!(
                    "look: {} is in a PR's untrusted worktree",
                    file.display()
                ));
            }
            let Some(kind) = Kind::of(&file) else {
                return Reply::fail(format!(
                    "look: {} isn't a picture, a video or a PDF",
                    file.display()
                ));
            };
            files.push((file, kind));
        }
        let client = self.client_for(pane);
        let tab = pane
            .and_then(|p| self.panes.get(&p))
            .map(|slot| slot.tab)
            .or_else(|| {
                let c = self.clients.get(&client?)?;
                Some(self.session(&c.session)?.current)
            });
        let pictures = files.iter().filter(|(_, k)| *k == Kind::Picture).count();
        let beside = match (client, tab) {
            (Some(client), Some(tab))
                if (1..=pictures::MOST).contains(&pictures)
                    && settings::is_on("images_beside_pane")
                    && matches!(self.look_spare(client, tab), Spare::At(_)) =>
            {
                Some((client, tab))
            }
            _ => None,
        };
        let Some((client, tab)) = beside else {
            let files = files.into_iter().map(|(f, _)| f).collect();
            open_outside(files, self.tx.clone(), client, false);
            return Reply::ok("");
        };
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let mut shown = Vec::new();
            for (file, kind) in files {
                if untrusted(&file, &tx, Some(client)) {
                    continue;
                }
                let read = (kind == Kind::Picture)
                    .then(|| picture::decode(&file).ok())
                    .flatten();
                match read {
                    Some(img) => shown.push(pictures::Shown {
                        title: file
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        file,
                        img,
                    }),
                    None => {
                        open_file(&file, &tx, Some(client));
                    }
                }
            }
            let _ = tx.send(Event::Ui(UiEvent::Look { client, tab, shown }));
        });
        Reply::ok("")
    }

    /// Where pictures shown beside a tab's pane go on a client: the empty
    /// columns at the right of its narrow lone pane, while the tab is the
    /// one in front, nothing fills the screen over it and the terminal
    /// draws the server's images.
    fn look_spare(&self, client: ClientId, tab: TabId) -> Spare {
        let Some(c) = self.clients.get(&client) else {
            return Spare::Gone;
        };
        let (Some(s), Some(t)) = (self.session(&c.session), self.tabs.get(&tab)) else {
            return Spare::Gone;
        };
        if self.graphics_of(c).is_none() {
            return Spare::Gone;
        }
        let covered = c.overlays.layers.iter().any(|l| l.overlay.covers_screen());
        if s.current != tab || covered {
            return Spare::Hidden;
        }
        crate::mux::beside::spare_of(s.spread_for(t), s.area_for(t))
            .filter(|r| r.width >= pictures::LEAST_COLS && r.height >= pictures::LEAST_HEIGHT)
            .map_or(Spare::Gone, Spare::At)
    }

    /// Keeps the pictures shown beside a tab's pane on the spare columns
    /// before a client is drawn: hidden while another tab or mission
    /// control is in front, closed once the columns have gone.
    pub fn looks_before_draw(&mut self, id: ClientId) {
        let Some(tab) = self
            .clients
            .get(&id)
            .and_then(|c| c.overlays.find::<Pictures>())
            .map(|p| p.tab)
        else {
            return;
        };
        let area = match self.look_spare(id, tab) {
            Spare::Gone => {
                self.close_overlays(id, "pictures");
                return;
            }
            Spare::Hidden => None,
            Spare::At(area) => Some(area),
        };
        if let Some(p) = self
            .clients
            .get_mut(&id)
            .and_then(|c| c.overlays.find_mut::<Pictures>())
        {
            p.area = area;
        }
    }

    /// `picture [-t pane] <file>`: the picture shown full size over the
    /// pane (the one it is asked from), read off the server's task. Fails
    /// at once, saying so, where the terminal doesn't draw the server's
    /// images, so the asker can open it another way (the notes app in
    /// Quick Look). A file that turns out to be no picture it can draw
    /// opens in Quick Look (or the desktop's viewer) from here instead.
    pub fn cmd_picture(&mut self, args: &[String], caller: Option<PaneId>) -> Reply {
        let (target, rest) = match args {
            [flag, t, rest @ ..] if flag == "-t" => (Some(t.as_str()), rest),
            rest => (None, rest),
        };
        let [file] = rest else {
            return Reply::fail("usage: switchboard picture [-t pane] <file>");
        };
        let pane = match target {
            Some(t) => match t.strip_prefix('%').and_then(|p| p.parse::<PaneId>().ok()) {
                Some(p) if self.panes.contains_key(&p) => Some(p),
                _ => return Reply::fail(format!("no pane {t}")),
            },
            None => caller,
        };
        let client = self.client_for(pane);
        let Some(pane) = pane.or_else(|| client.and_then(|c| self.current_pane(c))) else {
            return Reply::fail("picture: no pane to show it over");
        };
        let Some(client) = client.filter(|c| {
            self.clients
                .get(c)
                .is_some_and(|c| self.graphics_of(c).is_some())
        }) else {
            return Reply::fail("picture: this terminal doesn't draw the server's images");
        };
        if self.pane_rect(client, pane).is_none() {
            return Reply::fail(format!("picture: pane {pane} isn't on the screen"));
        }
        let file = std::path::PathBuf::from(file);
        if !file.is_absolute() || !file.is_file() {
            return Reply::fail(format!("picture: no file {}", file.display()));
        }
        let title = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let read = picture::decode(&file);
            // A file this can't draw (a kind the decoder doesn't read)
            // opens in Quick Look or the desktop's viewer, as it did
            // before, since the asker was already told it would show.
            if read.is_err() && crate::platform::preview(&file).is_ok() {
                return;
            }
            let _ = tx.send(Event::Ui(UiEvent::Picture {
                client,
                pane,
                title,
                read,
            }));
        });
        Reply::ok("")
    }

    /// Keeps each picture shown full size on its pane's place before a
    /// client is drawn, and closes one whose pane has gone or isn't on the
    /// screen any more.
    pub fn pictures_before_draw(&mut self, id: ClientId) {
        let Some(c) = self.clients.get(&id) else {
            return;
        };
        let panes: Vec<PaneId> = c
            .overlays
            .layers
            .iter()
            .filter_map(|l| l.overlay.as_ref().as_any().downcast_ref::<Picture>())
            .map(|p| p.pane)
            .collect();
        if panes.is_empty() {
            return;
        }
        let places: Vec<Option<Rect>> = panes.iter().map(|p| self.pane_rect(id, *p)).collect();
        if places.iter().any(Option::is_none) {
            self.close_overlays(id, "picture");
            return;
        }
        let c = self.clients.get_mut(&id).expect("checked above");
        let shown = c
            .overlays
            .layers
            .iter_mut()
            .filter_map(|l| l.overlay.as_mut().as_any_mut().downcast_mut::<Picture>());
        for (p, place) in shown.zip(places) {
            p.area = place;
        }
    }

    /// Closes a client's overlays whose time is up, and the status line's
    /// message once it has been shown long enough.
    fn overlays_due(&mut self, id: ClientId) {
        let now = Instant::now();
        let Some(c) = self.clients.get_mut(&id) else {
            return;
        };
        if c.overlays.note.as_ref().is_some_and(|(_, at)| *at <= now) {
            c.overlays.note = None;
            c.dirty = true;
        }
        let (due, kept): (Vec<Layer>, Vec<Layer>) = std::mem::take(&mut c.overlays.layers)
            .into_iter()
            .partition(|l| l.overlay.deadline().is_some_and(|d| d <= now));
        c.overlays.layers = kept;
        if due.is_empty() {
            return;
        }
        for layer in due {
            self.finished(id, layer, Outcome::Dismissed);
        }
        self.overlays_changed(id);
    }

    /// The client acting now in a session: its most recently active one.
    pub fn latest_client_of(&self, session: &str) -> Option<ClientId> {
        self.clients
            .values()
            .filter(|c| c.session == session)
            .max_by_key(|c| c.activity)
            .map(|c| c.id)
    }

    /// The client a command from a pane speaks to: the one used last on the
    /// pane's session, else the one used last anywhere.
    fn client_for(&self, caller: Option<PaneId>) -> Option<ClientId> {
        caller
            .and_then(|p| self.panes.get(&p))
            .and_then(|slot| self.session_of_tab(slot.tab))
            .and_then(|s| self.latest_client_of(&s.name))
            .or_else(|| self.latest_client())
    }

    // ── Messages ─────────────────────────────────────────────────────────────

    /// Says something in a box in the middle of a client's screen (the one
    /// used last when `None`), which any key closes and which closes itself
    /// after five seconds; with message_boxes off, on the status line for a
    /// moment, as tmux's message was.
    pub fn say(&mut self, client: Option<ClientId>, text: &str) {
        self.say_for(client, text, message::SECONDS);
    }

    /// `say`, up for this many seconds.
    pub fn say_for(&mut self, client: Option<ClientId>, text: &str, seconds: u64) {
        if text.trim().is_empty() {
            return;
        }
        let Some(id) = client
            .filter(|c| self.clients.contains_key(c))
            .or_else(|| self.latest_client())
        else {
            return;
        };
        // With no status line (UX 2.0), there is nowhere to put the line, so
        // it is said in a box whatever the setting.
        if !setting_on("message_boxes") && crate::status::lines() > 0 {
            if let Some(c) = self.clients.get_mut(&id) {
                let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
                // A message asked to stay longer than a box's usual time
                // (a link opening) stays as long on the status line.
                let shown_for = if seconds > message::SECONDS {
                    Duration::from_secs(seconds)
                } else {
                    DISPLAY_TIME
                };
                c.overlays.note = Some((line, Instant::now() + shown_for));
            }
            self.overlays_changed(id);
            return;
        }
        // One message at a time: a new one takes the place of one still up.
        self.close_overlays(id, "message");
        self.open(id, Box::new(Message::new(text, seconds)), None);
    }

    /// What a client's message says while it is up, in its box or on the
    /// status line (its words joined there by single spaces).
    pub fn message_shown(&self, client: ClientId) -> Option<String> {
        let c = self.clients.get(&client)?;
        if let Some(m) = c.overlays.find::<Message>() {
            return Some(m.text().to_string());
        }
        c.overlays.note.as_ref().map(|(line, _)| line.clone())
    }

    /// Closes a client's message, in its box or on the status line.
    pub fn unsay(&mut self, client: ClientId) {
        self.close_overlays(client, "message");
    }

    // ── Tips ─────────────────────────────────────────────────────────────────

    /// Shows a tip on a client (the one used last when `None`), unless it
    /// has been shown, the tips setting is off, or a box is up there already
    /// (a tip, a message, a menu, mission control), in which case it is left
    /// for another time, as tip.sh's tip that couldn't open behind another
    /// popup was. Gives whether it showed.
    pub fn tip(&mut self, name: &str, client: Option<ClientId>) -> bool {
        self.show_tip(name, None, client)
    }

    fn show_tip(&mut self, name: &str, key: Option<&str>, client: Option<ClientId>) -> bool {
        if self.tip_was_seen(name) {
            return false;
        }
        let Some(tip) = Tip::new(name, key) else {
            return false;
        };
        if !setting_on("tips") {
            return false;
        }
        let Some(id) = client
            .filter(|c| self.clients.contains_key(c))
            .or_else(|| self.latest_client())
        else {
            return false;
        };
        if !self.clients[&id].overlays.kinds().is_empty() {
            return false;
        }
        self.tip_seen(name);
        self.open(id, Box::new(tip), None);
        true
    }

    /// Whether a tip has been shown.
    pub fn tip_was_seen(&mut self, name: &str) -> bool {
        self.ui.seen().contains(name)
    }

    /// Marks a tip shown without showing it, as doing what it teaches does.
    pub fn tip_seen(&mut self, name: &str) {
        if self.ui.seen().insert(name.to_string()) {
            util::append_quietly(tip::seen_file(), format!("{name}\n"));
        }
    }

    /// Every tip shows again, once more ("Show the tips again" in M-p).
    pub fn reset_tips(&mut self) {
        let _ = std::fs::remove_file(tip::seen_file());
        self.ui.tips_seen = Some(BTreeSet::new());
    }

    /// The palette's "next time, press" a moment after a pick that has a
    /// key of its own, once for each key.
    fn key_tip_later(&mut self, client: ClientId, key: &str) {
        let (name, shown) = tip::key_tip(key);
        if self.tip_was_seen(&name) {
            return;
        }
        self.send_later(
            KEY_TIP_DELAY,
            Event::Ui(UiEvent::Tip {
                name,
                key: Some(shown),
                client,
            }),
        );
    }

    /// A tab switch: with five tabs or more, the tip on finding one (the
    /// `session-window-changed` hook's), once.
    pub fn tab_switched(&mut self, session: &str) {
        if self.session(session).is_none_or(|s| s.tabs.len() < 5) || self.tip_was_seen("jump") {
            return;
        }
        if let Some(client) = self.latest_client_of(session) {
            self.tip("jump", Some(client));
        }
    }

    // ── What the keys open ──────────────────────────────────────────────────

    /// The list of keys.
    pub fn list_keys(&mut self, client: ClientId) {
        let rows = self.bindings.list();
        self.open(client, Box::new(keylist::KeyList::new(rows)), None);
    }

    /// M-p: every command, to type in and pick from; the pick runs as its
    /// key would, then is remembered, and its key's tip comes a moment later.
    /// With Notes in front, the app is asked first, off the server's task,
    /// for the note it shows and its own commands, listed ahead of the rest
    /// (`palette::notes_entries`); with no answer, they are left out.
    pub fn open_palette(&mut self, client: ClientId) {
        let notes_in_front = self
            .notes_in_front(client)
            .filter(|_| tokio::runtime::Handle::try_current().is_ok());
        let Some(notes) = notes_in_front else {
            self.open_palette_with(client, None);
            return;
        };
        self.notes_off_task(
            move || {
                crate::notes::socket::send(&notes, "palette")
                    .ok()
                    .and_then(|reply| crate::notes::socket::InFront::read(&reply))
                    .map(|front| (front, notes))
            },
            move |server, front| {
                if server.clients.contains_key(&client) {
                    server.open_palette_with(client, front);
                }
            },
        );
    }

    /// The socket of the notes app when the client's pane in front is
    /// one (one started by hand found by the notes' lock), with neither the
    /// sidebar nor mission control over it.
    fn notes_in_front(&mut self, client: ClientId) -> Option<std::path::PathBuf> {
        let session = self.clients.get(&client).map(|c| c.session.clone())?;
        if self.sidebar.focused(&session) || self.mission_open(client) {
            return None;
        }
        let here = self.current_pane(client)?;
        let tab = self.panes.get(&here)?.tab;
        (self.notes_in_tab(tab) == Some(here)).then(|| crate::notes::serve::socket_of(here))
    }

    /// The palette, with what Notes in front answered (`front`).
    fn open_palette_with(
        &mut self,
        client: ClientId,
        front: Option<(crate::notes::socket::InFront, std::path::PathBuf)>,
    ) {
        let (front, socket) = front.unzip();
        let all = settings::read_settings();
        let on = |key: &str| {
            all.iter()
                .find(|s| s.key == key)
                .is_none_or(|s| s.value != serde_json::Value::Bool(false))
        };
        let learn = on("palette_learn");
        // The diff's commands only where they can act (src/diff).
        let mut entries = palette::entries(
            &self.bindings,
            on("github"),
            on("linear"),
            &self.plugin_views(),
        );
        let listed = self.diff_palette(client);
        entries.retain(|e| listed(&e.action));
        // Reviewing a contributor's PR only in a repository with the skill.
        let reviews = self
            .current_pane(client)
            .and_then(|p| self.pane_dir(p))
            .is_some_and(|dir| crate::cmd::work::review_repo(&dir).is_some());
        entries.retain(|e| reviews || e.action != crate::mux::keys::Action::ReviewPr);
        if self.autopilot.mode.on {
            palette::autopilot_on(&mut entries, &self.autopilot.mode.note);
        }
        let entries = palette::recent_first(entries, &palette::read_recent());
        let picks = if learn {
            palette::read_queries()
        } else {
            Vec::new()
        };
        let notes = front.map_or_else(Vec::new, |front| {
            palette::notes_entries(&front, on("autopilot"), on("notes_palette"))
        });
        let firsts = notes.len();
        let items: Vec<PickItem> = notes
            .iter()
            .map(|n| n.item.clone())
            .chain(entries.iter().map(Entry::item))
            .collect();
        let rows: Vec<(String, String)> = items
            .iter()
            .map(|i| (i.value.clone(), i.haystack()))
            .collect();
        let order: Order = Box::new(move |query, matched| {
            let sub: Vec<(String, String)> = matched.iter().map(|&i| rows[i].clone()).collect();
            palette::rank(query, &sub, &picks)
                .into_iter()
                .map(|j| matched[j])
                .collect()
        });
        let mut picker = Picker::new("Actions", items)
            .named("palette")
            .prompt("action> ")
            .exact()
            .order(order);
        // UX 2.0 draws it as board 15: the input atom, and the chosen
        // command's preview under the list.
        if settings::is_on("ux2") {
            let caps = self
                .clients
                .get(&client)
                .is_some_and(|c| crate::mux::graphics::round_ends(&c.term));
            let previews = notes
                .iter()
                .map(|n| n.preview.clone())
                .chain(entries.iter().map(Entry::preview))
                .collect();
            picker = picker.previews(previews).kit(caps);
        }
        self.pick(client, picker, move |server, client, picked| {
            let Some(picked) = picked else {
                return;
            };
            if let Some(n) = notes.get(picked.index) {
                palette::learn_query(&picked.value, &picked.query, learn);
                match &n.pick {
                    palette::NotesPick::WorkNote(note) => server.work_note(client, note.clone()),
                    palette::NotesPick::Run(name) => {
                        if let Some(socket) = &socket {
                            crate::notes::serve::tell_at(socket.clone(), format!("run {name}"))
                        }
                    }
                }
                return;
            }
            let Some(entry) = entries.get(picked.index - firsts) else {
                return;
            };
            palette::remember(&picked.value, &picked.query, learn);
            server.count_pick(entry);
            // Over mission control, a pick that isn't one of its views runs
            // in the pane it was opened over, once it has closed.
            if server.mission_open(client) && !entry.action.in_mission() {
                server.mission_close(client);
            }
            server.act(client, entry.action.clone());
            if let Some(key) = entry.keys.first() {
                server.key_tip_later(client, key);
            }
        });
    }

    /// The prefix's `&`: the tab closed once you say so, as tmux's
    /// `confirm-before kill-window`, the programs running in its panes
    /// listed under the question.
    pub fn close_tab(&mut self, client: ClientId, tab: TabId) {
        if !self.tabs.contains_key(&tab) {
            return;
        }
        let panes = self.panes_with_parked(tab);
        let index = self.tab_number(tab);
        let list: Vec<String> = panes
            .into_iter()
            .filter_map(|p| self.running(p))
            .filter(|r| !r.is_empty())
            .collect();
        let question = format!("Close tab {index} {}?", self.tab_label(tab));
        self.ask(
            client,
            Confirm::new(question).list(list),
            move |server, _, yes| {
                if yes && server.tabs.contains_key(&tab) {
                    server.kill_tab(tab);
                }
            },
        );
    }

    /// The sidebar's select mode's "Close these tabs": asked once, each
    /// tab's number and name listed with the programs running in its panes;
    /// yes closes each still open, the one in front last, so the focus goes
    /// where closing it alone would, and leaves select mode and the
    /// sidebar. No keeps the selection.
    pub fn close_tabs(&mut self, client: ClientId, session: &str, mut tabs: Vec<TabId>) {
        let front = self.session(session).map(|s| s.current);
        tabs.sort_by_key(|t| Some(*t) == front);
        // A line a tab, its programs after its name (a program parked
        // behind the diff too, which closing it ends), and no more lines
        // than a box fits on a small screen, so Yes and No stay in sight.
        const SHOWN: usize = 8;
        // What the question's box shows of a line on this client
        // (`Confirm::draw`'s width less 12), 68 when its size isn't known.
        let width = self
            .clients
            .get(&client)
            .map_or(68, |c| usize::from(c.cols).saturating_sub(12));
        let mut list: Vec<String> = Vec::new();
        for &tab in &tabs {
            if !self.tabs.contains_key(&tab) {
                continue;
            }
            let index = self.tab_number(tab);
            let running: Vec<String> = self
                .panes_with_parked(tab)
                .into_iter()
                .filter_map(|p| self.running_name(p))
                .collect();
            // The programs by name, whole, and the tab's name cut to what
            // is left of the line, so no program is cut away.
            let programs = running.join(", ");
            // The programs and their ` · ` only when there are any.
            let taken = if running.is_empty() {
                0
            } else {
                draw::width(&programs) + 3
            };
            // At least the tab's number and an `…` for its name.
            let room = width
                .saturating_sub(taken)
                .max(draw::width(&index.to_string()) + 2);
            let mut line = draw::cut(&format!("{index} {}", self.tab_label(tab)), room);
            if !running.is_empty() {
                line.push_str(" · ");
                line.push_str(&programs);
            }
            list.push(line);
        }
        if list.len() > SHOWN {
            let more = list.len() - (SHOWN - 1);
            list.truncate(SHOWN - 1);
            list.push(format!("… and {more} more"));
        }
        let n = tabs.len();
        let session = session.to_string();
        self.ask(
            client,
            Confirm::new(format!("Close {n} tabs?")).list(list),
            move |server, client, yes| {
                if !yes {
                    return;
                }
                let mut closed = 0;
                for tab in tabs {
                    if server.tabs.contains_key(&tab) {
                        server.kill_tab(tab);
                        closed += 1;
                    }
                }
                // Back to the pane of the tab now in front, select mode and
                // any / filter ended, as leaving the sidebar ends them, so
                // no cursor is left on a tab that is gone or hidden.
                server.sidebar.side(&session).select = None;
                server.sidebar_leave(&session);
                server.dirty_session(&session);
                let said = format!(
                    "Closed {closed} {}.",
                    if closed == 1 { "tab" } else { "tabs" }
                );
                server.say(Some(client), &said);
            },
        );
    }

    /// M-w on a tab's sidebar row, or in its Claude pane: the tab, asked as
    /// the prefix's `&` asks, or at once with confirm_close off.
    pub fn close_tab_from_key(&mut self, client: ClientId, tab: TabId) {
        if setting_on("confirm_close") {
            self.close_tab(client, tab);
        } else if self.tabs.contains_key(&tab) {
            self.kill_tab(tab);
        }
    }

    /// M-w: closes the pane, asking first when its program isn't a shell at
    /// its prompt (confirm.sh's question), unless the confirm_close setting
    /// is off; in the tab's Claude pane, or in a tab whose one pane it is,
    /// it closes the tab instead, since a lone pane closed alone would only
    /// get a fresh shell in its place. The pane
    /// is the one M-w was pressed in, whatever has the focus by the time it
    /// is answered.
    pub fn close_here(&mut self, client: ClientId, pane: PaneId) {
        let Some(tab) = self.panes.get(&pane).map(|s| s.tab) else {
            return;
        };
        let from_claude = setting_on("close_tab_from_claude");
        let is_tabs_one_pane = self
            .tabs
            .get(&tab)
            .is_some_and(|t| t.layout.panes() == [pane])
            && !self.closes_as_before(pane);
        if from_claude && (self.tab_claude_pane(tab) == Some(pane) || is_tabs_one_pane) {
            self.close_tab_from_key(client, tab);
            return;
        }
        self.close_pane_keeping(client, pane, from_claude);
    }

    /// The prefix's `x` and the palette's "Close the pane": the pane alone,
    /// asked about as M-w asks, its tab closing with its last pane.
    pub fn close_pane(&mut self, client: ClientId, pane: PaneId) {
        self.close_pane_keeping(client, pane, false);
    }

    fn close_pane_keeping(&mut self, client: ClientId, pane: PaneId, keep_tab: bool) {
        // Notes beside a pane goes back to its own tab instead (notes-open.sh
        // away, src/notes/serve.rs).
        if self.notes_close(pane) {
            return;
        }
        let running = self.running(pane);
        if running.is_none() || !setting_on("confirm_close") {
            self.close_only_pane(pane, keep_tab);
            return;
        }
        let list: Vec<String> = running.into_iter().filter(|r| !r.is_empty()).collect();
        self.ask(
            client,
            Confirm::new("Close pane?").list(list),
            move |server, _, yes| {
                if yes {
                    server.close_only_pane(pane, keep_tab);
                }
            },
        );
    }

    /// Ends a pane M-w closed and takes it out, leaving its tab: the tab's
    /// last pane (beside Notes, or left last while M-w's question waited)
    /// gets a shell in its place rather than closing the tab, which M-w
    /// closes from its Claude pane, its one pane or its sidebar row. A tool
    /// in a pane's place gives the place back, and Notes' own tab closes
    /// with its last pane, as before. With `keep_tab` off (Settings'
    /// close_tab_from_claude), the last pane closes its tab.
    pub(crate) fn close_only_pane(&mut self, pane: PaneId, keep_tab: bool) {
        let Some(slot) = self.panes.get(&pane) else {
            return;
        };
        // Notes brought beside it doesn't count: left alone, it would go
        // home and take the tab with it (notes_orphan).
        let is_tabs_last_pane = self.tabs.get(&slot.tab).is_some_and(|t| {
            let notes_beside = t.opts.get("@notes_beside").map(String::as_str) == Some("1")
                && t.opts.get("@notes").map(String::as_str) != Some("1");
            t.layout.panes().into_iter().all(|p| {
                p == pane
                    || notes_beside
                        && self
                            .panes
                            .get(&p)
                            .is_some_and(|s| s.opts.contains_key("@notes_pane"))
            })
        });
        if keep_tab && is_tabs_last_pane && !self.closes_as_before(pane) {
            // A fresh shell, not other work handed the tab: what the tab is
            // about (its directory, a sealed review) stays, which
            // respawn_pane clears.
            let tab = slot.tab;
            let kept: Vec<(String, String)> = self
                .tabs
                .get(&tab)
                .map(|t| {
                    t.opts
                        .iter()
                        .filter(|(k, _)| crate::mux::respawn::HANDED_OVER.contains(&k.as_str()))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect()
                })
                .unwrap_or_default();
            if let Err(e) = self.respawn_pane(pane, None, None, true) {
                self.say(None, &format!("Couldn't start a shell in the pane: {e}"));
            }
            if let Some(t) = self.tabs.get_mut(&tab) {
                t.opts.extend(kept);
            }
            return;
        }
        self.kill_pane(pane);
    }

    /// A tool in a pane's place, Notes, or anything in Notes' own tab (its
    /// placeholder while Notes is beside another tab): M-w closes it as the
    /// prefix's `x` does, never its tab.
    fn closes_as_before(&self, pane: PaneId) -> bool {
        self.panes.get(&pane).is_some_and(|slot| {
            slot.opts.contains_key(crate::diff::in_place::IN_PLACE)
                || slot.opts.contains_key("@notes_pane")
                || self
                    .tabs
                    .get(&slot.tab)
                    .is_some_and(|t| t.opts.get("@notes").map(String::as_str) == Some("1"))
        })
    }

    /// What a pane runs in front, its command line as `program_line` writes
    /// it, or `None` when it is a
    /// shell at its prompt or its program has ended. A shell running a
    /// command puts it in a process group of its own in front, so the
    /// group in front being the pane's own process, and a shell, is a
    /// prompt.
    fn running(&self, pane: PaneId) -> Option<String> {
        let slot = self.panes.get(&pane)?;
        if slot.pane.exited().is_some() {
            return None;
        }
        let front = slot.pane.foreground_pgid();
        let command = slot.pane.foreground_command().unwrap_or_default();
        if front.is_some() && front == slot.pane.pid() && is_shell(&command) {
            return None;
        }
        let args = front
            .and_then(crate::mux::procinfo::args_of)
            .filter(|args| args.first().is_some_and(|a| !a.trim().is_empty()))
            .unwrap_or_else(|| vec![command]);
        let own_exe = self.own_exe(pane, &args);
        Some(program_line(&args, &own_exe))
    }

    /// The path of the Switchboard that started what a pane runs, for
    /// `app_of`: the pane's own program when the server marked it as one
    /// of its views or tools and it is named as Switchboard is, whatever
    /// binary started it (one a restart moved away from among them), else
    /// the binary the server runs.
    fn own_exe(&self, pane: PaneId, args: &[String]) -> String {
        // src/views/serve.rs's marks, the notes pane's and a tool in a
        // pane's place.
        const MARKS: [&str; 8] = [
            "@pr_view",
            "@ticket_view",
            "@link_view",
            "@checks_view",
            "@notes_pane",
            "@notes_placeholder",
            "@aws_login",
            crate::diff::in_place::IN_PLACE,
        ];
        let marked = self
            .panes
            .get(&pane)
            .is_some_and(|s| MARKS.iter().any(|m| s.opts.contains_key(*m)));
        match args.first() {
            // Still a binary named as Switchboard's, so a mark alone (which
            // a pane's program may set) names no other program a view.
            Some(first) if marked && program_name(first).starts_with("switchboard") => {
                first.clone()
            }
            _ => crate::util::exe()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
        }
    }

    /// What a pane runs in front, by its program's name alone (`claude`
    /// for `/usr/local/bin/claude --resume`), from its first argument as
    /// the kernel has it, never from a command line cut short; `None` as
    /// for `running`.
    fn running_name(&self, pane: PaneId) -> Option<String> {
        self.running(pane)?;
        let slot = self.panes.get(&pane)?;
        let args = slot
            .pane
            .foreground_pgid()
            .and_then(crate::mux::procinfo::args_of)
            .unwrap_or_default();
        // One of Switchboard's apps by what it is, as the close-pane
        // question names it, in a word or two, since every program of a
        // tab shares one line.
        let own_exe = self.own_exe(pane, &args);
        if let Some(app) = app_of(&args, &own_exe, true) {
            return Some(app);
        }
        // Another command of Switchboard's by its subcommand too
        // (`switchboard update`), so it reads as more than Switchboard; only
        // one of its own commands, never another word an argument could be.
        if let [first, sub, ..] = args.as_slice()
            && (program_name(first) == "switchboard" || *first == own_exe)
            && crate::cmd::is_command(sub)
        {
            return Some(format!("switchboard {sub}"));
        }
        let first = args
            .into_iter()
            .next()
            .filter(|a| !a.trim().is_empty())
            .or_else(|| slot.pane.foreground_command())?;
        Some(match title_of(&first) {
            Some((exe, _)) if has_link(exe) => scheme_only(exe),
            Some((exe, _)) => program_name(exe),
            None if has_link(&first) => scheme_only(&first),
            None => program_name(&first),
        })
    }

    // ── Commands ─────────────────────────────────────────────────────────────

    /// `say [-t seconds] [--] text...`: a message on the client used last on
    /// the caller's session.
    pub fn cmd_say(&mut self, args: &[String], caller: Option<PaneId>) -> Reply {
        let mut seconds = message::SECONDS;
        let mut rest = args;
        while let Some(first) = rest.first() {
            match first.as_str() {
                "-t" => {
                    let Some(n) = rest.get(1).and_then(|n| n.parse::<u64>().ok()) else {
                        return Reply::fail("say: -t needs a number of seconds");
                    };
                    seconds = n;
                    rest = &rest[2..];
                }
                "--" => {
                    rest = &rest[1..];
                    break;
                }
                _ => break,
            }
        }
        let text = rest.join(" ");
        if text.trim().is_empty() {
            return Reply::fail("Usage: switchboard say [-t seconds] <text>");
        }
        let client = self.client_for(caller);
        self.say_for(client, &text, seconds);
        Reply::ok("")
    }

    /// `tip <name>` shows a tip (once), `tip seen <name>` marks it shown,
    /// `tip reset` shows every tip again.
    pub fn cmd_tip(&mut self, args: &[String], caller: Option<PaneId>) -> Reply {
        match args {
            [what] if what == "reset" => {
                self.reset_tips();
                Reply::ok("")
            }
            [what, name] if what == "seen" => {
                self.tip_seen(name);
                Reply::ok("")
            }
            [name] => {
                if tip::text_of(name, None).is_none() {
                    return Reply::fail(format!("tip: no tip named {name}"));
                }
                let client = self.client_for(caller);
                self.tip(name, client);
                Reply::ok("")
            }
            _ => Reply::fail(format!(
                "Usage: switchboard tip <name> | seen <name> | reset  (tips kept in {})",
                paths::tilde(tip::seen_file())
            )),
        }
    }
}

/// A program's name without its folder: `claude` for
/// `/usr/local/bin/claude`.
fn program_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// A command line as the close questions list it: the program by its name
/// rather than its path, its arguments after it with paths under the home
/// written from `~`, cut to 60 columns (`npm run dev`, `vim ~/code/a.rs`),
/// and Switchboard's own apps by what they are (Notes, the diff, the PR
/// view).
fn program_line(args: &[String], own_exe: &str) -> String {
    let Some((first, rest)) = args.split_first() else {
        return String::new();
    };
    if let Some(app) = app_of(args, own_exe, false) {
        return draw::cut(&app, 60);
    }
    let name = match title_of(first) {
        // A title that starts with a link, by its scheme alone.
        Some((exe, _)) if has_link(exe) => scheme_only(exe),
        Some((exe, title)) => {
            // Up to the first word holding a link, which may run on past a
            // space the title lost its boundary at, so nothing after it.
            let words: Vec<&str> = title.split(' ').collect();
            let shown = match words.iter().position(|w| has_link(w)) {
                Some(at) => {
                    let mut kept: Vec<String> = words[..at].iter().map(|w| w.to_string()).collect();
                    // Only its scheme: where its user or host ends is lost
                    // with the boundaries.
                    kept.push(scheme_only(words[at]));
                    kept.join(" ")
                }
                None => title.to_string(),
            };
            format!("{} {shown}", program_name(exe))
        }
        None if has_link(first) => scheme_only(first),
        None => program_name(first),
    };
    // A local path: an argument of its own, or an option's value after `=`
    // (`--dir=/…`). Nothing after a `:`, which may name a path on another
    // host (`scp host:/…`).
    // From `~`, the rest kept exactly as written (a trailing slash, or
    // rsync's `/./`, says what is copied).
    // The home without a trailing slash, so `HOME=/home/me/` matches too.
    let home = crate::paths::home().display().to_string();
    let home = home.trim_end_matches('/');
    let from_home = |path: &str| match path.strip_prefix(home) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => {
            format!("~{rest}")
        }
        _ => path.to_string(),
    };
    // A link's word first, so no path rule shows what follows its host.
    let short = |arg: &String| {
        let arg = without_query(arg);
        match arg.split_once('=') {
            Some((key, value)) if key.starts_with('-') && value.starts_with('/') => {
                format!("{key}={}", from_home(value))
            }
            _ if arg.starts_with('/') => from_home(&arg),
            _ => arg,
        }
    };
    let words = std::iter::once(name.clone()).chain(rest.iter().map(short));
    draw::cut(&words.collect::<Vec<_>>().join(" "), 60)
}

/// What one of Switchboard's own apps is, by its command line, as the
/// close questions name it: whole for the close-pane question (`the diff
/// of uncommitted changes`), and in a word or two with `short` for the
/// close-tabs one, where a tab's programs share a line (`uncommitted
/// diff`); `None` for any other program, the pager among them, which
/// lists by its command and file.
fn app_of(args: &[String], own_exe: &str, short: bool) -> Option<String> {
    let (first, rest) = args.split_first()?;
    // A rewritten title (`npm exec @team/switchboard-notes`, or one holding
    // a link) is never one, unless it is the very path of the binary the
    // server runs.
    if first != own_exe && (title_of(first).is_some() || has_link(first)) {
        return None;
    }
    // Switchboard's own command and its words: `switchboard`, or the
    // binary the server runs by its path, which starts the views under
    // whatever name it has.
    let mut words: Vec<&str> = match program_name(first).as_str() {
        "switchboard-notes" => vec!["notes"],
        "switchboard-diff" => vec!["diff"],
        _ if first == own_exe => vec![],
        "switchboard" => vec![],
        _ => return None,
    };
    words.extend(rest.iter().map(String::as_str));
    let (long, brief) = match words.as_slice() {
        ["notes", "away", ..] => ("the place Notes left in its tab", "Notes' place"),
        // The app itself, which takes no words; `open`, `shown` and the
        // rest only ask it something.
        ["notes"] => ("Notes", "Notes"),
        // Not the story told in the background (`--ahead`, `--plan`),
        // which draws nothing.
        ["diff", rest @ ..] if rest.iter().any(|a| *a == "--ahead" || *a == "--plan") => {
            return None;
        }
        ["diff", rest @ ..] if rest.contains(&"--working") => {
            ("the diff of uncommitted changes", "uncommitted diff")
        }
        ["diff", ..] => ("the diff", "diff"),
        ["pr-view", "--checks", ..] => ("the PR's failing checks", "PR checks"),
        ["pr-view", ..] => ("the PR view", "PR view"),
        ["ticket-view", ..] => ("the ticket view", "ticket view"),
        ["link-view", ..] => ("the link view", "link view"),
        ["pr-image", "--ticket", ..] => ("the ticket's images", "ticket images"),
        ["pr-image", ..] => ("the PR's images", "PR images"),
        ["aws-login", ..] => ("the AWS sign-in", "AWS sign-in"),
        ["setup", ..] => ("Switchboard's setup", "setup"),
        ["update", rest @ ..] if rest.contains(&"--setup") => ("Switchboard's setup", "setup"),
        _ => return None,
    };
    Some(if short { brief } else { long }.to_string())
}

/// Whether a word holds a link (`http://` or `https://`, in any case).
fn has_link(word: &str) -> bool {
    let lower = word.to_ascii_lowercase();
    lower.contains("http://") || lower.contains("https://")
}

/// A word holding a link up to the link's scheme, then `…`
/// (`next=https://…`), when nothing of the rest can be told apart safely.
fn scheme_only(word: &str) -> String {
    let lower = word.to_ascii_lowercase();
    let end = ["https://", "http://"]
        .iter()
        .filter_map(|scheme| lower.find(scheme).map(|at| at + scheme.len()))
        .min()
        .unwrap_or(word.len());
    format!("{}…", &word[..end])
}

/// A word holding a link (`http://`, `https://`, in any case, anywhere
/// in it) up to the link's host, its user and password left out and an
/// `…` for whatever follows the host, where a link keeps its credentials
/// (`--url=https://a.dev/…`); any other word as it is.
fn without_query(word: &str) -> String {
    let lower = word.to_ascii_lowercase();
    let Some((start, scheme)) = ["https://", "http://"]
        .iter()
        .filter_map(|scheme| lower.find(scheme).map(|at| (at, scheme.len())))
        .min()
    else {
        return word.to_string();
    };
    let authority_at = start + scheme;
    let rest = &word[authority_at..];
    // Its host, to the first character a host or port can't hold; none
    // when an `@` comes before the link's path, query or fragment, since
    // whatever is before it is a user and password, which may hold any
    // punctuation, so nothing of it is shown.
    let path_at = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host_end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || ".-_:[]".contains(c)))
        .unwrap_or(rest.len());
    // Shown only when it ends cleanly, at a path, a query, a fragment or
    // the word's end, so an escaped `@` or a quote can't hide a user.
    let ends_cleanly = host_end == path_at;
    let host = if rest[..path_at].contains('@') || !ends_cleanly {
        ""
    } else {
        &rest[..host_end]
    };
    let more = match () {
        _ if host.len() == rest.len() => "",
        _ if host.is_empty() => "…",
        _ => "/…",
    };
    format!("{}{host}{more}", &word[..authority_at])
}

/// A title a program wrote over its arguments (npm on Linux: `npm exec
/// @scope/x` all in its first one), as its program and the rest; `None`
/// for a word alone or a path, one with a space in a folder's name
/// included (`./Build Tools/bin/worker`), whose first word holds a `/`.
fn title_of(first: &str) -> Option<(&str, &str)> {
    // A path's title too when a link follows it, so the link's words are
    // shown as a title's are, never as part of the program's name.
    first
        .split_once(' ')
        .filter(|(exe, rest)| !exe.contains('/') || has_link(rest))
}

/// Whether a program is a shell, by its name (a login shell's has a `-`
/// before it).
fn is_shell(command: &str) -> bool {
    const SHELLS: &[&str] = &[
        "sh", "bash", "zsh", "fish", "dash", "ksh", "mksh", "oksh", "yash", "ash", "tcsh", "csh",
        "nu", "elvish", "xonsh", "pwsh",
    ];
    let name = command.trim_start_matches('-');
    let name = name.rsplit('/').next().unwrap_or(name);
    SHELLS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_close_question_lists_a_program_by_its_name() {
        let line = |args: &[&str]| {
            let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            program_line(&args, "/opt/sb/switchboard-0.2.0")
        };
        // As the formatter matches it, without a trailing slash.
        let home = crate::paths::home().display().to_string();
        let home = home.trim_end_matches('/');
        assert_eq!(line(&["/usr/local/bin/npm", "run", "dev"]), "npm run dev");
        assert_eq!(
            line(&["/usr/bin/vim", &format!("{home}/code/a.rs")]),
            "vim ~/code/a.rs"
        );
        assert_eq!(
            line(&["node", &format!("--dir={home}/code")]),
            "node --dir=~/code"
        );
        // A literal holding `=` and a path on another host stay as they
        // are (however long the home, which the 60 columns may cut).
        let literal = line(&["grep", &format!("p={home}/x")]);
        assert!(literal.starts_with("grep p=/") && !literal.contains('~'));
        let remote = line(&["scp", &format!("host:{home}/f"), "."]);
        assert!(remote.starts_with("scp host:/") && !remote.contains('~'));
        assert_eq!(line(&["lazygit"]), "lazygit");
        assert_eq!(
            line(&["rsync", &format!("{home}/p/"), "b:d"]),
            "rsync ~/p/ b:d"
        );
        assert_eq!(
            line(&["rsync", "-R", &format!("{home}/./d/"), "b/"]),
            "rsync -R ~/./d/ b/"
        );
        // A word holding a link shows it up to its host, nothing of its
        // user, password, path or query.
        let link = |args: &[&str], shown: &str| assert_eq!(line(args), shown);
        link(
            &["link-view", "https://a.dev/f?token=secret"],
            "link-view https://a.dev/…",
        );
        link(&["worker https://a.dev/f?token=secret"], "worker https://…");
        link(&["/usr/bin/worker https://a.dev"], "worker https://…");
        link(&["https://a.dev/f?token=secret"], "https://…");
        link(&["https://a.dev/f?sig=x https://b.dev/"], "https://…");
        link(
            &["worker https://a.dev/?q=two words&token=secret"],
            "worker https://…",
        );
        link(
            &["/usr/bin/worker https://a.dev/f?token=secret"],
            "worker https://…",
        );
        link(&["curl", "-4xhttp://u:pw@p.dev"], "curl -4xhttp://…");
        link(
            &["curl", "-d", "one=https://a.dev/&two=https://u:pw@b.dev/"],
            "curl -d one=https://a.dev/…",
        );
        link(
            &["wget", "--post-data=next=https://a.dev/?sig=x"],
            "wget --post-data=next=https://a.dev/…",
        );
        link(
            &["git", "-c", "http.proxy=https://u:pw@p.dev", "fetch"],
            "git -c http.proxy=https://… fetch",
        );
        link(
            &["curl", "--url=HTTPS://me:pw@a.dev"],
            "curl --url=HTTPS://…",
        );
        link(&["curl", "https://a.dev"], "curl https://a.dev");
        link(
            &["curl", "https://a.dev:8080/x"],
            "curl https://a.dev:8080/…",
        );
        link(
            &["curl", "next=https://me:s%40a.dev/f"],
            "curl next=https://…",
        );
        link(
            &["curl", "-d", "{\"u\":\"https://al:s\\u0040a.dev/f\"}"],
            "curl -d {\"u\":\"https://…",
        );
        link(&["curl", "https://w:s!@a.test/f"], "curl https://…");
        link(
            &["curl", "-d", "{\"url\":\"https://a.dev\",\"token\":\"x\"}"],
            "curl -d {\"url\":\"https://…",
        );
        link(
            &["curl", "--request-target=/r?next=https://a.dev/?sig=x"],
            "curl --request-target=/r?next=https://a.dev/…",
        );
        link(&["rg", "https?://a.dev"], "rg https?://a.dev");
        link(&["rg", "https:/a.dev/v1?"], "rg https:/a.dev/v1?");
        assert_eq!(line(&["./Build Tools/bin/worker", "-v"]), "worker -v");
        assert_eq!(
            line(&["npm exec @angular/cli serve"]),
            "npm exec @angular/cli serve"
        );
        let bin = "/usr/local/bin/switchboard".to_string();
        assert_eq!(line(&[&bin, "notes"]), "Notes");
        assert_eq!(line(&[&bin, "notes", "shown"]), "switchboard notes shown");
        assert_eq!(
            line(&[&bin, "notes", "away"]),
            "the place Notes left in its tab"
        );
        assert_eq!(line(&[&bin, "diff"]), "the diff");
        assert_eq!(
            line(&[&bin, "diff", "--working"]),
            "the diff of uncommitted changes"
        );
        assert_eq!(line(&[&bin, "diff", "--ahead"]), "switchboard diff --ahead");
        assert_eq!(line(&[&bin, "pr-view", "/x"]), "the PR view");
        assert_eq!(
            line(&[&bin, "pr-view", "--checks", "/x"]),
            "the PR's failing checks"
        );
        assert_eq!(line(&[&bin, "ticket-view", "X-1"]), "the ticket view");
        assert_eq!(
            line(&[&bin, "link-view", "https://a.dev/x"]),
            "the link view"
        );
        assert_eq!(
            line(&[&bin, "pr-image", "--ticket", "X-1"]),
            "the ticket's images"
        );
        assert_eq!(
            line(&[&bin, "view", "--log", &format!("{home}/job.log")]),
            "switchboard view --log ~/job.log"
        );
        assert_eq!(line(&[&bin, "tabs"]), "switchboard tabs");
        assert_eq!(line(&[&bin, "aws-login"]), "the AWS sign-in");
        assert_eq!(line(&[&bin, "setup", "--check"]), "Switchboard's setup");
        assert_eq!(
            line(&[&bin, "update", "--setup", "--hold"]),
            "Switchboard's setup"
        );
        assert_eq!(line(&["/opt/sb/switchboard-0.2.0", "diff"]), "the diff");
        assert_eq!(
            line(&["switchboard-backup", "notes"]),
            "switchboard-backup notes"
        );
        assert_eq!(line(&["/opt/x/pr-view", "--checks"]), "pr-view --checks");
        let brief = |args: &[&str]| {
            let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            app_of(&args, "/opt/sb/switchboard-0.2.0", true)
        };
        assert_eq!(
            brief(&[&bin, "diff", "--working"]).as_deref(),
            Some("uncommitted diff")
        );
        assert_eq!(brief(&["vim"]), None);
        assert_eq!(brief(&["npm exec @team/switchboard-notes"]), None);
        assert_eq!(brief(&["https://a.test/switchboard-notes"]), None);
        let spaced = "/opt/Sb Tools/switchboard".to_string();
        let diff = [spaced.clone(), "diff".to_string(), "--working".to_string()];
        let spaced_diff = [
            "/opt/Sb Tools/switchboard-diff".to_string(),
            "--working".to_string(),
        ];
        assert_eq!(
            app_of(&spaced_diff, "", true).as_deref(),
            Some("uncommitted diff")
        );
        assert_eq!(
            app_of(&diff, &spaced, true).as_deref(),
            Some("uncommitted diff")
        );
        let notes = "/x/switchboard-notes".to_string();
        assert_eq!(
            app_of(std::slice::from_ref(&notes), &notes, false).as_deref(),
            Some("Notes")
        );
        assert_eq!(line(&["/usr/bin/diff", "a", "b"]), "diff a b");
        let long = line(&["/bin/sleep", &"9".repeat(80)]);
        assert_eq!(draw::width(&long), 60);
        assert!(long.starts_with("sleep 999"));
    }

    #[test]
    fn a_program_is_named_without_its_folder() {
        assert_eq!(program_name("/usr/local/bin/claude"), "claude");
        assert_eq!(
            program_name("/Applications/Visual Studio Code.app/Contents/MacOS/Electron"),
            "Electron"
        );
        assert_eq!(program_name("sleep"), "sleep");
    }

    #[test]
    fn shells_are_known_by_name() {
        assert!(is_shell("zsh"));
        assert!(is_shell("-zsh"));
        assert!(is_shell("/bin/sh"));
        assert!(!is_shell("sleep"));
        assert!(!is_shell("claude"));
        assert!(!is_shell(""));
    }
}
