//! `switchboard notes` on the screen, ported from notes/lua/notes/init.lua,
//! sidebar.lua's keys and easy.lua's: the list of folders, tags and notes on
//! the left, a note on the right drawn the way Notes shows it, edited with
//! Vim's keys, saved as you go.
//!
//! In a note:
//!
//! ```text
//!   Ctrl-F             find a note (in the list too)
//!   Ctrl-B             show or put away the list (in the list too)
//!   Ctrl-Shift-P       every command, in a palette (in the list too)
//!   Enter              tick a checklist item, fold a heading, follow a link
//!   a click            the same, on a ☐, a heading's ▾ or a link
//!   Ctrl-P             the Format menu, with everything below
//!   the toolbar        a click on a button over the note: B, I, S, H1…
//!   Space b i U x h `  bold, italic, underline, strikethrough, highlight, monospaced
//!   Space 1 2 3 0      title, heading, subheading, body
//!   Space l 8 - 7 q    checklist, bulleted, dashed, numbered, block quote
//!   Space m            monospaced block       Space t  table
//!   Space k            link                   [[       link to a note
//! ```
//!
//! In Insert mode, Enter carries a list on, and Tab and Shift-Tab indent an
//! item or move between a table's cells.
//!
//! A note saves itself a moment after each change, and whenever you leave
//! it, and its file is renamed when its title changes (store.rs). Every save
//! first brings in what changed in the file from outside since it was read
//! (`check`, sync.rs), and a watch on the notes directory brings changes in
//! as they happen; nothing writes a note but `save`. Nothing in a note is
//! sent anywhere but by a squash, after a yes.
//!
//! The screen is drawn into a cell buffer and only what changed is sent,
//! through the multiplexer's renderer, so the app is tested by drawing it
//! into a buffer and reading it back (app/draw.rs).

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use ratatui::layout::Rect;

use super::editor::{Editor, Effect, Mode};
use super::format::{self, Character, Paragraph, Selection};
use super::images::{self, Images};
use super::menu::{self, Act as FormatAct, Ask, FormatMenu, Progress};
use super::palette::{self, Act as PaletteAct};
use super::render::{self, Kind};
use super::sidebar::{self, Item, Row};
use super::socket::{self, Request};
use super::store::{self, Retitled, Store};
use super::sync;
use super::table;
use super::text::Text;
use super::{easy, finder::Finder, squash};
use crate::mux::compose::{Frame, Screen};
use crate::mux::input::{Input, Key, KeyCode, Mods, Mouse, MouseButton, MouseKind, Parser};
use crate::ui::{
    Confirm, Look, Menu, MenuOption, Message, Outcome, Overlay, PickItem, Picker, Step,
};
use crate::{settings, util};

mod draw;

/// What the app's work off its own thread reports.
pub enum Msg {
    /// Bytes from the terminal.
    Input(Vec<u8>),
    /// Something in the notes directory changed.
    Watched,
    /// A squash's merged note, or why there is none.
    Squashed(Result<String, String>),
    /// A request on the socket, and where its reply goes.
    Socket(Request, Sender<String>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    List,
    Note,
}

/// The settings the app follows, read as it starts, and the plugin's own
/// options (init.lua's `M.options`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// A row of buttons over the note (notes_toolbar).
    pub toolbar: bool,
    /// Easy mode (easy_keys), with the keys named at the bottom
    /// (key_help_line).
    pub easy: bool,
    pub key_help: bool,
    /// What a click would press lit under the mouse (hover_highlight).
    pub hover: bool,
    /// Ctrl-Shift-P's palette (notes_palette).
    pub palette: bool,
    /// Ctrl-V pastes the clipboard's image while typing (notes_ctrl_v_image).
    pub ctrl_v_image: bool,
    /// Pictures drawn under their lines (notes_images).
    pub images: bool,
    pub light: bool,
    /// The list shown as the app starts (`NOTES_SIDEBAR=off` starts it
    /// put away, beside a tab).
    pub list: bool,
    /// A click on a link follows it, as in Notes.
    pub follow_links_on_click: bool,
    /// Line numbers relative to the cursor's line, for jumps like 5j.
    pub numbers: bool,
    /// A ticked item and what is under it struck through.
    pub strike_checked: bool,
    /// Ticked items move under the rest, a blank line between.
    pub move_checked_to_bottom: bool,
    pub gap_before_checked: bool,
    /// The M-arrows move between the list, the note and the panes beside
    /// (pane_arrows).
    pub pane_arrows: bool,
    /// Through the system clipboard; off in the tests.
    pub clipboard: bool,
    /// The workspace's server is told of notes moving (a pane of it).
    pub server: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            toolbar: true,
            easy: false,
            key_help: true,
            hover: true,
            palette: true,
            ctrl_v_image: true,
            images: true,
            light: false,
            list: true,
            follow_links_on_click: true,
            numbers: true,
            strike_checked: true,
            move_checked_to_bottom: true,
            gap_before_checked: true,
            pane_arrows: true,
            clipboard: false,
            server: false,
        }
    }
}

impl Options {
    pub fn read() -> Options {
        let all = settings::read_settings();
        let on = |key: &str| {
            all.iter()
                .find(|s| s.key == key)
                .is_none_or(|s| s.value != serde_json::Value::Bool(false))
        };
        Options {
            toolbar: on("notes_toolbar"),
            easy: all
                .iter()
                .any(|s| s.key == "easy_keys" && s.value == serde_json::Value::Bool(true)),
            key_help: on("key_help_line"),
            hover: on("hover_highlight"),
            palette: on("notes_palette"),
            ctrl_v_image: on("notes_ctrl_v_image"),
            images: on("notes_images") && images::terminal_can(),
            light: all
                .iter()
                .any(|s| s.key == "theme" && crate::status::theme::light_for(&s.value)),
            list: std::env::var("NOTES_SIDEBAR").map_or(true, |v| v != "off"),
            pane_arrows: on("pane_arrows"),
            clipboard: true,
            server: crate::mux::socket::in_pane(),
            ..Options::default()
        }
    }
}

/// The note open, and what is known of its file.
struct Open {
    path: PathBuf,
    /// The file as last read or written (sync.lua's base).
    base: Option<String>,
    /// The text as last saved, to tell whether there is anything unsaved.
    saved: Text,
    /// Its title as last saved, so a new one renames the tabs named for it.
    title: String,
    /// Its file went from under it, said once.
    gone: bool,
}

/// What comes of a box when it closes.
enum Then {
    Nothing,
    Squash(Vec<PathBuf>),
    SquashRunning,
    DeleteForGood(PathBuf),
    MoveTo(PathBuf, Vec<String>),
    NewFolder,
    Search,
    RenameNote(PathBuf),
    RenameFolder(String),
    Link(Option<Selection>),
    Palette(Vec<palette::Command>, PaletteFrom),
    Finder,
    Complete,
    Format(FormatFrom),
}

struct Layer {
    overlay: Box<dyn Overlay>,
    then: Then,
}

