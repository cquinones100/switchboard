//! The PR description as GitHub draws it, as pictures in the pane, beside
//! the PR view (M-v), which draws it as text: `switchboard pr-image [dir]`
//! for the branch's PR, and `pr-image --ticket <ID>` for a Linear ticket,
//! each opened by M-p in place of the pane it is asked from, as M-d's diff
//! is (src/diff/in_place.rs), its pane alone allowed to send the terminal
//! pictures (`@allow_passthrough`). Ported from tmux/pr-image.js and
//! pane-in-place.sh's `pr-image` and `ticket-image`.
//!
//! - `page`: the page, from the PR's HTML or the ticket's Markdown.
//! - `render`: Chrome drawing it, and the pages cut from its picture.
//! - `cache`: what the page loads and the pages drawn, kept (the
//!   image_cache setting).
//!
//! The PR or ticket as last read is drawn at once (from
//! `~/.cache/switchboard/pr-image/`, named as pr-image.js named it), its
//! pages shown from the cache without running Chrome when they were drawn
//! before at this size, and read again while it shows: the read is always
//! kept, for its pictures' fresh signatures, and the pages are drawn again
//! where they were when it changed, or when something the pages shown
//! loaded had failed. Each page is shown through the kitty graphics
//! protocol's Unicode placeholders, as the notes app draws a note's
//! pictures (src/notes/images.rs): the terminal is given the page's PNG by
//! its path, and the pane is filled with placeholder cells coloured with
//! the picture's id, which the multiplexer places as text (and, with
//! Settings' "Image protocol" on sixel, draws the page over as sixel
//! itself, src/mux/sixel, for Windows Terminal). Two ids take turns, a
//! page taking the one the page before it didn't, so every cell changes
//! from one page to the next and is drawn again. j, k, the arrows,
//! Space and b page; g and G go to the ends; r draws the page again; q
//! gives the pane back. A resize lays the pages out again for the new size,
//! once the resizes stop, and shows the page holding what was at the top.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal;
use serde_json::{Value, json};

use crate::diff::in_place::IN_PLACE;
use crate::integrations::{github, linear};
use crate::mux::server::{ClientId, Event as ServerEvent, Server};
use crate::notes::images::{self, Picture};
use crate::ui::Confirm;
use crate::views::ViewsEvent;
use crate::{paths, settings, util};

pub mod cache;
pub mod page;
pub mod render;

use cache::Cache;
use page::Parts;
use render::{Drawing, NEEDS_CHROME, Renderer, Shot, View};

/// The tools' names in `@in_place`, as pane-in-place.sh named them.
pub const PR_TOOL: &str = "pr-image";
pub const TICKET_TOOL: &str = "ticket-image";

/// What asked, for the request audit.
const SOURCE: &str = "PR as an image";
/// How long a run of resizes has to stop for before the pages are laid out
/// again.
const SETTLE: Duration = Duration::from_millis(250);

/// What the view shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asked {
    /// The PR of the branch checked out in this directory.
    Pr(PathBuf),
    /// A Linear ticket, by its identifier in capitals.
    Ticket(String),
}

impl Asked {
    fn what(&self) -> &'static str {
        match self {
            Asked::Pr(_) => "PR",
            Asked::Ticket(_) => "ticket",
        }
    }
}

/// The arguments: `--ticket <ID>`, else the directory (where it runs, by
/// default).
pub fn asked(args: &[String]) -> Result<Asked, String> {
    match args {
        [flag, id] if flag == "--ticket" => crate::views::ticket::id_of(id)
            .map(Asked::Ticket)
            .ok_or_else(|| "That isn't a Linear ticket ID.".to_string()),
        [flag] if flag == "--ticket" => Err("Which ticket? pr-image --ticket <ID>".into()),
        [] => Ok(Asked::Pr(
            std::env::current_dir().unwrap_or_else(|_| paths::home()),
        )),
        [dir] => Ok(Asked::Pr(PathBuf::from(dir))),
        _ => Err("Usage: switchboard pr-image [dir] | --ticket <ID>".into()),
    }
}

/// The file the last copy is kept in: the ticket's ID, or the checkout's
/// git directory and branch, each `/` a `%`.
fn cache_file(asked: &Asked) -> Option<PathBuf> {
    let key = match asked {
        Asked::Ticket(id) => id.clone(),
        Asked::Pr(dir) => {
            let place = crate::tabs::git::locate(dir)?;
            format!("{}:{}", place.git_dir.display(), place.branch)
        }
    };
    Some(
        paths::cache_dir()
            .join("pr-image")
            .join(format!("{}.json", key.replace('/', "%"))),
    )
}

