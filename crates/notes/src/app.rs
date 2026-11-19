//! `switchboard notes` on the screen, ported from notes/lua/notes/init.lua,
//! sidebar.lua's keys and easy.lua's: the list of folders, tags and notes on
//! the left, a note on the right drawn the way Notes shows it, edited with
//! Vim's keys, saved as you go.
//!
//! In a note:
//!
//! ```text
//!   Ctrl-F             find a note (in the sidebar too)
//!   Ctrl-B             show or put away the sidebar (in the sidebar too)
//!   Ctrl-Shift-P       every command, in a palette (in the sidebar too); in the
//!                      workspace, its M-p, which lists them
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
//! A note's unticked checklist items have keys of their
//! own, each falling back to Vim's where it has nothing to do: Enter shows
//! the item's plan (its `Plan: [[...]]` line) in a panel beside the note
//! (app/beside.rs), and again starts work on it, o goes to the tab working
//! on it, and `ga` types into the "Add an item" input at the bottom. M-p
//! is the workspace's palette, which lists autopilot working the note and
//! this app's own commands while Notes is in front (`palette` and `run`
//! on the socket). A section for parked work opens folded to one line with
//! its count of items, and the footer shows these keys.
//!
//! In Insert mode, Enter carries a list on, and Tab and Shift-Tab indent an
//! item or move between a table's cells.
//!
//! A note saves itself a moment after each change, and whenever you leave
//! it, and its file is renamed when its title changes (store.rs). Every save
//! first brings in what changed in the file from outside since it was read
//! (`check`, sync.rs), and a watch on the notes directory brings changes in
//! as they happen; nothing writes a note but `save`, and `open_note` giving
//! a note its ID. The open note renamed or moved outside the app is followed
//! by its ID (`follow_id`), so it is saved at its new path, never the old. Nothing in a note is sent anywhere but by a squash, after
//! a yes.
//!
//! A note's stable ID, its last line (id.rs), is kept out of the text the
//! editor holds: read off as a note opens, so it is never shown, typed over
//! or styled, and written back last on every save, so a save puts it back
//! when the file lost it and drops another note's ID pasted in. A note
//! opened without one, or a copy of another note's file holding the same,
//! is given one at once, its modified time kept, so opening an old note
//! doesn't move it up the list; a note in Recently Deleted is left as it
//! is. Only the file given another ID outside the app (wt or the mode
//! giving a copy one of its own) changes the one the note keeps.
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
use ratatui::style::{Color, Style};

use super::editor::{self, Editor, Effect, Mode};
use super::format::{self, Selection};
use super::id;
use super::images::{self, Images};
use super::jumps::{Jump, Jumps};
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
use crate::host::settings;
use crate::util;
use switchboard_ui::input::{Input, Key, KeyCode, Mods, Mouse, MouseButton, MouseKind, Parser};
use switchboard_ui::screen::Screen;
use switchboard_ui::{
    Confirm, Look, Menu, MenuOption, Message, Outcome, Overlay, PickItem, Picker, Step,
};

mod beside;
mod draw;

/// The jump list's key in `.notes.json`.
const JUMPS: &str = "jumps";

/// The file in the state folder holding the list's own width, in columns:
/// what a drag of its edge or the pane resize keys set. Missing, it takes
/// its width from the screen. Kept outside the notes folder, whose watcher
/// would have every open Notes read the notes again for a width.
const LIST_WIDTH: &str = "notes-list-width";

/// The file in the state folder holding the plan's own width beside its
/// note, as the list's is kept.
const PLAN_WIDTH: &str = "notes-plan-width";

/// The least width the list is given by hand, which keeps "⌘ Commands" and
/// "/ search" on its top line; the most is half the app's.
const LIST_LEAST: u16 = 30;

/// How far a press of a pane resize key moves an edge, as the workspace's
/// own keys move a divider.
const RESIZE_BY: i32 = 5;

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
    /// UX 2.0: the tabs on notes' items, from the background look.
    ItemTabs(HashMap<String, render::ItemTab>),
    /// UX 2.0: the items autopilot has put next in its queue, each its
    /// note's ID and its first line (`note_item_key`), from the same look.
    Next(HashSet<String>),
    /// A link opened: what was said meanwhile, and whether the browser
    /// came to the front, or why it couldn't open.
    Opened(String, Result<bool, String>),
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
    /// UX 2.0's "⌘ Commands" on the list's top line (palette_button),
    /// which Notes' own tab has in place of its header's.
    pub commands: bool,
    /// Ctrl-V pastes the clipboard's image while typing (notes_ctrl_v_image).
    pub ctrl_v_image: bool,
    /// Pictures drawn under their lines (notes_images).
    pub images: bool,
    /// A mermaid block drawn as its diagram while the cursor is outside
    /// it (notes_mermaid).
    pub mermaid: bool,
    pub light: bool,
    /// The list shown as the app starts (`NOTES_SIDEBAR=off` starts it
    /// put away, beside a tab).
    pub list: bool,
    /// A click on a link follows it, as in Notes.
    pub follow_links_on_click: bool,
    /// Line numbers in a gutter left of the note.
    pub numbers: bool,
    /// The numbers relative to the cursor's line, for jumps like 5j, as
    /// Vim's `relativenumber` (notes_relative_numbers); else each line's own.
    pub relative_numbers: bool,
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
    /// Chips at the right of a checklist item saying what is
    /// happening with it (notes_item_chips).
    pub item_chips: bool,
    /// The terminal draws the kit's half-circle ends itself (Ghostty).
    pub caps: bool,
    /// A scroll bar at the list's right edge and the note's while they run
    /// past the screen (scroll_bars).
    pub scroll_bars: bool,
    /// The notes opened lately under Recent in the list (notes_recent).
    pub recent: bool,
    /// An app in each tab (notes_per_tab): a lock a note rather than one
    /// on the whole directory, and a socket of its pane's own.
    pub per_note: bool,
    /// The list's edge takes a drag and the pane resize keys, and the
    /// width kept is followed (notes_resize).
    pub resize: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            toolbar: true,
            easy: false,
            key_help: true,
            hover: true,
            palette: true,
            commands: true,
            ctrl_v_image: true,
            images: true,
            mermaid: true,
            light: false,
            list: true,
            follow_links_on_click: true,
            numbers: true,
            relative_numbers: false,
            strike_checked: true,
            move_checked_to_bottom: true,
            gap_before_checked: true,
            pane_arrows: true,
            clipboard: false,
            server: false,
            item_chips: true,
            caps: false,
            scroll_bars: true,
            recent: true,
            per_note: false,
            resize: true,
        }
    }
}

impl Options {
    pub fn read() -> Options {
        let all = settings::Read::of(&[
            "notes_toolbar",
            "easy_keys",
            "key_help_line",
            "hover_highlight",
            "notes_palette",
            "palette_button",
            "notes_ctrl_v_image",
            "notes_images",
            "notes_mermaid",
            "theme",
            "pane_arrows",
            "notes_item_chips",
            "scroll_bars",
            "notes_recent",
            "notes_relative_numbers",
            "notes_per_tab",
            "notes_resize",
        ]);
        let on = |key: &str| all.on(key);
        Options {
            toolbar: on("notes_toolbar"),
            easy: all.set_on("easy_keys"),
            key_help: on("key_help_line"),
            hover: on("hover_highlight"),
            palette: on("notes_palette"),
            commands: on("palette_button"),
            ctrl_v_image: on("notes_ctrl_v_image"),
            images: on("notes_images") && crate::host::graphics::apps_draw(),
            mermaid: on("notes_mermaid"),
            light: all
                .value("theme")
                .is_some_and(switchboard_ui::theme::light_for),
            list: std::env::var("NOTES_SIDEBAR").map_or(true, |v| v != "off"),
            pane_arrows: on("pane_arrows"),
            clipboard: true,
            server: crate::host::socket::in_pane(),
            item_chips: on("notes_item_chips"),
            scroll_bars: on("scroll_bars"),
            recent: on("notes_recent"),
            relative_numbers: all.set_on("notes_relative_numbers"),
            per_note: on("notes_per_tab"),
            resize: on("notes_resize"),
            caps: crate::host::graphics::apps_round_ends(),
            ..Options::default()
        }
    }
}

/// How often the palette_button and notes_resize settings are read
/// again, which Settings changes while the app runs on.
const SETTINGS_EVERY: Duration = Duration::from_secs(2);

/// The open note's lock held by another app (`Store::lock_note`): the
/// note is shown read-only, saying which tab has it, until that app lets
/// it go.
#[derive(Clone, Debug)]
pub struct Held {
    pub key: String,
    pub holder: store::Holder,
    /// The number of the holder's tab, as the workspace last listed it.
    pub tab: Option<i64>,
    /// When the lock was last tried again.
    checked: Instant,
}

/// How often a note held by another app has its lock tried again.
const HELD_CHECK: Duration = Duration::from_secs(1);

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
    /// Its stable ID, written as its last line on every save; `None` for a
    /// note in Recently Deleted that had none.
    id: Option<String>,
    /// Its file as last read or written (`file_key`), which a rename keeps
    /// and a copy doesn't, so a note is followed by its ID only to the
    /// file it was moved to, never to a copy of it holding the same ID.
    file: Option<(u64, u64)>,
}

/// A file's device and inode.
/// A file's modified time and length, which change when it is written.
type FileStamp = (std::time::SystemTime, u64);

fn file_stamp(path: &Path) -> Option<FileStamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

fn file_key(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
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
    /// The box an edit tried on a held note opens: go to its tab, take it
    /// over, or keep reading.
    Held,
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
        /// UX 2.0's chips at the right of the line.
        chips: Vec<render::Chip>,
    },
    /// UX 2.0's chips on a line of their own: under a checklist item, at
    /// its right edge, when the item's line is shown whole and leaves no
    /// room beside it; or in the place of the item's plan line (`row`),
    /// from `at` columns in, as board 7 draws them. `item` is the item's
    /// row, which a click on one of them puts the cursor on.
    Chips {
        row: usize,
        item: usize,
        at: Option<u16>,
        chips: Vec<render::Chip>,
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
    /// A row of a mermaid block's diagram, in the place of the block, its
    /// first row the block's opening fence; `cut` where it ran past the
    /// note's edge.
    Diagram {
        row: usize,
        r: usize,
        text: String,
        cut: bool,
    },
    /// A dim line under a diagram, saying it was cut or isn't drawn.
    Aside {
        row: usize,
        text: String,
    },
}

impl Shown {
    fn row(&self) -> usize {
        match self {
            Shown::Text { row, .. }
            | Shown::Chips { row, .. }
            | Shown::Fold { row, .. }
            | Shown::Picture { row, .. }
            | Shown::Diagram { row, .. }
            | Shown::Aside { row, .. } => *row,
        }
    }
}

/// Where things were last drawn, for the mouse.
#[derive(Default)]
struct Geo {
    list: Rect,
    /// UX 2.0's "/ search" at the right of the list's top line (board 7),
    /// when it is drawn.
    search: Option<Rect>,
    /// And "⌘ Commands" beside it, when it is drawn.
    commands: Option<Rect>,
    /// "⌘ Commands" goes at the top right corner instead, the list's top
    /// line not drawn (the list put away, or a plan beside the note).
    corner: bool,
    note: Rect,
    /// The note's text, right of its numbers.
    text: Rect,
    toolbar: Vec<(usize, Rect)>,
    shown: Vec<Shown>,
    /// UX 2.0's chips at the right of the checklist items, where each was
    /// drawn: its row of the note, its place among the row's chips, and
    /// what it is.
    chips: Vec<(Rect, (usize, usize), render::Chip)>,
    /// UX 2.0's "Add an item" input's row, when it is drawn.
    input: Option<u16>,
    /// The row saying which tab has the note shown read-only, which a
    /// click takes there as Enter does.
    held_line: Option<u16>,
    /// UX 2.0's plan beside the note, where it was drawn, and its cards.
    plan: Rect,
    plan_hits: Vec<(Rect, beside::Stop)>,
    /// The note's scroll bar, when it is drawn: its column, first row and
    /// height, the shown lines' rows of the note (a closed fold's hidden
    /// ones left out) and how many show from the first.
    note_bar: Option<NoteBar>,
    /// The mermaid blocks drawn as their diagrams, their fences' rows, for
    /// the wheel.
    diagrams: Vec<(usize, usize)>,
    /// The line right of the list, where it was drawn, while a drag moves
    /// it (notes_resize).
    list_edge: Option<Rect>,
    /// The rule left of the plan beside its note, while a drag moves it
    /// (notes_resize).
    plan_edge: Option<Rect>,
}

/// The note's scroll bar as it was drawn, for the mouse.
struct NoteBar {
    x: u16,
    y: u16,
    height: u16,
    rows: Vec<usize>,
    /// The first line shown, which the wheel may have taken past `last`;
    /// the thumb is drawn at `last` then.
    top: usize,
    /// The first line that shows the note's end whole, the last place the
    /// top goes.
    last: usize,
    shown: usize,
}

impl NoteBar {
    /// What the bar measures, so its thumb's places are the lines from the
    /// first to `last`: those, and the lines shown.
    fn places(&self) -> usize {
        self.last + self.shown
    }
}

/// Which scroll bar's thumb is being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BarDrag {
    List,
    Note,
}

/// A note shown when there are none: what a note can do.
pub const WELCOME: &str = "# Welcome to Notes

This note shows what a note can do. Change it, or delete it from the sidebar on the left with `dd`. Press Ctrl-P for the Format menu.

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

/// A note's own keys over Vim's (`note_key`), for mission control's Keys
/// view (the tmux version's lib/sidebar/app-keys.json); the leader keys and
/// easy mode's Cmd keys follow them there, from the tables their code reads.
pub const NOTE_KEYS: &[(&str, &str)] = &[
    ("Enter", "tick, fold or follow what the cursor is on"),
    ("Ctrl-P", "the Format menu, on the word, line or selection"),
    (
        "Ctrl-Shift-P",
        "every Notes command, in a palette: in the workspace, M-p's",
    ),
    (
        "gw",
        "start work on the checklist item in a new tab: pick the repository and the branch to land on",
    ),
    (
        "Enter",
        "on an unticked item with a plan: its plan beside the note; again, start work on it",
    ),
    (
        "M-e",
        "in Notes' own tab: the plan of the item under the cursor beside the note, or put away",
    ),
    (
        "C-w l  C-w h",
        "the plan beside the note or the note: a, b choose an option, e opens the plan, Esc closes it",
    ),
    ("o", "on an item a tab works on: go to that tab"),
    (
        "ga",
        "Add an item, Enter adding it to the top list, M-Enter having a new tab plan it",
    ),
    ("Ctrl-F", "find a note"),
    ("Ctrl-B", "show or put away the sidebar"),
    (
        "Ctrl-O  Ctrl-I",
        "back, forward through the places jumped from, across notes",
    ),
    ("Ctrl-V", "paste the clipboard's image, while typing"),
    ("Enter", "carry the list on, while typing"),
    (
        "Tab  Shift-Tab",
        "next, previous table cell, or indent, outdent the item, while typing",
    ),
    ("Ctrl-C", "copy the selection"),
];

/// Notes' keys, a group each, for mission control's Keys view: a note's
/// (its own, then each leader key, then easy mode's Cmd keys while
/// `easy` is on), then the list's (what its `?` shows).
pub fn key_groups(easy: bool) -> Vec<(&'static str, Vec<(String, String)>)> {
    let owned = |rows: &[(&str, &str)]| -> Vec<(String, String)> {
        rows.iter()
            .map(|(k, d)| (k.to_string(), d.to_string()))
            .collect()
    };
    let mut note = owned(NOTE_KEYS);
    for item in menu::leader_items() {
        let does = match item.act {
            FormatAct::Table => format!("{}, below the line", item.label),
            _ => format!("{}, on the word, line or selection", item.label),
        };
        note.push((format!("Space {}", item.key), does));
    }
    if easy {
        for (_, _, shown, does) in easy::CMD_KEYS {
            note.push((shown.to_string(), format!("{does} (easy mode)")));
        }
    }
    vec![
        ("Notes: a note", note),
        ("Notes: the sidebar", owned(sidebar::KEYS)),
    ]
}

/// An item M-Enter handed to a new tab's `/shape --notes`: the note it is for,
/// its words, and how many checklist lines named it then. The note is
/// known by its stable ID, as autopilot knows one, so it is found however
/// it was renamed or moved, and a copy, given an ID of its own as it
/// opens, never takes it; by its path only when it has no ID (a note in
/// Recently Deleted).
#[derive(Debug)]
struct Planning {
    note: PathBuf,
    note_id: Option<String>,
    text: String,
    items: usize,
}

/// How many checklist lines in `lines` name `text`.
fn items_naming(lines: &[String], text: &str) -> usize {
    lines
        .iter()
        .filter(|l| {
            let l = l.trim_start();
            (l.starts_with("- [ ]") || l.starts_with("- [x]")) && l.contains(text)
        })
        .count()
}

/// How long after the last change a note is saved.
const AUTOSAVE: Duration = Duration::from_millis(800);
/// How long after a change in the directory it is looked at.
const WATCH_WAIT: Duration = Duration::from_millis(100);
const MESSAGE_FOR: Duration = Duration::from_secs(5);