/// Where the Format menu was opened: the lines it acts on, the selection,
/// and where you were typing.
#[derive(Clone, Copy, Debug)]
struct FormatFrom {
    first: usize,
    last: usize,
    selection: Option<Selection>,
    typing_col: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
struct PaletteFrom {
    list: bool,
    format: FormatFrom,
}

/// A line of the note window as drawn: a piece of a text line, a closed
/// fold, or a row of a picture under its line.
#[derive(Clone, Debug)]
pub(crate) enum Shown {
    Text {
        row: usize,
        cells: Vec<render::Cell>,
        first: bool,
        indent: u16,
        bg: Option<render::Hl>,
    },
    Fold {
        row: usize,
        text: String,
    },
    Picture {
        row: usize,
        pic: images::Picture,
        r: u16,
    },
}

impl Shown {
    fn row(&self) -> usize {
        match self {
            Shown::Text { row, .. } | Shown::Fold { row, .. } | Shown::Picture { row, .. } => *row,
        }
    }
}

/// Where things were last drawn, for the mouse.
#[derive(Default)]
struct Geo {
    list: Rect,
    note: Rect,
    /// The note's text, right of its numbers.
    text: Rect,
    toolbar: Vec<(usize, Rect)>,
    shown: Vec<Shown>,
}

/// A note shown when there are none: what a note can do.
pub const WELCOME: &str = "# Welcome to Notes

This note shows what a note can do. Change it, or delete it from the list on the left with `dd`. Press Ctrl-P for the Format menu.

## Formatting

**Bold**, *italic*, <u>underlined</u>, ~~struck through~~, ==highlighted== and `monospaced` text, each a Space key away: b, i, U, x, h and `.

### Lists

* A bulleted list
- A dashed list
1. A numbered list
2. Enter carries a list on, and Tab indents an item

### Checklist

- [ ] Click a box, or press Enter on its line, to tick it
- [x] Ticked
- [ ] Press Enter on a heading to fold its section

> A block quote, for what someone said.

```
A monospaced block, for code or anything that needs its spacing kept.
```

| Table | Tab moves between cells |
| ----- | ----------------------- |
| Rows  | keep themselves lined up |

## Links

A web link: https://www.apple.com/macos/, a [link with text](https://example.com), a link to a note: [[Welcome to Notes]], and a #tag.
";

/// How long after the last change a note is saved.
const AUTOSAVE: Duration = Duration::from_millis(800);
/// How long after a change in the directory it is looked at.
const WATCH_WAIT: Duration = Duration::from_millis(100);
const MESSAGE_FOR: Duration = Duration::from_secs(5);

pub struct App {
    pub store: Store,
    pub opts: Options,
    look: Look,
    cols: u16,
    rows: u16,
    pub focus: Focus,
    pub list: sidebar::State,
    pub list_rows: Vec<Row>,
    pub list_at: usize,
    list_top: usize,
    /// The list's cursor when the list last followed it: the list scrolls
    /// to the cursor only when the cursor has moved, so the wheel scrolls it
    /// past the cursor.
    list_followed: Option<usize>,
    /// The wheel moved the note's view: the cursor is brought into it as
    /// the note is next drawn, rather than the view back to the cursor.
    scrolled: bool,
    pub list_shown: bool,
    list_pending: Option<char>,
    pub editor: Editor,
    open: Option<Open>,
    layers: Vec<Layer>,
    /// The bottom line taking a search or an Ex command.
    prompt: Option<(char, String)>,
    pub message: Option<(String, Instant)>,
    leader: bool,
    /// Ctrl-V's next key typed as it is.
    literal: bool,
    images: Images,
    /// Escapes written after the frame: the clipboard's, the pictures'.
    pub out: Vec<u8>,
    last_change: Vec<Key>,
    recording: Option<(Vec<Key>, Text)>,
    replaying: bool,
    save_due: Option<Instant>,
    watch_due: Option<Instant>,
    squash_pid: Option<u32>,
    squash_paths: Vec<PathBuf>,
    squash_cancelled: bool,
    tx: Sender<Msg>,
    cursors: HashMap<PathBuf, (usize, usize, usize)>,
    /// Each note left while the app runs: its undo history and folds, put
    /// back when it opens again.
    kept: HashMap<PathBuf, super::editor::Kept>,
    titles: HashSet<String>,
    pub quit: bool,
    pub locked_by: Option<i64>,
    recent_file: PathBuf,
    /// Saves the clipboard's image to a file (the platform's; a test's).
    pub grab: fn(&Path) -> bool,
    /// The program that squashes notes: Claude Code.
    pub claude: Option<PathBuf>,
    geo: Geo,
    hover_button: Option<usize>,
    /// Where a press in the note landed, to tell a click from a drag.
    press: Option<((usize, usize), bool)>,
    parser: Parser,
    last_input: Instant,
}

fn grab_clipboard(file: &Path) -> bool {
    crate::platform::paste_image(file)
}

/// The list's width for the screen: its full width wherever the note keeps
/// room beside it, else narrower, never under its minimum.
pub fn list_width(cols: u16) -> u16 {
    sidebar::MIN_WIDTH.max(sidebar::WIDTH.min(cols.saturating_sub(60)))
}

impl App {
    pub fn new(store: Store, opts: Options, cols: u16, rows: u16, tx: Sender<Msg>) -> App {
        let mut editor = Editor::new(Text::default());
        editor.clipboard = opts.clipboard;
        editor.easy = opts.easy;
        editor.doc.opts.move_checked_to_bottom = opts.move_checked_to_bottom;
        editor.doc.opts.gap_before_checked = opts.gap_before_checked;
        // The boxes' look, from the settings read as the app started.
        let look = Look {
            hover: opts.hover,
            close: opts.easy,
            theme: if opts.light {
                crate::ui::Theme::LIGHT
            } else {
                crate::ui::Theme::DARK
            },
            ..Look::default()
        };
        App {
            recent_file: store.state.join("notes-palette-recent"),
            store,
            look,
            cols: cols.max(20),
            rows: rows.max(5),
            focus: Focus::Note,
            list: sidebar::State::default(),
            list_rows: Vec::new(),
            list_at: 0,
            list_top: 0,
            list_followed: None,
            scrolled: false,
            list_shown: opts.list,
            list_pending: None,
            editor,
            open: None,
            layers: Vec::new(),
            prompt: None,
            message: None,
            leader: false,
            literal: false,
            images: Images::new(opts.images),
            out: Vec::new(),
            last_change: Vec::new(),
            recording: None,
            replaying: false,
            save_due: None,
            watch_due: None,
            squash_pid: None,
            squash_paths: Vec::new(),
            squash_cancelled: false,
            tx,
            cursors: HashMap::new(),
            kept: HashMap::new(),
            titles: HashSet::new(),
            quit: false,
            locked_by: None,
            grab: grab_clipboard,
            claude: crate::diff::tell::claude_command(),
            geo: Geo::default(),
            hover_button: None,
            press: None,
            parser: Parser::new(),
            last_input: Instant::now(),
            opts,
        }
    }

    /// The palette's picks kept here rather than in the state directory.
    pub fn keep_recent_in(&mut self, file: PathBuf) {
        self.recent_file = file;
    }

    pub fn say(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), Instant::now()));
    }

    /// Takes the notes' lock and opens the note open last, or the newest; a
    /// welcome note when there are none. Another app holding the lock leaves
    /// this one saying so, its notes untouched.
    pub fn start(&mut self) {
        if let Err(pid) = self.store.lock() {
            self.locked_by = Some(pid);
            return;
        }
        let mut notes = self.store.notes(None);
        if notes.is_empty() {
            let path = self.store.create("", Some("Welcome to Notes"));
            let _ = std::fs::write(&path, WELCOME);
            notes = self.store.notes(None);
        }
        let first = self
            .store
            .last()
            .or_else(|| notes.first().map(|n| n.path.clone()));
        if let Some(path) = first {
            self.open_note(&path, false, false);
        } else {
            self.rebuild();
        }
        self.focus = Focus::Note;
    }

    /// Everything saved, the lock let go, the pictures taken back.
    pub fn stop(&mut self) {
        if self.locked_by.is_some() {
            return;
        }
        self.save();
        self.store.unlock();
        let gone = self.images.release_all();
        self.out.extend(gone);
    }

    // ── The note open ─────────────────────────────────────────────────────

    pub fn current_path(&self) -> Option<&Path> {
        self.open.as_ref().map(|o| o.path.as_path())
    }

    fn modified(&self) -> bool {
        self.open
            .as_ref()
            .is_some_and(|o| o.saved != self.editor.doc.text)
    }

    /// Opens a note in the note window. With `insert_title`, the cursor goes
    /// to the end of its first line in Insert mode, for a new note's title;
    /// with `stay`, the focus stays where it is.
    pub fn open_note(&mut self, path: &Path, stay: bool, insert_title: bool) {
        self.save();
        if let Some(o) = &self.open {
            self.cursors.insert(
                o.path.clone(),
                (self.editor.doc.row, self.editor.doc.col, self.editor.top),
            );
        }
        let Some(text) = util::read_text(path) else {
            self.say(format!("notes: can't read {}", self.store.relative(path)));
            return;
        };
        if let Some(o) = &self.open {
            let left = o.path.clone();
            let kept = self.editor.put_away();
            self.kept.insert(left, kept);
        }
        match self.kept.remove(path) {
            Some(kept) => self.editor.take_back(Text::from_file(&text), kept),
            None => self.editor.load(Text::from_file(&text)),
        }
        self.open = Some(Open {
            path: path.to_path_buf(),
            base: Some(text.clone()),
            saved: self.editor.doc.text.clone(),
            title: store::title_of(&text),
            gone: false,
        });
        if let Some(&(row, col, top)) = self.cursors.get(path) {
            self.editor.doc.row = row;
            self.editor.doc.col = col;
            self.editor.top = top;
            self.editor.clamp();
        }
        // Its tables lined up, as one change.
        self.editor.change(table::align_all);
        if self.modified() {
            self.save_due = Some(Instant::now() + AUTOSAVE);
        }
        let rel = self.store.relative(path);
        self.store.set_last(Some(&rel));
        self.store.opened_beside(&rel);
        self.rebuild();
        self.reveal(path);
        if !stay {
            self.focus = Focus::Note;
        }
        if insert_title {
            let len = self.editor.doc.line(0).len();
            self.editor.doc.row = 0;
            self.editor.insert_at(len);
        }
    }

    /// Brings in a change made to the open note's file from outside, if
    /// there is one: in place when nothing is unsaved, merged line by line
    /// with unsaved typing when something is. An item ticked there moves
    /// under the ones left to do, as ticking it here does. False when the
    /// note's file has gone (deleted or moved outside the app), so it isn't
    /// written back under its old name; said once.
    pub fn check(&mut self) -> bool {
        let Some(open) = &self.open else {
            return false;
        };
        let path = open.path.clone();
        let Some(disk) = util::read_text(&path) else {
            if !open.gone {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.say(format!(
                    "notes: {name} was moved or deleted outside Notes, so it isn't saved there any more"
                ));
                if let Some(o) = &mut self.open {
                    o.gone = true;
                }
            }
            return false;
        };
        let modified = self.modified();
        let Some(o) = &mut self.open else {
            return false;
        };
        o.gone = false;
        let Some(base) = o.base.clone() else {
            return true;
        };
        if disk == base {
            return true;
        }
        let before = self.editor.doc.text.lines();
        let new = if modified {
            let (merged, clashes) = sync::merge(&base, &self.editor.doc.text.file_text(), &disk);
            if clashes > 0 {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.say(format!(
                    "notes: {name} was changed outside Notes where you were typing; both versions are kept, yours first"
                ));
            }
            merged
        } else {
            disk.clone()
        };
        let new_text = Text::from_file(&new);
        self.editor.change(|doc| doc.text = new_text);
        self.editor.clamp();
        if let Some(o) = &mut self.open {
            o.base = Some(disk.clone());
            o.saved = Text::from_file(&disk);
        }
        let after = self.editor.doc.text.lines();
        self.editor.folds.follow(&before, &after);
        let mut rows = sync::changed_rows(&before, &after);
        if self.opts.move_checked_to_bottom && !rows.is_empty() {
            rows.sort_unstable_by(|a, b| b.cmp(a));
            let changed = self.editor.change(|doc| {
                for row in rows {
                    format::sort_checked(doc, row);
                }
            });
            if changed {
                self.save_due = Some(Instant::now());
            }
        }
        true
    }

    /// Saves the open note, and renames its file if its title changed. A
    /// change made to its file from outside is brought in first, so it
    /// isn't written over.
    pub fn save(&mut self) {
        self.save_due = None;
        if self.open.is_none() || !self.check() {
            return;
        }
        let Some(open) = &self.open else { return };
        let path = open.path.clone();
        if self.modified() {
            let text = self.editor.doc.text.file_text();
            if util::write_atomic(&path, &text).is_err() {
                self.say(format!(
                    "notes: couldn't save {}",
                    self.store.relative(&path)
                ));
                return;
            }
            if let Some(o) = &mut self.open {
                o.base = Some(text);
                o.saved = self.editor.doc.text.clone();
            }
        }
        if self.store.in_trash(&path) {
            return;
        }
        let renamed = self.store.rename_to_title(&path);
        if renamed != path {
            self.moved(&path, &renamed);
        }
        // A new title renames the tabs linked to the note and named after
        // it, its file renamed or not.
        let now = store::title_of(&self.editor.doc.text.file_text());
        let was = self
            .open
            .as_ref()
            .map(|o| o.title.clone())
            .unwrap_or_default();
        if let Some(o) = &mut self.open {
            o.title = now.clone();
        }
        if now != was {
            let retitled = Retitled {
                path: self.store.relative(&renamed),
                from: was,
                to: now,
            };
            self.store.relink(&|_| None, Some(&retitled));
        }
        self.rebuild();
    }

    /// A note moved or renamed: the open note follows, and so do the marks.
    fn moved(&mut self, from: &Path, to: &Path) {
        for p in &mut self.list.marked {
            if p == from {
                *p = to.to_path_buf();
            }
        }
        if let Some(o) = &mut self.open
            && o.path == from
        {
            o.path = to.to_path_buf();
        }
        if let Some(c) = self.cursors.remove(from) {
            self.cursors.insert(to.to_path_buf(), c);
        }
        if let Some(k) = self.kept.remove(from) {
            self.kept.insert(to.to_path_buf(), k);
        }
    }

    /// A folder renamed: the open note and the marks under it follow.
    fn folder_renamed(&mut self, from: &str, to: &str) {
        let (a, b) = (self.store.absolute(from), self.store.absolute(to));
        let follow = |p: &Path| p.strip_prefix(&a).ok().map(|rest| b.join(rest));
        for p in &mut self.list.marked {
            if let Some(n) = follow(p) {
                *p = n;
            }
        }
        if let Some(o) = &mut self.open
            && let Some(n) = follow(&o.path)
        {
            o.path = n;
        }
        rekey(&mut self.cursors, &follow);
        rekey(&mut self.kept, &follow);
    }

    /// A note going (deleted, squashed): saved, and the note window shows
    /// the next one in the list, or none.
    fn forget(&mut self, path: &Path) {
        // Its history goes with it, so a note made later by its name starts
        // with none.
        self.kept.remove(path);
        if self.current_path() != Some(path) {
            return;
        }
        self.save();
        let next = sidebar::next_note(&self.list.notes(&self.store), path);
        self.open = None;
        self.editor.load(Text::default());
        if let Some(next) = next {
            self.open_note(&next, true, false);
        }
    }

    /// Gives a note a new title, as typing it on its first line would: the
    /// line the title is read from is rewritten, keeping its # marks, and
    /// the save renames the file and carries its tabs' links along. Gives
    /// its new path.
    pub fn retitle(&mut self, path: &Path, title: &str) -> PathBuf {
        let rewrite = |lines: &mut Vec<String>| {
            let at = lines.iter().position(|l| {
                !store::title_of(l).is_empty()
                    && !l.trim().trim_start_matches('#').trim().is_empty()
            });
            match at {
                Some(i) => {
                    let marks: String = {
                        let l = &lines[i];
                        let n = l.len() - l.trim_start().len();
                        let hashes = l[n..].chars().take_while(|c| *c == '#').count();
                        if hashes == 0 {
                            "# ".into()
                        } else {
                            let after = &l[n + hashes..];
                            let spaces = after.len() - after.trim_start().len();
                            l[..n + hashes + spaces].to_string()
                        }
                    };
                    lines[i] = format!("{marks}{title}");
                }
                None => lines.insert(0, format!("# {title}")),
            }
        };
        if self.current_path() == Some(path) {
            let mut lines = self.editor.doc.text.lines();
            rewrite(&mut lines);
            let text = Text::from_lines(&lines);
            self.editor.change(|doc| doc.text = text);
            self.save();
            return self
                .current_path()
                .map(Path::to_path_buf)
                .unwrap_or_default();
        }
        let Some(text) = util::read_text(path) else {
            return path.to_path_buf();
        };
        let was = store::title_of(&text);
        let mut lines: Vec<String> = Text::from_file(&text).lines();
        rewrite(&mut lines);
        let _ = util::write_atomic(path, Text::from_lines(&lines).file_text());
        let to = self.store.rename_to_title(path);
        self.moved(path, &to);
        let now = store::title_of(&util::read_text(&to).unwrap_or_default());
        if now != was {
            let retitled = Retitled {
                path: self.store.relative(&to),
                from: was,
                to: now,
            };
            self.store.relink(&|_| None, Some(&retitled));
        }
        to
    }

    // ── The list ──────────────────────────────────────────────────────────

    /// The list built again from the notes on disk.
    pub fn rebuild(&mut self) {
        let width = list_width(self.cols);
        let current = self.open.as_ref().map(|o| o.path.clone());
        self.list_rows = self.list.build(&self.store, current.as_deref(), width);
        self.titles = self
            .store
            .titles()
            .into_iter()
            .map(|t| t.to_lowercase())
            .collect();
        self.list_at = self.list_at.min(self.list_rows.len().saturating_sub(1));
    }

    /// The list's cursor put on a note's row, so the note open is the one
    /// the cursor marks; a note the view doesn't list brings All Notes.
    pub fn reveal(&mut self, path: &Path) {
        let row = |rows: &[Row]| {
            rows.iter()
                .position(|r| r.item.as_ref() == Some(&Item::Note(path.to_path_buf())))
        };
        if row(&self.list_rows).is_none() {
            self.list.show(None, None, None);
            self.rebuild();
        }
        if let Some(at) = row(&self.list_rows) {
            self.list_at = at;
        }
    }

    /// Shows a folder, a tag or a search, the cursor on its first note;
    /// put away, the list comes back to show it.
    pub fn show(&mut self, folder: Option<String>, tag: Option<String>, query: Option<String>) {
        self.list.show(folder, tag, query);
        self.list_shown = true;
        self.rebuild();
        if let Some(at) = self
            .list_rows
            .iter()
            .position(|r| matches!(r.item, Some(Item::Note(_))))
        {
            self.list_at = at;
        }
    }

    /// The list shown or put away; shown again, its cursor on the open
    /// note when the view lists it.
    pub fn show_list(&mut self, show: bool) {
        if show == self.list_shown {
            return;
        }
        self.list_shown = show;
        if show {
            if let Some(path) = self.current_path().map(Path::to_path_buf)
                && let Some(at) = self
                    .list_rows
                    .iter()
                    .position(|r| r.item.as_ref() == Some(&Item::Note(path.clone())))
            {
                self.list_at = at;
            }
        } else {
            self.focus = Focus::Note;
        }
    }

    pub fn toggle_list(&mut self) {
        let shown = self.list_shown;
        self.show_list(!shown);
    }

    fn list_item(&self) -> Option<Item> {
        self.list_rows
            .get(self.list_at)
            .and_then(|r| r.item.clone())
    }

    fn list_note(&self) -> Option<PathBuf> {
        match self.list_item()? {
            Item::Note(p) => Some(p),
            _ => None,
        }
    }

    pub fn activate(&mut self) {
        match self.list_item() {
            Some(Item::Note(path)) => self.open_note(&path, false, false),
            Some(Item::Tag(tag)) => self.show(None, Some(tag), None),
            Some(Item::Folder(folder)) => self.show(folder, None, None),
            None => {}
        }
    }

    pub fn new_note(&mut self) {
        let here = self.list.here();
        let path = self.store.create(&here, None);
        self.rebuild();
        self.open_note(&path, false, true);
    }

    pub fn delete(&mut self) {
        let Some(path) = self.list_note() else { return };
        if self.store.in_trash(&path) {
            self.push(
                Box::new(Confirm::new(
                    "Delete this note for good? It can't be undone.",
                )),
                Then::DeleteForGood(path),
            );
            return;
        }
        self.delete_now(&path);
    }

    fn delete_now(&mut self, path: &Path) {
        self.forget(path);
        self.store.delete(path);
        self.rebuild();
        if let Some(p) = self.current_path().map(Path::to_path_buf) {
            self.reveal(&p);
        }
    }

    pub fn mark(&mut self) {
        let Some(path) = self.list_note() else { return };
        if self.store.in_trash(&path) {
            return;
        }
        match self.list.marked.iter().position(|p| *p == path) {
            Some(at) => {
                self.list.marked.remove(at);
            }
            None => self.list.marked.push(path),
        }
        self.rebuild();
    }

    pub fn squash(&mut self) {
        let marked: Vec<PathBuf> = self
            .list
            .marked
            .iter()
            .filter(|p| p.exists())
            .cloned()
            .collect();
        if marked.len() < 2 {
            self.say("Mark two or more notes with s, then S squashes them into one.");
            return;
        }
        let titles: Vec<String> = marked.iter().map(|p| sidebar::title_at(p)).collect();
        let (question, list) = squash::question(&titles);
        self.push(
            Box::new(Confirm::new(question).list(list)),
            Then::Squash(marked),
        );
    }

    fn start_squash(&mut self, paths: Vec<PathBuf>) {
        self.save();
        let Some(program) = self.claude.clone() else {
            self.say("Notes weren't squashed: Claude Code isn't installed");
            return;
        };
        let input = match squash::input(&self.store, &paths) {
            Ok(i) => i,
            Err(why) => return self.say(format!("Notes weren't squashed: {why}")),
        };
        let child = match squash::command(&program, squash::MODEL).spawn() {
            Ok(c) => c,
            Err(_) => return self.say("Notes weren't squashed: Claude Code isn't installed"),
        };
        crate::requests::count("Anthropic", "claude -p", "Notes: squash");
        self.squash_pid = Some(child.id());
        self.squash_cancelled = false;
        let titles = paths.iter().map(|p| sidebar::title_at(p)).collect();
        self.squash_paths = paths;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Squashed(squash::answer(child, &input)));
        });
        self.push(Box::new(Progress::new(titles)), Then::SquashRunning);
    }

    fn squashed(&mut self, result: Result<String, String>) {
        self.squash_pid = None;
        if std::mem::take(&mut self.squash_cancelled) {
            return;
        }
        self.layers
            .retain(|l| !matches!(l.then, Then::SquashRunning));
        let paths = std::mem::take(&mut self.squash_paths);
        match result {
            Err(why) => self.say(format!("Notes weren't squashed: {why}")),
            Ok(text) => {
                for p in &paths {
                    if self.current_path() == Some(p.as_path()) {
                        self.save();
                        self.open = None;
                    }
                }
                match squash::finish(&mut self.store, &paths, &text) {
                    Ok(path) => {
                        self.list.marked.clear();
                        self.open_note(&path, false, false);
                        self.say(format!("Squashed into {}", sidebar::title_at(&path)));
                    }
                    Err(why) => self.say(format!("Notes weren't squashed: {why}")),
                }
            }
        }
    }

    pub fn restore(&mut self) {
        let Some(path) = self.list_note() else { return };
        if !self.store.in_trash(&path) {
            return;
        }
        self.forget(&path);
        self.store.restore(&path);
        self.rebuild();
    }

    pub fn pin(&mut self) {
        let Some(path) = self.list_note() else { return };
        if self.store.in_trash(&path) {
            return;
        }
        let rel = self.store.relative(&path);
        let on = !self.store.is_pinned(&rel);
        self.store.set_pinned(&rel, on);
        self.rebuild();
        self.reveal(&path);
    }

    pub fn move_note(&mut self) {
        let Some(path) = self.list_note() else { return };
        let mut rels = vec![String::new()];
        let mut options = vec![MenuOption::new("Notes")];
        for f in self.store.folders() {
            options.push(MenuOption::new(format!(
                "{}{}",
                "  ".repeat(f.depth),
                f.name
            )));
            rels.push(f.rel);
        }
        self.push(
            Box::new(Menu::new("Move to", options)),
            Then::MoveTo(path, rels),
        );
    }

    pub fn rename(&mut self) {
        match self.list_item() {
            Some(Item::Note(path)) => {
                if self.store.in_trash(&path) {
                    return;
                }
                let current = sidebar::title_at(&path);
                self.push(
                    Box::new(Ask::new("Rename note", "Rename note:", &current)),
                    Then::RenameNote(path),
                );
            }
            Some(Item::Folder(Some(f))) if !f.is_empty() && f != store::TRASH => {
                let name = f.rsplit('/').next().unwrap_or(&f).to_string();
                self.push(
                    Box::new(Ask::new("Rename folder", "Rename folder:", &name)),
                    Then::RenameFolder(f),
                );
            }
            _ => {}
        }
    }

    pub fn search(&mut self) {
        let q = self.list.query.clone().unwrap_or_default();
        self.push(
            Box::new(Ask::new("Search notes", "Search notes:", &q)),
            Then::Search,
        );
    }

    pub fn new_folder(&mut self) {
        self.push(
            Box::new(Ask::new("New folder", "New folder:", "")),
            Then::NewFolder,
        );
    }

    pub fn help(&mut self) {
        let text = format!("Notes\n\n{}", sidebar::HELP.join("\n"));
        self.push(Box::new(Message::new(&text, 3600)), Then::Nothing);
    }

    fn finder(&mut self) {
        self.push(Box::new(Finder::new(self.store.notes(None))), Then::Finder);
    }

    /// A Switchboard command for Notes, asked of the workspace's server.
    fn press(&mut self, name: &str, arg: Option<&str>) {
        if !self.opts.server {
            self.say("notes: that needs Switchboard's workspace");
            return;
        }
        let mut args = vec!["press", name];
        if let Some(a) = arg {
            args.push(a);
        }
        match crate::mux::socket::ask_ok(&args) {
            Ok(_) => {}
            Err(e) => self.say(format!("notes: {e}")),
        }
    }

    fn note_tab(&mut self) {
        let path = match self.focus {
            Focus::List => self.list_note(),
            Focus::Note => self.current_path().map(Path::to_path_buf),
        };
        let Some(path) = path.filter(|p| !self.store.in_trash(p)) else {
            return;
        };
        let rel = self.store.relative(&path);
        self.press("note-tab", Some(&rel));
    }

    // ── Boxes ─────────────────────────────────────────────────────────────

    fn push(&mut self, overlay: Box<dyn Overlay>, then: Then) {
        self.layers.push(Layer { overlay, then });
    }

    pub fn top_kind(&self) -> Option<&'static str> {
        self.layers.last().map(|l| l.overlay.kind())
    }

    fn step(&mut self, step: Step) {
        if let Step::Close(outcome) = step
            && let Some(layer) = self.layers.pop()
        {
            self.closed(layer.then, outcome);
        }
    }

    fn closed(&mut self, then: Then, outcome: Outcome) {
        let typed = |o: &Outcome| match o {
            Outcome::Picked(p) => Some(p.value.clone()),
            _ => None,
        };
        match then {
            Then::Nothing => {}
            Then::Squash(paths) => {
                if outcome == Outcome::Yes {
                    self.start_squash(paths);
                }
            }
            Then::SquashRunning => {
                // Esc or the ×: the merge stopped, the notes as they were.
                if let Some(pid) = self.squash_pid.take() {
                    // SAFETY: a signal to the merge this app started.
                    unsafe {
                        libc::kill(pid as libc::pid_t, libc::SIGTERM);
                    }
                }
                self.squash_cancelled = true;
                self.squash_paths.clear();
                self.say("Squash canceled; the notes are as they were.");
            }
            Then::DeleteForGood(path) => {
                if outcome == Outcome::Yes {
                    self.delete_now(&path);
                }
            }
            Then::MoveTo(path, rels) => {
                if let Outcome::Chose(i) = outcome
                    && let Some(rel) = rels.get(i)
                {
                    if self.current_path() == Some(path.as_path()) {
                        self.save();
                    }
                    let to = self.store.move_note(&path, rel);
                    self.moved(&path, &to);
                    self.rebuild();
                }
            }
            Then::NewFolder => {
                if let Some(name) = typed(&outcome).filter(|n| !n.trim().is_empty()) {
                    let here = self.list.here();
                    let rel = if here.is_empty() {
                        name
                    } else {
                        format!("{here}/{name}")
                    };
                    if let Some(rel) = self.store.create_folder(&rel) {
                        self.show(Some(rel), None, None);
                    }
                }
            }
            Then::Search => {
                if let Some(q) = typed(&outcome) {
                    let q = q.trim().to_string();
                    if q.is_empty() {
                        let folder = self.list.folder.clone();
                        self.show(folder, None, None);
                    } else {
                        self.show(None, None, Some(q));
                    }
                }
            }
            Then::RenameNote(path) => {
                if let Some(name) = typed(&outcome) {
                    let name = name.trim();
                    if !name.is_empty() && name != sidebar::title_at(&path) {
                        let to = self.retitle(&path, name);
                        self.rebuild();
                        self.reveal(&to);
                    }
                }
            }
            Then::RenameFolder(from) => {
                if let Some(name) = typed(&outcome).filter(|n| !n.trim().is_empty()) {
                    self.save();
                    match self.store.rename_folder(&from, &name) {
                        Ok(to) => {
                            self.folder_renamed(&from, &to);
                            self.show(Some(to), None, None);
                        }
                        Err(why) => self.say(format!("notes: didn't rename the folder: {why}")),
                    }
                }
            }
            Then::Link(selection) => {
                if let Some(url) = typed(&outcome) {
                    let url = url.trim().to_string();
                    if !url.is_empty() && url != "https://" {
                        self.editor.change(|doc| format::link(doc, selection, &url));
                        self.changed();
                    }
                }
            }
            Then::Palette(commands, from) => {
                if let Outcome::Picked(p) = outcome
                    && let Some(c) = p.value.parse::<usize>().ok().and_then(|i| commands.get(i))
                {
                    palette::remember(&self.recent_file, &c.name);
                    let act = c.act;
                    self.run_palette(act, from);
                }
            }
            Then::Finder => {
                if let Outcome::Picked(p) = outcome {
                    self.open_note(Path::new(&p.value), false, false);
                }
            }
            Then::Complete => {
                if let Outcome::Picked(p) = outcome {
                    self.editor.type_text(&format!("{}]]", p.value));
                    self.changed();
                }
            }
            Then::Format(from) => {
                if let Outcome::Chose(i) = outcome
                    && let Some((_, item)) = menu::items().get(i).copied()
                {
                    self.run_format(item.act, from);
                }
            }
        }
    }

    // ── The Format menu ───────────────────────────────────────────────────

    fn format_from(&self, typing_col: Option<usize>) -> FormatFrom {
        let selection = self.editor.selection();
        let (first, last) = match selection {
            Some(s) => (s.start.0, s.end.0),
            None => (self.editor.doc.row, self.editor.doc.row),
        };
        FormatFrom {
            first,
            last,
            selection,
            typing_col,
        }
    }

    /// The Format menu, from where the cursor or the selection is.
    fn open_menu(&mut self) {
        let typing = (self.editor.mode == Mode::Insert).then_some(self.editor.doc.col);
        let from = self.format_from(typing);
        self.leave_modes();
        self.push(Box::<FormatMenu>::default(), Then::Format(from));
    }

    /// Out of Insert and Visual mode, as a menu or a button takes the keys.
    fn leave_modes(&mut self) {
        match self.editor.mode {
            Mode::Insert => {
                self.editor.key(Key::new(KeyCode::Esc, Mods::NONE));
                self.after_insert();
            }
            Mode::Visual { .. } => self.editor.leave_visual(),
            Mode::Normal => {}
        }
    }

    /// Runs a Format menu action: a style picked to write in starts typing
    /// after it, through the note's own `a` (so it starts after a line's
    /// marker), unless it already has; not over a selection, which you were
    /// formatting rather than writing.
    fn run_format(&mut self, act: FormatAct, from: FormatFrom) {
        if self.open.is_none() {
            return;
        }
        let sel = from.selection;
        let (first, last, typing) = (from.first, from.last, from.typing_col);
        match act {
            FormatAct::Link => {
                self.push(
                    Box::new(Ask::new("Link", "Link to:", "https://")),
                    Then::Link(sel),
                );
                return;
            }
            FormatAct::PasteImage => {
                self.paste_image();
                return;
            }
            _ => {}
        }
        self.editor.change(|doc| match act {
            FormatAct::Paragraph(style) => format::paragraph(doc, first, last, style),
            FormatAct::Monospaced => format::monospaced(doc, first, last),
            FormatAct::Character(style) => format::character(doc, style, sel, typing),
            FormatAct::Tick => {
                format::toggle_check(doc, first);
            }
            FormatAct::SortChecked => format::sort_checked(doc, first),
            FormatAct::UncheckAll => format::uncheck_all(doc),
            FormatAct::DeleteChecked => format::delete_checked(doc),
            FormatAct::Table => table::insert(doc),
            FormatAct::AddRow => table::add_row(doc, first),
            FormatAct::AddColumn => table::add_column(doc),
            FormatAct::DeleteRow => table::delete_row(doc),
            FormatAct::DeleteColumn => table::delete_column(doc),
            FormatAct::Link | FormatAct::PasteImage => {}
        });
        self.take_insert();
        if act.typing() && sel.is_none() && self.editor.mode != Mode::Insert {
            self.note_a();
        }
        self.changed();
    }

    /// Insert mode, when an edit asked for it.
    fn take_insert(&mut self) {
        if let Some(how) = self.editor.doc.insert.take() {
            let col = if how == format::Insert::End {
                self.editor.line().len()
            } else {
                self.editor.doc.col
            };
            self.editor.insert_at(col);
        }
    }

    /// The note's own `a`, which starts typing after a line's marker.
    fn note_a(&mut self) {
        let line = self.editor.line();
        match format::insert_point(&line, self.editor.doc.col, 'a') {
            Some(at) => self.editor.insert_at(at),
            None => self.editor.key(Key::new(KeyCode::Char('a'), Mods::NONE)),
        }
    }

    fn paste_image(&mut self) {
        let Some(path) = self.current_path().map(Path::to_path_buf) else {
            return;
        };
        let grab = self.grab;
        let mut result = Ok(String::new());
        self.editor.change(|doc| {
            result = images::paste(doc, &path, &|f| grab(f));
        });
        match result {
            Ok(_) => self.changed(),
            Err(why) => self.say(format!("Notes: {why}")),
        }
    }

    // ── Acting on a place in a note ───────────────────────────────────────

    /// Whatever Enter or a click does at a place in a note: tick a checklist
    /// item, fold a heading, follow a link. `clicked` limits it to the ☐
    /// and the ▾ themselves. Whether it did something.
    pub fn act(&mut self, row: usize, col: Option<usize>, clicked: bool) -> bool {
        let lines = self.editor.doc.text.lines();
        let kinds = render::kinds(&lines);
        if let Some((s, _)) = self.editor.folds.closed_at(&kinds, row) {
            self.editor.folds.open(&kinds, s);
            return true;
        }
        let Some(k) = kinds.get(row).copied() else {
            return false;
        };
        let at = col.unwrap_or(0);
        if let Kind::Check { indent, marker, .. } = k
            && (!clicked || (col.is_some() && at >= indent && at < indent + marker))
        {
            let done = self.editor.change(|doc| {
                format::toggle_check(doc, row);
            });
            if done {
                self.changed();
            }
            return true;
        }
        if k.level().is_some() && (!clicked || (k.marker() > 0 && at < k.marker())) {
            return self.editor.folds.toggle(&kinds, row);
        }
        if !clicked || self.opts.follow_links_on_click {
            return self.follow(row, at);
        }
        false
    }

    /// Follows what is at a byte of a row: a web address in the browser, an
    /// image in Quick Look, a [[Note title]] to that note (made when there is
    /// none), a #tag to the notes that have it. Whether there was something.
    pub fn follow(&mut self, row: usize, col: usize) -> bool {
        let line = self.editor.doc.line(row);
        let Some(span) = render::span_at(&line, col) else {
            return false;
        };
        if let Some(image) = span.image {
            let Some(note) = self.current_path().map(Path::to_path_buf) else {
                return false;
            };
            if images::open(&self.store, &note, &image) {
                return true;
            }
            if image.starts_with("http://") || image.starts_with("https://") {
                open_link(image);
                return true;
            }
            return false;
        }
        if let Some(url) = span.url {
            open_link(url);
            return true;
        }
        if let Some(title) = span.note {
            let path = match self.store.find_by_title(&title) {
                Some(n) => n.path,
                None => {
                    let here = self.list.here();
                    self.store.create(&here, Some(&title))
                }
            };
            self.open_note(&path, false, false);
            return true;
        }
        if let Some(tag) = span.tag {
            self.show(None, Some(tag), None);
            return true;
        }
        false
    }

    // ── After a change ────────────────────────────────────────────────────

    /// The note changed: saved a moment from now.
    fn changed(&mut self) {
        self.save_due = Some(Instant::now() + AUTOSAVE);
    }

    /// Leaving Insert mode: an empty pair of markers put in to type into
    /// goes, and the table the cursor is in lines up again.
    fn after_insert(&mut self) {
        format::drop_empty(&mut self.editor.doc);
        let row = self.editor.doc.row;
        table::align(&mut self.editor.doc, row);
        self.editor.clamp();
    }

    // ── Input ─────────────────────────────────────────────────────────────

    pub fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Input(bytes) => {
                self.last_input = Instant::now();
                for parsed in self.parser.feed(&bytes) {
                    self.input(parsed.input);
                }
            }
            Msg::Watched => {
                if self.watch_due.is_none() {
                    self.watch_due = Some(Instant::now() + WATCH_WAIT);
                }
            }
            Msg::Squashed(result) => self.squashed(result),
            Msg::Socket(request, back) => {
                let reply = self.request(request);
                let _ = back.send(reply);
            }
        }
    }

    /// A request on the socket.
    pub fn request(&mut self, request: Request) -> String {
        match request {
            Request::Ping => "ok".into(),
            Request::Shown => match self.current_path().map(Path::to_path_buf) {
                Some(p) => format!("ok {}", self.store.relative(&p)),
                None => "ok".into(),
            },
            Request::Sidebar(on) => {
                self.show_list(on);
                "ok".into()
            }
            Request::Open(rel) => {
                if rel.split('/').any(|part| part == "..") || rel.starts_with('/') {
                    return "error not a note in the notes directory".into();
                }
                let path = self.store.absolute(&rel);
                if !path.is_file() {
                    return format!("error no note {rel}");
                }
                self.open_note(&path, true, false);
                "ok".into()
            }
        }
    }

    /// What the timers do: a lone Esc let go, the note saved, the
    /// directory's changes brought in, a message gone. Whether the screen
    /// changed.
    pub fn tick(&mut self) -> bool {
        let now = Instant::now();
        let mut changed = false;
        if self.parser.pending()
            && now.duration_since(self.last_input) >= crate::mux::input::ESC_TIMEOUT
        {
            for parsed in self.parser.flush() {
                self.input(parsed.input);
            }
            changed = true;
        }
        if self.save_due.is_some_and(|due| now >= due) {
            self.save();
            changed = true;
        }
        if self.watch_due.is_some_and(|due| now >= due) {
            self.watch_due = None;
            self.check();
            self.rebuild();
            changed = true;
        }
        if self
            .message
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() >= MESSAGE_FOR)
        {
            self.message = None;
            changed = true;
        }
        let expired = self
            .layers
            .last()
            .and_then(|l| l.overlay.deadline())
            .is_some_and(|d| now >= d);
        if expired {
            self.step(Step::Close(Outcome::Dismissed));
            changed = true;
        }
        changed || self.squash_pid.is_some()
    }

    pub fn input(&mut self, input: Input) {
        match input {
            Input::Key(key) => self.key(key),
            Input::Mouse(m) => self.mouse(m),
            Input::Paste(text) => self.paste(&text),
            Input::FocusIn => {
                // The notes_images setting read again, as changing it in
                // Settings takes the focus away.
                if images::terminal_can() {
                    self.images.on = settings::read_settings()
                        .iter()
                        .find(|s| s.key == "notes_images")
                        .is_none_or(|s| s.value != serde_json::Value::Bool(false));
                }
                self.images.forget_sent();
                self.check();
                self.rebuild();
            }
            Input::FocusOut => self.save(),
            _ => {}
        }
    }

    pub fn key(&mut self, key: Key) {
        if self.locked_by.is_some() {
            if matches!(key.code, KeyCode::Char('q' | 'Q') | KeyCode::Esc) {
                self.quit = true;
            }
            return;
        }
        if let Some(layer) = self.layers.last_mut() {
            let step = layer.overlay.key(key, &self.look);
            return self.step(step);
        }
        if self.prompt.is_some() {
            return self.prompt_key(key);
        }
        if self.message.is_some() && self.focus == Focus::Note && self.editor.mode == Mode::Normal {
            self.message = None;
        }
        if let Some(dir) = alt_arrow(&key) {
            // With pane_arrows off the workspace passes them on unasked,
            // and they do nothing here, as Neovim's mapping fed them back.
            if self.opts.pane_arrows {
                self.window_move(dir);
            }
            return;
        }
        let palette_key = key.mods.ctrl
            && matches!(key.code, KeyCode::Char(c) if c == 'P' || (c == 'p' && key.mods.shift));
        if palette_key {
            return self.palette();
        }
        match self.focus {
            Focus::List if self.list_shown => self.list_key(key),
            _ => self.note_key(key),
        }
    }

    fn prompt_key(&mut self, key: Key) {
        let Some((kind, mut text)) = self.prompt.take() else {
            return;
        };
        match key.code {
            KeyCode::Esc => return,
            KeyCode::Enter => {
                self.editor.begin();
                match kind {
                    ':' => self.editor.ex(&text),
                    _ => self.editor.search_for(&text, kind == '/'),
                }
                if self.editor.end().is_some() {
                    self.changed();
                }
                self.effects();
                return;
            }
            KeyCode::Backspace => {
                if text.pop().is_none() {
                    return;
                }
            }
            KeyCode::Char('u') if key.mods.ctrl => text.clear(),
            KeyCode::Char(c) if !key.mods.ctrl && !key.mods.alt => text.push(c),
            _ => {}
        }
        self.prompt = Some((kind, text));
    }

    fn list_key(&mut self, key: Key) {
        let ctrl = key.mods.ctrl;
        let pending = self.list_pending.take();
        let c = match key.code {
            KeyCode::Char(c) if !ctrl && !key.mods.alt => Some(c),
            _ => None,
        };
        if pending == Some('d') && c == Some('d') {
            return self.delete();
        }
        if pending == Some('g') && c == Some('g') {
            self.list_at = 0;
            return;
        }
        if pending == Some('w') {
            return self.window_key(key);
        }
        let last = self.list_rows.len().saturating_sub(1);
        // Half the list's height, as Vim's Ctrl-D and Ctrl-U.
        let half = (self.geo.list.height as usize / 2).max(1);
        match key.code {
            KeyCode::Enter => self.activate(),
            KeyCode::Down => self.list_at = (self.list_at + 1).min(last),
            KeyCode::Up => self.list_at = self.list_at.saturating_sub(1),
            KeyCode::Tab => self.focus = Focus::Note,
            KeyCode::Char('f') if ctrl => self.finder(),
            KeyCode::Char('b') if ctrl => self.toggle_list(),
            KeyCode::Char('w') if ctrl => self.list_pending = Some('w'),
            KeyCode::Char('d') if ctrl => self.list_at = (self.list_at + half).min(last),
            KeyCode::Char('u') if ctrl => self.list_at = self.list_at.saturating_sub(half),
            _ => match c {
                Some('j') => self.list_at = (self.list_at + 1).min(last),
                Some('k') => self.list_at = self.list_at.saturating_sub(1),
                Some('G') => self.list_at = last,
                Some('g') | Some('d') => self.list_pending = c,
                Some('n') => self.new_note(),
                Some('N') => self.new_folder(),
                Some('t') => self.note_tab(),
                Some('s') => self.mark(),
                Some('S') => self.squash(),
                Some('D') => self.delete(),
                Some('r') => self.restore(),
                Some('p') => self.pin(),
                Some('m') => self.move_note(),
                Some('R') => self.rename(),
                Some('/') => self.search(),
                Some('x') => {
                    let folder = self.list.folder.clone();
                    self.show(folder, None, None);
                }
                Some('q') => self.quit = true,
                Some('?') => self.help(),
                _ => {}
            },
        }
    }

    /// The key after C-w: a side (h, j, k, l or an arrow), or w and p for
    /// the other window.
    fn window_key(&mut self, key: Key) {
        let dir = match key.code {
            KeyCode::Left => 'h',
            KeyCode::Down => 'j',
            KeyCode::Up => 'k',
            KeyCode::Right => 'l',
            KeyCode::Char('w') if key.mods.ctrl => 'w',
            KeyCode::Char(c) if !key.mods.ctrl && !key.mods.alt => c,
            _ => return,
        };
        self.window_move(dir);
    }

    /// C-w with a side, and the M-arrows: the list or the note beside,
    /// else the workspace's pane on that side, as vim-tmux-navigator moved
    /// between Neovim's windows and handed off to tmux at the edges. Going
    /// from the note to the list stops typing and saves it, as leaving its
    /// buffer did.
    fn window_move(&mut self, dir: char) {
        let list = self.focus == Focus::List && self.list_shown;
        match dir {
            'h' | 'w' | 'p' if !list && self.list_shown => {
                self.leave_modes();
                self.save();
                self.focus = Focus::List;
            }
            'l' | 'w' | 'p' if list => self.focus = Focus::Note,
            'h' => self.beside("-L"),
            'l' => self.beside("-R"),
            'j' => self.beside("-D"),
            'k' => self.beside("-U"),
            _ => {}
        }
    }

    /// The workspace's pane on a side of this one (`select-pane -L`), off
    /// the screen's thread; nothing outside the workspace.
    fn beside(&self, side: &'static str) {
        if !self.opts.server {
            return;
        }
        std::thread::spawn(move || {
            let _ = crate::mux::socket::ask(&["select-pane", side]);
        });
    }

    fn palette(&mut self) {
        if !self.opts.palette {
            self.say("notes: the palette is off (Settings, Notes command palette)");
            return;
        }
        let from_list = self.focus == Focus::List && self.list_shown;
        let typing = (self.editor.mode == Mode::Insert).then_some(self.editor.doc.col);
        let format = self.format_from(typing);
        self.leave_modes();
        let commands = palette::commands(from_list, !from_list && self.open.is_some());
        let items = palette::items(&commands, &palette::recent(&self.recent_file));
        let picker = Picker::new("Notes", items)
            .prompt("command> ")
            .exact()
            .order(Box::new(|_, matched: &[usize]| {
                // The list's own order, the last picked first, as fzf's
                // --no-sort kept it.
                let mut m = matched.to_vec();
                m.sort_unstable();
                m
            }));
        self.push(
            Box::new(picker),
            Then::Palette(
                commands,
                PaletteFrom {
                    list: from_list,
                    format,
                },
            ),
        );
    }

    fn run_palette(&mut self, act: PaletteAct, from: PaletteFrom) {
        // A command on the note open, from a note: the list's cursor on it.
        let on_note = |app: &mut App| {
            if from.list {
                return true;
            }
            let Some(path) = app.current_path().map(Path::to_path_buf) else {
                return false;
            };
            if app.store.in_trash(&path) && app.list.folder.as_deref() != Some(store::TRASH) {
                app.show(Some(store::TRASH.into()), None, None);
            }
            app.list_shown = true;
            app.reveal(&path);
            app.list_note().as_deref() == Some(path.as_path())
        };
        match act {
            PaletteAct::ToggleList => self.toggle_list(),
            PaletteAct::Find => self.finder(),
            PaletteAct::Search => {
                self.list_shown = true;
                self.search();
            }
            PaletteAct::ClearSearch => {
                let folder = self.list.folder.clone();
                self.show(folder, None, None);
            }
            PaletteAct::OtherWindow => {
                if from.list {
                    self.focus = Focus::Note;
                } else {
                    self.list_shown = true;
                    self.focus = Focus::List;
                }
            }
            PaletteAct::ListHelp => self.help(),
            PaletteAct::OpenListItem => self.activate(),
            PaletteAct::NewNote => self.new_note(),
            PaletteAct::NewFolder => self.new_folder(),
            PaletteAct::Squash => self.squash(),
            PaletteAct::Quit => self.quit = true,
            PaletteAct::Press(name) => self.press(name, None),
            PaletteAct::NoteTab => self.note_tab(),
            PaletteAct::Rename
            | PaletteAct::Move
            | PaletteAct::Pin
            | PaletteAct::Delete
            | PaletteAct::Restore
            | PaletteAct::Mark => {
                if !on_note(self) {
                    return self.say("notes: couldn't find the open note in the list");
                }
                match act {
                    PaletteAct::Rename => self.rename(),
                    PaletteAct::Move => self.move_note(),
                    PaletteAct::Pin => self.pin(),
                    PaletteAct::Delete => self.delete(),
                    PaletteAct::Restore => self.restore(),
                    _ => self.mark(),
                }
            }
            PaletteAct::Act => {
                let (row, col) = (self.editor.doc.row, self.editor.doc.col);
                self.act(row, Some(col), false);
            }
            PaletteAct::Indent | PaletteAct::Outdent => {
                let step = if act == PaletteAct::Indent { 1 } else { -1 };
                if self.editor.change(|doc| {
                    format::tab(doc, step);
                }) {
                    self.changed();
                }
            }
            PaletteAct::FoldHere => {
                let kinds = render::kinds(&self.editor.doc.text.lines());
                let row = self.editor.doc.row;
                self.editor.folds.toggle(&kinds, row);
            }
            PaletteAct::FoldAll => {
                let kinds = render::kinds(&self.editor.doc.text.lines());
                self.editor.folds.close_all(&kinds);
            }
            PaletteAct::UnfoldAll => self.editor.folds.open_all(),
            PaletteAct::Format(f) => {
                self.focus = Focus::Note;
                self.run_format(f, from.format);
            }
        }
    }

    /// A key in the note: the note's own keys first (Enter, the Format
    /// menu, the leader keys, Ctrl-F and Ctrl-B, easy mode's Cmd keys),
    /// then Vim's.
    fn note_key(&mut self, key: Key) {
        if self.open.is_none() {
            if self.list_pending.take() == Some('w') {
                return self.window_key(key);
            }
            match key.code {
                KeyCode::Char('b') if key.mods.ctrl => self.toggle_list(),
                KeyCode::Char('f') if key.mods.ctrl => self.finder(),
                KeyCode::Char('w') if key.mods.ctrl => self.list_pending = Some('w'),
                KeyCode::Char('q') => self.quit = true,
                _ => {}
            }
            return;
        }
        let fresh = !self.replaying && self.editor.idle() && !self.leader;
        if fresh {
            self.recording = Some((Vec::new(), self.editor.doc.text.clone()));
        }
        if !self.replaying
            && let Some((keys, _)) = &mut self.recording
        {
            keys.push(key);
        }
        let mode_before = self.editor.mode;
        let text_before = self.editor.doc.text.clone();
        self.editor.begin();
        self.note_key_inner(key);
        // Saved a moment after each change, typing included.
        if self.editor.doc.text != text_before {
            self.changed();
        }
        if mode_before == Mode::Insert && self.editor.mode != Mode::Insert {
            self.after_insert();
        } else if self.editor.mode == Mode::Normal && self.editor.idle() {
            let row = self.editor.doc.row;
            table::align(&mut self.editor.doc, row);
            self.editor.clamp();
        }
        if let Some(before) = self.editor.end() {
            self.editor
                .folds
                .follow(&before.lines(), &self.editor.doc.text.lines());
            self.changed();
        }
        self.effects();
        if self.editor.idle()
            && !self.leader
            && self.layers.is_empty()
            && !self.replaying
            && let Some((keys, before)) = self.recording.take()
        {
            // What `.` does again: the last change, not an undo or a redo.
            let undoing = keys.first().is_some_and(|k| {
                matches!(k.code, KeyCode::Char('u' | '.')) && k.mods.is_empty()
                    || (k.mods.ctrl && k.code == KeyCode::Char('r'))
                    || easy::cmd_key(k).is_some()
            });
            if before != self.editor.doc.text && !undoing {
                self.last_change = keys;
            }
        }
    }

    fn note_key_inner(&mut self, key: Key) {
        let mode = self.editor.mode;
        if self.opts.easy
            && let Some(cmd) = easy::cmd_key(&key)
        {
            return self.cmd_key(cmd);
        }
        if self.literal {
            self.literal = false;
            if mode == Mode::Insert {
                let text = match key.code {
                    KeyCode::Tab => "\t".to_string(),
                    KeyCode::Enter => "\r".to_string(),
                    KeyCode::Char(c) => c.to_string(),
                    _ => String::new(),
                };
                self.editor.type_text(&text);
            }
            return;
        }
        if self.leader {
            self.leader = false;
            return self.leader_key(key);
        }
        let ctrl = key.mods.ctrl && !key.mods.alt;
        let idle = !self.editor.pending();
        match mode {
            Mode::Normal if idle => match key.code {
                KeyCode::Enter if key.mods.is_empty() => {
                    let (row, col) = (self.editor.doc.row, self.editor.doc.col);
                    if !self.act(row, Some(col), false) {
                        self.editor.key(key);
                    }
                    return;
                }
                KeyCode::Char('p') if ctrl => return self.open_menu(),
                KeyCode::Char('f') if ctrl => return self.finder(),
                KeyCode::Char('b') if ctrl => return self.toggle_list(),
                KeyCode::Char(' ') if key.mods.is_empty() => {
                    self.leader = true;
                    return;
                }
                KeyCode::Char('.') if key.mods.is_empty() => return self.repeat(),
                KeyCode::Char(c @ ('i' | 'a' | 'I'))
                    if key.mods.is_empty() || (c == 'I' && key.mods == Mods::SHIFT) =>
                {
                    let line = self.editor.line();
                    match format::insert_point(&line, self.editor.doc.col, c) {
                        Some(at) => self.editor.insert_at(at),
                        None => self.editor.key(key),
                    }
                    return;
                }
                _ => {}
            },
            Mode::Insert => match key.code {
                KeyCode::Char('p') if ctrl => return self.open_menu(),
                KeyCode::Char('f') if ctrl => {
                    self.leave_modes();
                    return self.finder();
                }
                KeyCode::Char('b') if ctrl => return self.toggle_list(),
                KeyCode::Char('v') if ctrl => {
                    if !(self.opts.ctrl_v_image && self.try_paste_image()) {
                        self.literal = true;
                    }
                    return;
                }
                KeyCode::Enter if key.mods.is_empty() => {
                    if !format::enter(&mut self.editor.doc) {
                        self.editor.key(key);
                    }
                    return;
                }
                KeyCode::Tab => {
                    if !format::tab(&mut self.editor.doc, 1) {
                        self.editor.key(key);
                    }
                    return;
                }
                KeyCode::BackTab => {
                    format::tab(&mut self.editor.doc, -1);
                    return;
                }
                KeyCode::Char('[') if key.mods.is_empty() => {
                    self.editor.key(key);
                    let line = self.editor.line();
                    if line[..self.editor.doc.col.min(line.len())].ends_with("[[") {
                        self.complete();
                    }
                    return;
                }
                _ => {}
            },
            Mode::Visual { .. } => match key.code {
                KeyCode::Char('p') if ctrl => return self.open_menu(),
                KeyCode::Char('c') if ctrl => {
                    if let Some(text) = self.editor.selected_text() {
                        self.clip(&text);
                    }
                    self.editor.leave_visual();
                    return;
                }
                KeyCode::Char(' ')
                    if key.mods.is_empty()
                        && !matches!(mode, Mode::Visual { select: true, .. }) =>
                {
                    self.leader = true;
                    return;
                }
                _ => {}
            },
            _ => {}
        }
        self.editor.key(key);
    }

    /// The note's leader keys (Space, then a key): the styles, on the word
    /// or line, or on the selection.
    fn leader_key(&mut self, key: Key) {
        let Some(c) = (match key.code {
            KeyCode::Char(c) if !key.mods.ctrl && !key.mods.alt => Some(c),
            _ => None,
        }) else {
            return;
        };
        let from = self.format_from(None);
        let in_visual = from.selection.is_some();
        let act = match c {
            'b' => FormatAct::Character(Character::Bold),
            'i' => FormatAct::Character(Character::Italic),
            'U' => FormatAct::Character(Character::Underline),
            'x' => FormatAct::Character(Character::Strike),
            'h' => FormatAct::Character(Character::Highlight),
            '`' => FormatAct::Character(Character::Code),
            '1' => FormatAct::Paragraph(Paragraph::Title),
            '2' => FormatAct::Paragraph(Paragraph::Heading),
            '3' => FormatAct::Paragraph(Paragraph::Subheading),
            '0' => FormatAct::Paragraph(Paragraph::Body),
            'l' => FormatAct::Paragraph(Paragraph::Check),
            '8' => FormatAct::Paragraph(Paragraph::Bullet),
            '-' => FormatAct::Paragraph(Paragraph::Dash),
            '7' => FormatAct::Paragraph(Paragraph::Number),
            'q' => FormatAct::Paragraph(Paragraph::Quote),
            'm' => FormatAct::Monospaced,
            't' if !in_visual => FormatAct::Table,
            'k' => FormatAct::Link,
            _ => return,
        };
        if in_visual {
            self.editor.leave_visual();
        }
        // The leader keys style in place, as the plugin's maps did: no
        // typing after them, unless the style is for what is typed next.
        let sel = from.selection;
        let (first, last) = (from.first, from.last);
        match act {
            FormatAct::Link => {
                self.push(
                    Box::new(Ask::new("Link", "Link to:", "https://")),
                    Then::Link(sel),
                );
            }
            FormatAct::Table => {
                self.editor.change(table::insert);
                self.take_insert();
            }
            FormatAct::Monospaced => {
                self.editor
                    .change(|doc| format::monospaced(doc, first, last));
            }
            FormatAct::Paragraph(style) => {
                self.editor
                    .change(|doc| format::paragraph(doc, first, last, style));
            }
            FormatAct::Character(style) => {
                self.editor
                    .change(|doc| format::character(doc, style, sel, None));
                self.take_insert();
            }
            _ => {}
        }
        self.changed();
    }

    /// Easy mode's Cmd keys.
    fn cmd_key(&mut self, cmd: easy::Cmd) {
        match cmd {
            // Saved without a word, as easy.lua's :write was.
            easy::Cmd::Save => self.save(),
            easy::Cmd::Undo | easy::Cmd::Redo => {
                if matches!(self.editor.mode, Mode::Visual { .. }) {
                    self.editor.leave_visual();
                }
                let keep_insert = self.editor.mode == Mode::Insert;
                let done = if cmd == easy::Cmd::Undo {
                    self.editor.undo()
                } else {
                    self.editor.redo()
                };
                if keep_insert {
                    self.editor.mode = Mode::Insert;
                }
                if done {
                    self.changed();
                }
            }
            easy::Cmd::Cut => {
                if let Some(text) = self.editor.selected_text() {
                    self.clip(&text);
                    let keep = self.editor.clipboard;
                    self.editor.clipboard = false;
                    self.editor.cut();
                    self.editor.clipboard = keep;
                    self.changed();
                }
            }
        }
    }

    /// `.`: the last change made again.
    fn repeat(&mut self) {
        let keys = self.last_change.clone();
        if keys.is_empty() {
            return;
        }
        self.replaying = true;
        for key in keys {
            self.note_key_inner(key);
        }
        if self.editor.mode == Mode::Insert {
            self.editor.key(Key::new(KeyCode::Esc, Mods::NONE));
            self.after_insert();
        }
        self.replaying = false;
    }

    /// Ctrl-V while typing: the clipboard's image pasted under the line,
    /// when it holds one. Whether it did.
    fn try_paste_image(&mut self) -> bool {
        let Some(path) = self.current_path().map(Path::to_path_buf) else {
            return false;
        };
        let grab = self.grab;
        images::paste(&mut self.editor.doc, &path, &|f| grab(f)).is_ok()
    }

    /// Typing [[: every note's title, to link to.
    fn complete(&mut self) {
        let titles = self.store.titles();
        if titles.is_empty() {
            return;
        }
        let items: Vec<PickItem> = titles
            .into_iter()
            .map(|t| PickItem::new(t.clone(), t))
            .collect();
        self.push(
            Box::new(Picker::new("Link to a note", items)),
            Then::Complete,
        );
    }

    fn paste(&mut self, text: &str) {
        if let Some(layer) = self.layers.last_mut() {
            let step = layer.overlay.paste(text, &self.look);
            return self.step(step);
        }
        if let Some((_, t)) = &mut self.prompt {
            t.push_str(text.lines().next().unwrap_or(""));
            return;
        }
        if self.open.is_none() || self.focus != Focus::Note {
            return;
        }
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let mode = self.editor.mode;
        // One step to undo, whatever mode it came in.
        self.editor.begin_any();
        match mode {
            Mode::Insert => self.editor.type_text(&text),
            Mode::Visual { .. } => {
                let keep = std::mem::replace(&mut self.editor.clipboard, false);
                self.editor.cut();
                self.editor.clipboard = keep;
                let col = self.editor.doc.col;
                self.editor.insert_at(col);
                self.editor.type_text(&text);
                self.editor.key(Key::new(KeyCode::Esc, Mods::NONE));
            }
            Mode::Normal => {
                // Pasted in Normal mode: after the cursor, as typed there.
                let line = self.editor.line();
                let col = self.editor.doc.col.min(line.len());
                let after = line[col..]
                    .chars()
                    .next()
                    .map_or(col, |c| col + c.len_utf8());
                self.editor.insert_at(after);
                self.editor.type_text(&text);
                self.editor.key(Key::new(KeyCode::Esc, Mods::NONE));
            }
        }
        if self.editor.end().is_some() || mode == Mode::Insert {
            self.changed();
        }
    }

    fn effects(&mut self) {
        for effect in self.editor.effects() {
            match effect {
                Effect::Message(m) => self.say(m),
                Effect::Copy(text) => self.clip(&text),
                Effect::Save => self.save(),
                Effect::Quit => self.quit = true,
                Effect::SaveQuit => {
                    self.save();
                    self.quit = true;
                }
                Effect::Prompt(c) => self.prompt = Some((c, String::new())),
                Effect::Window(dir) => self.window_move(dir),
            }
        }
    }

    /// Puts text on the clipboard: through the terminal (OSC 52), which the
    /// multiplexer passes on, and the desktop's own outside it.
    fn clip(&mut self, text: &str) {
        if !self.opts.clipboard {
            return;
        }
        self.out.extend_from_slice(
            format!("\x1b]52;c;{}\x07", util::base64(text.as_bytes())).as_bytes(),
        );
        if std::env::var_os("SWITCHBOARD").is_none() {
            let text = text.to_string();
            std::thread::spawn(move || {
                let _ = crate::platform::copy(&text);
            });
        }
    }

    // ── The mouse ─────────────────────────────────────────────────────────

    pub fn mouse(&mut self, m: Mouse) {
        if self.locked_by.is_some() {
            return;
        }
        if let Some(layer) = self.layers.last_mut() {
            let step = layer.overlay.mouse(m, &self.look);
            return self.step(step);
        }
        let (x, y) = (m.col, m.row);
        let inside = |r: Rect| x >= r.x && x < r.right() && y >= r.y && y < r.bottom();
        let button = self
            .geo
            .toolbar
            .iter()
            .find(|(_, r)| inside(*r))
            .map(|(i, _)| *i);
        match (m.kind, m.button) {
            (MouseKind::Move, _) => {
                if self.opts.hover {
                    self.hover_button = button;
                }
            }
            (MouseKind::Press, MouseButton::WheelDown | MouseButton::WheelUp) => {
                let down = m.button == MouseButton::WheelDown;
                if inside(self.geo.list) {
                    self.list_top = if down {
                        (self.list_top + 3).min(self.list_rows.len().saturating_sub(1))
                    } else {
                        self.list_top.saturating_sub(3)
                    };
                } else if inside(self.geo.note) {
                    self.scroll(3, down);
                }
            }
            (MouseKind::Press, MouseButton::Left) => {
                if let Some(b) = button {
                    return self.button(b);
                }
                if inside(self.geo.list) {
                    // Leaving the note saves it, as leaving its buffer did.
                    if self.focus == Focus::Note {
                        self.save();
                    }
                    self.focus = Focus::List;
                    let at = self.list_top + (y - self.geo.list.y) as usize;
                    if at < self.list_rows.len() {
                        self.list_at = at;
                    }
                    return;
                }
                if inside(self.geo.text) {
                    self.focus = Focus::Note;
                    if let Some(pos) = self.at_screen(x, y) {
                        if matches!(self.editor.mode, Mode::Visual { .. }) {
                            self.editor.leave_visual();
                        }
                        self.editor.doc.row = pos.0;
                        self.editor.doc.col = pos.1.unwrap_or(0);
                        self.editor.clamp();
                        self.press = Some(((pos.0, pos.1.unwrap_or(usize::MAX)), false));
                    }
                } else if inside(self.geo.note) {
                    self.focus = Focus::Note;
                }
            }
            (MouseKind::Drag, MouseButton::Left) => {
                if let Some((from, _)) = self.press
                    && let Some(pos) = self.at_screen(x, y)
                {
                    let from = (from.0, if from.1 == usize::MAX { 0 } else { from.1 });
                    let to = (pos.0, pos.1.unwrap_or(0));
                    if to != from || matches!(self.editor.mode, Mode::Visual { .. }) {
                        if self.editor.mode == Mode::Insert {
                            self.editor.key(Key::new(KeyCode::Esc, Mods::NONE));
                            self.after_insert();
                        }
                        self.editor.select(from, to);
                        self.press = Some(((from.0, from.1), true));
                    }
                }
            }
            (MouseKind::Release, _) => {
                if inside(self.geo.list) && self.focus == Focus::List {
                    let at = self.list_top + (y - self.geo.list.y) as usize;
                    if at == self.list_at {
                        self.activate();
                    }
                    return;
                }
                if let Some(((row, col), dragged)) = self.press.take()
                    && !dragged
                {
                    let col = (col != usize::MAX).then_some(col);
                    self.act(row, col, true);
                }
            }
            _ => {}
        }
    }

    /// The wheel over the note: the view moves, and the cursor is brought
    /// into it as it is drawn, at the scrolloff's distance from its edges,
    /// as Vim moves it (`draw_note`).
    fn scroll(&mut self, by: usize, down: bool) {
        let last = self.editor.doc.text.len() - 1;
        self.editor.top = if down {
            (self.editor.top + by).min(last)
        } else {
            self.editor.top.saturating_sub(by)
        };
        self.scrolled = true;
    }

    /// A toolbar button: on the selection when there is one, else on the
    /// line or the word the cursor is on, as the Format menu does.
    fn button(&mut self, index: usize) {
        let b = easy::BUTTONS[index];
        self.focus = Focus::Note;
        let typing = (self.editor.mode == Mode::Insert).then_some(self.editor.doc.col);
        let from = self.format_from(typing);
        self.leave_modes();
        match b.item.and_then(menu::by_label) {
            None => self.push(Box::<FormatMenu>::default(), Then::Format(from)),
            Some(item) => self.run_format(item.act, from),
        }
    }
}