/// The PR or ticket read anew, or why it can't be, in words for the person.
async fn fetch(asked: &Asked) -> Result<Parts, String> {
    match asked {
        Asked::Pr(dir) => {
            let found = github::branch_pr(dir, SOURCE)
                .await
                .map_err(|e| e.to_string())?;
            let Some((repo, pr)) = found else {
                return Err("No PR for this branch.".into());
            };
            let full = github::rest_rendered(
                &format!("repos/{}/{}/pulls/{}", repo.owner, repo.name, pr.number),
                SOURCE,
            )
            .await
            .map_err(|e| format!("Couldn't read the description as GitHub renders it: {e}"))?;
            Ok(page::pr_parts(&full))
        }
        Asked::Ticket(id) => {
            let data = linear::graphql(page::TICKET_QUERY, json!({ "id": id }), SOURCE)
                .await
                .map_err(|e| format!("Couldn't read {id} from Linear: {e}"))?;
            data.get("issue")
                .filter(|i| i.is_object())
                .map(page::ticket_parts)
                .ok_or_else(|| format!("Linear has no ticket {id}."))
        }
    }
}

/// The terminal's cell in device pixels. The workspace gives its panes the
/// cell the attached terminal says it has (src/mux/terminal.rs), and a
/// stand-in of 8 by 16 where it doesn't say, as a terminal that doesn't is
/// taken to have too (`images::cell_size`); pr-image.js guessed a 13-point
/// font on a Retina screen when the terminal didn't say, so that stand-in
/// is doubled, and a page is laid out about as wide as the text around it.
fn cell() -> (u32, u32) {
    let (w, h) = images::cell_size();
    if (w, h) == (8, 16) { (16, 32) } else { (w, h) }
}

/// A kitty graphics command, wrapped for tmux when the view runs inside it.
fn graphics(body: &str) -> String {
    let seq = format!("\x1b_G{body}\x1b\\");
    if std::env::var_os("TMUX").is_some_and(|t| !t.is_empty()) {
        format!("\x1bPtmux;{}\x1b\\", seq.replace('\x1b', "\x1b\x1b"))
    } else {
        seq
    }
}

/// The two ids the pages take turns with: 24 bits each, never 0, and this
/// view's own, so two views open at once don't take each other's pictures.
fn ids_for(pid: u32) -> [u32; 2] {
    let base = ((pid as u64).wrapping_mul(2_654_435_761) % 0x7f_ffff) as u32 + 1;
    [base, base | 0x80_0000]
}

/// The bytes that show `shot` in a pane `cols` wide as picture `id`: the
/// screen cleared first (clearing it after would delete the picture just
/// sent with it), both ids' pictures taken back, the page given by its
/// path, and its placeholder cells, each carrying its own row and column.
pub fn page_bytes(shot: &Shot, cols: u16, id: u32, ids: [u32; 2]) -> String {
    let mut out = String::from("\x1b[H\x1b[2J");
    for old in ids {
        out.push_str(&graphics(&format!("a=d,d=I,i={old},q=2")));
    }
    out.push_str(&graphics(&format!(
        "a=T,U=1,f=100,t=f,q=2,i={id},c={cols},r={};{}",
        shot.rows,
        util::base64(shot.file.to_string_lossy().as_bytes())
    )));
    out.push_str(&format!(
        "\x1b[38;2;{};{};{}m",
        (id >> 16) & 0xff,
        (id >> 8) & 0xff,
        id & 0xff
    ));
    for row in 0..shot.rows {
        out.push_str(&format!("\x1b[{};1H", row + 1));
        for col in 0..cols {
            out.push_str(&Picture::cell(row, col));
        }
    }
    out.push_str("\x1b[39m");
    out
}

/// What the work off the view's own thread hands back.
enum Msg {
    Fresh(Result<Parts, String>),
    Laid(u64, Result<Drawing, String>),
}