pub struct App {
    pub store: Store,
    pub opts: Options,
    /// UX 2.0: the tabs on the note's items, by each item's first line, as
    /// the background look (`look_at_tabs`) last found them; drawing reads
    /// only this.
    pub item_tabs: HashMap<String, render::ItemTab>,
    /// The items autopilot has put next, as `Msg::Next` hands them.
    pub next_items: HashSet<String>,
    /// What spares opening a note from reading every note again for a copy
    /// holding its ID: the notes directory's note files, and each note's
    /// file stamp when its ID was last found to be its own. A new note file
    /// (a copy among them) clears it, and a note whose file changed is
    /// checked again.
    ids_checked: (Vec<PathBuf>, HashMap<PathBuf, FileStamp>),
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
    /// UX 2.0: the plan of an item shown beside the note (board 8).
    beside: Option<beside::Beside>,
    pub editor: Editor,
    open: Option<Open>,
    layers: Vec<Layer>,
    /// The bottom line taking a search or an Ex command.
    prompt: Option<(char, String)>,
    pub message: Option<(String, Instant)>,
    leader: bool,
    /// Ctrl-V's next key typed as it is.
    literal: bool,
    /// Mermaid blocks drawn, by their code and width.
    diagrams: super::diagram::Diagrams,
    /// The notes whose diagrams show as their code (the Format menu's
    /// "Show diagram code"), until "Show diagrams".
    diagram_code: HashSet<PathBuf>,
    pub images: Images,
    /// Escapes written after the frame: the clipboard's, the pictures'.
    pub out: Vec<u8>,
    last_change: Vec<Key>,
    recording: Option<(Vec<Key>, Text)>,
    replaying: bool,
    save_due: Option<Instant>,
    watch_due: Option<Instant>,
    /// When the settings file was last looked at, and its modified time
    /// then, so a palette_button changed in Settings reaches "⌘ Commands"
    /// without the focus coming back.
    settings_seen: Instant,
    squash_pid: Option<u32>,
    squash_paths: Vec<PathBuf>,
    /// The squashed notes' locks this app took for the squash, held until
    /// it ends, so no other app edits a note the squash is about to replace.
    squash_locks: Vec<String>,
    squash_cancelled: bool,
    tx: Sender<Msg>,
    /// The title last named on the pane (`@notes_open`), so it is said
    /// again only when it changes.
    told: Option<String>,
    /// What the message line says while a link opens, which stays until
    /// the browser comes up (`Msg::Opened`).
    opening: Option<String>,
    /// What a unit test opened, links and images, in place of the
    /// browser and Quick Look.
    #[cfg(test)]
    pub opened: Vec<String>,
    cursors: HashMap<PathBuf, (usize, usize, usize)>,
    /// Each note left while the app runs: its undo history and folds, put
    /// back when it opens again.
    kept: HashMap<PathBuf, super::editor::Kept>,
    /// The jump list, across notes (Ctrl-O, Ctrl-I), and the open note's
    /// text its entries were last carried to (`follow_jumps`).
    jumps: Jumps,
    jumps_seen: Text,
    /// Ctrl-O or Ctrl-I opening another note, which isn't a jump of its own.
    jumping: bool,
    /// The jump list as `.notes.json` last had it (`keep_jumps`).
    jumps_kept: Jumps,
    titles: HashSet<String>,
    pub quit: bool,
    /// UX 2.0's "Add an item" input while it is typed into (`ga`, or a
    /// click on it): what is typed so far.
    pub adding: Option<String>,
    /// The items handed to a new tab's `/shape --notes` (M-Enter), each kept
    /// until it shows in its note: the repository's picker closed, or the
    /// tab not opened, the last of a note's is back in the input the next
    /// time it opens there.
    planning: Vec<Planning>,
    pub locked_by: Option<i64>,
    /// The open note's lock key while this app holds it (notes_per_tab).
    note_lock: Option<String>,
    /// The open note held by another app, shown read-only.
    pub held: Option<Held>,
    recent_file: PathBuf,
    /// Saves the clipboard's image to a file (the platform's; a test's).
    pub grab: fn(&Path) -> bool,
    /// The program that squashes notes: Claude Code.
    pub claude: Option<PathBuf>,
    geo: Geo,
    hover_button: Option<usize>,
    /// Where the mouse last moved to, for the item's chip under it to be
    /// lit wherever the chips are drawn next.
    mouse_at: Option<(u16, u16)>,
    /// Where a press in the note landed, to tell a click from a drag.
    press: Option<((usize, usize), bool)>,
    /// A scroll bar's thumb being dragged, its row it was taken by
    /// (`kit::scroll_grab`), and how many of the note's lines showed then,
    /// kept for the drag so the thumb keeps its length as the lines it
    /// passes wrap to other heights.
    bar_drag: Option<(BarDrag, switchboard_ui::kit::Grab, usize)>,
    /// The list's own width, from a drag of its edge or the pane resize
    /// keys while it has the keys; none to take the screen's
    /// (`list_width`). Kept in the state folder (`notes-list-width`), followed only while
    /// Settings' notes_resize is on.
    list_width: Option<u16>,
    /// Whether the mouse has moved since the press that began a drag of
    /// an edge, off the column it landed on (`edge_from`), so a press let
    /// go with the edge where it was keeps nothing.
    edge_moved: bool,
    edge_from: u16,
    /// What the list shows as last read from the store (`rebuild`), and
    /// the width its rows were last laid out for.
    list_read: super::sidebar::Read,
    list_built_for: u16,
    /// The list's width as a tick last found it and since when, so a width
    /// still changing (the pane being resized) isn't built for.
    list_width_seen: (u16, Option<Instant>),
    /// When the resize keys last moved the list's edge, its width kept
    /// once they stop.
    keys_moved: Option<Instant>,
    /// Whether those keys moved the plan's edge, else the list's.
    keys_plan: bool,
    /// Whether the list's edge is being dragged, and when it was last
    /// pressed, for a double click.
    edge_drag: bool,
    edge_pressed_at: Option<Instant>,
    /// Whether the press held is a double click's second.
    edge_double: bool,
    /// The edge's own width as the press found it, and the width shown.
    edge_start: (Option<u16>, u16),
    /// The plan's own width beside its note, from a drag of its edge or the
    /// pane resize keys while it has the keys; none for board 8's share.
    /// Kept in the state folder (`notes-plan-width`), followed only while
    /// Settings' notes_resize is on.
    plan_width: Option<u16>,
    /// The edge pressed last is the plan's, else the list's.
    edge_plan: bool,
    parser: Parser,
    last_input: Instant,
}

fn grab_clipboard(file: &Path) -> bool {
    crate::host::platform::paste_image(file)
}

/// The list's width for the screen: its full width wherever the note keeps
/// room beside it, else narrower, never under its minimum.
pub fn list_width(cols: u16) -> u16 {
    sidebar::MIN_WIDTH.max(sidebar::WIDTH.min(cols.saturating_sub(60)))
}