/// A map by note moved to the notes' new paths, where `follow` gives one.
fn rekey<V>(map: &mut HashMap<PathBuf, V>, follow: &dyn Fn(&Path) -> Option<PathBuf>) {
    let moved: Vec<PathBuf> = map
        .keys()
        .filter(|p| follow(p).is_some())
        .cloned()
        .collect();
    for from in moved {
        if let (Some(v), Some(to)) = (map.remove(&from), follow(&from)) {
            map.insert(to, v);
        }
    }
}

/// The side an M-arrow moves to (h, j, k or l), for an arrow with Alt
/// alone.
fn alt_arrow(key: &Key) -> Option<char> {
    if key.mods != Mods::ALT {
        return None;
    }
    match key.code {
        KeyCode::Left => Some('h'),
        KeyCode::Down => Some('j'),
        KeyCode::Up => Some('k'),
        KeyCode::Right => Some('l'),
        _ => None,
    }
}

/// Opens a link in the browser, in its site's Chrome profile, off the
/// screen's thread.
fn open_link(url: String) {
    std::thread::spawn(move || {
        let _ = crate::links::open_url(&url);
    });
}

// ── Running it ──────────────────────────────────────────────────────────────

/// The terminal as the app wants it while it runs, and put back however it
/// ends: raw, on the alternate screen, clicks, drags and the wheel reported
/// (every movement too with the hover on), pastes bracketed and focus told,
/// and keys told apart (the kitty keyboard protocol's flag 1, and
/// modifyOtherKeys where the terminal knows only that), as Neovim asked for
/// them, so Ctrl-Shift-P reaches the app as itself rather than as Ctrl-P.
/// In a pane the workspace sends keys so only while the app asks.
struct Term;