struct App {
    asked: Asked,
    /// Where the PR or ticket as last read is kept.
    last_read: Option<PathBuf>,
    renderer: Renderer,
    cell: (u32, u32),
    view: View,
    parts: Option<Parts>,
    shots: Vec<Shot>,
    at: usize,
    layouts: u64,
    /// A layout is being made; another is wanted after it.
    busy: bool,
    again: bool,
    /// Everything the pages shown loaded did; until then, a read of the
    /// same text draws them again.
    complete: bool,
    /// A read of the same text came while a layout was being made, which
    /// is made again if it turns out something failed to load.
    recheck: bool,
    /// The footer's note: why the last layout failed, or that the page's
    /// scripts didn't load.
    said: String,
    failed: Option<String>,
    ids: [u32; 2],
    drawings: u64,
    resized: Option<(Instant, u16, u16)>,
    tx: Sender<Msg>,
}

impl App {
    fn write(&self, text: &str) {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(text.as_bytes());
        let _ = out.flush();
    }

    fn footer(&self, text: &str) {
        self.write(&format!(
            "\x1b[{};1H\x1b[2K\x1b[38;5;244m {text}\x1b[0m",
            self.view.rows + 1
        ));
    }

    fn fail(&mut self, message: &str) {
        self.failed = Some(message.to_string());
        self.write(&format!(
            "\x1b[H\x1b[2J\r\n  \x1b[31m{message}\x1b[0m\r\n\r\n  \x1b[38;5;244mPress any key to close.\x1b[0m"
        ));
    }

    /// Lays the pages out for the view as it is, off this thread; one asked
    /// for while one is being made is made after it. Pages drawn before for
    /// this text at this size are shown from the cache, without Chrome.
    fn lay_out(&mut self) {
        let Some(parts) = self.parts.clone() else {
            return;
        };
        if self.busy {
            self.again = true;
            return;
        }
        self.busy = true;
        self.layouts += 1;
        let kept = self.renderer.cache.as_ref();
        if let Some(shots) = kept.and_then(|c| c.pages_for(&parts, &self.view)) {
            let drawing = Drawing {
                shots,
                loaded: true,
                complete: true,
            };
            return self.laid(self.layouts, Ok(drawing));
        }
        if self.shots.is_empty() {
            self.footer("Drawing it…");
        }
        let (n, view, renderer, tx) = (
            self.layouts,
            self.view,
            self.renderer.clone(),
            self.tx.clone(),
        );
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Laid(n, renderer.render(&parts, &view, n)));
        });
    }

    fn laid(&mut self, n: u64, result: Result<Drawing, String>) {
        self.busy = false;
        match result {
            Ok(drawing) => {
                let from = self.shots.get(self.at).map_or(0.0, |s| s.from);
                self.at = drawing
                    .shots
                    .iter()
                    .rposition(|s| s.from <= from + 1e-6)
                    .unwrap_or(0);
                self.shots = drawing.shots;
                self.complete = drawing.complete;
                self.renderer.forget_others(n);
                self.said = if drawing.loaded {
                    String::new()
                } else {
                    "drawn without Mermaid and marked: the CDN couldn't be reached".into()
                };
            }
            Err(e) if self.shots.is_empty() => return self.fail(&e),
            Err(e) => self.said = e,
        }
        if std::mem::take(&mut self.recheck) && !self.complete {
            self.again = true;
        }
        if self.again {
            self.again = false;
            self.lay_out();
        } else {
            self.draw();
        }
    }

    /// The PR or ticket read anew: kept even when it shows the same, for
    /// its pictures' fresh signatures, and drawn again when it changed, or
    /// when something the pages shown loaded had failed.
    fn fresh(&mut self, result: Result<Parts, String>) {
        match result {
            Ok(parts) => {
                if self.parts.as_ref() != Some(&parts)
                    && let Some(file) = &self.last_read
                {
                    let _ = util::write_atomic(file, util::json_compact(&parts));
                }
                let changed = self
                    .parts
                    .as_ref()
                    .is_none_or(|old| cache::same_text(old) != cache::same_text(&parts));
                self.parts = Some(parts);
                if changed {
                    self.lay_out();
                } else if self.busy {
                    self.recheck = true;
                } else if !self.complete {
                    self.lay_out();
                }
            }
            Err(e) if self.parts.is_none() => self.fail(&e),
            Err(_) => {}
        }
    }

    fn draw(&mut self) {
        let Some(shot) = self.shots.get(self.at) else {
            return;
        };
        let id = self.ids[(self.drawings % 2) as usize];
        self.drawings += 1;
        self.write(&page_bytes(shot, self.view.cols, id, self.ids));
        let mut text = format!(
            "page {}/{}   j k space b: page   r: redraw   q: close",
            self.at + 1,
            self.shots.len()
        );
        if !self.said.is_empty() {
            text.push_str(&format!("   \x1b[31m{}", self.said));
        }
        self.footer(&text);
    }

    /// A key; false once the view is to close.
    fn key(&mut self, code: KeyCode, mods: KeyModifiers) -> bool {
        if self.failed.is_some() {
            return false;
        }
        let ctrl_c = code == KeyCode::Char('c') && mods.contains(KeyModifiers::CONTROL);
        if matches!(code, KeyCode::Char('q') | KeyCode::Esc) || ctrl_c {
            return false;
        }
        if self.busy || self.shots.is_empty() {
            return true;
        }
        let last = self.shots.len() - 1;
        match code {
            KeyCode::Char('j' | ' ') | KeyCode::Down | KeyCode::PageDown => {
                self.at = (self.at + 1).min(last)
            }
            KeyCode::Char('k' | 'b') | KeyCode::Up | KeyCode::PageUp => {
                self.at = self.at.saturating_sub(1)
            }
            KeyCode::Char('g') | KeyCode::Home => self.at = 0,
            KeyCode::Char('G') | KeyCode::End => self.at = last,
            KeyCode::Char('r') => {}
            _ => return true,
        }
        self.draw();
        true
    }

    /// Once the resizes stop: the pages laid out for the pane's new size.
    fn settle(&mut self) {
        let Some((at, cols, rows)) = self.resized else {
            return;
        };
        if at.elapsed() < SETTLE {
            return;
        }
        self.resized = None;
        self.view = View::new(cols, rows, self.cell);
        if self.failed.is_some() || self.parts.is_none() {
            return;
        }
        self.write("\x1b[H\x1b[2J");
        self.footer("Laying it out for the new size…");
        self.lay_out();
    }
}