/// A width asked for the list, held to what an app `cols` wide lets it
/// be: 30 columns at the least (`LIST_LEAST`), half the app's at the most.
fn list_clamp(width: u16, cols: u16) -> u16 {
    width.clamp(LIST_LEAST, (cols / 2).max(LIST_LEAST))
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
                switchboard_ui::Theme::LIGHT
            } else {
                switchboard_ui::Theme::DARK
            },
            scroll_bars: opts.scroll_bars,
            ..Look::default()
        };
        App {
            recent_file: store.state.join("notes-palette-recent"),
            store,
            look,
            cols: cols.max(20),
            rows: rows.max(5),
            focus: Focus::Note,
            list: sidebar::State {
                recent: opts.recent,
                ..sidebar::State::default()
            },
            list_rows: Vec::new(),
            list_at: 0,
            list_top: 0,
            list_followed: None,
            scrolled: false,
            list_shown: opts.list,
            list_pending: None,
            beside: None,
            editor,
            open: None,
            layers: Vec::new(),
            told: None,
            prompt: None,
            message: None,
            leader: false,
            literal: false,
            images: Images::new(opts.images),
            diagrams: Default::default(),
            diagram_code: HashSet::new(),
            out: Vec::new(),
            last_change: Vec::new(),
            recording: None,
            replaying: false,
            save_due: None,
            watch_due: None,
            settings_seen: Instant::now(),
            item_tabs: HashMap::new(),
            next_items: HashSet::new(),
            ids_checked: Default::default(),
            squash_pid: None,
            squash_paths: Vec::new(),
            squash_locks: Vec::new(),
            squash_cancelled: false,
            tx,
            opening: None,
            #[cfg(test)]
            opened: Vec::new(),
            cursors: HashMap::new(),
            kept: HashMap::new(),
            jumps: Jumps::default(),
            jumps_seen: Text::default(),
            jumping: false,
            jumps_kept: Jumps::default(),
            titles: HashSet::new(),
            quit: false,
            adding: None,
            planning: Vec::new(),
            locked_by: None,
            note_lock: None,
            held: None,
            grab: grab_clipboard,
            claude: crate::util::claude_command(),
            geo: Geo::default(),
            hover_button: None,
            mouse_at: None,
            press: None,
            bar_drag: None,
            list_width: None,
            edge_moved: false,
            edge_from: 0,
            list_read: Default::default(),
            list_built_for: 0,
            list_width_seen: (0, None),
            keys_moved: None,
            keys_plan: false,
            edge_drag: false,
            edge_pressed_at: None,
            edge_double: false,
            edge_start: (None, 0),
            plan_width: None,
            edge_plan: false,
            parser: Parser::new(),
            last_input: Instant::now(),
            opts,
        }
    }

    /// Where the note is drawn, as it last was.
    pub fn note_area(&self) -> Rect {
        self.geo.text
    }

    /// The palette's picks kept here rather than in the state directory.
    pub fn keep_recent_in(&mut self, file: PathBuf) {
        self.recent_file = file;
    }

    pub fn say(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), Instant::now()));
    }

    /// Opens a link in the browser, in its site's Chrome profile, off the
    /// screen's thread, saying where it is going on the message line
    /// (link_opening_message), since a profile's Chrome can take seconds.
    fn open_link(&mut self, url: String) {
        let words = crate::host::links::opening_words(&url);
        if let Some(words) = &words {
            self.say(words.clone());
            self.opening = Some(words.clone());
        }
        if self.stand_in(&url) {
            return;
        }
        let tx = self.tx.clone();
        std::thread::spawn(move || match words {
            Some(words) => {
                let shown = crate::host::links::open_url_watched(&url);
                let _ = tx.send(Msg::Opened(words, shown));
            }
            // With nothing said on the way, only a link that couldn't be
            // opened is said.
            None => {
                if let Err(why) = crate::host::links::open_url(&url) {
                    let _ = tx.send(Msg::Opened(String::new(), Err(why)));
                }
            }
        });
    }

    /// A link has opened: its message gone while it is still the one shown,
    /// else said how it went.
    fn link_opened(&mut self, words: &str, shown: Result<bool, String>) {
        let ours = self.message.as_ref().is_some_and(|(m, _)| m == words);
        // Another link opened since keeps its own message.
        if self.opening.as_deref() == Some(words) {
            self.opening = None;
        }
        let nothing_else = ours || self.message.is_none();
        match shown {
            Ok(true) if ours => self.message = None,
            Ok(true) => {}
            Ok(false) if nothing_else => {
                self.say(format!("{words} The browser hasn't shown it yet."));
            }
            Ok(false) => {}
            Err(why) => self.say(format!("Couldn't open it: {why}")),
        }
    }

    /// Takes the notes' lock and opens the note open last, or the newest; a
    /// welcome note when there are none. Another app holding the lock leaves
    /// this one saying so, its notes untouched.
    pub fn start(&mut self) {
        if !self.opts.per_note
            && let Err(pid) = self.store.lock()
        {
            self.locked_by = Some(pid);
            return;
        }
        self.list_width = self.kept_width();
        self.plan_width = self.kept_plan_width();
        let mut meta = self.store.meta();
        let saved = meta.remove(JUMPS).unwrap_or_default();
        self.jumps = Jumps::from_saved(&saved, |rel| self.store.absolute(rel), Path::is_file);
        self.jumps_kept = self.jumps.clone();
        let mut notes = self.store.notes(None);
        if notes.is_empty() {
            let path = self.store.create("", Some("Welcome to Notes"));
            let _ = std::fs::write(&path, WELCOME);
            notes = self.store.notes(None);
        }
        // The note the workspace started it on (`NOTES_OPEN`, a tab's),
        // else the one its pane showed before a restart brought it back
        // (`@notes_note`, kept with the layout), else the one open last.
        let asked = std::env::var("NOTES_OPEN")
            .ok()
            .or_else(|| self.pane_shown_before())
            .filter(|rel| store::in_notes(rel))
            .map(|rel| self.store.absolute(&rel))
            .filter(|path| path.is_file());
        let first = asked
            .or_else(|| self.store.last())
            .or_else(|| notes.first().map(|n| n.path.clone()));
        if let Some(path) = first {
            self.open_note(&path, false, false);
        } else {
            self.rebuild();
        }
        self.focus = Focus::Note;
        if let Some(words) = std::env::var("NOTES_SAY").ok().filter(|w| !w.is_empty()) {
            self.say(words);
        }
    }

    /// Everything saved, the lock let go, the pictures taken back.
    pub fn stop(&mut self) {
        if self.locked_by.is_some() {
            return;
        }
        self.keep_keyed();
        self.put_title_back();
        self.save();
        self.follow_jumps();
        // Where Notes ends is the newest place, as Neovim's shada keeps it,
        // so Notes started again on another note (a tab's) goes back to it.
        if let Some(o) = &self.open {
            self.jumps.record(Jump {
                note: o.path.clone(),
                row: self.editor.doc.row,
                col: self.editor.doc.col,
            });
        }
        self.keep_jumps();
        self.store.unlock();
        if let Some(key) = self.note_lock.take() {
            self.store.unlock_note(&key);
        }
        let gone = self.images.release_all();
        self.out.extend(gone);
    }

    // ── The note open ─────────────────────────────────────────────────────

    pub fn current_path(&self) -> Option<&Path> {
        self.open.as_ref().map(|o| o.path.as_path())
    }

    /// Whether the open note's mermaid blocks are drawn as their diagrams:
    /// notes_mermaid on, and the note not switched to their code.
    pub(crate) fn shows_diagrams(&self) -> bool {
        self.opts.mermaid
            && self
                .current_path()
                .is_none_or(|p| !self.diagram_code.contains(p))
    }

    /// The Format menu's "Show diagram code", or "Show diagrams" once it
    /// was: the open note's diagrams shown as their code, or drawn again.
    fn switch_diagram_code(&mut self) {
        let Some(path) = self.current_path().map(Path::to_path_buf) else {
            return;
        };
        if !self.diagram_code.remove(&path) {
            self.diagram_code.insert(path);
        }
    }

    /// What the Format menu and the palette call "Show diagram code" for
    /// the open note: "Show diagrams" while it shows their code.
    fn diagram_label(&self) -> &'static str {
        if self.opts.mermaid && !self.shows_diagrams() {
            menu::SHOW_DIAGRAMS
        } else {
            menu::SHOW_DIAGRAM_CODE
        }
    }

    /// The open note's tabs by its items' first lines, for its chips and
    /// `o`: a tab keyed under the note's ID first, then under its path, then
    /// one whose note isn't known under the item alone, so a tab on another
    /// note's item of the same words isn't this note's.
    fn note_tabs(&self) -> HashMap<String, render::ItemTab> {
        let Some(open) = &self.open else {
            return HashMap::new();
        };
        let path = self.store.relative(&open.path);
        let id = open.id.as_deref().unwrap_or("");
        let mut out = HashMap::new();
        for note in [id, path.as_str(), ""] {
            for (key, tab) in &self.item_tabs {
                let item = match key.split_once('\0') {
                    Some((of, item)) if !note.is_empty() && of == note => item,
                    None if note.is_empty() && tab.note.is_empty() && tab.note_id.is_empty() => {
                        key.as_str()
                    }
                    _ => continue,
                };
                out.entry(item.to_string()).or_insert_with(|| tab.clone());
            }
        }
        out
    }

    /// Whether autopilot has put the open note's item next in its queue.
    fn is_next(&self, item: &str) -> bool {
        self.open
            .as_ref()
            .and_then(|o| o.id.as_deref())
            .is_some_and(|id| self.next_items.contains(&note_item_key(id, item)))
    }

    /// Tells the workspace which note is open (`name_open_note`), from the
    /// app that owns the notes directory in a pane of the workspace only.
    fn tell_open(&mut self) {
        let title = self.open.as_ref().map(|o| o.title.clone());
        self.tell_title(title);
    }

    /// Names `title` on the pane when it isn't the one named there last.
    fn tell_title(&mut self, title: Option<String>) {
        if self.opts.server && self.locked_by.is_none() && self.told != title {
            name_open_note(title.as_deref());
            self.told = title;
        }
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
        // The plan beside the note is the open note's item's: another note
        // puts it away.
        if self.current_path() != Some(path) {
            self.close_beside();
        }
        let (text, note_id) = self.give_id(path, text);
        self.follow_jumps();
        if let Some(o) = &self.open
            && !self.jumping
            && o.path != path
        {
            self.jumps.record(Jump {
                note: o.path.clone(),
                row: self.editor.doc.row,
                col: self.editor.doc.col,
            });
            self.editor.leave_as_jump();
        }
        if let Some(o) = &self.open {
            let left = o.path.clone();
            let kept = self.editor.put_away();
            self.kept.insert(left, kept);
        }
        if self.opts.per_note {
            let key = note_id.clone().unwrap_or_else(|| self.store.relative(path));
            self.take_note_lock(key);
        }
        let shown = Text::from_file(&id::body_of(&text));
        match self.kept.remove(path) {
            Some(kept) => self.editor.take_back(shown, kept),
            None => {
                self.editor.load(shown);
                self.fold_parked();
            }
        }
        self.jumps_seen = self.editor.seen().clone();
        self.adding = None;
        self.open = Some(Open {
            path: path.to_path_buf(),
            base: Some(text.clone()),
            saved: self.editor.doc.text.clone(),
            title: store::title_of(&text),
            gone: false,
            id: note_id,
            file: file_key(path),
        });
        if let Some(&(row, col, top)) = self.cursors.get(path) {
            self.editor.doc.row = row;
            self.editor.doc.col = col;
            self.editor.top = top;
            self.editor.clamp();
        }
        // Its tables lined up, as one change.
        self.editor.change_outside(table::align_all);
        if self.modified() {
            self.save_due = Some(Instant::now() + AUTOSAVE);
        }
        self.tell_open();
        let rel = self.store.relative(path);
        self.store.opened(&rel);
        self.store.opened_beside(&rel);
        self.publish_shown();
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
        self.keep_jumps();
    }

    /// The jump list written to `.notes.json` when it changed, so Notes
    /// started again (in another tab, after a restart) goes back through
    /// it.
    fn keep_jumps(&mut self) {
        if self.jumps.same_entries(&self.jumps_kept) {
            return;
        }
        let saved = self.jumps.saved(|p| self.store.relative(p));
        self.store.update_meta(|meta| {
            meta.insert(JUMPS.into(), saved);
            true
        });
        self.jumps_kept = self.jumps.clone();
    }

    /// The note this app's pane showed before (`@notes_note`), with an app
    /// in each tab: a pane a restart brought back keeps its options.
    fn pane_shown_before(&self) -> Option<String> {
        if !self.opts.per_note || !self.opts.server {
            return None;
        }
        let pane = crate::host::pane()?;
        crate::host::socket::ask_ok(&["show", "-t", &pane, "@notes_note"])
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    }

    /// The note open, kept in this app's pane's `@notes_note` for the
    /// workspace's link, picture and tab commands, with an app in each tab.
    fn publish_shown(&self) {
        if self.opts.per_note && self.opts.server {
            let rel = self.current_path().map(|p| self.store.relative(p));
            set_pane_option("@notes_note", rel.as_deref());
        }
    }

    /// Whether another app has `path` open (notes_per_tab), said when it
    /// has: a note is changed (renamed, moved, deleted, squashed) only by
    /// the app that holds it.
    fn held_elsewhere(&mut self, path: &Path) -> bool {
        if !self.opts.per_note || self.current_path() == Some(path) && self.held.is_none() {
            return false;
        }
        let key = self.lock_key(path);
        let Some(holder) = self.store.held_by_other(&key) else {
            return false;
        };
        let words = match holder.pane.as_deref().and_then(|p| self.tab_of(p)) {
            Some(n) => format!("That note is open in tab {n}: change it there"),
            None => "That note is open in another Notes: change it there".to_string(),
        };
        self.say(words);
        true
    }

    /// A note's lock key: its ID, else its path.
    fn lock_key(&self, path: &Path) -> String {
        util::read_text(path)
            .and_then(|text| id::in_text(&text))
            .unwrap_or_else(|| self.store.relative(path))
    }

    /// The squashed notes' locks taken for the squash (notes_per_tab), or,
    /// another app holding one, none and said so.
    fn lock_for_squash(&mut self, paths: &[PathBuf]) -> bool {
        if !self.opts.per_note {
            return true;
        }
        for path in paths {
            // Given its ID first, as opening it would, so the key is the
            // one another app opening it takes.
            let key = match util::read_text(path) {
                Some(text) => self
                    .give_id(path, text)
                    .1
                    .unwrap_or_else(|| self.store.relative(path)),
                None => self.store.relative(path),
            };
            // The open note's too, so opening another meanwhile keeps it.
            if self.store.lock_note(&key).is_err() {
                self.unlock_squashed();
                self.held_elsewhere(path);
                return false;
            }
            self.squash_locks.push(key);
        }
        true
    }

    /// The locks `lock_for_squash` took let go.
    fn unlock_squashed(&mut self) {
        for key in std::mem::take(&mut self.squash_locks) {
            if self.note_lock.as_ref() != Some(&key) {
                self.store.unlock_note(&key);
            }
        }
    }

    /// Whether another app has any note under `folder` open, said as
    /// `held_elsewhere` says it.
    fn folder_held_elsewhere(&mut self, folder: &str) -> bool {
        let prefix = format!("{folder}/");
        let paths: Vec<PathBuf> = self
            .store
            .notes(None)
            .into_iter()
            .filter(|n| n.rel.starts_with(&prefix))
            .map(|n| n.path)
            .collect();
        paths.iter().any(|p| self.held_elsewhere(p))
    }

    /// The open note's lock taken, the last one's let go; held by another
    /// app, the note is shown read-only (`held`), naming its tab.
    fn take_note_lock(&mut self, key: String) {
        if let Some(old) = self.note_lock.take()
            && old != key
            && !self.squash_locks.contains(&old)
        {
            self.store.unlock_note(&old);
        }
        match self.store.lock_note(&key) {
            Ok(()) => {
                self.note_lock = Some(key);
                self.held = None;
            }
            Err(holder) => {
                let tab = holder.pane.as_deref().and_then(|p| self.tab_of(p));
                self.held = Some(Held {
                    key,
                    holder,
                    tab,
                    checked: Instant::now(),
                });
            }
        }
    }

    /// The number of the tab a pane is in, asked of the workspace.
    fn tab_of(&self, pane: &str) -> Option<i64> {
        if !self.opts.server {
            return None;
        }
        let list: serde_json::Value =
            serde_json::from_str(&crate::host::socket::ask_ok(&["list"]).ok()?).ok()?;
        list["sessions"]
            .as_array()?
            .iter()
            .flat_map(|s| s["tabs"].as_array().into_iter().flatten())
            .find(|t| {
                t["panes"]
                    .as_array()
                    .is_some_and(|ps| ps.iter().any(|p| p["id"].as_str() == Some(pane)))
            })
            .and_then(|t| t["index"].as_i64())
    }

    /// What a held note's line says: where it is open, and that Enter
    /// goes there.
    pub fn held_line(&self) -> Option<String> {
        let held = self.held.as_ref()?;
        Some(match held.tab {
            Some(n) => format!("Open in tab {n}, read-only here · Enter or a click: go there"),
            None => "Open in another Notes, read-only here".to_string(),
        })
    }

    /// A held note's lock tried again: let go by its app, it is this
    /// one's, its text read again and editable.
    fn check_held(&mut self) -> bool {
        let Some(held) = &mut self.held else {
            return false;
        };
        if held.checked.elapsed() < HELD_CHECK {
            return false;
        }
        held.checked = Instant::now();
        let key = held.key.clone();
        match self.store.lock_note(&key) {
            Ok(()) => {
                self.became_yours(key, "This note is yours to edit now".into());
                true
            }
            Err(holder) => {
                let changed = self.held.as_ref().is_some_and(|h| h.holder != holder);
                if changed {
                    let tab = holder.pane.as_deref().and_then(|p| self.tab_of(p));
                    if let Some(h) = &mut self.held {
                        h.holder = holder;
                        h.tab = tab;
                    }
                }
                changed
            }
        }
    }

    /// A held note's lock taken: its text read again and editable, the
    /// box asking about it closed, and `words` said.
    fn became_yours(&mut self, key: String, words: String) {
        self.note_lock = Some(key);
        // Read while still held, so a rename the app that had it made is
        // followed as that app's (`follow_id`).
        self.check();
        self.held = None;
        if matches!(
            self.layers.last(),
            Some(Layer {
                then: Then::Held,
                ..
            })
        ) {
            self.layers.pop();
        }
        self.rebuild();
        self.say(words);
    }

    /// The box an edit tried on a held note opens, saying why it is
    /// read-only; with no tab to name (the one Notes has it), the held
    /// line said instead.
    fn explain_held(&mut self) {
        let Some(held) = &self.held else { return };
        // Going to the holder's tab and taking the note over from it are
        // asked of the workspace's Notes through the workspace: alone, or
        // outside a pane, the line says who has it and no box offers them.
        if held.holder.pane.is_none() || !self.opts.server {
            if let Some(line) = self.held_line() {
                self.say(line);
            }
            return;
        }
        let tab = match held.tab {
            Some(n) => format!("tab {n}"),
            None => "another tab".to_string(),
        };
        let title = self
            .current_path()
            .map(sidebar::title_at)
            .unwrap_or_else(|| "This note".into());
        let options = vec![
            MenuOption::new(format!("Go to {tab}")),
            MenuOption::new("Take it over"),
            MenuOption::new("Keep reading"),
        ];
        self.push(
            Box::new(
                Menu::new("Read-only here", options)
                    .body(format!("{title} is open in {tab}, so it's read-only here")),
            ),
            Then::Held,
        );
    }

    /// "Take it over": the app that has the held note asked over its
    /// socket to save it and let it go, and its lock taken here.
    fn take_over(&mut self) {
        let Some(held) = self.held.clone() else {
            return;
        };
        let Some(pane) = held.holder.pane.clone() else {
            return;
        };
        let from = match held.tab {
            Some(n) => format!("tab {n}"),
            None => "its tab".to_string(),
        };
        let mine = crate::host::pane().unwrap_or_else(|| "-".into());
        let request = format!("give {} {mine} {}", std::process::id(), held.key);
        if let Err(why) = socket::send(&socket::path_for_pane(&pane), &request) {
            return self.say(format!(
                "The Notes in {from} didn't let it go ({why}), so it stays read-only"
            ));
        }
        match self.store.lock_note(&held.key) {
            Ok(()) => self.became_yours(held.key, format!("Took it over from {from}")),
            Err(_) => self.say(format!("Couldn't take it over from {from}")),
        }
    }

    /// Asked by another app to let the note open here go (`give`): saved,
    /// its lock let go, and shown read-only, naming that app's tab.
    fn give(&mut self, key: String, to: store::Holder) -> String {
        if self.held.is_some() || self.note_lock.as_deref() != Some(key.as_str()) {
            return "error that note isn't open here".into();
        }
        if self.squash_locks.contains(&key) {
            return "error it's being squashed".into();
        }
        if matches!(self.editor.mode, Mode::Insert | Mode::Replace) {
            self.editor.mode = Mode::Normal;
        }
        self.save();
        // A save that failed (a full disk) keeps the note here, so its
        // edits aren't left behind.
        if self.modified() {
            return "error its changes couldn't be saved".into();
        }
        self.note_lock = None;
        self.store.unlock_note(&key);
        let tab = to.pane.as_deref().and_then(|p| self.tab_of(p));
        self.held = Some(Held {
            key,
            holder: to,
            tab,
            checked: Instant::now(),
        });
        self.rebuild();
        match tab {
            Some(n) => self.say(format!("Tab {n} took this note over")),
            None => self.say("Another Notes took this note over"),
        }
        "ok".into()
    }

    /// Enter on a held note: to the tab that has it.
    fn go_to_holder(&mut self) {
        let Some(pane) = self.held.as_ref().and_then(|h| h.holder.pane.clone()) else {
            return;
        };
        if self.opts.server {
            let _ = crate::host::socket::ask(&["select-pane", "-t", &pane]);
        }
    }

    /// Runs `act` on a held note, then takes back any change it made to
    /// the note's text, the editor left out of Insert mode: the note is
    /// another app's to edit.
    fn read_only(&mut self, act: impl FnOnce(&mut Self)) {
        if self.held.is_none() {
            return act(self);
        }
        let before = self.editor.doc.text.clone();
        let history = self.editor.history();
        let note = self.current_path().map(Path::to_path_buf);
        act(self);
        if self.held.is_none() {
            return;
        }
        // Another note opened meanwhile has its own text, not this one's.
        let same_note = self.current_path() == note.as_deref();
        let changed = same_note && self.editor.doc.text != before;
        if changed {
            self.editor.doc.text = before;
            // Its undo and redo too, so nothing done here is lost by it.
            self.editor.set_history(history);
            self.editor.clamp();
        }
        let typing = matches!(self.editor.mode, Mode::Insert | Mode::Replace);
        if typing {
            self.editor.mode = Mode::Normal;
        }
        let tried_an_edit = same_note && (changed || typing || self.save_due.is_some());
        self.save_due = None;
        // Once a key: none while a box is open (this one, or one the key
        // opened); a menu that made the edit has closed.
        if tried_an_edit && self.layers.is_empty() {
            self.explain_held();
        }
    }

    /// A note's ID as it opens: the one its file holds, else a new one,
    /// written at once with the file's times kept, as is a copy of another
    /// note's file holding the same, said once. A note in Recently Deleted
    /// keeps what it has. Gives the file's text as it is now, and the ID.
    fn give_id(&mut self, path: &Path, text: String) -> (String, Option<String>) {
        let held = id::in_text(&text);
        if self.store.in_trash(path) {
            return (text, held);
        }
        let files = id::note_files(self.store.root());
        if self.ids_checked.0 != files {
            self.ids_checked = (files, HashMap::new());
        }
        let stamp = file_stamp(path);
        let checked_since_written =
            stamp.is_some() && self.ids_checked.1.get(path) == stamp.as_ref();
        if held.is_some() && checked_since_written {
            return (text, held);
        }
        let copy = held
            .as_deref()
            .and_then(|held| id::copy_of(path, held, self.store.root()));
        if held.is_some() && copy.is_none() {
            if let Some(stamp) = stamp {
                self.ids_checked.1.insert(path.to_path_buf(), stamp);
            }
            return (text, held);
        }
        let fresh = id::new();
        let given = id::with(&text, &fresh);
        let written = std::fs::metadata(path).and_then(|was| {
            util::write_atomic(path, &given)?;
            id::keep_times(path, &was)
        });
        if written.is_err() {
            // Written with the next save instead.
            return (text, Some(fresh));
        }
        if let Some(original) = copy {
            let name = |p: &Path| {
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            };
            self.say(format!(
                "notes: {} is a copy of {}, so it has an ID of its own now",
                name(path),
                name(&original)
            ));
        }
        if let Some(stamp) = file_stamp(path) {
            self.ids_checked.1.insert(path.to_path_buf(), stamp);
        }
        (given, Some(fresh))
    }

    /// Brings in a change made to the open note's file from outside, if
    /// there is one: in place when nothing is unsaved, merged line by line
    /// with unsaved typing when something is. An item ticked there moves
    /// under the ones left to do, as ticking it here does. False when the
    /// note's file has gone and no note holds its ID (deleted, or moved to
    /// Recently Deleted, outside the app), so it isn't written back; said
    /// once. A note renamed or moved outside the app is followed by its ID
    /// first (`follow_id`), and what changed brought in from its new file.
    pub fn check(&mut self) -> bool {
        let Some(open) = &self.open else {
            return false;
        };
        let mut path = open.path.clone();
        if !path.is_file()
            && let Some(to) = self.follow_id(&path)
        {
            path = to;
        }
        let Some(open) = &self.open else {
            return false;
        };
        let Some(disk) = util::read_text(&path) else {
            if !open.gone {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.say(format!(
                    "notes: {name} was deleted outside Notes, so it isn't saved any more"
                ));
                if let Some(o) = &mut self.open {
                    o.gone = true;
                }
            }
            return false;
        };
        // The item M-Enter handed on is in its note, a checklist line naming
        // it that wasn't there before: nothing to give back.
        if self.planning.iter().any(|p| self.handed_from_open(p)) {
            let lines: Vec<String> = disk.lines().map(str::to_string).collect();
            let planning = std::mem::take(&mut self.planning);
            self.planning = planning
                .into_iter()
                .filter(|p| !self.handed_from_open(p) || items_naming(&lines, &p.text) <= p.items)
                .collect();
        }
        let modified = self.modified();
        let Some(o) = &mut self.open else {
            return false;
        };
        o.gone = false;
        // Read on every look, as another editor may have saved it into a
        // new file with nothing changed.
        o.file = file_key(&path);
        let Some(base) = o.base.clone() else {
            return true;
        };
        if disk == base {
            return true;
        }
        let before = self.editor.doc.text.lines();
        // The ID line kept apart, as the editor holds the note without it:
        // the file's own taken when it has one, so one given outside the
        // app (a copy given its own) is the note's from now on.
        let disk_body = id::body_of(&disk);
        let disk_id = id::in_text(&disk);
        if let Some(given) = &disk_id {
            o.id = Some(given.clone());
        }
        let key = o.id.clone();
        // The note's lock follows its ID (notes_per_tab): an ID given
        // outside is locked as the note's own, and one an outside rewrite
        // dropped is written back at once, so another app opening the note
        // finds it held by this one rather than giving it another.
        // Shown read-only, the lock waited for is the one the ID now names.
        if let (Some(held), Some(given)) = (&mut self.held, &disk_id) {
            held.key = given.clone();
        }
        if self.opts.per_note
            && self.held.is_none()
            && let Some(key) = key
        {
            if disk_id.is_none() {
                self.save_due = Some(Instant::now());
            } else if self.note_lock.as_ref() != Some(&key) {
                self.take_note_lock(key);
            }
        }
        let new = if modified {
            let (merged, clashes) = sync::merge(
                &id::body_of(&base),
                &self.editor.doc.text.file_text(),
                &disk_body,
            );
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
            disk_body.clone()
        };
        // A title the outside change or the merge lost is put back, and
        // written with the next save, unless you are typing it again: in
        // Insert mode on the first line with the title gone from the editor.
        let base_body = id::body_of(&base);
        let typing_title = self.editor.is_typing()
            && self.editor.doc.row == 0
            && sync::keep_title(&base_body, &self.editor.doc.text.file_text()).is_some();
        let kept = sync::keep_title(&base_body, &new).filter(|_| !typing_title);
        let new = match kept {
            Some(kept) => {
                self.save_due = Some(Instant::now());
                kept
            }
            None => new,
        };
        let new_text = Text::from_file(&new);
        // Followed on its own, as a key's change is, so a later change
        // from outside can't make a gone line look changed.
        self.follow_lines();
        self.editor.change_outside(|doc| doc.text = new_text);
        self.follow_lines();
        self.editor.clamp();
        if let Some(o) = &mut self.open {
            o.base = Some(disk.clone());
            o.saved = Text::from_file(&disk_body);
        }
        // A change from outside names the note on its pane by the title it
        // now has, whatever it was named there last; the open note's own
        // title stays the one it was saved under, which a save compares
        // with to rename its file.
        self.tell_title(Some(store::title_of(&new)));
        let after = self.editor.doc.text.lines();
        self.editor.folds.follow(&before, &after);
        let mut rows = sync::changed_rows(&before, &after);
        if self.opts.move_checked_to_bottom && !rows.is_empty() {
            rows.sort_unstable_by(|a, b| b.cmp(a));
            let changed = self.editor.change_outside(|doc| {
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

    /// Whether an item M-Enter handed on is for the open note: by its ID,
    /// else by its path.
    fn handed_from_open(&self, p: &Planning) -> bool {
        let Some(open) = &self.open else {
            return false;
        };
        match &p.note_id {
            Some(note_id) => open.id.as_ref() == Some(note_id),
            None => p.note == open.path,
        }
    }

    /// The open note's file gone from `from`: the note holding its ID, where
    /// a rename or a move outside the app put it, taken as the open note,
    /// its cursor, history and marks with it, and its pin, the last note
    /// open and its tabs following (`Store::moved_outside`). Gives its path;
    /// `None` when no note outside Recently Deleted holds the ID in the file
    /// the note was last read from or written to, moved: a copy of its file,
    /// which holds the ID until it is opened, is never taken for it, so
    /// typing isn't saved into the copy of a note deleted outside. Reads
    /// only the notes' last bytes, and only while the open note's file is
    /// gone.
    fn follow_id(&mut self, from: &Path) -> Option<PathBuf> {
        let open = self.open.as_ref()?;
        let note_id = open.id.clone()?;
        let file = open.file?;
        let holding = id::holding(&note_id, self.store.root());
        // Held by the app that has it open (notes_per_tab), the one file
        // holding its ID is the note that app saved and renamed, written
        // whole into a new file, never a copy.
        let owner_moved_it = self.held.is_some() && holding.len() == 1;
        let to = holding
            .into_iter()
            .find(|p| owner_moved_it || file_key(p) == Some(file))?;
        if to == from {
            return None;
        }
        let (from_rel, to_rel) = (self.store.relative(from), self.store.relative(&to));
        let was = self.open.as_ref()?.title.clone();
        let now = store::title_of(&util::read_text(&to).unwrap_or_default());
        let retitled = (now != was).then(|| Retitled {
            path: to_rel.clone(),
            from: was,
            to: now.clone(),
        });
        self.moved(from, &to);
        if let Some(o) = &mut self.open {
            o.title = now;
        }
        self.tell_open();
        // The app that moved it has carried its tabs' links along.
        if !owner_moved_it {
            self.store
                .moved_outside(&from_rel, &to_rel, retitled.as_ref());
        }
        // The images pasted since the last save, named only in the editor.
        if let (Some(old), Some(new)) = (from.parent(), to.parent())
            && old != new
        {
            store::copy_attachments(&self.editor.doc.text.file_text(), &[old], new);
        }
        self.rebuild();
        self.reveal(&to);
        Some(to)
    }

    /// Saves the open note, and renames its file if its title changed. A
    /// change made to its file from outside is brought in first, so it
    /// isn't written over.
    pub fn save(&mut self) {
        self.save_with(true);
    }

    /// Saves as `save` does; with `keep_title` off, a first line that no
    /// longer reads as the title is written as it is, for a rename asked for.
    fn save_with(&mut self, keep_title: bool) {
        self.save_due = None;
        if self.held.is_some() {
            return;
        }
        let on_disk = |app: &Self| app.open.as_ref().map(|o| (o.path.clone(), o.base.clone()));
        let before = on_disk(self);
        // Checked again after `check`, which may find the note now holds
        // an ID another app has open.
        if self.open.is_none() || !self.check() || self.held.is_some() {
            return;
        }
        let Some(open) = &self.open else { return };
        let path = open.path.clone();
        // Another note's ID pasted in, or formatting wrapped round one,
        // taken out: the note keeps its own, written last below.
        let lines = self.editor.doc.text.lines();
        if id::any_in(&lines) {
            let kept = Text::from_lines(&id::without_ids(&lines));
            self.editor.change_outside(|doc| doc.text = kept);
            self.editor.clamp();
        }
        // A title lost in the editor is always written to the file, so the
        // note keeps it and its name. Typing a new title over the old one
        // autosaves as it goes, the line empty for a moment: the editor gets
        // it back only once that's done.
        let typing_title = self.editor.is_typing() && self.editor.doc.row == 0;
        if keep_title && !typing_title {
            self.put_title_back();
        }
        let Some(open) = &self.open else { return };
        let kept = open
            .base
            .as_deref()
            .filter(|_| keep_title)
            .and_then(|base| {
                sync::keep_title(&id::body_of(base), &self.editor.doc.text.file_text())
            });
        // What is written, which is what counts as saved: a title being typed
        // over stays unsaved in the editor, so a change from outside merges
        // around it.
        let written = match &kept {
            Some(kept) => Text::from_file(kept),
            None => self.editor.doc.text.clone(),
        };
        let text = match &open.id {
            Some(note_id) => id::file_of(&written.lines(), note_id),
            None => written.file_text(),
        };
        // Written when typed in, or when the file lost its ID.
        if open.base.as_deref() != Some(text.as_str()) {
            if util::write_atomic(&path, &text).is_err() {
                self.say(format!(
                    "notes: couldn't save {}",
                    self.store.relative(&path)
                ));
                return;
            }
            if let Some(o) = &mut self.open {
                o.base = Some(text);
                o.file = file_key(&path);
                o.saved = written;
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
        // Read from the file as written, which keeps a title the editor
        // has emptied for a moment.
        let now = match self.open.as_ref().and_then(|o| o.base.as_deref()) {
            Some(written) => store::title_of(written),
            None => store::title_of(&self.editor.doc.text.file_text()),
        };
        let was = self
            .open
            .as_ref()
            .map(|o| o.title.clone())
            .unwrap_or_default();
        if let Some(o) = &mut self.open {
            o.title = now.clone();
        }
        // Said whatever it was, since a change from outside may have named
        // another title on the pane than the one it was saved under.
        self.tell_open();
        if now != was {
            let retitled = Retitled {
                path: self.store.relative(&renamed),
                from: was,
                to: now,
            };
            self.store.relink(&|_| None, Some(&retitled));
        }
        // The list read again only when the note's file changed, here or
        // outside, or moved: opening another note saves the one left, and
        // reading every note for a list that is the same made it pause.
        if on_disk(self) != before {
            self.rebuild();
        }
    }

    /// A title lost in the editor (its line emptied or deleted) put back from
    /// the file as last read, so the note is never written without it, nor
    /// renamed after its first item.
    fn put_title_back(&mut self) {
        let Some(base) = self.open.as_ref().and_then(|o| o.base.as_deref()) else {
            return;
        };
        let Some(kept) = sync::keep_title(&id::body_of(base), &self.editor.doc.text.file_text())
        else {
            return;
        };
        // Not a step of its own to undo: undoing goes back past the edit
        // that lost it, to the note as it was.
        let was = self.editor.doc.text.lines();
        self.editor.doc.text = Text::from_file(&kept);
        // A title line put in above the cursor moves it down with its line.
        let now = self.editor.doc.text.lines();
        let put_in_at = was.iter().zip(&now).take_while(|(a, b)| a == b).count();
        if now.len() > was.len() && self.editor.doc.row >= put_in_at {
            self.editor.doc.row += now.len() - was.len();
        }
        self.editor.folds.follow(&was, &now);
        self.editor.clamp();
    }

    /// A note moved or renamed: the open note follows, and so do the marks
    /// and the items M-Enter handed on from it.
    fn moved(&mut self, from: &Path, to: &Path) {
        for p in &mut self.list.marked {
            if p == from {
                *p = to.to_path_buf();
            }
        }
        for p in &mut self.planning {
            if p.note == from {
                p.note = to.to_path_buf();
            }
        }
        if let Some(o) = &mut self.open
            && o.path == from
        {
            o.path = to.to_path_buf();
            self.publish_shown();
        }
        if let Some(c) = self.cursors.remove(from) {
            self.cursors.insert(to.to_path_buf(), c);
        }
        if let Some(k) = self.kept.remove(from) {
            self.kept.insert(to.to_path_buf(), k);
        }
        self.jumps
            .rekey(&|p: &Path| (p == from).then(|| to.to_path_buf()));
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
            self.publish_shown();
        }
        rekey(&mut self.cursors, &follow);
        rekey(&mut self.kept, &follow);
        self.jumps.rekey(&follow);
        for p in &mut self.planning {
            if let Some(n) = follow(&p.note) {
                p.note = n;
            }
        }
    }

    /// A note going (deleted, squashed): saved, and the note window shows
    /// the next one in the list, or none.
    fn forget(&mut self, path: &Path) {
        // Its history goes with it, so a note made later by its name starts
        // with none.
        self.kept.remove(path);
        self.jumps.forget(path);
        if self.current_path() != Some(path) {
            return;
        }
        self.save();
        // The next note listed, else, the folder's last one gone, the note
        // opened before it, else any; never one outside Recently Deleted
        // while it is shown, where the next dd would trash a live note.
        let listed = sidebar::next_note(&self.list.notes(&self.store), path);
        let trash_shown = self.list.folder == store::TRASH
            && self.list.tag.is_none()
            && self.list.query.is_none();
        let next = if trash_shown {
            listed
        } else {
            listed
                .or_else(|| sidebar::next_note(&self.store.recent(), path))
                .or_else(|| sidebar::next_note(&self.store.notes(None), path))
        };
        self.open = None;
        self.editor.load(Text::default());
        match next {
            Some(next) => self.open_note(&next, true, false),
            None => {
                self.tell_open();
                self.publish_shown();
            }
        }
    }

    /// Gives a note a new title, as typing it on its first line would: the
    /// line the title is read from is rewritten, keeping its # marks, and
    /// the save renames the file and carries its tabs' links along. Gives
    /// its new path.
    pub fn retitle(&mut self, path: &Path, title: &str) -> PathBuf {
        let rewrite = |lines: &mut Vec<String>| {
            // Never its hidden ID line, which a note not open still holds.
            let at = lines.iter().position(|l| {
                !id::is_line(l) && !l.trim().trim_start_matches('#').trim().is_empty()
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
            self.save_with(false);
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
        self.list_read = self.list.read(&self.store);
        self.lay_out_list();
        self.titles = self
            .store
            .titles()
            .into_iter()
            .map(|t| t.to_lowercase())
            .collect();
        self.list_at = self.list_at.min(self.list_rows.len().saturating_sub(1));
    }

    /// The list's rows laid out again for its width from what was last
    /// read, reading no note: what a change of width alone needs.
    fn lay_out_list(&mut self) {
        let width = self.list_cols(self.cols);
        self.list_built_for = width;
        let current = self.open.as_ref().map(|o| o.path.clone());
        self.list_rows = self
            .list
            .lay_out(&self.list_read, current.as_deref(), width);
        self.list_at = self.list_at.min(self.list_rows.len().saturating_sub(1));
    }

    /// The list's cursor put on a note's row, so the note open is the one
    /// the cursor marks; a note the view doesn't list brings its folder.
    /// Its row under Recent doesn't count.
    pub fn reveal(&mut self, path: &Path) {
        let row = |rows: &[Row]| rows.iter().position(|r| r.listed() == Some(path));
        if row(&self.list_rows).is_none() {
            let rel = self.store.relative(path);
            let folder = rel.rsplit_once('/').map_or("", |(dir, _)| dir).to_string();
            self.list.show(Some(folder), None, None);
            self.rebuild();
        }
        if let Some(at) = row(&self.list_rows) {
            self.list_at = at;
        }
    }

    /// Shows a folder, a tag or a search, the cursor on its first note (not
    /// a row under Recent), or on its heading when it has none; put away,
    /// the list comes back to show it.
    pub fn show(&mut self, folder: Option<String>, tag: Option<String>, query: Option<String>) {
        self.list.show(folder, tag, query);
        // The list shown puts the plan beside the note away, as the list's
        // other ways in do.
        if self.beside.is_some() {
            self.close_beside();
        }
        self.list_shown = true;
        self.rebuild();
        let first = self.list_rows.iter().position(|r| r.listed().is_some());
        // With nothing shown, the cursor on the heading over where its notes
        // would be, so no row of another view stays chosen.
        let heading = || {
            self.list_rows
                .iter()
                .position(|r| r.look == sidebar::Look::Rule)
                .map(|rule| rule + 1)
        };
        if let Some(at) = first.or_else(heading) {
            self.list_at = at.min(self.list_rows.len().saturating_sub(1));
        }
    }

    /// The list shown or put away; shown again, its cursor on the open
    /// note when the view lists it.
    pub fn show_list(&mut self, show: bool) {
        // The list asked for puts away the plan beside the note, which it
        // shares the room with.
        if show && self.beside.is_some() {
            self.close_beside();
            self.list_shown = true;
            return;
        }
        if show == self.list_shown {
            return;
        }
        self.list_shown = show;
        if show {
            if let Some(path) = self.current_path().map(Path::to_path_buf)
                && let Some(at) = self
                    .list_rows
                    .iter()
                    .position(|r| r.listed() == Some(path.as_path()))
            {
                self.list_at = at;
            }
        } else {
            self.focus = Focus::Note;
        }
    }

    pub fn toggle_list(&mut self) {
        let shown = self.list_shown && self.beside.is_none();
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
            Some(Item::Folder(folder)) => self.show(Some(folder), None, None),
            Some(Item::Recent) => self.recent_finder(),
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
        if self.held_elsewhere(&path) {
            return;
        }
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
        if !self.lock_for_squash(&paths) {
            return;
        }
        let input = match squash::input(&self.store, &paths) {
            Ok(i) => i,
            Err(why) => {
                self.unlock_squashed();
                return self.say(format!("Notes weren't squashed: {why}"));
            }
        };
        let child = match squash::command(&program, squash::MODEL).spawn() {
            Ok(c) => c,
            Err(_) => {
                self.unlock_squashed();
                return self.say("Notes weren't squashed: Claude Code isn't installed");
            }
        };
        crate::host::requests::count("Anthropic", "claude -p", "Notes: squash");
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
            self.unlock_squashed();
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
        self.unlock_squashed();
    }

    pub fn restore(&mut self) {
        let Some(path) = self.list_note() else { return };
        if !self.store.in_trash(&path) || self.held_elsewhere(&path) {
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
            Some(Item::Folder(f)) if !f.is_empty() && f != store::TRASH => {
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
        let text = format!("Notes\n\n{}", sidebar::help_text());
        self.push(Box::new(Message::new(&text, 3600)), Then::Nothing);
    }

    fn finder(&mut self) {
        let notes = self.store.notes(None);
        self.push(Box::new(Finder::new(notes, "Find a note")), Then::Finder);
    }

    /// Every note opened lately, newest first, in the finder's box.
    pub fn recent_finder(&mut self) {
        let notes = self.store.recent();
        if notes.is_empty() {
            self.say("notes: no note opened lately");
            return;
        }
        self.push(Box::new(Finder::new(notes, "Recent notes")), Then::Finder);
    }

    /// A Switchboard command for Notes, asked of the workspace's server.
    /// Whether the server took it.
    fn press(&mut self, words: &[&str]) -> bool {
        if !self.opts.server {
            self.say("notes: that needs Switchboard's workspace");
            return false;
        }
        let mut args = vec!["press"];
        args.extend(words);
        match crate::host::socket::ask_ok(&args) {
            Ok(_) => true,
            Err(e) => {
                self.say(format!("notes: {e}"));
                false
            }
        }
    }

    fn note_tab(&mut self) {
        self.press_for_note("note-tab");
    }

    /// "Attach this note to a tab": the workspace lists its tabs, and asks
    /// before the note takes the place of a tab's.
    fn attach_note(&mut self) {
        self.press_for_note("attach-note");
    }

    /// Asks the workspace for `action` on the note under the list's cursor,
    /// or the one open, from a note.
    fn press_for_note(&mut self, action: &str) {
        let path = match self.focus {
            Focus::List => self.list_note(),
            Focus::Note => self.current_path().map(Path::to_path_buf),
        };
        let Some(path) = path.filter(|p| !self.store.in_trash(p)) else {
            return;
        };
        let rel = self.store.relative(&path);
        self.press(&[action, &rel]);
    }

    /// `gw`: work on the checklist item on the cursor's line in a new tab,
    /// asked of the workspace (`press work-on-item <note> <line>`), which
    /// asks the repository and the branch to land on; on any other line,
    /// why not. The note is saved first, so the line the server reads is
    /// the one on screen.
    fn work_on_item(&mut self) {
        if let Some(words) = self.item_press() {
            let words: Vec<&str> = words.iter().map(String::as_str).collect();
            self.press(&words);
        }
    }

    /// What `gw` presses for the cursor's line, the note saved; `None`,
    /// having said why, when it is no unticked top-level item.
    fn item_press(&mut self) -> Option<Vec<String>> {
        let path = self.current_path()?.to_path_buf();
        if self.store.in_trash(&path) {
            self.say("notes: restore the note to start work on its items");
            return None;
        }
        self.leave_modes();
        // Saving can rename the note after its title, and put a lost title
        // back above the cursor, so its path and the item's line are read
        // after.
        self.save();
        let path = self.current_path()?.to_path_buf();
        if self.modified() {
            return None;
        }
        let line = self.editor.doc.row + 1;
        let text = self.editor.doc.text.lines().join("\n");
        if let Some(why) = crate::host::host().item_refused(&text, line) {
            self.say(format!("notes: that line {why}"));
            return None;
        }
        let rel = self.store.relative(&path);
        Some(vec![crate::WORK_ON_ITEM.to_string(), rel, line.to_string()])
    }

    // ── UX 2.0's item keys ────────────────────────────────────────────────

    /// A parked section (`## Defered`, `## Later`) folded, as a note opens,
    /// to one line with its count of items.
    fn fold_parked(&mut self) {
        let lines = self.editor.doc.text.lines();
        let kinds = render::kinds(&lines);
        for (row, line) in lines.iter().enumerate() {
            // One inside a parked section folded already stays in it.
            if render::parked(line, kinds[row])
                && self.editor.folds.closed_at(&kinds, row).is_none()
            {
                self.editor.folds.toggle(&kinds, row);
            }
        }
    }

    /// An unticked item's plan beside the note (its chip, the palette),
    /// kept where it shows already. Whether it had one.
    fn open_plan(&mut self) -> bool {
        self.beside_is_of_cursor() || self.open_beside()
    }

    /// Whether a click on an item's chip does something: its tab's goes to
    /// the tab, its plan's opens the plan.
    fn clickable(chip: &render::Chip) -> bool {
        matches!(
            chip,
            render::Chip::Working(..) | render::Chip::Waits(..) | render::Chip::Plan
        )
    }

    /// A click on the chip of the item on `row`: the cursor on the item,
    /// then its tab, as `o` goes there, or its plan, as Enter opens it.
    fn click_chip(&mut self, row: usize, chip: &render::Chip) {
        if self.focus == Focus::List {
            self.focus = Focus::Note;
        }
        self.finish_editing();
        self.editor.doc.row = row;
        self.editor.doc.col = 0;
        self.editor.clamp();
        match chip {
            render::Chip::Plan => {
                self.open_plan();
            }
            _ => {
                self.go_to_item_tab();
            }
        }
    }

    /// `o` on an unticked item a tab works on or waits in: that tab, in the
    /// workspace, unless the tab is linked to another note. Whether there
    /// was one.
    fn go_to_item_tab(&mut self) -> bool {
        let row = self.editor.doc.row;
        let line = self.editor.doc.line(row);
        let kinds = render::kinds(&self.editor.doc.text.lines());
        let kind = kinds[row];
        if !matches!(kind, Kind::Check { done: false, .. })
            || self.editor.folds.closed_at(&kinds, row).is_some()
        {
            return false;
        }
        let item = render::item_text(&line, kind);
        let Some(id) = self
            .note_tabs()
            .get(item)
            .map(|t| t.id.clone())
            .filter(|id| !id.is_empty())
        else {
            return false;
        };
        if !self.opts.server {
            self.say("notes: that needs Switchboard's workspace");
        } else if let Err(e) = crate::host::socket::ask_ok(&["switch-client", "-t", &id]) {
            self.say(format!("notes: {e}"));
        }
        true
    }

    /// Out of Insert or Visual mode with what was typed one step to undo,
    /// and the change for `.` to make again, for a key or click that
    /// leaves the note's editing.
    fn finish_editing(&mut self) {
        self.leave_modes();
        if let Some(before) = self.editor.end() {
            self.editor
                .folds
                .follow(&before.lines(), &self.editor.doc.text.lines());
            self.changed();
        }
        self.finish_recording();
    }

    /// `ga`, or a click on it: the "Add an item" input, typed into.
    fn start_adding(&mut self) {
        if self.open.is_none() {
            return;
        }
        // Another app has the note open: nothing is added to it here, nor
        // handed to a tab that would write it (M-Enter).
        if let Some(line) = self.held_line() {
            return self.say(line);
        }
        self.finish_editing();
        // A click on the input while it is typed into keeps what is typed;
        // an item M-Enter handed on that never reached the note comes back.
        if self.adding.is_none() {
            // One whose item reached the note while another was open is done.
            let lines = self.editor.doc.text.lines();
            let planning = std::mem::take(&mut self.planning);
            self.planning = planning
                .into_iter()
                .filter(|p| !self.handed_from_open(p) || items_naming(&lines, &p.text) <= p.items)
                .collect();
            let given_back = self
                .planning
                .iter()
                .rposition(|p| self.handed_from_open(p))
                .map(|i| self.planning.remove(i));
            self.adding = Some(given_back.map(|p| p.text).unwrap_or_default());
        }
    }

    /// A key while "Add an item" is typed into: Enter adds it to the top
    /// list, M-Enter has a new tab's Claude add it and write its plan
    /// (`/shape --notes`, which puts the item in the note itself), Esc leaves
    /// it.
    fn adding_key(&mut self, key: Key) {
        let Some(text) = self.adding.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.adding = None,
            KeyCode::Enter if key.mods == Mods::ALT => self.add_item(true),
            KeyCode::Enter if key.mods.is_empty() => self.add_item(false),
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Char(c) if !key.mods.ctrl && !key.mods.alt => text.push(c),
            _ => {}
        }
    }

    fn add_item(&mut self, with_plan: bool) {
        let text = self.adding.take().unwrap_or_default().trim().to_string();
        if text.is_empty() {
            return;
        }
        if let Some(line) = self.held_line() {
            return self.say(line);
        }
        if with_plan {
            // Saving can rename the note after its title, so its path is
            // read after.
            self.save();
            let Some(path) = self.current_path().map(Path::to_path_buf) else {
                return;
            };
            let rel = self.store.relative(&path);
            // Kept to try again when the workspace didn't take it.
            if self.press(&[crate::SHAPE_ITEM, &rel, &text]) {
                let lines = self.editor.doc.text.lines();
                self.planning.push(Planning {
                    items: items_naming(&lines, &text),
                    note: path,
                    note_id: self.open.as_ref().and_then(|o| o.id.clone()),
                    text,
                });
            } else {
                self.adding = Some(text);
            }
            return;
        }
        let before = self.editor.doc.text.lines();
        let kinds = render::kinds(&before);
        let row = render::new_item_row(&before, &kinds);
        let line = format!("- [ ] {text}");
        let added = self.editor.change(|doc| {
            doc.text.set_lines(row, row, &[line]);
            doc.row = row;
            doc.col = 0;
        });
        if added {
            self.editor
                .folds
                .follow(&before, &self.editor.doc.text.lines());
            self.editor.clamp();
            self.changed();
        }
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
            Then::Held => match outcome {
                Outcome::Chose(0) => self.go_to_holder(),
                Outcome::Chose(1) => self.take_over(),
                _ => {}
            },
            Then::Squash(paths) => {
                if outcome == Outcome::Yes && !paths.iter().any(|p| self.held_elsewhere(p)) {
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
                if outcome == Outcome::Yes && !self.held_elsewhere(&path) {
                    self.delete_now(&path);
                }
            }
            Then::MoveTo(path, rels) => {
                if let Outcome::Chose(i) = outcome
                    && let Some(rel) = rels.get(i)
                    && !self.held_elsewhere(&path)
                {
                    if self.current_path() == Some(path.as_path()) {
                        self.save();
                    }
                    let to = self.store.move_note(&path, rel);
                    self.moved(&path, &to);
                    self.rebuild();
                    // The list follows it to its folder, the cursor on it,
                    // rather than on whatever took its row.
                    self.reveal(&to);
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
                    let tag = q
                        .strip_prefix('#')
                        .filter(|t| !t.is_empty() && !t.contains(char::is_whitespace));
                    if q.is_empty() {
                        self.show(None, None, None);
                    } else if let Some(tag) = tag {
                        self.show(None, Some(tag.to_string()), None);
                    } else {
                        self.show(None, None, Some(q));
                    }
                }
            }
            Then::RenameNote(path) => {
                if let Some(name) = typed(&outcome) {
                    let name = name.trim();
                    if !name.is_empty()
                        && name != sidebar::title_at(&path)
                        && !self.held_elsewhere(&path)
                    {
                        let to = self.retitle(&path, name);
                        self.rebuild();
                        self.reveal(&to);
                    }
                }
            }
            Then::RenameFolder(from) => {
                if let Some(name) = typed(&outcome).filter(|n| !n.trim().is_empty())
                    && !self.folder_held_elsewhere(&from)
                {
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
                    self.editor.paste_text(&format!("{}]]", p.value));
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

    /// Where the cursor is typing, in Insert or Replace mode (`R`'s count
    /// typed out first, as leaving it would).
    fn typing_col(&mut self) -> Option<usize> {
        let before = self.editor.doc.text.clone();
        self.editor.settle_replace_count();
        if self.editor.doc.text != before {
            self.changed();
        }
        self.editor.is_typing().then_some(self.editor.doc.col)
    }

    /// The Format menu, from where the cursor or the selection is.
    fn open_menu(&mut self) {
        let typing = self.typing_col();
        let from = self.format_from(typing);
        self.leave_modes();
        self.push(
            Box::new(FormatMenu::new(self.diagram_label() == menu::SHOW_DIAGRAMS)),
            Then::Format(from),
        );
    }

    /// Out of Insert, Replace and Visual mode, as a menu or a button takes
    /// the keys.
    fn leave_modes(&mut self) {
        match self.editor.mode {
            Mode::Insert | Mode::Replace => {
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
            FormatAct::WorkOnItem => {
                self.work_on_item();
                return;
            }
            FormatAct::DiagramCode => {
                self.switch_diagram_code();
                return;
            }
            FormatAct::AttachNote => {
                self.attach_note();
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
            FormatAct::Link
            | FormatAct::PasteImage
            | FormatAct::WorkOnItem
            | FormatAct::DiagramCode
            | FormatAct::AttachNote => {}
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

    /// Normal mode's Enter at a place in a note, the first of these that
    /// applies: a web address, an image (under the cursor or just right of
    /// it, its markup hidden) or a #tag under the cursor followed, on a
    /// folded line too; a closed fold opened; a checklist item ticked or
    /// unticked; on an item's plan link, its plan beside the
    /// note, and while it shows, work started on the item; any other note
    /// link followed; a heading's section folded. Whether it did
    /// something; when not, Enter moves down a line.
    pub fn enter_at(&mut self, row: usize, col: usize) -> bool {
        let lines = self.editor.doc.text.lines();
        let kinds = render::kinds(&lines);
        let Some(line) = lines.get(row) else {
            return false;
        };
        let span = render::span_at(line, col).or_else(|| {
            render::inline(line, 0)
                .into_iter()
                .find(|s| s.image.is_some() && s.whole().0 == col + 1)
        });
        if let Some(span) = span.clone().filter(|s| s.note.is_none())
            && self.open_span(span)
        {
            return true;
        }
        if let Some((s, _)) = self.editor.folds.closed_at(&kinds, row) {
            self.editor.folds.open(&kinds, s);
            return true;
        }
        let Some(k) = kinds.get(row).copied() else {
            return false;
        };
        if matches!(k, Kind::Check { .. }) {
            return self.toggle_item(row);
        }
        if let Some(item) = render::item_of_plan_line(&lines, &kinds, row) {
            if self.beside_is_of(item) {
                self.editor.doc.row = item;
                self.editor.doc.col = 0;
                self.editor.clamp();
                self.work_on_item();
            } else {
                self.open_beside_at(item);
            }
            return true;
        }
        if let Some(span) = span
            && self.open_span(span)
        {
            return true;
        }
        k.level().is_some() && self.editor.folds.toggle(&kinds, row)
    }

    /// What a click does at a place in a note: a closed fold opened, the ☐
    /// ticked, the ▾ folding its section, and, while the app's options
    /// follow links on a click, what is there followed. Whether it did
    /// something.
    pub fn click_at(&mut self, row: usize, col: Option<usize>) -> bool {
        let lines = self.editor.doc.text.lines();
        let kinds = render::kinds(&lines);
        if let Some((s, _)) = self.editor.folds.closed_at(&kinds, row) {
            self.editor.folds.open(&kinds, s);
            return true;
        }
        let Some(k) = kinds.get(row).copied() else {
            return false;
        };
        // The ▾ is drawn before the line, so a click on it has no column.
        let at = col.unwrap_or(0);
        if let Kind::Check { indent, marker, .. } = k
            && col.is_some()
            && at >= indent
            && at < indent + marker
        {
            return self.toggle_item(row);
        }
        if k.level().is_some() && k.marker() > 0 && at < k.marker() {
            return self.editor.folds.toggle(&kinds, row);
        }
        self.opts.follow_links_on_click && self.follow(row, at)
    }

    /// The checklist item on `row` ticked or unticked.
    fn toggle_item(&mut self, row: usize) -> bool {
        if self.editor.change(|doc| {
            format::toggle_check(doc, row);
        }) {
            self.changed();
        }
        true
    }

    /// Follows what is at a byte of a row: a web address in the browser, an
    /// image full size over the pane (else in Quick Look), a [[Note title]] to that note (made when there is
    /// none), a #tag to the notes that have it. Whether there was something.
    pub fn follow(&mut self, row: usize, col: usize) -> bool {
        let line = self.editor.doc.line(row);
        match render::span_at(&line, col) {
            Some(span) => self.open_span(span),
            None => false,
        }
    }

    /// A unit test's link or image kept in `opened`, never opened.
    #[cfg(test)]
    fn stand_in(&mut self, what: &str) -> bool {
        self.opened.push(what.to_string());
        true
    }

    #[cfg(not(test))]
    fn stand_in(&mut self, _what: &str) -> bool {
        false
    }

    /// What `follow` does with a span of a row.
    fn open_span(&mut self, span: render::Span) -> bool {
        if let Some(image) = span.image {
            let Some(note) = self.current_path().map(Path::to_path_buf) else {
                return false;
            };
            let web = image.starts_with("http://") || image.starts_with("https://");
            // A picture that isn't there is said, never passed on to what
            // Enter would do next on the line.
            if !web && images::resolve(self.store.root(), &note, &image).is_none() {
                self.say(format!("notes: no picture at {image}"));
                return true;
            }
            if self.stand_in(&image) || images::open(&self.store, &note, &image, self.opts.server) {
                return true;
            }
            if web {
                self.open_link(image);
                return true;
            }
            self.say(format!("notes: couldn't open {image}"));
            return true;
        }
        if let Some(url) = span.url {
            self.open_link(url);
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
        // Tidying of the app's own, which the change list leaves out.
        self.editor.tidy(|doc| {
            format::drop_empty(doc);
            let row = doc.row;
            table::align(doc, row);
        });
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
            Msg::ItemTabs(tabs) => self.item_tabs = tabs,
            Msg::Next(items) => self.next_items = items,
            Msg::Opened(words, shown) => self.link_opened(&words, shown),
        }
    }

    /// A request on the socket.
    pub fn request(&mut self, request: Request) -> String {
        match request {
            Request::Ping => "ok".into(),
            Request::Say(words) => {
                self.say(words);
                "ok".into()
            }
            Request::Palette => self.palette_request(),
            Request::Give { pid, pane, key } => self.give(key, store::Holder { pid, pane }),
            Request::Run(name) => {
                let mut reply = String::new();
                self.read_only(|app| reply = app.run_request(&name));
                reply
            }
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
            && now.duration_since(self.last_input) >= switchboard_ui::input::ESC_TIMEOUT
        {
            for parsed in self.parser.flush() {
                self.input(parsed.input);
            }
            changed = true;
        }
        if self.check_held() {
            changed = true;
        }
        // The list's rows are cut to its width: laid out again here once
        // its width has settled (a drag let go, a key, the pane's size held a
        // moment), never as it is drawn or dragged.
        // A width the resize keys set, kept once they stop, so a key held
        // writes its file once.
        // Never while a drag is held: its release keeps what it ends on.
        if !self.edge_drag
            && self
                .keys_moved
                .is_some_and(|at| now.duration_since(at) >= LIST_SETTLES)
        {
            self.keep_keyed();
        }
        let width = self.list_cols(self.cols);
        // With notes_resize off the list is as on main: nothing done for a
        // width alone.
        if self.list_width_seen.0 != width && self.opts.resize {
            self.list_width_seen = (width, Some(now));
            // A change of the list's width ends its d of dd, as any resize
            // of it does.
            if self.list_pending != Some('w') {
                self.list_pending = None;
            }
        }
        let settled = self
            .list_width_seen
            .1
            .is_none_or(|since| now.duration_since(since) >= LIST_SETTLES);
        let list_moved =
            // Drawn only with no plan beside the note, as draw lays it out.
            self.list_shown
                && self.beside.is_none()
                && self.locked_by.is_none()
                && self.list_built_for != width
                && self.opts.resize
                && settled;
        // Laid out again from what was read, no note read for a width.
        if list_moved && !self.edge_drag {
            self.lay_out_list();
            changed = true;
        }
        if now.duration_since(self.settings_seen) >= SETTINGS_EVERY {
            let was = (self.opts.commands, self.opts.resize);
            self.opts.commands = settings::is_on("palette_button");
            self.opts.resize = settings::is_on("notes_resize");
            if was.1 != self.opts.resize {
                // The list's rows laid out for its width as it now is.
                self.edge_drag = false;
                if self.list_pending != Some('w') {
                    self.list_pending = None;
                }
                self.lay_out_list();
            }
            changed |= was != (self.opts.commands, self.opts.resize);
            self.settings_seen = now;
        }
        if self.save_due.is_some_and(|due| now >= due) {
            self.save();
            // The note saved after an edit: the plan beside it goes once
            // its item no longer links it.
            self.check_beside();
            changed = true;
        }
        if self.watch_due.is_some_and(|due| now >= due) {
            self.watch_due = None;
            self.check();
            self.rebuild();
            self.reread_beside();
            self.check_beside();
            changed = true;
        }
        if self
            .message
            .as_ref()
            .is_some_and(|(m, at)| at.elapsed() >= MESSAGE_FOR && self.opening.as_ref() != Some(m))
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
            Input::Mouse(m) => {
                self.follow_lines();
                self.read_only(|app| app.mouse(m));
                self.follow_lines();
            }
            Input::Paste(text) => {
                self.follow_lines();
                self.read_only(|app| app.paste(&text));
                self.follow_lines();
            }
            Input::FocusIn => {
                // The notes_images, notes_relative_numbers, notes_mermaid and
                // palette_button settings read again, as changing them in
                // Settings takes the focus away.
                let all = settings::Read::of(&[
                    "notes_relative_numbers",
                    "notes_images",
                    "notes_mermaid",
                    "palette_button",
                ]);
                self.opts.relative_numbers = all.set_on("notes_relative_numbers");
                if crate::host::graphics::apps_draw() {
                    self.images.on = all.on("notes_images");
                }
                self.opts.mermaid = all.on("notes_mermaid");
                // And palette_button, whose "⌘ Commands" Notes draws.
                self.opts.commands = all.on("palette_button");
                self.images.forget_sent();
                self.check();
                self.rebuild();
            }
            Input::FocusOut => self.save(),
            _ => {}
        }
    }

    /// A key, wherever it goes (the note, a prompt, a box), with what is
    /// kept by line followed across an outside change before it and its
    /// own edit after, each on its own, so no line takes a gone one's place.
    pub fn key(&mut self, key: Key) {
        self.follow_lines();
        let to_holder = self.held.is_some()
            && self.focus == Focus::Note
            && self.layers.is_empty()
            && self.prompt.is_none()
            && key.code == KeyCode::Enter
            && key.mods.is_empty();
        if to_holder {
            self.go_to_holder();
        } else {
            self.read_only(|app| app.key_inner(key));
        }
        self.follow_lines();
    }

    fn key_inner(&mut self, key: Key) {
        // A pane resize key, which the workspace passes on to Notes: its
        // own while the list has the keys, and handed back
        // from anywhere else, a box or a prompt included.
        // With notes_resize off the workspace keeps the key, so one that
        // reaches Notes is an ordinary key of its own.
        if let Some(by) = resize_key(&key).filter(|_| self.opts.resize) {
            // What is half typed elsewhere (the note's d of dd, the plan's
            // Ctrl-W) it leaves alone, as the workspace's key did before it
            // reached Notes; a resize of the list ends the list's own.
            let elsewhere = self.locked_by.is_some()
                || !self.layers.is_empty()
                || self.prompt.is_some()
                || self.adding.is_some();
            return self.resize_by_key(by, elsewhere);
        }
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
        if self.adding.is_some() {
            return self.adding_key(key);
        }
        if self.message.is_some() && self.focus == Focus::Note && self.editor.mode == Mode::Normal {
            self.message = None;
        }
        if key.mods.alt && !key.mods.ctrl && key.code == KeyCode::Char('e') && self.open.is_some() {
            return self.toggle_beside();
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
        if self.beside.as_ref().is_some_and(|b| b.focused) {
            if self.list_pending.take() == Some('w') {
                return self.window_key(key);
            }
            return self.plan_key(key);
        }
        match self.focus {
            Focus::List if self.list_shown && self.beside.is_none() => self.list_key(key),
            _ => self.note_key(key),
        }
    }

    fn prompt_key(&mut self, key: Key) {
        let Some((kind, mut text)) = self.prompt.take() else {
            return;
        };
        match key.code {
            KeyCode::Esc => return self.editor.prompt_done(),
            KeyCode::Enter => {
                let before = self.editor.doc.text.clone();
                self.editor.begin();
                match kind {
                    ':' => self.editor.ex(&text),
                    _ => self.editor.search_for(&text, kind == '/'),
                }
                self.editor.prompt_done();
                if self.editor.end().is_some() || self.editor.doc.text != before {
                    self.changed();
                }
                self.effects();
                return;
            }
            KeyCode::Backspace => {
                if text.pop().is_none() {
                    // Backspace on an empty prompt cancels it, as Esc does.
                    return self.editor.prompt_done();
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
                Some('x') => self.show(None, None, None),
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
        if let Some(focused) = self.beside.as_ref().map(|b| b.focused) {
            // The plan is the window right of the note, the list put away.
            let narrow = self.cols < beside::NARROW;
            match (focused, dir) {
                (false, 'l' | 'w' | 'p') => {
                    self.leave_modes();
                    self.save();
                    if let Some(b) = self.beside.as_mut() {
                        b.focused = true;
                        b.show_note = false;
                    }
                }
                (true, 'h' | 'w' | 'p') => {
                    if let Some(b) = self.beside.as_mut() {
                        // The card lit kept for the keys coming back.
                        b.focused = false;
                        b.show_note = narrow;
                    }
                    self.focus = Focus::Note;
                }
                (_, 'h') => self.beside("-L"),
                (_, 'l') => self.beside("-R"),
                (_, 'j') => self.beside("-D"),
                (_, 'k') => self.beside("-U"),
                _ => {}
            }
            return;
        }
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
            let _ = crate::host::socket::ask(&["select-pane", side]);
        });
    }

    /// Ctrl-Shift-P: in the workspace, its palette (M-p), which lists this
    /// one's commands while Notes is in front; alone, the app's own.
    fn palette(&mut self) {
        if self.opts.server {
            self.press(&["palette"]);
            return;
        }
        if !self.opts.palette {
            self.say("notes: the palette is off (Settings, Notes command palette)");
            return;
        }
        let (commands, from) = self.palette_here();
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
        self.push(Box::new(picker), Then::Palette(commands, from));
    }

    /// The palette's commands, the Diagram item named as the Format menu
    /// names it for the note.
    fn palette_commands(&self, from_list: bool) -> Vec<palette::Command> {
        let mut commands = palette::commands(from_list, !from_list && self.open.is_some());
        if self.beside.is_some() {
            commands.push(palette::note_or_plan());
        }
        // Offered while a width is kept here, or in the state folder by another
        // tab's Notes since this one started, read as the palette opens.
        // With a plan showing it is the plan's width; a plan hidden behind
        // its note leaves it the list's.
        let kept = if self.plan_cols() > 0 {
            self.plan_width.is_some() || self.kept_plan_width().is_some()
        } else {
            self.list_width.is_some() || self.kept_width().is_some()
        };
        if self.opts.resize && kept {
            commands.push(palette::default_width());
        }
        // With no workspace to ask, what only it does is left out.
        if !self.opts.server {
            commands.retain(|c| !c.act.needs_workspace());
        }
        // Read as the palette opens, since Settings changes it while the
        // app runs on.
        if !settings::is_on("notes_preview") {
            commands.retain(|c| c.act != PaletteAct::Preview);
        }
        for c in &mut commands {
            if c.act == PaletteAct::Format(FormatAct::DiagramCode) {
                c.name = format!("Diagram: {}", self.diagram_label());
            }
        }
        commands
    }

    /// The palette's commands where the app is, and where they were asked
    /// from, out of Insert and Visual mode as a command needs it.
    fn palette_here(&mut self) -> (Vec<palette::Command>, PaletteFrom) {
        let from_list = self.focus == Focus::List && self.list_shown;
        let typing = self.typing_col();
        let format = self.format_from(typing);
        self.leave_modes();
        let commands = self.palette_commands(from_list);
        let from = PaletteFrom {
            list: from_list,
            format,
        };
        (commands, from)
    }

    /// The workspace's M-p asking for the note shown and the commands
    /// listed here (`palette` on the socket), leaving out the workspace's
    /// own, which its palette lists already. The note is saved first, so
    /// autopilot picked there reads it as it is on screen.
    fn palette_request(&mut self) -> String {
        self.save();
        let from_list = self.focus == Focus::List && self.list_shown;
        let note = self
            .current_path()
            .filter(|p| !self.store.in_trash(p))
            .map(|p| self.store.relative(p));
        let mut commands = self.palette_commands(from_list);
        // The checklist item's commands lead, the rest in the app's order.
        commands.sort_by_key(|c| !c.act.on_item());
        let commands = commands
            .into_iter()
            .filter(|c| !matches!(c.act, PaletteAct::Press(_)))
            .map(|c| socket::Listed {
                name: c.name,
                key: c.key,
                tag: c.tag.to_string(),
            })
            .collect();
        format!("ok {}", socket::InFront { note, commands }.reply())
    }

    /// One of the commands `palette` listed, picked in the workspace's M-p
    /// (`run` on the socket), found by its words where the app is now.
    fn run_request(&mut self, name: &str) -> String {
        let (commands, from) = self.palette_here();
        let Some(act) = commands.iter().find(|c| c.name == name).map(|c| c.act) else {
            return format!("error no command {name} here");
        };
        // What takes the keys first goes, as Esc would put it away: the
        // app's boxes (the finder, a question), a prompt being typed, and,
        // for any command but Add an item, that input, as a click elsewhere
        // leaves it, so the keys after it reach what the command went to.
        while let Some(layer) = self.layers.pop() {
            self.closed(layer.then, Outcome::Dismissed);
        }
        if self.prompt.take().is_some() {
            self.editor.prompt_done();
        }
        if act != PaletteAct::AddItem {
            self.adding = None;
        }
        self.run_palette(act, from);
        "ok".into()
    }

    fn run_palette(&mut self, act: PaletteAct, from: PaletteFrom) {
        self.run_palette_act(act, from);
        // A command that went to the list puts the plan beside the note away.
        if self.focus == Focus::List && self.beside.is_some() {
            self.close_beside();
            self.list_shown = true;
        }
        // One that edits the note, or left it being typed or selected in,
        // gives it the keys, shown where the plan hid it.
        let on_the_note = matches!(
            act,
            PaletteAct::Act
                | PaletteAct::Indent
                | PaletteAct::Outdent
                | PaletteAct::FoldHere
                | PaletteAct::FoldAll
                | PaletteAct::UnfoldAll
                | PaletteAct::Format(_)
        );
        if (on_the_note || self.editor.mode != Mode::Normal)
            && let Some(b) = self.beside.as_mut()
        {
            b.focused = false;
            b.show_note = self.cols < beside::NARROW;
            self.focus = Focus::Note;
        }
    }

    fn run_palette_act(&mut self, act: PaletteAct, from: PaletteFrom) {
        // A command on the note open, from a note: the list's cursor on it.
        let on_note = |app: &mut App| {
            if from.list {
                return true;
            }
            let Some(path) = app.current_path().map(Path::to_path_buf) else {
                return false;
            };
            if app.store.in_trash(&path) && app.list.folder != store::TRASH {
                app.show(Some(store::TRASH.into()), None, None);
            }
            app.list_shown = true;
            app.reveal(&path);
            app.list_note().as_deref() == Some(path.as_path())
        };
        match act {
            PaletteAct::ToggleList => self.toggle_list(),
            // A palette open as notes_resize was turned off keeps the widths.
            PaletteAct::DefaultWidth if self.opts.resize && self.plan_cols() > 0 => {
                self.default_plan_width()
            }
            PaletteAct::DefaultWidth if self.opts.resize => self.default_width(),
            PaletteAct::DefaultWidth => {}
            PaletteAct::Find => self.finder(),
            PaletteAct::Recent => self.recent_finder(),
            PaletteAct::Search => {
                self.list_shown = true;
                self.search();
            }
            PaletteAct::ClearSearch => self.show(None, None, None),
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
            PaletteAct::Press(name) => {
                self.press(&[name]);
            }
            PaletteAct::NoteTab => self.note_tab(),
            PaletteAct::Preview => self.press_for_note("preview-note"),
            PaletteAct::AttachNote => self.attach_note(),
            PaletteAct::WorkOnItem => self.work_on_item(),
            PaletteAct::OpenPlan => {
                if !self.open_plan() {
                    self.say("notes: that line is no unticked item with a plan");
                }
            }
            PaletteAct::ItemTab => {
                if !self.go_to_item_tab() {
                    self.say("notes: no tab of this note works on that line's item");
                }
            }
            PaletteAct::AddItem => self.start_adding(),
            PaletteAct::NoteOrPlan => {
                let narrow = self.cols < beside::NARROW;
                if let Some(b) = self.beside.as_mut() {
                    // Side by side, the note gets the keys; narrow, it shows
                    // in the plan's place, and again the plan.
                    b.show_note = narrow && !b.show_note;
                    b.focused = narrow && !b.show_note;
                    if !b.focused {
                        self.focus = Focus::Note;
                    }
                }
            }
            PaletteAct::Rename
            | PaletteAct::Move
            | PaletteAct::Pin
            | PaletteAct::Delete
            | PaletteAct::Restore
            | PaletteAct::Mark => {
                if !on_note(self) {
                    return self.say("notes: couldn't find the open note in the sidebar");
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
                self.enter_at(row, col);
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
    /// then Vim's. `NOTE_KEYS` lists them for mission control's Keys view.
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
        if self.editor.is_typing() {
            // The note's own typing keys (Tab, Enter in a list) move the
            // column j and k keep as the editor's do.
            self.editor.forget_want(key);
        }
        self.editor.begin();
        self.note_key_inner(key);
        // Saved a moment after each change, typing included.
        if self.editor.doc.text != text_before {
            self.changed();
        }
        let stopped_typing = matches!(mode_before, Mode::Insert | Mode::Replace)
            && !self.editor.is_typing()
            && !self.editor.one_command();
        if stopped_typing {
            self.after_insert();
        } else if self.editor.mode == Mode::Normal && self.editor.idle() {
            let row = self.editor.doc.row;
            self.editor.tidy(|doc| {
                table::align(doc, row);
            });
            self.editor.clamp();
        }
        if let Some(before) = self.editor.end() {
            self.editor
                .folds
                .follow(&before.lines(), &self.editor.doc.text.lines());
            self.changed();
        }
        self.effects();
        if self.editor.idle() && !self.leader && self.layers.is_empty() {
            self.finish_recording();
        }
    }

    /// The keys of a change just ended kept for `.` to make again: the last
    /// change, not an undo or a redo.
    /// What was typed before Insert mode's Ctrl-O and a jump kept for `.`
    /// as the insert it was, the Ctrl-O, its count and the jump key left
    /// off, before the jump moves away.
    fn finish_typing_before_jump(&mut self) {
        if let Some((keys, _)) = &mut self.recording {
            keys.pop();
            while keys
                .last()
                .is_some_and(|k| matches!(k.code, KeyCode::Char('0'..='9')) && k.mods.is_empty())
            {
                keys.pop();
            }
            if keys
                .last()
                .is_some_and(|k| k.mods.ctrl && k.code == KeyCode::Char('o'))
            {
                keys.pop();
            }
        }
        self.finish_recording();
    }

    fn finish_recording(&mut self) {
        if self.replaying {
            return;
        }
        let Some((keys, before)) = self.recording.take() else {
            return;
        };
        let past_count = keys
            .iter()
            .find(|k| !(matches!(k.code, KeyCode::Char('0'..='9')) && k.mods.is_empty()));
        let undoing = past_count.is_some_and(|k| {
            matches!(k.code, KeyCode::Char('u' | '.')) && k.mods.is_empty()
                || (k.mods.ctrl && k.code == KeyCode::Char('r'))
                || easy::cmd_key(k).is_some()
        });
        if before != self.editor.doc.text && !undoing {
            self.last_change = keys;
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
                self.editor.paste_text(&text);
            }
            return;
        }
        if self.leader {
            self.leader = false;
            return self.leader_key(key);
        }
        let ctrl = key.mods.ctrl && !key.mods.alt;
        let idle = !self.editor.pending();
        let jump_key = if ctrl && key.code == KeyCode::Char('o') {
            Some(true)
        } else if (ctrl && key.code == KeyCode::Char('i'))
            || (key.code == KeyCode::Tab && key.mods.is_empty())
        {
            Some(false)
        } else {
            None
        };
        if mode == Mode::Normal
            && let Some(back) = jump_key
        {
            // Insert mode's Ctrl-O waits for a command with no keys typed.
            let count = if idle || self.editor.no_keys() {
                Some(1)
            } else {
                self.editor.take_count()
            };
            if let Some(count) = count {
                let typing = self.editor.one_command();
                if typing {
                    self.finish_typing_before_jump();
                }
                self.jump(back, count);
                if typing && !self.editor.one_command() && self.editor.mode == Mode::Normal {
                    // Another note opened, which starts in Normal mode:
                    // typing goes on at the place gone to.
                    let col = self.editor.col();
                    self.editor.insert_at(col);
                } else {
                    self.editor.command_done();
                }
                if typing && self.editor.mode == Mode::Insert {
                    // What is typed from here is what `.` makes again, as
                    // an insert started where the jump went, not the jump.
                    let i = Key::new(KeyCode::Char('i'), Mods::NONE);
                    self.recording = Some((vec![i], self.editor.doc.text.clone()));
                }
                return;
            }
        }
        if mode == Mode::Normal && key.code == KeyCode::Char('.') && key.mods.is_empty() {
            if idle {
                return self.repeat(None);
            }
            if let Some(count) = self.editor.take_count() {
                return self.repeat(Some(count));
            }
        }
        match mode {
            Mode::Normal if idle => match key.code {
                KeyCode::Esc if key.mods.is_empty() && self.beside.is_some() => {
                    // On a narrow screen the note shown in the plan's place
                    // gives the plan back; else the panel closes.
                    match self.beside.as_mut().filter(|b| b.show_note) {
                        Some(b) => {
                            b.show_note = false;
                            b.focused = true;
                        }
                        None => self.close_beside(),
                    }
                }
                // An item's tab.
                KeyCode::Char('o') if key.mods.is_empty() && self.go_to_item_tab() => {
                    return;
                }
                KeyCode::Enter if key.mods.is_empty() => {
                    let (row, col) = (self.editor.doc.row, self.editor.doc.col);
                    if !self.enter_at(row, col) {
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
            Mode::Insert if idle => match key.code {
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
            Mode::Replace => match key.code {
                KeyCode::Char('p') if ctrl => return self.open_menu(),
                KeyCode::Char('f') if ctrl => {
                    self.leave_modes();
                    return self.finder();
                }
                KeyCode::Char('b') if ctrl => return self.toggle_list(),
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
    /// or line, or on the selection, each under its key in the Format menu
    /// (`menu::leader`, which mission control's Keys view lists too); a
    /// table only outside a selection.
    fn leader_key(&mut self, key: Key) {
        let Some(c) = (match key.code {
            KeyCode::Char(c) if !key.mods.ctrl && !key.mods.alt => Some(c),
            _ => None,
        }) else {
            return;
        };
        let from = self.format_from(None);
        let in_visual = from.selection.is_some();
        let act = match menu::leader(c).map(|item| item.act) {
            Some(FormatAct::Table) if in_visual => return,
            Some(act) => act,
            None => return,
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

    /// `.`: the last change made again; with a count, with that count in
    /// place of the change's own, kept for the next `.` as Vim does, when
    /// the change takes one (`editor::recount`).
    fn repeat(&mut self, count: Option<usize>) {
        if let Some(n) = count
            && let Some(keys) = editor::recount(&self.last_change, n)
        {
            self.last_change = keys;
        }
        let keys = self.last_change.clone();
        if keys.is_empty() {
            return;
        }
        self.replaying = true;
        for key in keys {
            self.note_key_inner(key);
            // Each replayed key followed on its own, as a typed one is,
            // a jump it made recorded before the next key edits the text.
            self.effects();
            self.follow_lines();
        }
        if self.editor.is_typing() {
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
        // The plan beside the note takes no text, and the note behind it
        // none while the plan has the keys.
        if self.beside.as_ref().is_some_and(|b| b.focused)
            && self.prompt.is_none()
            && self.adding.is_none()
        {
            return;
        }
        if let Some((_, t)) = &mut self.prompt {
            t.push_str(text.lines().next().unwrap_or(""));
            return;
        }
        if let Some(t) = &mut self.adding {
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
            Mode::Insert => self.editor.paste_text(&text),
            Mode::Replace => {
                self.editor.paste_over(&text);
                // Kept as the keys that type it, so . types it over again.
                if !self.replaying
                    && let Some((keys, _)) = &mut self.recording
                {
                    keys.extend(text.chars().map(|c| match c {
                        '\n' => Key::new(KeyCode::Enter, Mods::NONE),
                        c => Key::new(KeyCode::Char(c), Mods::NONE),
                    }));
                }
            }
            Mode::Visual { .. } => {
                let keep = std::mem::replace(&mut self.editor.clipboard, false);
                self.editor.cut_to_type();
                self.editor.clipboard = keep;
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
        if self.editor.end().is_some() || self.editor.is_typing() {
            self.changed();
        }
    }

    /// The open note's jump list entries carried across whatever changed
    /// its text since they last were, before the list is read or added to.
    fn follow_jumps(&mut self) {
        let Some(o) = &self.open else { return };
        if self.jumps_seen == self.editor.doc.text {
            return;
        }
        self.jumps.follow(
            &o.path,
            &self.jumps_seen.lines(),
            &self.editor.doc.text.lines(),
        );
        self.jumps_seen = self.editor.doc.text.clone();
    }

    /// The marks, the change list and the jump list carried across what
    /// changed the open note's text since they last were: called around
    /// each key, paste and click, so each change is followed on its own.
    fn follow_lines(&mut self) {
        self.editor.catch_up();
        self.follow_jumps();
    }

    /// Ctrl-O (`back`) and Ctrl-I or Tab: `count` places through the jump
    /// list, in the open note or another.
    fn jump(&mut self, back: bool, count: usize) {
        self.follow_jumps();
        let Some(o) = &self.open else { return };
        let exists = |p: &Path| p.is_file();
        let to = if back {
            let now = Jump {
                note: o.path.clone(),
                row: self.editor.doc.row,
                col: self.editor.doc.col,
            };
            self.jumps.back(count, now, exists)
        } else {
            self.jumps.forward(count, exists)
        };
        let Some(to) = to else { return };
        if o.path != to.note {
            // Another note's text in place isn't a change for `.` to make
            // again.
            self.recording = None;
            self.jumping = true;
            self.open_note(&to.note, false, false);
            self.jumping = false;
            // Its entries carried across a change made to its file while
            // it was away, before the one gone to is read again.
            self.follow_jumps();
        }
        if self.current_path() == Some(to.note.as_path())
            && let Some(at) = self.jumps.current().filter(|j| j.note == to.note)
        {
            self.editor.go_to(at.row, at.col);
        }
        self.keep_jumps();
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
                Effect::Prompt(c) => {
                    // A change that goes through a prompt under Ctrl-O
                    // isn't kept for . to make again, which would replay
                    // its keys without the prompt.
                    if self.editor.one_command() {
                        self.recording = None;
                    }
                    self.prompt = Some((c, String::new()));
                }
                Effect::Window(dir) => self.window_move(dir),
                Effect::WorkOnItem => self.work_on_item(),
                Effect::AddItem => self.start_adding(),
                Effect::Jump { from: (row, col) } => {
                    self.follow_jumps();
                    if let Some(o) = &self.open {
                        self.jumps.record(Jump {
                            note: o.path.clone(),
                            row,
                            col,
                        });
                    }
                }
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
                crate::host::platform::copy(&text);
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
        if m.kind == MouseKind::Press
            && m.button == MouseButton::Left
            && self.geo.held_line == Some(y)
        {
            return self.go_to_holder();
        }
        let inside = |r: Rect| x >= r.x && x < r.right() && y >= r.y && y < r.bottom();
        let button = self
            .geo
            .toolbar
            .iter()
            .find(|(_, r)| inside(*r))
            .map(|(i, _)| *i);
        let chip = self
            .geo
            .chips
            .iter()
            .find(|(r, _, c)| inside(*r) && Self::clickable(c))
            .map(|(_, at, c)| (*at, c.clone()));
        match (m.kind, m.button) {
            (MouseKind::Move, _) => {
                if self.opts.hover {
                    self.hover_button = button;
                    self.mouse_at = Some((x, y));
                    self.plan_hover(x, y);
                }
            }
            (MouseKind::Press, MouseButton::WheelDown | MouseButton::WheelUp) => {
                let down = m.button == MouseButton::WheelDown;
                if let Some(b) = self.beside.as_mut().filter(|_| inside(self.geo.plan)) {
                    // The reader moved: the next rewrap starts from here.
                    b.held = None;
                    b.scroll = if down {
                        b.scroll + 3
                    } else {
                        b.scroll.saturating_sub(3)
                    };
                } else if inside(self.geo.list) {
                    self.list_top = if down {
                        (self.list_top + 3).min(self.list_rows.len().saturating_sub(1))
                    } else {
                        self.list_top.saturating_sub(3)
                    };
                } else if inside(self.geo.note) {
                    self.scroll(3, down);
                }
            }
            // The plan's edge, as the list's below: a drag sets its width,
            // kept once let go, and a double click puts it back.
            (MouseKind::Press, MouseButton::Left) if self.on_plan_edge(x, y) => {
                self.edge_double = self.edge_plan
                    && self
                        .edge_pressed_at
                        .is_some_and(|at| at.elapsed() < DOUBLE_CLICK);
                self.edge_plan = true;
                self.edge_drag = true;
                self.edge_moved = false;
                self.edge_from = x;
                self.keep_keyed();
                self.edge_start = (self.plan_width, self.plan_cols());
                self.edge_pressed_at = (!self.edge_double).then(Instant::now);
            }
            (MouseKind::Drag, MouseButton::Left) if self.edge_drag && self.edge_plan => {
                self.edge_double = false;
                if self.edge_moved || x != self.edge_from {
                    let was = (self.plan_cols(), self.plan_width);
                    self.drag_plan_edge(x);
                    self.edge_moved |= was != (self.plan_cols(), self.plan_width);
                }
            }
            (MouseKind::Release, _) if self.edge_drag && self.edge_plan => {
                self.edge_drag = false;
                self.mouse_at = Some((x, y));
                if std::mem::take(&mut self.edge_double) {
                    self.default_plan_width();
                } else if self.edge_moved {
                    self.edge_pressed_at = None;
                    let (kept, shown) = self.edge_start;
                    // Let go where it started keeps what was kept, writing
                    // nothing over what another tab may have kept since,
                    // unless it ended on board 8's share, which clears it.
                    let back = self.plan_cols() == shown;
                    if back && (self.plan_width.is_some() || kept.is_none()) {
                        self.plan_width = kept;
                    } else {
                        self.keep_plan_width();
                    }
                }
            }
            (MouseKind::Press, MouseButton::Left) if self.on_edge(x, y) => {
                self.edge_double = !self.edge_plan
                    && self
                        .edge_pressed_at
                        .is_some_and(|at| at.elapsed() < DOUBLE_CLICK);
                self.edge_plan = false;
                // A second press within a moment is a double click: "Use
                // the default width", which the palette offers too, on its
                // release, so that release lands on nothing else.
                self.edge_drag = true;
                self.edge_moved = false;
                self.edge_from = x;
                // A width the keys set and not yet kept is kept first, so the
                // drag starts from it and the tick never keeps a drag's.
                self.keep_keyed();
                self.edge_start = (self.list_width, self.list_cols(self.cols));
                self.edge_pressed_at = (!self.edge_double).then(Instant::now);
            }
            (MouseKind::Drag, MouseButton::Left) if self.edge_drag => {
                // Nothing is set until the mouse leaves the column the
                // press landed on, and nothing is kept unless the edge
                // itself moved, pulled against its limit or not; in a Notes
                // with no room for a width of its own, it takes only the
                // double click.
                self.edge_double = false;
                let room = self.cols / 2 > LIST_LEAST;
                if room && (self.edge_moved || x != self.edge_from) {
                    let was = (self.list_cols(self.cols), self.list_width);
                    self.drag_edge(x);
                    self.edge_moved |= was != (self.list_cols(self.cols), self.list_width);
                }
            }
            (MouseKind::Release, _) if self.edge_drag => {
                // Kept once the drag is over, never as the mouse moves; the
                // edge stays lit where the mouse let it go.
                self.edge_drag = false;
                self.mouse_at = Some((x, y));
                if std::mem::take(&mut self.edge_double) {
                    self.default_width();
                } else if self.edge_moved {
                    // A drag is no first click of a double click, and one
                    // let go where it started keeps nothing, the width of
                    // its own as it was.
                    self.edge_pressed_at = None;
                    let (kept, shown) = self.edge_start;
                    if self.list_cols(self.cols) == shown {
                        self.list_width = kept;
                        self.keep_keyed();
                    } else {
                        self.keep_width();
                    }
                }
            }
            (MouseKind::Press, MouseButton::Left) if self.on_scroll_bar(x, y).is_some() => {
                if let Some(bar) = self.on_scroll_bar(x, y) {
                    // The press focuses the side its bar is on, as a press
                    // there does, leaving the note saving it.
                    match bar {
                        BarDrag::List if self.focus == Focus::Note => {
                            self.save();
                            self.focus = Focus::List;
                        }
                        BarDrag::List => self.focus = Focus::List,
                        BarDrag::Note => {
                            // The note's bar gives the note the keys, as a
                            // click in it does.
                            if let Some(b) = self.beside.as_mut() {
                                b.focused = false;
                            }
                            self.focus = Focus::Note;
                        }
                    }
                    let shown = self.geo.note_bar.as_ref().map_or(1, |b| b.shown);
                    self.bar_drag = Some((bar, self.bar_grab(bar, y), shown));
                    self.scroll_bar_to(bar, y, None);
                }
            }
            (MouseKind::Drag, MouseButton::Left) if self.bar_drag.is_some() => {
                if let Some((bar, grab, shown)) = self.bar_drag {
                    self.scroll_bar_to(bar, y, Some((grab, shown)));
                }
            }
            (MouseKind::Release, _) if self.bar_drag.is_some() => {
                self.bar_drag = None;
            }
            (MouseKind::Press, MouseButton::Left) => {
                if self.plan_click(x, y) {
                    return;
                }
                // A click outside the plan gives the note the keys, but
                // only where the note shows.
                let narrow = self.cols < beside::NARROW;
                if let Some(b) = self.beside.as_mut() {
                    b.focused = narrow && !b.show_note;
                }
                if let Some(b) = button {
                    return self.button(b);
                }
                if let Some(((row, _), chip)) = chip {
                    return self.click_chip(row, &chip);
                }
                if self.geo.commands.is_some_and(inside) {
                    return self.palette();
                }
                if self.geo.search.is_some_and(inside) {
                    // The list takes the keys, as `/` there has them, so the
                    // results are what j, k and Enter then move through;
                    // leaving the note saves it.
                    if self.focus == Focus::Note {
                        self.finish_editing();
                        self.save();
                    }
                    self.adding = None;
                    self.focus = Focus::List;
                    return self.search();
                }
                if self.geo.input == Some(y) {
                    self.focus = Focus::Note;
                    return self.start_adding();
                }
                if self.adding.is_some() {
                    self.adding = None;
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
                    self.click_at(row, col);
                }
            }
            _ => {}
        }
    }

    // ── The widths ────────────────────────────────────────────────────────

    /// The list's width in an app `cols` wide: its own (a drag of its edge,
    /// the pane resize keys) between 30 columns and half the app's, else
    /// the screen's. With Settings' notes_resize off, its own is left
    /// where it is kept and not followed.
    pub(crate) fn list_cols(&self, cols: u16) -> u16 {
        // A Notes with no room for a width of its own (under twice the
        // least) keeps the screen's, a width kept waiting for a wider one.
        match self.own_width(cols) {
            Some(w) => list_clamp(w, cols),
            None => list_width(cols),
        }
    }

    /// The list's own width while it is followed: Settings' notes_resize
    /// on and room for one in an app `cols` wide.
    fn own_width(&self, cols: u16) -> Option<u16> {
        self.list_width
            .filter(|_| self.opts.resize && cols / 2 > LIST_LEAST)
    }

    /// Whether screen cell (`x`, `y`) is on the list's edge, while it
    /// takes a drag.
    fn on_edge(&self, x: u16, y: u16) -> bool {
        self.opts.resize
            && self
                .geo
                .list_edge
                .is_some_and(|r| x == r.x && y >= r.y && y < r.bottom())
    }

    /// How the list's edge is drawn while it is dragged, or under the
    /// mouse with Settings' hover_highlight on; none to draw it as the line
    /// it is.
    pub(crate) fn edge_lit(&self) -> Option<Style> {
        let under = self.opts.hover && self.mouse_at.is_some_and(|(x, y)| self.on_edge(x, y));
        (self.edge_drag || under).then(|| Style::new().fg(Color::Indexed(81)))
    }

    /// The list's edge dragged to column `x`: the list ends there.
    fn drag_edge(&mut self, x: u16) {
        // A width of its own only once the edge would move, and only the
        // way the mouse goes, so a list past half a narrow pane isn't
        // pulled back to the limit by a drag outward. From the width as it
        // is now, not as it was last drawn: several of the mouse's reports
        // may come before a draw.
        let shown = self.list_cols(self.cols).min(self.cols.saturating_sub(10));
        let now = i32::from(shown);
        // Under the least width of its own, the screen's narrower width is
        // where a drag past it stops, the list following the screen again.
        let screen = list_width(self.cols);
        let x = if x <= screen && screen < LIST_LEAST {
            screen
        } else {
            x
        };
        let to = i32::from(if x == screen {
            screen
        } else {
            list_clamp(x, self.cols)
        });
        let toward_mouse = (to - now).signum() == (i32::from(x) - now).signum();
        let on_screen_s = to == i32::from(screen) && self.list_width.is_some();
        if (to != now && toward_mouse) || on_screen_s {
            self.set_list_width(to as u16);
        }
    }

    /// The list given a width of its own; its rows are cut to it again
    /// by the next tick once it has settled. It ends the list's command
    /// half typed (the d of dd), so a note is never deleted across a
    /// resize of the list.
    fn set_list_width(&mut self, width: u16) {
        // The screen's own width is no width of its own: reached by a drag
        // or the keys, the list follows the screen again.
        let screen = list_width(self.cols);
        self.list_width = (width != screen).then(|| list_clamp(width, self.cols));
        if self.list_pending != Some('w') {
            self.list_pending = None;
        }
    }

    /// A pane resize key (`by` columns, right positive): the list's edge
    /// moved while the list has the keys, its width kept once they stop
    /// (the next tick after `LIST_SETTLES`, or as Notes ends). Anywhere
    /// else (`elsewhere` a box or a prompt up, the note or a plan with the
    /// keys, a list that can't move that way) the key is the workspace's,
    /// which moves its pane's divider with it; alone, it does nothing.
    fn resize_by_key(&mut self, by: i32, elsewhere: bool) {
        // While the mouse holds the list's edge, the drag alone moves it.
        if self.edge_drag {
            return;
        }
        // The plan with the keys beside its note moves its own edge.
        if !elsewhere && self.opts.resize && self.plan_resize_key(by) {
            self.keys_moved_on(true);
            return;
        }
        // A list that can't move the way the key asks (a Notes under twice
        // its least width, or the list at its limit that way) hands the key
        // back, so the pane's divider still moves.
        let list = self.beside.is_none()
            && self.focus == Focus::List
            && self.list_shown
            && self.cols / 2 > LIST_LEAST;
        let now = self.list_cols(self.cols);
        let asked = (i32::from(now) + by).clamp(0, i32::from(u16::MAX)) as u16;
        // Reaching or passing the screen's width, the keys stop on it, even
        // under the least width of its own, so they always reach it, and
        // with it the list follows the screen again.
        let screen = list_width(self.cols);
        let passes = screen >= LIST_LEAST && now.min(asked) <= screen && screen <= now.max(asked);
        // From the least width of its own, the next narrowing press goes to
        // the screen's narrower width.
        let below_least = by < 0 && now <= LIST_LEAST && screen < LIST_LEAST;
        let to = if screen != now && (passes || below_least) {
            screen
        } else {
            list_clamp(asked, self.cols)
        };
        let moves = (i32::from(to) - i32::from(now)).signum() == by.signum();
        // In a Notes too narrow to follow it, a width kept is cleared by a
        // narrowing key with the list's keys, as the double click does.
        let kept_unshown = !elsewhere
            && self.opts.resize
            && by < 0
            && self.beside.is_none()
            && self.focus == Focus::List
            && self.list_shown
            && self.cols / 2 <= LIST_LEAST
            && (self.list_width.is_some() || self.kept_width().is_some());
        if kept_unshown {
            return self.default_width();
        }
        // With notes_resize off the key is the workspace's everywhere.
        if !elsewhere && self.opts.resize && list && moves {
            self.set_list_width(to);
            self.keys_moved_on(false);
        } else if self.opts.server {
            let action = if by < 0 {
                format!("resize-left-{}", -by)
            } else {
                format!("resize-right-{by}")
            };
            std::thread::spawn(move || {
                let _ = crate::host::socket::ask(&["press", &action]);
            });
        }
    }

    /// "Use the default width": the list back to the width the screen
    /// gives it, and nothing kept.
    fn default_width(&mut self) {
        self.list_width = None;
        if !self.keys_plan {
            self.keys_moved = None;
        }
        self.lay_out_list();
        // The list's own half-typed command (the d of dd), shown or not,
        // never a Ctrl-W waiting for its direction.
        if self.list_pending != Some('w') {
            self.list_pending = None;
        }
        self.keep_width();
    }

    /// The list's width written to the state folder, so Notes started
    /// again has it: after a drag is let go and after a key, never as the
    /// mouse moves. Only the width that moved is written, so one another
    /// tab's Notes kept since this one read it stays.
    fn keep_width(&mut self) {
        keep_in(&self.store.state.join(LIST_WIDTH), self.list_width);
    }

    /// The width the resize keys set kept, the list's or the plan's,
    /// once they stop (or a drag starts, or Notes ends).
    fn keep_keyed(&mut self) {
        if self.keys_moved.take().is_none() {
            return;
        }
        if self.keys_plan {
            self.keep_plan_width();
        } else {
            self.keep_width();
        }
    }

    /// A resize key moved the plan's edge (`plan`) or the list's: one the
    /// other edge's keys set and not yet kept is kept first.
    fn keys_moved_on(&mut self, plan: bool) {
        if self.keys_plan != plan {
            self.keep_keyed();
        }
        self.keys_plan = plan;
        self.keys_moved = Some(Instant::now());
    }

    /// The list's width as the state folder keeps it, another tab's Notes
    /// having written it since this one started included.
    fn kept_width(&self) -> Option<u16> {
        util::read_text(self.store.state.join(LIST_WIDTH)).and_then(|text| text.trim().parse().ok())
    }

    /// Whether the list's rows run past it, so it has a scroll bar.
    fn list_overflows(&self) -> bool {
        self.opts.scroll_bars && self.list_rows.len() > self.geo.list.height as usize
    }

    /// The scroll bar at screen cell (`x`, `y`), when one is drawn there.
    fn on_scroll_bar(&self, x: u16, y: u16) -> Option<BarDrag> {
        let list = self.geo.list;
        let in_list = x + 1 == list.right() && y >= list.y && y < list.bottom();
        if in_list && self.list_overflows() {
            return Some(BarDrag::List);
        }
        let bar = self.geo.note_bar.as_ref()?;
        (x == bar.x && y >= bar.y && y < bar.y + bar.height).then_some(BarDrag::Note)
    }

    /// The thumb's row a press at screen row `y` takes a scroll bar by.
    fn bar_grab(&self, bar: BarDrag, y: u16) -> switchboard_ui::kit::Grab {
        use switchboard_ui::kit::scroll_grab;
        match bar {
            BarDrag::List => {
                let h = self.geo.list.height as usize;
                let row = y.saturating_sub(self.geo.list.y) as usize;
                scroll_grab(self.list_rows.len(), self.list_top, h, h, row)
            }
            BarDrag::Note => self.geo.note_bar.as_ref().map_or((0, 0), |b| {
                let row = y.saturating_sub(b.y) as usize;
                scroll_grab(b.places(), b.top, b.shown, b.height as usize, row)
            }),
        }
    }

    /// A press or a drag (`drag`, the thumb's row it was taken by and the lines
    /// shown as it was) on a scroll bar at screen row `y`.
    fn scroll_bar_to(
        &mut self,
        bar: BarDrag,
        y: u16,
        drag: Option<(switchboard_ui::kit::Grab, usize)>,
    ) {
        use switchboard_ui::kit::{scroll_drag, scroll_press};
        match bar {
            BarDrag::List => {
                let h = self.geo.list.height as usize;
                let total = self.list_rows.len();
                let row = y.saturating_sub(self.geo.list.y) as usize;
                self.list_top = if let Some((grab, _)) = drag {
                    scroll_drag(total, h, h, row, grab)
                } else {
                    scroll_press(total, self.list_top, h, h, row).unwrap_or(self.list_top)
                };
            }
            BarDrag::Note => {
                let Some(b) = &self.geo.note_bar else {
                    return;
                };
                let (h, places) = (b.height as usize, b.places());
                let row = y.saturating_sub(b.y) as usize;
                let to = if let Some((grab, shown)) = drag {
                    scroll_drag(b.last + shown, shown, h, row, grab)
                } else {
                    scroll_press(places, b.top, b.shown, h, row).unwrap_or(b.top)
                };
                if let Some(&top) = b.rows.get(to) {
                    self.editor.top = top;
                    self.scrolled = true;
                }
            }
        }
    }

    /// The wheel over the note: the view moves, and the cursor is brought
    /// into it as it is drawn, at the scrolloff's distance from its edges,
    /// as Vim moves it (`draw_note`).
    /// A diagram drawn in its block's place is one line to the wheel: going
    /// down it stops with the diagram at the top, and never in its rows.
    fn scroll(&mut self, by: usize, down: bool) {
        let last = self.editor.doc.text.len() - 1;
        let top = self.editor.top;
        let mut to = if down {
            (top + by).min(last)
        } else {
            top.saturating_sub(by)
        };
        let drawn = &self.geo.diagrams;
        if down && let Some(&(start, _)) = drawn.iter().find(|(s, _)| top < *s && *s <= to) {
            to = start;
        } else if let Some(&(start, end)) = drawn.iter().find(|(s, e)| *s < to && to <= *e) {
            to = if down { (end + 1).min(last) } else { start };
        }
        self.editor.top = to;
        self.scrolled = true;
    }

    /// A toolbar button: on the selection when there is one, else on the
    /// line or the word the cursor is on, as the Format menu does.
    fn button(&mut self, index: usize) {
        let b = easy::BUTTONS[index];
        self.focus = Focus::Note;
        let typing = self.typing_col();
        let from = self.format_from(typing);
        self.leave_modes();
        match b.item.and_then(menu::by_label) {
            None => self.push(
                Box::new(FormatMenu::new(self.diagram_label() == menu::SHOW_DIAGRAMS)),
                Then::Format(from),
            ),
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

/// A width of its own written to `file`, or the file removed for none;
/// nothing written when the file already says it.
fn keep_in(file: &Path, width: Option<u16>) {
    let kept = util::read_text(file).and_then(|text| text.trim().parse::<u16>().ok());
    if kept == width {
        return;
    }
    let _ = match width {
        Some(width) => util::write_atomic(file, width.to_string()),
        None => std::fs::remove_file(file),
    };
}

/// How soon a second press on the list's edge makes a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// How long the list's width holds before its rows are built for it, so a
/// pane being resized (the workspace's sidebar dragged) reads the notes once
/// it stops rather than at each column.
const LIST_SETTLES: Duration = Duration::from_millis(250);

/// The columns a pane resize key moves an edge by, right positive: the
/// workspace's own keys for a divider, `C-M-S-h` and `C-M-S-l`.
fn resize_key(key: &Key) -> Option<i32> {
    let key = key.normalized();
    // With Shift, as bound: the workspace passes the chord on only to a
    // pane it reaches whole, so Ctrl-Alt h or l without it is no resize.
    if !(key.mods.ctrl && key.mods.alt && key.mods.shift) {
        return None;
    }
    match key.code {
        KeyCode::Char('h') => Some(-RESIZE_BY),
        KeyCode::Char('l') => Some(RESIZE_BY),
        _ => None,
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

/// How often UX 2.0's background look asks the server for its tabs.
const TABS_EVERY: Duration = Duration::from_secs(3);

/// UX 2.0's background look, off the drawing path: every few seconds while
/// the app runs, the server's tabs (`switchboard list` over its socket),
/// matched to the items they work on, handed to the app only when they
/// change. Ends with the app.
fn look_at_tabs(tx: Sender<Msg>) {
    std::thread::spawn(move || {
        let mut last = None;
        let mut last_next = None;
        loop {
            // Autopilot's items put next, from its file: those of the note
            // it works, while it is on.
            let next: HashSet<String> = match crate::host::host().queued() {
                Some((note_id, items)) => items
                    .iter()
                    .map(|text| note_item_key(&note_id, text))
                    .collect(),
                None => HashSet::new(),
            };
            if last_next.as_ref() != Some(&next) {
                last_next = Some(next.clone());
                if tx.send(Msg::Next(next)).is_err() {
                    return;
                }
            }
            let found = crate::host::socket::ask_ok(&["list"])
                .ok()
                .and_then(|out| serde_json::from_str::<serde_json::Value>(&out).ok())
                .map(|list| {
                    item_tabs_of(&list, |dir| crate::host::host().item_claim(Path::new(dir)))
                })
                .unwrap_or_default();
            if last.as_ref() != Some(&found) {
                last = Some(found.clone());
                if tx.send(Msg::ItemTabs(found)).is_err() {
                    return;
                }
            }
            std::thread::sleep(TABS_EVERY);
        }
    });
}

/// The tabs working on an item or waiting on you, from `switchboard list`'s
/// JSON: the item from the tab's `@tab_title` or `@autopilot_item`, else
/// from `wt_claim` on its directory, keyed by `note_item_key` under the
/// note's ID (the claim's), else the note it is linked to (`@tab_note`),
/// else by the item's first line alone; working with its
/// Claude's phase, or waiting with what it waits on.
fn item_tabs_of(
    list: &serde_json::Value,
    wt_claim: impl Fn(&str) -> Option<(Option<String>, String)>,
) -> HashMap<String, render::ItemTab> {
    let mut out = HashMap::new();
    let sessions = list["sessions"].as_array().into_iter().flatten();
    for tab in sessions.flat_map(|s| s["tabs"].as_array().into_iter().flatten()) {
        let waits = match tab["agent"].as_str() {
            Some("waiting") => true,
            Some("working") => false,
            _ => continue,
        };
        let opt = |key: &str| {
            tab["options"][key]
                .as_str()
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        };
        let claim = tab["path"].as_str().and_then(&wt_claim);
        let Some(item) = opt("@tab_title")
            .or_else(|| opt("@autopilot_item"))
            .or_else(|| claim.as_ref().map(|(_, item)| item.clone()))
        else {
            continue;
        };
        let state = if waits { "waiting" } else { "working" };
        let detail = tab["panes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| &p["agent"])
            .find(|a| a["state"].as_str() == Some(state))
            .and_then(|a| a[if waits { "wait" } else { "phase" }].as_str())
            .unwrap_or("")
            .to_string();
        let index = tab["index"].as_u64().unwrap_or(0) as usize;
        let id = tab["id"].as_str().unwrap_or("").to_string();
        let color = tab["color"]
            .as_str()
            .filter(|hex| hex.len() == 6)
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .map(|n| ratatui::style::Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8));
        let found = render::ItemTab {
            id,
            note: opt("@tab_note").unwrap_or_default(),
            // The claim's note only when the claim is for this item: the
            // directory may be another worktree's, a split's say.
            note_id: claim
                .filter(|(_, claimed)| claimed.trim() == item.trim())
                .and_then(|(id, _)| id)
                .unwrap_or_default(),
            index,
            waits,
            detail,
            color,
        };
        // Under its note's ID, else the note it is linked to (which may be
        // another note than the one its branch claims an item of), so
        // another note's item of the same words gets no chip of it; under
        // the item alone only when neither is known.
        let item = item.trim();
        let key = [&found.note_id, &found.note]
            .into_iter()
            .find(|note| !note.is_empty())
            .map_or_else(|| item.to_string(), |note| note_item_key(note, item));
        out.entry(key).or_insert(found);
    }
    out
}

/// `item_tabs_of`'s key for an item's tab in a note, by its ID or its path.
fn note_item_key(note: &str, item: &str) -> String {
    format!("{note}\0{item}")
}

/// Marks the pane Notes runs in, or lets the mark go: `@notes_cmd_keys`
/// for the workspace to pass it easy mode's Cmd keys, `@allow_passthrough`
/// to pass its pictures' graphics commands on to the terminal, as bin/notes
/// set its pane's allow-passthrough (src/mux/passthrough.rs).
/// A pane option of this app's pane set to `value`, or unset.
fn set_pane_option(option: &str, value: Option<&str>) {
    let Some(pane) = crate::host::pane() else {
        return;
    };
    let args: Vec<&str> = match value {
        Some(v) => vec!["set", "-t", &pane, option, v],
        None => vec!["set", "-t", &pane, "-u", option],
    };
    let _ = crate::host::socket::ask(&args);
}

pub fn mark_pane(option: &str, on: bool) {
    let Some(pane) = crate::host::pane() else {
        return;
    };
    let args: Vec<&str> = if on {
        vec!["set", "-t", &pane, option, "1"]
    } else {
        vec!["set", "-t", &pane, "-u", option]
    };
    let _ = crate::host::socket::ask(&args);
}

/// Names on the pane Notes runs in the note it shows (`@notes_open`, its
/// title), or lets the name go with none, so the sidebar's line for the
/// Notes tab says which note is open from what the server holds.
pub(crate) fn name_open_note(title: Option<&str>) {
    let Some(pane) = crate::host::pane() else {
        return;
    };
    let args: Vec<&str> = match title {
        Some(title) => vec!["set", "-t", &pane, "@notes_open", title],
        None => vec!["set", "-t", &pane, "-u", "@notes_open"],
    };
    let _ = crate::host::socket::ask(&args);
}

/// Runs the app on this terminal until it is quit.
pub fn run(program: &str, store: Store) -> ExitCode {
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
    let term = match Term::enter(opts.hover) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{program}: can't take the terminal: {e}");
            return ExitCode::FAILURE;
        }
    };
    // The terminal's background, for the `system` theme, which Neovim
    // followed through Ghostty's; a key typed meanwhile is read as typed.
    let (colors, typed) = switchboard_ui::terminal::ask(Duration::from_millis(200));
    if let Some(light) = colors.light() {
        switchboard_ui::theme::set_system(light);
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
    let socket_path = socket::own_path(opts.per_note);
    let listener = if app.locked_by.is_none() {
        socket::listen(&socket_path, tx.clone())
    } else {
        None
    };
    // With an app in each tab there is no lock on the directory to find
    // an app started by hand by, so each marks its own pane as Notes'.
    if opts.per_note && opts.server {
        mark_pane("@notes_pane", true);
    }
    // The workspace passes the pane resize keys on only to a pane marked
    // as handing them back, which a Notes from an older build never is.
    // Listed as the diff lists its own (`keys:` and each key with its
    // columns): the default chords, which are the ones Notes knows.
    if opts.server
        && let Some(pane) = crate::host::pane()
    {
        let _ = crate::host::socket::ask(&[
            "set",
            "-t",
            &pane,
            "@resize_keys",
            "keys:C-M-S-h=-5 C-M-S-l=5",
        ]);
    }
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
    look_at_tabs(tx.clone());
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
            let mut buf = ratatui::buffer::Buffer::empty(Rect::new(0, 0, size.0, size.1));
            let cursor = app.draw(&mut buf);
            let shape = match app.editor.mode {
                Mode::Insert if app.focus == Focus::Note => {
                    crossterm::cursor::SetCursorStyle::SteadyBar
                }
                Mode::Replace if app.focus == Focus::Note => {
                    crossterm::cursor::SetCursorStyle::SteadyUnderScore
                }
                _ => crossterm::cursor::SetCursorStyle::SteadyBlock,
            };
            let mut bytes = screen.render_shaped(&buf, cursor, shape);
            bytes.append(&mut app.out);
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(&bytes);
            let _ = out.flush();
            dirty = false;
        }
    }
    app.stop();
    if opts.per_note && opts.server {
        set_pane_option("@notes_note", None);
        mark_pane("@notes_pane", false);
    }
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(&app.out);
    let _ = out.flush();
    drop(out);
    if listener.is_some() {
        let _ = std::fs::remove_file(&socket_path);
    }
    if opts.server {
        mark_pane("@resize_keys", false);
    }
    if app.locked_by.is_none() && opts.server {
        mark_pane("@allow_passthrough", false);
        name_open_note(None);
        if opts.easy {
            mark_pane("@notes_cmd_keys", false);
        }
    }
    drop(term);
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests;