impl Term {
    fn enter(motion: bool) -> std::io::Result<Term> {
        crossterm::terminal::enable_raw_mode()?;
        let mut out = std::io::stdout();
        out.write_all(
            b"\x1b[?1049h\x1b[?25l\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[?2004h\x1b[?1004h\x1b[>4;2m\x1b[>1u",
        )?;
        if motion {
            out.write_all(b"\x1b[?1003h")?;
        }
        out.flush()?;
        Ok(Term)
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        let mut out = std::io::stdout();
        let _ = out.write_all(
            b"\x1b[<u\x1b[>4m\x1b[?1004l\x1b[?2004l\x1b[?1003l\x1b[?1006l\x1b[?1002l\x1b[?1000l\x1b[0m\x1b[2 q\x1b[?25h\x1b[?1049l",
        );
        let _ = out.flush();
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// Set by SIGHUP or SIGTERM: the app saves and ends at the next turn.
static ENDED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    ENDED.store(true, Ordering::Relaxed);
}

/// Marks the pane Notes runs in, or lets the mark go: `@notes_cmd_keys`
/// for the workspace to pass it easy mode's Cmd keys, `@allow_passthrough`
/// to pass its pictures' graphics commands on to the terminal, as bin/notes
/// set its pane's allow-passthrough (src/mux/passthrough.rs).
fn mark_pane(option: &str, on: bool) {
    let Ok(pane) = std::env::var("SWITCHBOARD_PANE") else {
        return;
    };
    let args: Vec<&str> = if on {
        vec!["set", "-t", &pane, option, "1"]
    } else {
        vec!["set", "-t", &pane, "-u", option]
    };
    let _ = crate::mux::socket::ask(&args);
}

/// Runs the app on this terminal until it is quit.
pub fn run(store: Store) -> ExitCode {
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let (tx, rx): (Sender<Msg>, Receiver<Msg>) = channel();
    let opts = Options::read();
    let mut store = store;
    store.server = opts.server;
    for signal in [libc::SIGHUP, libc::SIGTERM] {
        // SAFETY: a handler that only stores to an atomic.
        unsafe {
            libc::signal(signal, on_signal as *const () as libc::sighandler_t);
        }
    }
    let term = match Term::enter(opts.hover && (opts.toolbar || opts.easy)) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("switchboard notes: can't take the terminal: {e}");
            return ExitCode::FAILURE;
        }
    };
    // The terminal's background, for the `system` theme, which Neovim
    // followed through Ghostty's; a key typed meanwhile is read as typed.
    let (colors, typed) = crate::mux::terminal::ask(Duration::from_millis(200));
    if let Some(light) = colors.light() {
        crate::status::theme::set_system(light);
    }
    let opts = Options {
        light: Options::read().light,
        ..opts
    };
    let mut app = App::new(store, opts, cols, rows, tx.clone());
    if !typed.is_empty() {
        let _ = tx.send(Msg::Input(typed));
    }
    app.start();
    let _watch = if app.locked_by.is_none() {
        let tx = tx.clone();
        sync::watch(app.store.root(), move || {
            let _ = tx.send(Msg::Watched);
        })
    } else {
        None
    };
    let socket_path = socket::path();
    let listener = if app.locked_by.is_none() {
        socket::listen(&socket_path, tx.clone())
    } else {
        None
    };
    if app.locked_by.is_none() && opts.server {
        mark_pane("@allow_passthrough", true);
        if opts.easy {
            mark_pane("@notes_cmd_keys", true);
        }
    }
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin();
            let mut buf = [0u8; 4096];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(Msg::Input(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }
    let mut screen = Screen::default();
    let mut size = (cols, rows);
    let mut dirty = true;
    while !app.quit && !ENDED.load(Ordering::Relaxed) {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(msg) => {
                app.handle(msg);
                dirty = true;
                while let Ok(msg) = rx.try_recv() {
                    app.handle(msg);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if app.tick() {
            dirty = true;
        }
        let now = crossterm::terminal::size().unwrap_or(size);
        if now != size {
            size = now;
            screen.invalidate();
            app.images.forget_sent();
            dirty = true;
        }
        if dirty && !app.quit {
            let mut frame = Frame::new(size.0, size.1);
            frame.cursor = app.draw(&mut frame.buf);
            frame.cursor_style = Some(
                if app.editor.mode == Mode::Insert && app.focus == Focus::Note {
                    crossterm::cursor::SetCursorStyle::SteadyBar
                } else {
                    crossterm::cursor::SetCursorStyle::SteadyBlock
                },
            );
            let mut bytes = screen.render(&frame);
            bytes.append(&mut app.out);
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(&bytes);
            let _ = out.flush();
            dirty = false;
        }
    }
    app.stop();
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(&app.out);
    let _ = out.flush();
    drop(out);
    if listener.is_some() {
        let _ = std::fs::remove_file(&socket_path);
    }
    if app.locked_by.is_none() && opts.server {
        mark_pane("@allow_passthrough", false);
        if opts.easy {
            mark_pane("@notes_cmd_keys", false);
        }
    }
    drop(term);
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests;