/// The terminal as the view wants it while it runs, and put back however it
/// ends: raw, on the alternate screen, the cursor hidden, and the pictures
/// taken back.
struct Screen {
    ids: [u32; 2],
}

impl Screen {
    fn enter(ids: [u32; 2]) -> std::io::Result<Screen> {
        terminal::enable_raw_mode()?;
        let mut out = std::io::stdout();
        out.write_all(b"\x1b[?1049h\x1b[?25l\x1b[H\x1b[2J")?;
        out.flush()?;
        Ok(Screen { ids })
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let mut out = std::io::stdout();
        for id in self.ids {
            let _ = out.write_all(graphics(&format!("a=d,d=I,i={id},q=2")).as_bytes());
        }
        let _ = out.write_all(b"\x1b[0m\x1b[H\x1b[2J\x1b[?25h\x1b[?1049l");
        let _ = out.flush();
        let _ = terminal::disable_raw_mode();
    }
}

/// The view's working directory, gone when it ends.
struct Work(PathBuf);

impl Drop for Work {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Why the view can't draw at all, said in place of the pages: no Chrome
/// to draw them with (and no pages kept to show without it), or a terminal
/// that can't show them.
pub fn cannot_draw(chrome: bool, terminal: bool) -> Option<&'static str> {
    if !chrome {
        Some(NEEDS_CHROME)
    } else if !terminal {
        Some(
            "The image view needs a terminal that draws kitty's pictures, as Ghostty and kitty do, or, in the workspace, one that draws sixel, as Windows Terminal does, with Settings' Image protocol on sixel. M-v shows the description as text.",
        )
    } else {
        None
    }
}

/// `switchboard pr-image [dir] | --ticket <ID>`.
pub fn main(args: Vec<String>) -> ExitCode {
    let asked = match asked(&args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let last_read = cache_file(&asked);
    let cached: Option<Parts> = last_read
        .as_ref()
        .and_then(util::read_json)
        .and_then(|v: Value| serde_json::from_value(v).ok());
    let kept = settings::is_on("image_cache").then(Cache::here);
    let work =
        Work(std::env::temp_dir().join(format!("switchboard-pr-image-{}", std::process::id())));
    // For checking the drawing without a terminal: the pages as PNGs, and out.
    if let Some(out) = std::env::var_os("PR_IMAGE_DUMP") {
        return dump(&asked, cached, kept, &work.0, Path::new(&out));
    }
    let ids = ids_for(std::process::id());
    let screen = match Screen::enter(ids) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("switchboard pr-image: can't take the terminal: {e}");
            return ExitCode::FAILURE;
        }
    };
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let (tx, rx): (Sender<Msg>, Receiver<Msg>) = channel();
    let cell = cell();
    let view = View::new(cols, rows, cell);
    let chrome = render::find_chrome();
    // The pages kept for the text last read at this size, shown without
    // Chrome; and both caches held to their limit as the view starts, off
    // its thread, those pages kept.
    let ready = match (&kept, &cached) {
        (Some(c), Some(p)) => c.pages_for(p, &view).map(|_| cache::page_key(p, &view)),
        _ => None,
    };
    if let Some(c) = kept.clone() {
        let keep = ready.clone();
        std::thread::spawn(move || {
            c.prune(cache::LIMIT, std::time::SystemTime::now(), keep.as_deref())
        });
    }
    let mut app = App {
        asked: asked.clone(),
        last_read,
        renderer: Renderer {
            chrome: chrome.clone(),
            work: work.0.clone(),
            cache: kept,
        },
        cell,
        view,
        parts: None,
        shots: Vec::new(),
        at: 0,
        layouts: 0,
        busy: false,
        again: false,
        complete: true,
        recheck: false,
        said: String::new(),
        failed: None,
        ids,
        drawings: 0,
        resized: None,
        tx: tx.clone(),
    };
    if let Some(why) = cannot_draw(chrome.is_some() || ready.is_some(), images::terminal_can()) {
        app.fail(why);
    } else {
        match cached {
            Some(parts) => {
                app.parts = Some(parts);
                app.lay_out();
            }
            None => app.footer(&format!("Reading the {}…", app.asked.what())),
        }
        let fetching = asked.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Fresh(util::block_on(fetch(&fetching))));
        });
    }
    loop {
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Fresh(result) => app.fresh(result),
                Msg::Laid(n, result) => app.laid(n, result),
            }
        }
        app.settle();
        match event::poll(Duration::from_millis(100)) {
            Ok(true) => match event::read() {
                Ok(Event::Key(k)) if k.kind != KeyEventKind::Release => {
                    if !app.key(k.code, k.modifiers) {
                        break;
                    }
                }
                Ok(Event::Resize(c, r)) => app.resized = Some((Instant::now(), c, r)),
                Ok(_) => {}
                Err(_) => break,
            },
            Ok(false) => {}
            Err(_) => break,
        }
    }
    drop(screen);
    drop(work);
    ExitCode::SUCCESS
}

/// `PR_IMAGE_DUMP=<dir>`: the pages written there as `page-<n>.png`, for a
/// pane of the terminal's size (or 100 by 50), with no terminal needed:
/// those kept, when the cache holds them, else drawn.
fn dump(
    asked: &Asked,
    cached: Option<Parts>,
    kept: Option<Cache>,
    work: &Path,
    out: &Path,
) -> ExitCode {
    let chrome = render::find_chrome();
    if chrome.is_none() && cached.is_none() {
        eprintln!("{NEEDS_CHROME}");
        return ExitCode::FAILURE;
    }
    let parts = match cached {
        Some(p) => p,
        None => match util::block_on(fetch(asked)) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
        },
    };
    let (cols, rows) = terminal::size().unwrap_or((100, 50));
    let view = View::new(cols, rows, cell());
    let drawn = match kept.as_ref().and_then(|c| c.pages_for(&parts, &view)) {
        Some(shots) => Ok(Drawing {
            shots,
            loaded: true,
            complete: true,
        }),
        None => Renderer {
            chrome,
            work: work.to_path_buf(),
            cache: kept,
        }
        .render(&parts, &view, 1),
    };
    match drawn {
        Ok(drawing) => {
            let _ = std::fs::create_dir_all(out);
            for (i, shot) in drawing.shots.iter().enumerate() {
                let _ = std::fs::copy(&shot.file, out.join(format!("page-{}.png", i + 1)));
            }
            eprintln!(
                "{} page(s) in {}{}",
                drawing.shots.len(),
                out.display(),
                if drawing.loaded {
                    ""
                } else {
                    " (the page's scripts didn't load)"
                }
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

// ── The server's side ───────────────────────────────────────────────────────

/// This binary, for the tool's pane.
fn exe() -> String {
    util::exe()
        .ok()
        .filter(|p| p.exists())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "switchboard".into())
}

impl Server {
    /// M-p's "View the branch's PR description as an image" (`ticket`
    /// false) and "View the branch's Linear ticket as an image": opened in
    /// place of the pane, or closed when asked for again from it or from
    /// the pane it stands in for, as tmux.conf's binding checked
    /// `@in_place` first.
    pub fn image_view(&mut self, client: ClientId, ticket: bool) {
        let (tool, service) = if ticket {
            (TICKET_TOOL, "linear")
        } else {
            (PR_TOOL, "github")
        };
        if !settings::is_on(service) {
            self.service_off(client, service);
            return;
        }
        let Some(pane) = self.current_pane(client) else {
            return;
        };
        if let Some(open) = self.tool_for(pane).filter(|t| {
            self.panes
                .get(t)
                .and_then(|s| s.opts.get(IN_PLACE))
                .is_some_and(|v| v == tool)
        }) {
            self.kill_pane(open);
            return;
        }
        let Some((tab, dir)) = self
            .panes
            .get(&pane)
            .map(|s| (s.tab, self.pane_dir(pane).unwrap_or_else(paths::home)))
        else {
            return;
        };
        let mut command = vec![exe(), "pr-image".into()];
        if ticket {
            let Some(id) = self.ticket_here(tab, &dir) else {
                self.say(Some(client), "No Linear ticket in this branch or tab name");
                return;
            };
            command.extend(["--ticket".into(), id]);
        } else {
            command.push(dir.to_string_lossy().into_owned());
        }
        match self.open_in_place(pane, tool, command, Some(client)) {
            Ok(view) => {
                // Its pane alone sends the terminal pictures, as
                // pane-in-place.sh set allow-passthrough for it.
                if let Some(slot) = self.panes.get_mut(&view) {
                    slot.opts.insert("@allow_passthrough".into(), "1".into());
                }
            }
            Err(e) => self.say(Some(client), &format!("Couldn't open the image view: {e}")),
        }
    }

    /// M-p's "Clear the PR and ticket image cache": the copies, the pages
    /// drawn and the text last read removed after a yes, off the server's
    /// task, and said once they are.
    pub fn clear_image_cache(&mut self, client: ClientId) {
        self.ask(
            client,
            Confirm::new("Clear the PR and ticket image cache?"),
            |server, client, yes| {
                if !yes {
                    return;
                }
                let tx = server.tx.clone();
                tokio::task::spawn_blocking(move || {
                    Cache::here().clear();
                    let text = "The PR and ticket image cache is cleared.".to_string();
                    let _ = tx.send(ServerEvent::Views(ViewsEvent::Say { client, text }));
                });
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arguments_name_a_ticket_or_a_directory() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            asked(&args(&["--ticket", "eng-42"])),
            Ok(Asked::Ticket("ENG-42".into()))
        );
        assert!(asked(&args(&["--ticket", "$(x)"])).is_err());
        assert_eq!(
            asked(&args(&["/tmp/w"])),
            Ok(Asked::Pr(PathBuf::from("/tmp/w")))
        );
        assert!(asked(&args(&["a", "b"])).is_err());
    }

    #[test]
    fn it_says_plainly_when_there_is_no_chrome_to_draw_with() {
        let dir = std::env::temp_dir().join(format!("sb-pr-image-path-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let found = render::chrome_in(Some(dir.as_os_str()), &[]);
        assert_eq!(found, None);
        let said = cannot_draw(found.is_some(), true).unwrap();
        assert!(said.contains("needs Google Chrome or Chromium"), "{said}");
        assert!(cannot_draw(true, false).unwrap().contains("kitty"));
        assert_eq!(cannot_draw(true, true), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_page_is_given_by_its_path_and_drawn_as_placeholders_of_its_id() {
        let shot = Shot {
            file: PathBuf::from("/tmp/page-1-2.png"),
            rows: 3,
            from: 0.5,
        };
        let ids = ids_for(4242);
        assert!(ids[0] != ids[1] && ids.iter().all(|i| *i > 0 && *i < 1 << 24));
        let bytes = page_bytes(&shot, 4, ids[1], ids);
        assert!(
            bytes.starts_with("\x1b[H\x1b[2J"),
            "the screen cleared first"
        );
        for id in ids {
            assert!(bytes.contains(&format!("a=d,d=I,i={id},q=2")));
        }
        assert!(bytes.contains(&format!(
            "a=T,U=1,f=100,t=f,q=2,i={},c=4,r=3;{}",
            ids[1],
            util::base64(b"/tmp/page-1-2.png")
        )));
        assert_eq!(bytes.matches(images::PLACEHOLDER).count(), 12);
        assert!(bytes.contains(&Picture::cell(2, 3)));
    }
}
