//! `switchboard diff`: what M-d showed in diffview.nvim, as a program of
//! Switchboard's own run in a pane's place (nvim/lua/plugins/diffview.lua,
//! nvim/diff.lua). The changed files in reading order on the left, the
//! chosen file's diff side by side on the right, an added file or a deleted
//! one alone in a single pane; git's green and red over the background, the
//! words that changed in a stronger shade, syntax colours, and the file
//! opened on its first changed word.
//!
//! With the diff_story setting on, the list is told as a story (story.rs,
//! tell.rs): the diff waits under a cover while Haiku reads it, blank for
//! its first 300ms so a story known by then shows no words flashing away,
//! then saying what is read and each chapter as Haiku names it (`t` shows
//! the answer itself); Esc shows the diff now. Each chapter heads its first
//! file in the list with its number, title and why, each hunk worth it
//! carries a note over it in the diff, ]c and [c move between the chapters,
//! ]p and [p open them a page at a time, and yS copies the story as
//! Markdown. Without a story, the files keep the kinds' order and Haiku's
//! order of the code by its paths (order.rs).
//!
//! A story told as a map (the diff_story_map setting) opens on the map of
//! its chapters, a mode beside the file view (app/map.rs): `g m` and `g f`
//! switch between the two, and `m` asks which.
//!
//! `ga` asks Haiku a question about the change (ask.rs): a box to type it
//! in, then a window on the right half with the answer streamed in.
//!
//! `c` goes through the branch's commits one at a time, oldest first, as
//! diffview's `:DiffviewFileHistory --reverse` did: each commit's own diff,
//! a diff of its own (`History`) drawn in this one's place, `]p` and `[p`
//! moving to the next commit and the one before, and `c`, `q` or `Esc`
//! back to the whole branch.
//!
//! With the diff_uncommitted setting on, M-d's change runs from the merge
//! base to the working tree (git.rs's `read_to_work_tree`, diffview.lua's
//! `uncommitted_mode` as upstream's 7b85aa7 added it): each file with
//! changes not yet committed is marked ● in the list and "uncommitted" in
//! its bar, and `yl` gives no link from its working-tree side. The story
//! stays the commits' own, the one told ahead, with the files only
//! uncommitted in a last "Not committed yet" chapter drawn without asking
//! Haiku (story.rs's `with_uncommitted`), the commits in a "Committed"
//! chapter before it when they have no story, and with nothing committed
//! that chapter alone, under no cover; a file changed since its commits
//! has no notes, its story's hunks being at the committed lines. With
//! nothing to show at all, the workspace says so in its box and the diff
//! closes. `R` reads the change again, as diffview's refresh did, so edits
//! made while it is open come in, the story kept while its key is the same.
//!
//! Asked for (`--working`), it shows the working tree's changes instead,
//! the changes not staged and those staged in a section each, as
//! diffview's plain `:DiffviewOpen` listed them (git.rs's `read_working`),
//! a file both staged and changed since in both.
//!
//! `yp` copies the cursor's `path:line` and `yl` a GitHub link to it; `v`
//! or `V` in a side of the diff selects lines, as Visual mode did in
//! diffview's buffers, and then `yp` and `yl` copy the lines selected
//! (`path:12-20`, `#L12-L20`), and `y` their text. A drag of the mouse
//! selects any text on the screen and copies it (select.rs).
//!
//! Alone (`switchboard-diff`), it heads its screen with a line of its own,
//! keeps the row of buttons at the bottom with a Settings button before
//! Close, and has a settings view of its own on M-m (own.rs); `m` in
//! the file list, or a right click, opens its menu either way.
//!
//! The screen is drawn into a cell buffer and only what changed is sent,
//! through the renderer the multiplexer's is (`switchboard_ui::screen`), so the
//! program is tested by drawing into a buffer and reading it back.

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use crossterm::event::{
    self, Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton,
    MouseEvent, MouseEventKind,
};
use crossterm::terminal;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Span;

use super::ask::{self, Asking, Said};
use super::git::{self, Change, Entry, Section};
use super::highlight::{self, Hl};
use super::order;
use super::sides::{self, Item};
use super::story::{self, Look as StoryLook, Progress, Reading, Story};
use super::tell::{self, Asked, Heard, Teller};
use switchboard_ui::draw::width;
use switchboard_ui::kit::{self, Tone};
use switchboard_ui::screen::Screen;

use crate::host::host;
use crate::{settings, util};

mod draw;
mod map;
mod own;
mod select;

pub use map::Mode;
use map::{Hit, Laid, Place_};

/// The list's narrow width, and how wide a list must be to head a chapter
/// with its whole why (narrower, its first sentence, the rest in the bar).
const LIST_WIDTH: u16 = 30;
const SHORT_WHY: u16 = 48;
/// Lines J and K scroll the diff from the list.
const SCROLL: usize = 5;
/// How long the terminal is given to say its colours as the diff starts.
const ASK_WAIT: Duration = Duration::from_millis(200);
/// How long the cover stays blank, and how long a message stays.
const COVER_BLANK: Duration = Duration::from_millis(300);
const MESSAGE_FOR: Duration = Duration::from_secs(5);
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// git's own colours, mixed over the terminal's background, as GitHub
/// shows a diff.
const ADDED: u32 = 0x2ea043;
const REMOVED: u32 = 0xf85149;
const ACCENT: Color = Color::Indexed(81);

/// Which of the diff's scroll bars a press is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BarDrag {
    List,
    Diff,
    Answer,
}

/// The settings the diff follows, read as it opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Files in reading order (diff_order); off, A to Z.
    pub order: bool,
    /// The diff told as a story (diff_story, with diff_order).
    pub story: bool,
    /// ]p and [p open chapters a page at a time (diff_story_pages).
    pub pages: bool,
    /// The cover shows what is read and each chapter (diff_story_progress).
    pub progress: bool,
    /// Haiku's notes over hunks (diff_story_notes).
    pub notes: bool,
    /// The list opens wide enough for its longest path (diff_list_wide).
    pub list_wide: bool,
    /// M-d's change runs to the working tree, what isn't committed yet
    /// marked apart (diff_uncommitted); off, the commits alone.
    pub uncommitted: bool,
    /// ga asks Haiku about the diff (diff_ask).
    pub ask: bool,
    /// A row of buttons for the list's keys (easy_keys), with the keys
    /// named after them (key_help_line).
    pub easy: bool,
    pub key_help: bool,
    /// UX 2.0 (ux2): the list under the kit's labels, the chapter's why over
    /// the diff, one bar over both sides, a hunk's note as a card under it,
    /// and the files read as one scroll.
    pub ux2: bool,
    /// The button under the mouse lit (hover_highlight).
    pub hover: bool,
    /// The light theme's colours (theme).
    pub light: bool,
    /// The M-arrows move between the list, the sides and the panes beside
    /// (pane_arrows).
    pub pane_arrows: bool,
    /// Whether Haiku may be asked at all; off in the tests.
    pub haiku: bool,
    /// The workspace's server is asked for the pane beside at the diff's
    /// edges (a pane of it); off in the tests.
    pub server: bool,
    /// Outside the workspace, through the desktop's clipboard too; off in
    /// the tests.
    pub clipboard: bool,
    /// A scroll bar at the list's right edge and the diff's while they run
    /// past the screen (scroll_bars), the diff's column kept for it.
    pub scroll_bars: bool,
    /// The diff runs alone, as `switchboard-diff`: its own header, its
    /// Settings button and its settings view on M-m.
    pub alone: bool,
    /// The story told as a map, with a Map mode beside the files
    /// (diff_story_map, with diff_story).
    pub map: bool,
    /// The mode used last and the chapters read are kept between runs;
    /// off in the tests.
    pub keep: bool,
    /// A click takes an option of a menu, and a right click opens one
    /// (menu_mouse).
    pub menu_mouse: bool,
}

impl Options {
    pub fn read() -> Options {
        let on = settings::is_on;
        let order = on("diff_order");
        let story = order && on("diff_story");
        Options {
            order,
            story,
            pages: story && on("diff_story_pages"),
            progress: story && on("diff_story_progress"),
            notes: story && on("diff_story_notes"),
            list_wide: on("diff_list_wide"),
            uncommitted: on("diff_uncommitted"),
            ask: on("diff_ask"),
            easy: on("easy_keys"),
            key_help: on("key_help_line"),
            ux2: settings::value("ux2") == Some(serde_json::Value::Bool(true)),
            hover: on("hover_highlight"),
            light: settings::value("theme")
                .is_some_and(|theme| switchboard_ui::theme::light_for(&theme)),
            pane_arrows: on("pane_arrows"),
            scroll_bars: on("scroll_bars"),
            alone: crate::host::is_alone(),
            map: story && on("diff_story_map"),
            keep: true,
            menu_mouse: on("menu_mouse"),
            haiku: true,
            server: host().in_pane(),
            clipboard: true,
        }
    }
}

/// Which part of the screen has the keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    List,
    Old,
    New,
}

/// What the program's work off the screen's thread reports.
pub enum Msg {
    /// What it says while it reads the change.
    Loading(String),
    /// The change, with anything to say about how it was read.
    Loaded(Result<(Box<Change>, Option<String>), String>),
    /// The change has nothing to show, and what to say so.
    Nothing(String),
    /// The change read again (R), by the reading's number.
    Refreshed(u64, Result<Box<Change>, String>),
    /// What telling a story comes to, by the story's number, so an answer
    /// to one asked before the change was read again is dropped.
    Heard(u64, Heard),
    /// Haiku's order of the code, by path.
    Order(Vec<String>),
    /// A side of a file coloured, by the file's key (`Entry::key`).
    Coloured {
        path: String,
        new_side: bool,
        lines: Vec<Vec<Hl>>,
    },
    /// What a question's answer says, by the question's number.
    Asked(u64, Said),
    /// What a commit's diff of the history was told, by its number.
    History(u64, Box<Msg>),
    /// What a reading of the change asked for by a setting said, by its
    /// number, so one a later change of the setting outran is dropped.
    Reloaded(u64, Box<Msg>),
    /// What the look for a newer release found, alone.
    Newer(Option<crate::update::Newer>),
    /// What an update asked for from the settings view came to.
    Updated(Result<crate::update::Outcome, String>),
}

/// What a commit's diff asks of the branch's diff it was opened from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wants {
    /// The next commit (1) or the one before (-1).
    Step(i32),
    /// Back to the whole branch.
    Leave,
    /// The settings view, which is the branch's diff's to show.
    Settings,
}

/// The branch's commits one at a time: the commits, oldest first, which is
/// shown, and its own diff, whose messages come back through
/// `Msg::History` under its number, so one for a commit left behind is
/// dropped.
struct History {
    commits: Vec<(String, String)>,
    at: usize,
    id: u64,
    app: Box<App>,
}

/// A file's two sides, read once.
struct Sides {
    old: Vec<String>,
    new: Vec<String>,
    rows: Vec<sides::Row>,
    /// Shown alone: Some(true) the new side (an added file), Some(false)
    /// the old (a deleted one).
    alone: Option<bool>,
    binary: bool,
    old_hl: Option<Vec<Vec<Hl>>>,
    new_hl: Option<Vec<Vec<Hl>>>,
    opened: HashSet<usize>,
    all_open: bool,
}

impl Sides {
    /// The columns of a side of `w` its line numbers take before its text,
    /// as board 12 draws them: a blank, the number at the right of at
    /// least three columns, a blank; none on a side too narrow to spare
    /// them.
    fn gutter(&self, w: usize) -> usize {
        let digits = self.old.len().max(self.new.len()).max(1).to_string().len();
        let g = digits.max(3) + 2;
        if w >= g + 20 { g } else { 0 }
    }
}

/// One side of a screen line of the diff.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Cell {
    /// A piece of a line: its index and the bytes of it shown here.
    Text { line: usize, range: Range<usize> },
    /// The gap facing a line only the other side has.
    Gap,
}

/// One screen line of the diff.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Line {
    Pair {
        row: usize,
        old: Cell,
        new: Cell,
    },
    Fold {
        first: usize,
        count: usize,
    },
    /// A note over a hunk, on one side (`new_side`), the other blank.
    Note {
        new_side: bool,
        text: String,
    },
    /// With UX 2.0, a row of the card a hunk's note is drawn in under the
    /// hunk, across both sides.
    Card(Vec<Span<'static>>),
    /// The line over a hunk a chapter shows from another, across both
    /// sides: that chapter's number and the hunk's id (none for the line at
    /// a file's head), which Enter and a click go to.
    Also(usize, Option<String>, Vec<Span<'static>>),
}

/// A hunk's note card with the last lines of its hunk, a side and a
/// line each, still to be passed before it goes in.
type WaitingCard = (Vec<(bool, usize)>, Vec<Line>);

/// The label over generated files in UX 2.0's list, last.
const GENERATED_LAST: &str = "Generated, last";

/// One row of the list.
#[derive(Clone, Debug, PartialEq)]
enum ListRow {
    Header(Section),
    Loading,
    Heading(String, StoryLook),
    File(usize),
    /// With UX 2.0: a chapter's number and title, a section's label in its
    /// tone with its count, and the blank row between chapters.
    Chapter(usize),
    Label(String, Tone, Option<usize>),
    Blank,
    /// The whole change's files and lines added and deleted, with their
    /// bar, as GitHub's PR header shows them; filtered, the files left's.
    Totals,
    /// A faint line under the list, saying what the path filter hides.
    Hint(String),
}

/// The whole change's files, lines added and lines deleted, summed once
/// each time the change is read; a binary file counts as a file with no
/// lines, and a file both staged and changed since as one file.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Totals {
    files: usize,
    added: usize,
    deleted: usize,
}

impl Totals {
    fn of<'a>(entries: impl IntoIterator<Item = &'a Entry> + Clone) -> Totals {
        let files: HashSet<&str> = entries
            .clone()
            .into_iter()
            .map(|e| e.path.as_str())
            .collect();
        Totals {
            files: files.len(),
            added: entries.clone().into_iter().map(|e| e.additions).sum(),
            deleted: entries.into_iter().map(|e| e.deletions).sum(),
        }
    }
}

/// The story's state.
#[derive(Clone, Debug, PartialEq)]
enum Telling {
    Off,
    Waiting,
    Told,
    Untold(String),
}

/// The box a question is typed in: its text, the byte the typing cursor is
/// at, and where the diff's cursor was.
struct AskBox {
    text: String,
    pos: usize,
    at: ask::Cursor,
}

impl AskBox {
    fn insert(&mut self, text: &str) {
        self.text.insert_str(self.pos, text);
        self.pos += text.len();
    }

    /// The character before the cursor's start, or after it.
    fn char_before(&self) -> Option<usize> {
        self.text[..self.pos]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
    }

    fn char_after(&self) -> Option<usize> {
        let c = self.text[self.pos..].chars().next()?;
        Some(self.pos + c.len_utf8())
    }

    /// The start of the cursor's line, and its end.
    fn line_start(&self) -> usize {
        self.text[..self.pos].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self) -> usize {
        self.text[self.pos..]
            .find('\n')
            .map_or(self.text.len(), |i| self.pos + i)
    }
}

/// The window an answer streams into.
struct Answer {
    id: u64,
    question: String,
    body: String,
    done: bool,
    top: usize,
    cursor: usize,
    asking: Asking,
    /// Who was asked, once that is known: the tab's session on its model,
    /// or Haiku in its place.
    who: Option<String>,
    /// Where a selection of lines started (`v`), as a line of the answer.
    visual: Option<usize>,
}

/// A button of the row at the bottom, a row of the settings view, or a
/// button of a box's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Button {
    Bar(usize),
    /// The row's Settings button, alone.
    Settings,
    /// A row of the settings view.
    Option(usize),
    /// One of the two modes, in the switch at the head of the row.
    Mode(Mode),
    AskSend,
    AskCancel,
    AnswerClose,
    AnswerAgain,
    AnswerCopy,
}

/// The row's buttons (easy mode's, and the diff alone's): each presses its
/// keys in the list.
const BUTTONS: [(&str, &str); 9] = [
    ("◀ Chapter", "[c"),
    ("Chapter ▶", "]c"),
    ("◀ Page", "[p"),
    ("Page ▶", "]p"),
    ("Widen", "E"),
    ("Copy link", "yl"),
    ("Refresh", "R"),
    ("Ask", "ga"),
    ("Close", "q"),
];

/// The ● beside a file with changes not yet committed, and its bar's word:
/// the terminal's yellow, base16's base0A, as diffview.lua linked them to
/// the colorscheme's Type.
const UNCOMMITTED: Color = Color::Indexed(3);

/// One of the diff's keys for mission control's Keys view: the keys that do
/// it, apart by spaces and written as the views write keys (`esc`, `↵`,
/// `^c`), what it does, and whether it is `ga`'s (the diff_ask setting).
pub struct KeyRow {
    pub keys: &'static str,
    pub does: &'static str,
    pub ask: bool,
}

const fn key_row(keys: &'static str, does: &'static str) -> KeyRow {
    KeyRow {
        keys,
        does,
        ask: false,
    }
}

const fn ask_row(keys: &'static str, does: &'static str) -> KeyRow {
    KeyRow {
        keys,
        does,
        ask: true,
    }
}

/// The diff's own keys where each works, for mission control's Keys view
/// (M-/; the tmux version's lib/sidebar/app-keys.json), under a heading for
/// each place. `the_keys_view_lists_keys_the_diff_takes` presses each where
/// it is listed, so one the diff stops taking fails it.
pub const KEYS: &[(&str, &[KeyRow])] = &[
    (
        "M-d's diff",
        &[
            key_row("]c [c", "next, last chapter of the story, in the file list"),
            key_row(
                "]p [p",
                "next chapter as a page of its own, the one before (from the first, the whole diff)",
            ),
            key_row("]f [f", "next, last file, opened at its first change"),
            key_row("J K", "scroll the chosen file down, up from the file list"),
            key_row("E", "widen the file list, or narrow it again"),
            key_row(
                "/",
                "filter the file list by path (!word leaves it out); ↵ keeps it, esc clears it",
            ),
            key_row(
                "c",
                "the branch's commits, one at a time; c again goes back",
            ),
            key_row(
                "R",
                "read the change again, edits made since it opened included",
            ),
            key_row(
                "v V",
                "select lines in the file list or a side of the diff: y then copies their text",
            ),
            key_row("yp", "copy path:line"),
            key_row(
                "yl",
                "copy a link to the line on GitHub (the lines selected)",
            ),
            key_row("yS", "copy the story as Markdown"),
            key_row(
                "gm gf",
                "the story's map, and the chosen chapter's file in the file view",
            ),
            key_row("m", "the view's menu: the map or the files"),
            ask_row("ga", "ask Claude about the diff"),
            key_row(
                "m",
                "the diff's menu, in the file list: its settings, and what its buttons do",
            ),
            key_row("S-F10", "the diff's menu, from the list or the diff"),
            key_row("q esc", "close the diff (esc in the file list)"),
            key_row("^c", "close the diff and go back to the pane"),
        ],
    ),
    (
        "M-d: the story's map",
        &[
            key_row(
                "j k h l",
                "down a level to what the chapter builds on, up to what needs it, across a level",
            ),
            key_row("↵", "open the chosen chapter, with its own map"),
            key_row(
                "]c [c",
                "the next chapter read from the goal down, the one before",
            ),
            key_row("]s [s", "the next story's map, the one before"),
            key_row("gf", "the chosen chapter's file in the file view"),
        ],
    ),
    (
        "M-d: the cover while the story is told",
        &[
            key_row("esc q", "show the diff now, without waiting for the story"),
            key_row("t", "show Haiku's answer so far, or the chapters again"),
        ],
    ),
    (
        "M-d: asking a question (ga)",
        &[
            ask_row("↵", "ask it"),
            ask_row("⇧↵ ^j", "a new line"),
            ask_row("esc ^c", "cancel"),
        ],
    ),
    (
        "M-d: an answer (ga)",
        &[
            ask_row("ga", "ask another question"),
            ask_row("v", "select lines of the answer; y copies them"),
            ask_row("yy", "copy the answer"),
            ask_row("yS", "copy the story as Markdown"),
            ask_row(
                "gf",
                "open the path:line under the cursor, in the diff when the change has that file",
            ),
            ask_row("q esc", "close the answer, stopping it while it runs"),
        ],
    ),
];

/// The diff's keys as mission control's Keys view lists them, a group
/// each, `ga`'s only with `ask` (diff_ask), a group left with none left
/// out.
pub fn key_groups(ask: bool) -> Vec<(&'static str, Vec<(String, String)>)> {
    KEYS.iter()
        .map(|(title, rows)| {
            let rows: Vec<(String, String)> = rows
                .iter()
                .filter(|r| ask || !r.ask)
                .map(|r| (r.keys.to_string(), r.does.to_string()))
                .collect();
            (*title, rows)
        })
        .filter(|(_, rows)| !rows.is_empty())
        .collect()
}

/// What came of a key or the mouse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    Same,
    Redraw,
    Quit,
}

pub struct App {
    opts: Options,
    tx: Sender<Msg>,
    cols: u16,
    rows: u16,
    /// While the change is read, what it says; and why it couldn't be.
    loading: Option<String>,
    failed: Option<String>,
    /// Why there is nothing to show, said on the diff's own screen outside
    /// the workspace; in it, the workspace says so and the diff closes.
    nothing: Option<String>,
    closed: bool,
    change: Option<Change>,
    totals: Totals,
    /// The files the path filter leaves, summed as each sort narrows them,
    /// and how many files (by path) the list may show before it filters.
    left: Totals,
    listed_files: usize,
    /// The diff as the story reads it: its hunks and what Haiku is given.
    hunks: story::Hunks,
    input: String,
    /// The story's context (`story::plan_context`), read from the tab once.
    context: Option<String>,
    reading: Reading,
    /// Indexes into the change's files, in the order shown (a page's alone).
    order: Vec<usize>,
    /// The list's cursor (an index into `order`), the file it shows.
    at: usize,
    /// What `/` filters the list's paths by (`path_matches`), kept while
    /// the diff is open, and whether it is being typed.
    path_filter: String,
    filtering: bool,
    /// The list's first row on the screen, and its width.
    list_top: usize,
    list_width: u16,
    /// The list's row the cursor was on when the list last followed it:
    /// the list scrolls to the cursor only when the cursor has moved, so
    /// the wheel scrolls it past the cursor.
    list_followed: Option<usize>,
    /// Whether the view was scrolled off the cursor's file, in the one
    /// scroll of files: the cursor stays where it is, out of sight, until
    /// a key moves it, which brings the view back.
    adrift: bool,
    /// Whether the end of a file has said how to go on to the next.
    said_end: bool,
    /// The scroll bar whose thumb is being dragged, the screen row its
    /// first row is on, and where the drag started (`kit::scroll_grab`).
    bar_drag: Option<(BarDrag, u16, kit::Grab)>,
    focus: Focus,
    /// The user has moved through the list, so a story arriving keeps the
    /// file shown.
    moved: bool,
    /// Each file's two sides once read, and each text read, by commit and path.
    files: HashMap<String, Sides>,
    texts: HashMap<(String, String), Option<String>>,
    /// Each file's screen lines by its key, for the sides' widths they
    /// were laid out for.
    lines: HashMap<String, (u16, u16, Vec<Line>)>,
    /// The diff's cursor (a screen line of the shown file and a column),
    /// and the view's first line: line `top` of the list's file `view`.
    /// That is the shown file, but while UX 2.0 reads the files as one
    /// scroll, where it may be a file before it, and `top` past its lines
    /// (on the rule and the bar before the next).
    cur: usize,
    col: usize,
    top: usize,
    view: usize,
    /// Haiku's order of the code by its paths, when no story is told.
    smart: HashMap<String, usize>,
    /// The story shown, and the one told of the change (of its commits
    /// alone, in a change running to the working tree, whose files not
    /// committed yet the story shown adds in a last chapter).
    story: Option<Story>,
    told: Option<Story>,
    /// The key of the story asked for, and its number, so an answer to one
    /// asked before the change was read again is dropped.
    story_key: Option<String>,
    tells: u64,
    /// How many times the change has been read again (R), for the number
    /// of the latest, and whether Haiku's order of the files was asked for.
    refreshes: u64,
    asked_order: bool,
    telling: Telling,
    /// The answer so far while the story is told, and when the cover went
    /// up (none once it is down); `raw` shows the answer itself on it.
    progress: Option<Progress>,
    cover: Option<Instant>,
    raw: bool,
    /// The chapter shown as a page, and the file the whole diff showed.
    page: Option<usize>,
    whole_at: Option<String>,
    /// With a story told as a map (map.rs): which of the two modes is
    /// shown, the story whose map it is, the chapter chosen on it, and the
    /// chapters read.
    mode: Mode,
    thread: usize,
    map_at: usize,
    read: HashSet<usize>,
    /// The open chapter's own map laid out, and the thing chosen on it
    /// while it has the keys.
    things: Option<Laid>,
    thing: Option<usize>,
    /// Where each thing of the map a click lands on was last drawn, and
    /// the one under the mouse.
    hits: Vec<(Hit, Rect)>,
    lit: Option<Hit>,
    /// The chosen chapter a press landed on, opened once the mouse is let
    /// go with nothing dragged over.
    open_on_up: Option<usize>,
    /// The "also in chapter" line a press landed on, gone to once the
    /// mouse is let go with nothing dragged over.
    also_on_up: Option<(usize, Option<String>)>,
    /// Where Files mode was left, for the way back from the map.
    files_place: Option<Place_>,
    /// The chapter open on the map when the menu switched to the files,
    /// which the menu's Map opens again.
    map_page: Option<usize>,
    map_place: Option<Place_>,
    ask_box: Option<AskBox>,
    answer: Option<Answer>,
    asks: u64,
    /// The tab's own Claude session, which a question asks a copy of
    /// (diff_ask_session), when the diff was opened in its pane's place.
    session: Option<ask::Session>,
    /// A message in a box over the screen, and when it was said.
    message: Option<(String, Instant)>,
    /// The first key of a pair (y, g, ], [, z, C-w as w), and a count typed.
    pending: Option<char>,
    /// Where a selection of lines started (`v`, `V`), as a screen line of
    /// the diff, on the side with the focus; where one started in the list,
    /// as the file the cursor was on, kept only while the list has the
    /// focus; and whether `y` has just copied its text, so the key after it
    /// ends it.
    visual: Option<usize>,
    list_visual: Option<usize>,
    yanked: bool,
    /// What the mouse selected (select.rs), the rows of text last drawn,
    /// where its button was pressed, and the last press with how many in a
    /// row it made.
    selection: Option<select::Selection>,
    spots: Vec<select::Spot>,
    pressed: Option<(u16, u16)>,
    clicks: Option<(Instant, u16, u16, u8)>,
    /// The path:line of an answer the button was pressed on, gone to when
    /// it is let go with nothing selected, so a drag can select it.
    pressed_cite: Option<ask::Cited>,
    /// Where the diff's rows were last drawn from, how many rows that is
    /// from where they were first, and what a screen row adds to for its
    /// row's number.
    diff_drawn: Option<(usize, usize)>,
    diff_scrolled: i64,
    diff_base: i64,
    count: Option<usize>,
    spin: usize,
    hover: Option<Button>,
    /// Where each button was last drawn.
    buttons: Vec<(Button, Rect)>,
    /// Where the boxes were last drawn, for clicks outside them.
    ask_rect: Option<Rect>,
    answer_rect: Option<Rect>,
    /// The answer window's scroll bar where it was last drawn: its column,
    /// first row and height.
    answer_bar: Option<(u16, u16, usize)>,
    /// Escapes to write after the frame: the clipboard's.
    pub out: Vec<u8>,
    teller: Teller,
    /// The branch's commits one at a time, while `c` shows them; how many
    /// commits' diffs have been made, for their numbers.
    history: Option<History>,
    histories: u64,
    /// Set in a commit's diff of the history: what it asks of the diff it
    /// was opened from, and what its list is headed with (the commit and
    /// its subject).
    in_history: bool,
    wants: Option<Wants>,
    title: Option<(String, String)>,
    /// The colours the terminal said it draws in, as the diff started
    /// (src/mux/terminal.rs): its background, for the tints, and its text.
    term: switchboard_ui::terminal::Colors,
    /// Set while the screen is drawn, when no file is read.
    drawing: bool,
    /// The settings view while it is open, and the menu while it is up.
    settings: Option<own::View>,
    menu: Option<own::Open>,
    /// A release newer than this binary, as the day's look found, and
    /// whether the view's update is under way.
    newer: Option<crate::update::Newer>,
    updating: bool,
    /// An update was installed in this run, which offers no other.
    installed: bool,
    /// What the diff was opened on (the directory, the base named, whether
    /// the working tree's changes), to read it anew when a setting changes
    /// what is read.
    opened: Option<(PathBuf, Option<String>, bool)>,
    /// Whether the terminal draws a button's rounded ends.
    round: bool,
    /// The repository as the header names it (`~/...`), and the settings
    /// file as the settings view does.
    top_shown: String,
    settings_shown: String,
    settings_path: PathBuf,
    /// How many times a setting has had the change read anew, for the
    /// number of the latest.
    reloads: u64,
}

/// Whether `path` is left by the path filter `filter`: every word in it,
/// case aside, a word starting with `!` one it must not hold.
fn path_matches(filter: &str, path: &str) -> bool {
    let path = path.to_lowercase();
    filter
        .to_lowercase()
        .split_whitespace()
        .all(|word| match word.strip_prefix('!') {
            Some("") => true,
            Some(not) => !path.contains(not),
            None => path.contains(word),
        })
}

/// `fg` mixed over `bg`, `amount` of it.
fn blend(fg: u32, bg: u32, amount: f64) -> Color {
    let channel = |c: u32, shift: u32| ((c >> shift) & 0xff) as f64;
    let mix =
        |shift| (channel(fg, shift) * amount + channel(bg, shift) * (1.0 - amount)).round() as u8;
    Color::Rgb(mix(16), mix(8), mix(0))
}

impl App {
    pub fn new(opts: Options, cols: u16, rows: u16, tx: Sender<Msg>) -> App {
        App {
            opts,
            tx,
            cols: cols.max(20),
            rows: rows.max(5),
            loading: Some("Reading the change…".into()),
            failed: None,
            nothing: None,
            closed: false,
            change: None,
            totals: Totals::default(),
            left: Totals::default(),
            listed_files: 0,
            hunks: Default::default(),
            input: String::new(),
            context: None,
            reading: Reading::default(),
            order: Vec::new(),
            at: 0,
            path_filter: String::new(),
            filtering: false,
            list_top: 0,
            list_width: LIST_WIDTH,
            list_followed: None,
            adrift: false,
            said_end: false,
            bar_drag: None,
            focus: Focus::List,
            moved: false,
            files: HashMap::new(),
            texts: HashMap::new(),
            lines: HashMap::new(),
            cur: 0,
            col: 0,
            top: 0,
            view: 0,
            smart: HashMap::new(),
            story: None,
            told: None,
            story_key: None,
            tells: 0,
            refreshes: 0,
            asked_order: false,
            telling: Telling::Off,
            progress: None,
            cover: None,
            raw: false,
            page: None,
            whole_at: None,
            mode: Mode::Files,
            thread: 0,
            map_at: 1,
            read: HashSet::new(),
            things: None,
            thing: None,
            hits: Vec::new(),
            lit: None,
            open_on_up: None,
            also_on_up: None,
            files_place: None,
            map_page: None,
            map_place: None,
            ask_box: None,
            answer: None,
            asks: 0,
            session: None,
            message: None,
            pending: None,
            visual: None,
            list_visual: None,
            yanked: false,
            selection: None,
            spots: Vec::new(),
            pressed: None,
            clicks: None,
            pressed_cite: None,
            diff_drawn: None,
            diff_scrolled: 0,
            diff_base: 0,
            count: None,
            spin: 0,
            hover: None,
            buttons: Vec::new(),
            ask_rect: None,
            answer_rect: None,
            answer_bar: None,
            out: Vec::new(),
            teller: Teller::default(),
            history: None,
            histories: 0,
            in_history: false,
            wants: None,
            title: None,
            term: Default::default(),
            drawing: false,
            settings: None,
            menu: None,
            newer: None,
            updating: false,
            installed: false,
            opened: None,
            round: false,
            top_shown: String::new(),
            settings_shown: String::new(),
            settings_path: PathBuf::new(),
            reloads: 0,
        }
    }

    /// A file's text at a commit, read once (tests give it beforehand).
    pub fn give_text(&mut self, commit: &str, path: &str, text: Option<String>) {
        self.texts.insert((commit.into(), path.into()), text);
    }

    fn text(&mut self, commit: &str, path: &str) -> Option<String> {
        let key = (commit.to_string(), path.to_string());
        if let Some(t) = self.texts.get(&key) {
            return t.clone();
        }
        let top = self.change.as_ref()?.top.clone();
        let t = git::text_at(&top, commit, path);
        self.texts.insert(key, t.clone());
        t
    }

    /// Ends the work under way, as the program quits.
    pub fn stop(&mut self) {
        self.teller.stop();
        if let Some(a) = &self.answer {
            a.asking.stop();
        }
        if let Some(h) = &mut self.history {
            h.app.stop();
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.resize_now(cols, rows);
        self.read_view();
    }

    fn resize_now(&mut self, cols: u16, rows: u16) {
        // The header's row, alone, is none of the diff's.
        let rows = rows.saturating_sub(u16::from(self.has_header()));
        self.cols = cols.max(20);
        self.rows = rows.max(5);
        self.lines.clear();
        self.selection = None;
        if let Some(h) = &mut self.history {
            h.app.resize(cols, rows);
        }
    }

    fn say(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), Instant::now()));
    }

    /// Whether anything on the screen moves by itself (a spinner, a
    /// message's time, the cover's first moments).
    pub fn ticking(&self) -> bool {
        self.loading.is_some()
            || self.cover.is_some()
            || self.message.is_some()
            || self.telling == Telling::Waiting
            || self.answer.as_ref().is_some_and(|a| !a.done)
            || self.history.as_ref().is_some_and(|h| h.app.ticking())
    }

    /// The spinner's next frame, and a message gone once its time is up;
    /// whether the screen changed.
    pub fn tick(&mut self) -> bool {
        self.spin = (self.spin + 1) % SPINNER.len();
        let mut changed = false;
        if self
            .message
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() >= MESSAGE_FOR)
        {
            self.message = None;
            changed = true;
        }
        if let Some(h) = &mut self.history {
            changed |= h.app.tick();
        }
        self.ticking() || changed
    }

    fn spinner(&self) -> &'static str {
        SPINNER[self.spin % SPINNER.len()]
    }

    // ── The change ──────────────────────────────────────────────────────────

    pub fn event(&mut self, msg: Msg) {
        self.event_now(msg);
        self.read_view();
    }

    fn event_now(&mut self, msg: Msg) {
        match msg {
            // The latest reading a setting asked for, and no other.
            Msg::Reloaded(id, msg) => {
                if id == self.reloads {
                    self.event_apply(*msg);
                }
            }
            // The reading the diff opened with, once a setting has asked
            // for another, is of the setting before: dropped.
            Msg::Loading(_) | Msg::Loaded(_) | Msg::Nothing(_) if self.reloads > 0 => {}
            msg => self.event_apply(msg),
        }
    }

    fn event_apply(&mut self, msg: Msg) {
        // What lays the screen out again ends the mouse's selection, whose
        // rows would be other text.
        let same_rows = matches!(
            msg,
            Msg::Coloured { .. }
                | Msg::Asked(..)
                | Msg::History(..)
                | Msg::Heard(_, Heard::Progress(_))
                | Msg::Newer(_)
        );
        if !same_rows {
            self.selection = None;
        }
        match msg {
            Msg::Loading(text) => self.loading = Some(text),
            Msg::Loaded(Err(why)) => {
                self.loading = None;
                self.failed = Some(why);
            }
            Msg::Loaded(Ok((change, said))) => {
                self.loading = None;
                // Read anew after a setting changed what is read: what the
                // reading before said (nothing to show, or why it failed)
                // no longer holds.
                self.nothing = None;
                self.failed = None;
                if self.change.is_some() {
                    self.files.clear();
                    self.texts
                        .retain(|(commit, _), _| commit != git::INDEX && commit != git::WORK_TREE);
                    self.lines.clear();
                    self.visual = None;
                }
                // A question being typed stays about its file where the
                // change read anew still has it.
                let asked_about = self.ask_box.as_ref().and(self.current()).map(Entry::key);
                self.loaded(*change);
                if self.ask_box.is_some() {
                    let entries = self.entries();
                    let found = asked_about
                        .as_ref()
                        .and_then(|key| self.order.iter().position(|&i| entries[i].key() == *key));
                    match found {
                        // Its file is still there: the question keeps the
                        // place it was typed about, side and line.
                        Some(at) => {
                            self.moved = true;
                            self.show_at(at);
                        }
                        None => {
                            self.say(
                                "The file the question was about is no longer in the diff: it asks about the one shown now.",
                            );
                            let at = self.ask_cursor();
                            if let Some(b) = &mut self.ask_box {
                                b.at = at;
                            }
                        }
                    }
                }
                if let Some(said) = said {
                    self.say(said);
                }
            }
            Msg::Nothing(text) => self.nothing_to_show(text),
            Msg::Refreshed(id, read) if id == self.refreshes => match read {
                Ok(change) => self.refreshed(*change),
                Err(why) => self.say(format!("Couldn't read the change again: {why}")),
            },
            Msg::Refreshed(..) => {}
            // An answer to a story asked before the change was read again.
            Msg::Heard(id, _) if id != self.tells => {}
            Msg::Heard(_, Heard::Progress(p)) => self.progress = Some(p),
            Msg::Heard(_, Heard::Told(told)) => self.told(*told),
            Msg::Heard(_, Heard::Untold(why)) => {
                self.telling = Telling::Untold(why);
                self.cover = None;
                self.show_story(None);
                let _ = self.settle_mode();
                self.sort();
                self.ask_order();
            }
            Msg::Order(order) => {
                self.smart = order::places(&order);
                self.sort();
            }
            Msg::Coloured {
                path,
                new_side,
                lines,
            } => {
                if let Some(s) = self.files.get_mut(&path) {
                    if new_side {
                        s.new_hl = Some(lines);
                    } else {
                        s.old_hl = Some(lines);
                    }
                }
            }
            Msg::Asked(id, said) => self.asked(id, said),
            Msg::Reloaded(..) => {}
            // One found before the look was turned off is not said.
            Msg::Newer(newer) => {
                self.newer = newer.filter(|_| !self.installed && crate::update::turned_on());
            }
            Msg::Updated(outcome) => self.updated(outcome),
            Msg::History(id, msg) => {
                if let Some(h) = self.history.as_mut().filter(|h| h.id == id) {
                    h.app.event(*msg);
                }
            }
        }
    }

    /// The change is read: its files listed, the first one someone wrote
    /// shown, and its story asked for.
    pub fn loaded(&mut self, change: Change) {
        self.loaded_now(change);
        self.read_view();
    }

    fn loaded_now(&mut self, change: Change) {
        self.loading = None;
        self.read_input(&change);
        self.totals = Totals::of(&change.entries);
        // The header's words for the repository, worked out here rather
        // than as it draws.
        self.top_shown = util::tilde(&change.top);
        self.change = Some(change);
        self.order = (0..self.entries().len()).collect();
        self.show_story(None);
        self.sort();
        if self.opts.list_wide {
            self.list_width = self.fit_width();
        }
        self.pick_first();
        self.ask_story(true);
    }

    /// The story's context: the tab's plan note, with the diff_story_plan
    /// setting on and the diff in a pane of the workspace, asked of the
    /// server once.
    fn story_context(&mut self) -> String {
        if let Some(context) = &self.context {
            return context.clone();
        }
        let context = if self.opts.server {
            story::plan_context(None)
        } else {
            String::new()
        };
        self.context = Some(context.clone());
        context
    }

    /// What Haiku is given of the whole change for a question (`ga`), and
    /// its hunks, which a question's cursor is found in: the whole diff
    /// shown, what isn't committed yet included.
    fn read_input(&mut self, change: &Change) {
        let (diffs, hunks) = story::split(&change.runs());
        let commits = story::commits(&change.raw_commits);
        let context = self.story_context();
        self.input = story::input(&story_entries(change), &diffs, &hunks, &commits, &context);
        self.hunks = hunks;
    }

    /// The change has nothing to show, as pane-in-place.sh found before it
    /// opened the diff: in the workspace, its box says so and the diff
    /// closes, giving the pane its place back; elsewhere, or with no
    /// answer from the workspace, the diff's own screen says so.
    fn nothing_to_show(&mut self, text: String) {
        self.loading = None;
        // A reading before that failed no longer holds.
        self.failed = None;
        let said = self.opts.server
            && host()
                .command(&["say", "--", &text])
                .is_some_and(|(code, ..)| code == 0);
        if said {
            self.closed = true;
        } else {
            self.nothing = Some(text);
        }
    }

    /// Whether the diff has closed itself, its work done.
    pub fn closed(&self) -> bool {
        self.closed
    }

    /// The story the list shows: the one told (`told`), and, in a change
    /// running to the working tree with files not committed yet, those
    /// files in a last chapter of their own (`story::with_uncommitted`),
    /// the commits in a "Committed" chapter before it when they have no
    /// story, as diffview.lua's `show_story` showed them. A page the story
    /// no longer has goes back to the whole diff.
    fn show_story(&mut self, told: Option<Story>) {
        self.story = match &self.change {
            Some(change) if self.opts.story => {
                let (dirty, committed) = uncommitted_orders(change);
                if dirty.is_empty() {
                    told.clone()
                } else {
                    Some(story::with_uncommitted(told.as_ref(), &dirty, &committed))
                }
            }
            _ => told.clone(),
        };
        self.told = told;
        if !self.mapped() {
            self.thing = None;
            self.things = None;
        }
        if self
            .page
            .is_some_and(|p| self.story.as_ref().is_none_or(|s| p > s.chapters.len()))
        {
            self.page = None;
            self.whole_at = None;
            self.lines.clear();
        }
    }

    /// No map, and none coming (the change read again has no story, or
    /// one that is told as a list): the files as they were left before
    /// the map took their place. A map still being told keeps the mode.
    fn settle_mode(&mut self) -> bool {
        let falls = !self.mapped() && self.mode == Mode::Map && self.telling != Telling::Waiting;
        if falls {
            self.files_again();
        }
        falls
    }

    /// R: the change read again off the screen's thread, as it was read
    /// (the same base, or the working tree's changes), so edits made since
    /// it opened come in, as diffview's refresh read them. A commit of the
    /// history, which doesn't change, isn't.
    fn refresh(&mut self) {
        if self.in_history {
            return;
        }
        let Some(change) = &self.change else {
            return;
        };
        let (top, base) = (change.top.clone(), change.base.clone());
        let (working, to_work_tree) = (change.working(), change.to_work_tree());
        self.refreshes += 1;
        let id = self.refreshes;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let read = if working {
                git::read_working(&top)
            } else if to_work_tree {
                git::read_to_work_tree(&top, &base)
            } else {
                git::read_change(&top, &base)
            };
            let _ = tx.send(Msg::Refreshed(
                id,
                read.map(Box::new).map_err(|e| e.to_string()),
            ));
        });
    }

    /// The change read again: its files listed anew, the cursor kept on its
    /// file (the file now in its place when it is gone), and the story kept
    /// while its key is the same (the commits' own, for a change running to
    /// the working tree, so editing tells no new story), what isn't
    /// committed put in its last chapter again; else asked for, with no
    /// cover over a diff already open. What its base is to the branch is
    /// the open change's, since it is read against the same base.
    fn refreshed(&mut self, mut change: Change) {
        if let Some(open) = &self.change {
            change.why = open.why.clone();
        }
        let keep = self.current().map(Entry::key);
        let was = self.at;
        // The view's file by key, read before the change is replaced.
        let view = self
            .order
            .get(self.view)
            .and_then(|&i| self.entry(i))
            .map(Entry::key);
        // Each file's sides read again, the index and the files on disk
        // anew; a commit's text doesn't change.
        self.files.clear();
        self.texts
            .retain(|(commit, _), _| commit != git::INDEX && commit != git::WORK_TREE);
        self.lines.clear();
        self.visual = None;
        self.read_input(&change);
        let (key, _) = story::key_of(change.told_part());
        self.totals = Totals::of(&change.entries);
        // The header's words for the repository, worked out here rather
        // than as it draws.
        self.top_shown = util::tilde(&change.top);
        self.change = Some(change);
        self.order = (0..self.entries().len()).collect();
        if self.story_key.as_deref() == Some(key.as_str()) {
            let told = self.told.clone();
            self.show_story(told);
            // Haiku's order of the files listed now, when it was asked for
            // those listed before.
            if self.asked_order {
                self.ask_order();
            }
        } else {
            self.asked_order = false;
            // A story still being told of what the diff showed before is
            // of no use now: it ends, and this one's is asked for.
            if self.telling == Telling::Waiting {
                self.teller.stop();
                self.teller = Teller::default();
            }
            // The story to come is another one, whose chapters aren't
            // this one's by number: a chapter open from the map closes,
            // and the new story's map shows.
            if self.mode == Mode::Map {
                self.page = None;
                self.whole_at = None;
                self.thing = None;
                self.things = None;
            }
            self.forget_pages();
            self.show_story(None);
            self.ask_story(false);
            // The files where they were left: nothing of the chapter's
            // place is put back over them.
            if self.settle_mode() {
                return;
            }
        }
        self.sort();
        let entries = self.entries();
        let at = keep.and_then(|k| self.order.iter().position(|&i| entries[i].key() == k));
        match at {
            Some(at) => {
                self.at = at;
                self.load_shown();
            }
            None => self.show_at(was),
        }
        let entries = self.entries();
        self.view = view
            .and_then(|v| self.order.iter().position(|&i| entries[i].key() == v))
            .unwrap_or(self.at);
        self.fit_view();
    }

    /// The view's first line and the cursor kept within their files as
    /// they are laid out now, which reading the change again may shorten.
    fn fit_view(&mut self) {
        if !self.one_scroll() || self.order.is_empty() {
            return;
        }
        self.fix_view();
        let n = self.stream_len(self.view);
        self.top = self.top.min(n.saturating_sub(1));
        let n = self.screen_lines().len();
        self.cur = self.cur.min(n.saturating_sub(1));
    }

    fn entries(&self) -> &[Entry] {
        self.change.as_ref().map_or(&[], |c| c.entries.as_slice())
    }

    fn entry(&self, i: usize) -> Option<&Entry> {
        self.entries().get(i)
    }

    /// The file under the list's cursor.
    fn current(&self) -> Option<&Entry> {
        self.order.get(self.at).and_then(|&i| self.entry(i))
    }

    /// The files the list may show: the page's, else all of them.
    fn listable(&self) -> Vec<usize> {
        let entries = self.entries();
        let page: Option<HashSet<&str>> = self.page.and_then(|p| {
            self.story
                .as_ref()
                .and_then(|s| s.chapters.get(p - 1))
                .map(|c| c.touches.iter().map(String::as_str).collect())
        });
        // A file listed twice (staged, and changed since) is on the page
        // by the diff its hunks there are from; one with no hunk of the
        // story's (binary, renamed) by its path alone.
        let of_page = |entry: &Entry| -> bool {
            // A story told as a list lists a page's files by their path,
            // as it did.
            if !self.mapped() {
                return true;
            }
            let (Some(page), Some(story), Some(run)) = (self.page, &self.story, self.run_of(entry))
            else {
                return true;
            };
            let hunks: Vec<&story::HunkNote> = story
                .hunks
                .get(&entry.path)
                .map(Vec::as_slice)
                .unwrap_or(&[])
                .iter()
                .filter(|h| {
                    h.chapter == page || story.repeats.get(&h.id).is_some_and(|c| c.contains(&page))
                })
                .collect();
            // An entry whose diff has no hunk of the file's at all (a
            // staged rename beside an unstaged edit) by its path.
            let own_run = story
                .hunks
                .get(&entry.path)
                .is_some_and(|all| all.iter().any(|h| h.run == run));
            hunks.is_empty() || !own_run || hunks.iter().any(|h| h.run == run)
        };
        (0..entries.len())
            .filter(|&i| {
                page.as_ref()
                    .is_none_or(|p| p.contains(entries[i].path.as_str()) && of_page(&entries[i]))
            })
            .collect()
    }

    /// The files in the order known now, those the path filter leaves,
    /// the cursor kept on its file.
    fn sort(&mut self) {
        // The list's selection is by its files' places, which change here.
        self.list_visual = None;
        let keep = self.current().map(Entry::key);
        let view = self
            .order
            .get(self.view)
            .and_then(|&i| self.entry(i))
            .map(Entry::key);
        let listable = self.listable();
        self.listed_files = Totals::of(listable.iter().map(|&i| &self.entries()[i])).files;
        let mut order: Vec<usize> = listable
            .into_iter()
            .filter(|&i| path_matches(&self.path_filter, &self.entries()[i].path))
            .collect();
        let entries = self.entries().to_vec();
        let story = self.story.as_ref();
        let smart = &self.smart;
        let by_order = self.opts.order;
        let apart = self.not_committed_apart();
        order.sort_by(|&a, &b| {
            let (pa, pb) = (&entries[a].path, &entries[b].path);
            // Generated files stay the order's last, after what isn't
            // committed yet, so they keep one GENERATED, LAST label; with
            // a story, nothing is apart and its chapters place them.
            let generated = |p: &String| !apart.is_empty() && by_order && order::generated(p);
            let last = |p: &String| apart.contains(p.as_str());
            entries[a]
                .section
                .cmp(&entries[b].section)
                .then_with(|| generated(pa).cmp(&generated(pb)))
                .then_with(|| last(pa).cmp(&last(pb)))
                .then_with(|| {
                    if by_order {
                        order::reading_order(pa, pb, story, smart)
                    } else {
                        order::alphabetical(pa, pb)
                    }
                })
        });
        self.left = Totals::of(order.iter().map(|&i| &entries[i]));
        self.order = order;
        if let Some(keep) = keep
            && let Some(at) = self.order.iter().position(|&i| entries[i].key() == keep)
        {
            self.at = at;
        }
        self.at = self.at.min(self.order.len().saturating_sub(1));
        // The view stays on its file where that file still is, else on
        // the shown one; fix_view puts one now after it back on it.
        self.view = view
            .and_then(|v| self.order.iter().position(|&i| entries[i].key() == v))
            .unwrap_or(self.at);
    }

    /// The first file someone wrote, else the first.
    fn pick_first(&mut self) {
        let first = self
            .order
            .iter()
            .position(|&i| !order::generated(&self.entries()[i].path))
            .unwrap_or(0);
        self.show_at(first);
    }

    /// The width that fits the list's longest path, the status and counts
    /// beside it, at most half the screen.
    fn fit_width(&self) -> u16 {
        let longest = self
            .entries()
            .iter()
            .map(|e| width(&e.path))
            .max()
            .unwrap_or(0) as u16;
        LIST_WIDTH.max((longest + 14).min(self.cols / 2))
    }

    /// The story asked for, under the cover (with `cover`), unless there is
    /// none to tell: two files of someone's at least, since generated files
    /// go last whatever is said. In a change running to the working tree it
    /// is its commits' story, the one told ahead, found under the same key;
    /// with nothing committed there is none to wait for, only the files not
    /// committed yet in their chapter.
    fn ask_story(&mut self, cover: bool) {
        let context = self.story_context();
        let Some(change) = &self.change else {
            return;
        };
        let told = change.told_part();
        let (key, paths) = story::key_of(told);
        let entries = story_entries(told);
        let (diffs, hunks) = story::split(&told.runs());
        let input = if change.to_work_tree() {
            let commits = story::commits(&told.raw_commits);
            story::input(&entries, &diffs, &hunks, &commits, &context)
        } else {
            self.input.clone()
        };
        let nothing_committed = change.to_work_tree() && told.entries.is_empty();
        let conflict = change.entries.iter().any(|e| e.status == 'U');
        // What Haiku is given, for the cover's first line.
        self.reading = Reading {
            files: entries.len(),
            additions: entries.iter().map(|e| e.additions).sum(),
            deletions: entries.iter().map(|e| e.deletions).sum(),
            estimate: Some(story::estimate(story::count(&hunks))),
        };
        self.story_key = Some(key.clone());
        // A new number, so an answer to a story asked before goes unheard.
        self.tells += 1;
        let id = self.tells;
        if !self.opts.story {
            self.ask_order();
            return;
        }
        if nothing_committed {
            self.telling = Telling::Untold("nothing is committed yet".into());
            return;
        }
        let written = paths.iter().filter(|p| !order::generated(p)).count();
        if paths.len() < 2 || written == 0 {
            self.telling = Telling::Untold(if paths.len() < 2 {
                "it changes only one file".into()
            } else {
                "it changes only generated files".into()
            });
            return;
        }
        // A file still in conflict has no story, its markers no change of
        // anyone's to tell (diffview.lua's guard).
        if conflict {
            self.telling = Telling::Untold("a merge conflict is unresolved".into());
            return;
        }
        if !self.opts.haiku {
            self.telling = Telling::Untold("Haiku isn't asked here".into());
            return;
        }
        if tell::claude_command().is_none() {
            self.telling = Telling::Untold("claude isn't installed".into());
            return;
        }
        self.telling = Telling::Waiting;
        if cover {
            self.cover = Some(Instant::now());
        }
        let asked = Asked {
            key,
            input,
            paths,
            hunks,
        };
        let teller = self.teller.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            teller.tell(&asked, &|heard| {
                let _ = tx.send(Msg::Heard(id, heard));
            });
        });
    }

    /// Haiku's order of the code by its paths, when no story is told.
    fn ask_order(&mut self) {
        if !self.opts.order || !self.opts.haiku {
            return;
        }
        let Some(change) = &self.change else {
            return;
        };
        let files = order::ordered(change);
        if files.is_empty() {
            return;
        }
        self.asked_order = true;
        let (left, right) = change.order_sides();
        let (top, left, right) = (change.top.clone(), left.to_string(), right.to_string());
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            if let Some(order) = order::ask_order(&top, &left, &right, &files) {
                let _ = tx.send(Msg::Order(order));
            }
        });
    }

    /// The story is told: the list in its order, headed by its chapters.
    pub fn told(&mut self, story: Story) {
        self.told_now(story);
        self.read_view();
    }

    fn told_now(&mut self, story: Story) {
        if let Some(missed) = story::missed_text(&story) {
            self.say(format!("{missed}."));
        }
        self.show_story(Some(story));
        self.telling = Telling::Told;
        self.cover = None;
        self.enter_story();
        self.sort();
        if !self.moved {
            self.pick_first();
        }
        self.lines.clear();
    }

    // ── The file shown ──────────────────────────────────────────────────────

    /// Shows the list's file at `at`, opened at its first change.
    fn show_at(&mut self, at: usize) {
        self.visual = None;
        self.adrift = false;
        if self.order.is_empty() {
            return;
        }
        self.at = at.min(self.order.len() - 1);
        self.view = self.at;
        self.load_shown();
        self.first_change();
    }

    /// The shown file now the list's file at `at`, the cursor on its line
    /// `cur`, the view where it was: a change, a click or the scroll bar's
    /// end taking the cursor from one file into another while UX 2.0 reads
    /// them as one scroll.
    fn cross_to(&mut self, at: usize, cur: usize) {
        self.visual = None;
        self.adrift = false;
        self.at = at;
        self.load_shown();
        self.cur = cur;
        self.col = 0;
        self.moved = true;
        self.keep_side();
    }

    /// The side with the focus kept where the shown file has it, else its
    /// other: an added file has no old side, a deleted one no new.
    fn keep_side(&mut self) {
        let (ow, nw) = self.side_widths();
        self.focus = match self.focus {
            Focus::New if nw == 0 => Focus::Old,
            Focus::Old if ow == 0 => Focus::New,
            focus => focus,
        };
    }

    /// The files the view reaches read, before it is drawn: the scroll is
    /// drawn only as far as the files already read, so drawing never runs
    /// git, which a key or the wheel moving the view does here, as moving
    /// to a file always read the shown one.
    fn read_view(&mut self) {
        if !self.one_scroll() || self.order.is_empty() {
            return;
        }
        // The cursor in the view first, reading the files between them,
        // then the files the view reaches.
        self.fit_view();
        self.keep_cursor();
        let h = self.body_height();
        self.rows_left(self.view, self.top, h);
    }

    /// Whether the list's file `k` has its sides read.
    fn is_read(&self, k: usize) -> bool {
        self.order
            .get(k)
            .and_then(|&i| self.entry(i))
            .is_some_and(|e| self.files.contains_key(&e.key()))
    }

    /// Reads the shown file's two sides, once, and asks for their colours.
    fn load_shown(&mut self) {
        self.load(self.at);
    }

    /// Reads the two sides of the list's file at `k`, once, and asks for
    /// their colours.
    fn load(&mut self, k: usize) {
        if self.drawing {
            return;
        }
        let Some(entry) = self.order.get(k).and_then(|&i| self.entry(i)).cloned() else {
            return;
        };
        if self.files.contains_key(&entry.key()) {
            return;
        }
        let Some(change) = &self.change else {
            return;
        };
        let (left, right) = change.sides(&entry);
        let old_text = if entry.status == 'A' {
            Some(String::new())
        } else {
            self.text(&left, entry.old())
        };
        let new_text = if entry.status == 'D' {
            Some(String::new())
        } else {
            self.text(&right, &entry.path)
        };
        let binary = entry.binary || old_text.is_none() || new_text.is_none();
        let old = sides::lines_of(&old_text.unwrap_or_default());
        let new = sides::lines_of(&new_text.unwrap_or_default());
        let alone = match entry.status {
            'A' => Some(true),
            'D' => Some(false),
            _ => None,
        };
        let rows = match alone {
            Some(true) => sides::alone(&new, true),
            Some(false) => sides::alone(&old, false),
            None => sides::align(&old, &new),
        };
        for (new_side, lines) in [(false, old.clone()), (true, new.clone())] {
            if lines.is_empty() {
                continue;
            }
            let tx = self.tx.clone();
            let path = entry.key();
            let named = if new_side {
                entry.path.clone()
            } else {
                entry.old().to_string()
            };
            std::thread::spawn(move || {
                if let Some(lines) = highlight::highlight(&named, &lines) {
                    let _ = tx.send(Msg::Coloured {
                        path,
                        new_side,
                        lines,
                    });
                }
            });
        }
        self.files.insert(
            entry.key(),
            Sides {
                old,
                new,
                rows,
                alone,
                binary,
                old_hl: None,
                new_hl: None,
                opened: HashSet::new(),
                all_open: false,
            },
        );
    }

    /// The diff area's columns, and each side's.
    fn diff_area(&self) -> Rect {
        let x = self.list_width.min(self.cols.saturating_sub(10)) + 1;
        let bottom = u16::from(self.has_buttons() || self.has_footer());
        Rect::new(
            x,
            0,
            self.cols.saturating_sub(x),
            self.rows.saturating_sub(bottom),
        )
    }

    /// Whether the screen's last row is the footer of keys (UX 2.0, as
    /// board 12 draws it): not under the row of buttons (easy mode's, and
    /// the diff alone's), which is there instead, nor on a screen too short
    /// to spare it.
    pub(super) fn has_footer(&self) -> bool {
        self.opts.ux2 && !self.has_buttons() && self.rows >= 5
    }

    /// The columns of the old side and the new; one of them 0 for a file
    /// shown alone.
    fn side_widths(&self) -> (u16, u16) {
        match self.current() {
            Some(entry) => self.widths_of(entry),
            None => self.widths_of_alone(None),
        }
    }

    /// The columns of a file's old side and its new.
    fn widths_of(&self, entry: &Entry) -> (u16, u16) {
        self.widths_of_alone(self.files.get(&entry.key()).and_then(|s| s.alone))
    }

    fn widths_of_alone(&self, alone: Option<bool>) -> (u16, u16) {
        let w =
            self.diff_area().width - u16::from(self.opts.scroll_bars).min(self.diff_area().width);
        match alone {
            Some(true) => (0, w),
            Some(false) => (w, 0),
            None => {
                let old = w.saturating_sub(1) / 2;
                (old, w.saturating_sub(old + 1))
            }
        }
    }

    /// The rows over the diff's text: the chapter's heading with UX 2.0,
    /// then the bar.
    fn head_height(&self) -> usize {
        self.chapter_head(self.diff_area().width as usize).len() + 1
    }

    /// The rows the diff's text takes, below the chapter's heading and the
    /// bar.
    fn body_height(&self) -> usize {
        (self.diff_area().height as usize).saturating_sub(self.head_height())
    }

    /// Whether the files read as one scroll, each following the last (UX
    /// 2.0).
    fn one_scroll(&self) -> bool {
        self.opts.ux2
    }

    /// The rows of the list's file `k` in the scroll: its lines, and while
    /// the files read as one, a rule and the next file's bar after them.
    fn stream_len(&mut self, k: usize) -> usize {
        let n = self.lines_of(k).len();
        if self.one_scroll() && k + 1 < self.order.len() {
            n + 2
        } else {
            n
        }
    }

    /// The place `by` rows on from row `top` of file `k` in the scroll,
    /// stopping at the last file's last row.
    fn stream_ahead(&mut self, mut k: usize, mut top: usize, mut by: usize) -> (usize, usize) {
        loop {
            let n = self.stream_len(k);
            if top + by < n {
                return (k, top + by);
            }
            if k + 1 >= self.order.len() {
                return (k, n.saturating_sub(1));
            }
            by -= n.saturating_sub(top);
            k += 1;
            top = 0;
        }
    }

    /// The place `by` rows back from row `top` of file `k` in the scroll,
    /// stopping at the first file's first row.
    fn stream_back(&mut self, mut k: usize, mut top: usize, mut by: usize) -> (usize, usize) {
        loop {
            if top >= by {
                return (k, top - by);
            }
            if k == 0 {
                return (0, 0);
            }
            by -= top + 1;
            k -= 1;
            top = self.stream_len(k).saturating_sub(1);
        }
    }

    /// How many rows the scroll's place (`k`, `top`) is after the view's
    /// first; `None` when it is before it.
    fn rows_from_view(&mut self, k: usize, top: usize) -> Option<usize> {
        if (k, top) < (self.view, self.top) {
            return None;
        }
        let mut rows = 0;
        let mut at = self.view;
        let mut from = self.top;
        while at < k {
            rows += self.stream_len(at).saturating_sub(from);
            at += 1;
            from = 0;
        }
        Some(rows + top - from)
    }

    /// Which of the story's runs a file's changes are in: a working tree's
    /// staged changes are its first, those not staged its second; a
    /// branch's change has one, and every hunk counts.
    fn run_of(&self, entry: &Entry) -> Option<usize> {
        match entry.section {
            Section::Committed => None,
            Section::Staged => Some(1),
            Section::Unstaged => Some(2),
        }
    }

    /// Whether a file has changes not yet committed, in a change running to
    /// the working tree: marked ● in the list and "uncommitted" in its bar.
    fn changed_since(&self, entry: &Entry) -> bool {
        self.change.as_ref().is_some_and(|c| c.dirty(&entry.path))
    }

    /// The chapter a file is shown under: the page's, else its own.
    fn home(&self, path: &str) -> Option<usize> {
        self.page.or_else(|| {
            self.story
                .as_ref()
                .and_then(|s| s.chapter.get(path).copied())
        })
    }

    /// The shown file's screen lines, laid out for the sides' widths.
    fn screen_lines(&mut self) -> &[Line] {
        self.lines_of(self.at)
    }

    /// The screen lines of the list's file `k`, read first if it hasn't
    /// been, laid out for its sides' widths.
    fn lines_of(&mut self, k: usize) -> &[Line] {
        self.load(k);
        let Some(entry) = self.order.get(k).and_then(|&i| self.entry(i)).cloned() else {
            return &[];
        };
        let key = entry.key();
        // Not laid out until read, so nothing empty is kept for it.
        if !self.files.contains_key(&key) {
            return &[];
        }
        let (ow, nw) = self.widths_of(&entry);
        let fresh = self
            .lines
            .get(&key)
            .is_some_and(|(o, n, _)| *o == ow && *n == nw);
        if !fresh {
            let lines = self.build_lines(&entry, ow as usize, nw as usize);
            self.lines.insert(key.clone(), (ow, nw, lines));
        }
        &self.lines[&key].2
    }

    fn build_lines(&self, entry: &Entry, ow: usize, nw: usize) -> Vec<Line> {
        let Some(s) = self.files.get(&entry.key()) else {
            return Vec::new();
        };
        // A file changed since its commits has its hunks at lines the
        // working tree may have moved: one it shows from another chapter
        // is said at the file's head, where it can still be gone to.
        let mut head: Vec<Line> = Vec::new();
        if self.opts.notes
            && self.changed_since(entry)
            && let Some(story) = &self.story
        {
            for home in self.also_any(&entry.path, self.run_of(entry)) {
                head.push(self.also_line(story, home, None));
            }
        }
        // In the one scroll a binary file says so in its place.
        if self.opts.ux2 && s.binary {
            let faint = Tone::Faint.style(&self.theme());
            head.push(Line::Card(vec![Span::styled(
                " A binary file: no text to compare.",
                faint,
            )]));
            return head;
        }
        // An empty file, added or deleted, gets a line saying so, for the
        // cursor and ]c to stop on.
        if self.opts.ux2 && s.rows.is_empty() {
            let faint = Tone::Faint.style(&self.theme());
            head.push(Line::Card(vec![Span::styled(" An empty file.", faint)]));
            return head;
        }
        // Each note goes over the line its hunk starts at, on the side it
        // changes: the new, or the old for lines only taken out. A file
        // changed since its commits has none: its story's hunks are at the
        // committed lines, which the working tree may have moved. With UX
        // 2.0 each is a card under the hunk's last line instead.
        let mut notes: HashMap<(bool, usize), Vec<Line>> = HashMap::new();
        // Each card with the last line of each side its hunk has, both
        // to be passed before the card goes in.
        let mut cards: Vec<WaitingCard> = Vec::new();
        if self.opts.notes
            && !self.changed_since(entry)
            && let Some(story) = &self.story
            && let Some(home) = self.home(&entry.path)
        {
            let run = self.run_of(entry);
            for hunk in story
                .hunks
                .get(&entry.path)
                .map(Vec::as_slice)
                .unwrap_or(&[])
                .iter()
                .filter(|h| run.is_none_or(|r| h.run == r))
            {
                let new_side = match s.alone {
                    Some(side) => side,
                    None => hunk.new_count != 0,
                };
                let (first, count) = if new_side {
                    (hunk.new_start, hunk.new_count)
                } else {
                    (hunk.old_start, hunk.old_count)
                };
                // A hunk shown here from another chapter, which this one
                // can't be read without: a line over it saying where its
                // home is, its note left there.
                if hunk.chapter != home
                    && story
                        .repeats
                        .get(&hunk.id)
                        .is_some_and(|shown| shown.contains(&home))
                {
                    // Over the hunk's earliest row as drawn, on either side.
                    let within = |line: Option<usize>, start: usize, span: usize| {
                        line.is_some_and(|l| l + 1 >= start && l + 1 < start + span.max(1))
                    };
                    let row = s.rows.iter().find(|r| {
                        within(r.old, hunk.old_start, hunk.old_count)
                            || within(r.new, hunk.new_start, hunk.new_count)
                    });
                    let key = match row {
                        Some(r) => match (r.old, r.new) {
                            (Some(o), _) => (false, o),
                            (None, Some(n)) => (true, n),
                            (None, None) => (new_side, first.max(1) - 1),
                        },
                        None => {
                            let line = (if count == 0 { first + 1 } else { first }).max(1);
                            (new_side, line - 1)
                        }
                    };
                    notes.entry(key).or_default().push(self.also_line(
                        story,
                        hunk.chapter,
                        Some(hunk.id.clone()),
                    ));
                    continue;
                }
                if self.opts.ux2 {
                    // A card under the hunk, after the last line of each side
                    // it has, which a replacement can leave on different rows.
                    let mut ends = Vec::new();
                    for (side, start, span) in [
                        (false, hunk.old_start, hunk.old_count),
                        (true, hunk.new_start, hunk.new_count),
                    ] {
                        let shown = match s.alone {
                            Some(alone) => alone == side,
                            None => true,
                        };
                        if shown && span > 0 {
                            ends.push((side, (start + span - 1).max(1) - 1));
                        }
                    }
                    if ends.is_empty() {
                        ends.push((new_side, (first.max(1)) - 1));
                    }
                    if let Some(card) = self.note_card(story, hunk, home, ow, nw) {
                        cards.push((ends, card));
                    }
                    continue;
                }
                let line = (if count == 0 { first + 1 } else { first }).max(1);
                let cols = if new_side { nw } else { ow }.max(10);
                if let Some(text) = story::note_lines(story, hunk, home, cols) {
                    notes
                        .entry((new_side, line - 1))
                        .or_default()
                        .extend(text.into_iter().map(|text| Line::Note { new_side, text }));
                }
            }
        }
        let mut out = Vec::new();
        out.append(&mut head);
        let put_notes = |out: &mut Vec<Line>,
                         key: (bool, usize),
                         notes: &mut HashMap<(bool, usize), Vec<Line>>| {
            if let Some(lines) = notes.remove(&key) {
                out.extend(lines);
            }
        };
        let put_cards = |out: &mut Vec<Line>, row: &sides::Row, cards: &mut Vec<WaitingCard>| {
            let passed = [row.old.map(|o| (false, o)), row.new.map(|n| (true, n))];
            for (ends, _) in cards.iter_mut() {
                ends.retain(|e| !passed.contains(&Some(*e)));
            }
            let (done, waiting): (Vec<_>, Vec<_>) = std::mem::take(cards)
                .into_iter()
                .partition(|(ends, _)| ends.is_empty());
            *cards = waiting;
            for (_, lines) in done {
                out.extend(lines);
            }
        };
        for item in sides::items(&s.rows, &s.opened, s.all_open || s.alone.is_some()) {
            match item {
                Item::Fold { first, count } => {
                    for row in &s.rows[first..first + count] {
                        if let Some(o) = row.old {
                            put_notes(&mut out, (false, o), &mut notes);
                        }
                        if let Some(n) = row.new {
                            put_notes(&mut out, (true, n), &mut notes);
                        }
                    }
                    out.push(Line::Fold { first, count });
                    for row in &s.rows[first..first + count] {
                        put_cards(&mut out, row, &mut cards);
                    }
                }
                Item::Row(r) => {
                    let row = &s.rows[r];
                    if let Some(o) = row.old {
                        put_notes(&mut out, (false, o), &mut notes);
                    }
                    if let Some(n) = row.new {
                        put_notes(&mut out, (true, n), &mut notes);
                    }
                    let old_pieces = row
                        .old
                        .map(|o| sides::wrap(&s.old[o], (ow - s.gutter(ow)).max(1)))
                        .unwrap_or_default();
                    let new_pieces = row
                        .new
                        .map(|n| sides::wrap(&s.new[n], (nw - s.gutter(nw)).max(1)))
                        .unwrap_or_default();
                    let tall = match s.alone {
                        Some(true) => new_pieces.len(),
                        Some(false) => old_pieces.len(),
                        None => old_pieces.len().max(new_pieces.len()),
                    }
                    .max(1);
                    for k in 0..tall {
                        let cell = |line: Option<usize>, pieces: &[Range<usize>]| match (
                            line,
                            pieces.get(k),
                        ) {
                            (Some(line), Some(range)) => Cell::Text {
                                line,
                                range: range.clone(),
                            },
                            _ => Cell::Gap,
                        };
                        out.push(Line::Pair {
                            row: r,
                            old: cell(row.old, &old_pieces),
                            new: cell(row.new, &new_pieces),
                        });
                    }
                    put_cards(&mut out, row, &mut cards);
                }
            }
        }
        // A card whose line the file doesn't reach goes at its end.
        for (_, lines) in cards {
            out.extend(lines);
        }
        out
    }

    /// The kit's colors for the theme the diff is drawn in.
    fn theme(&self) -> switchboard_ui::Theme {
        if self.opts.light {
            switchboard_ui::Theme::LIGHT
        } else {
            switchboard_ui::Theme::DARK
        }
    }

    /// A hunk's note as UX 2.0 draws it: a card across both sides, titled
    /// "✎ Note", or with the chapter's number and title for a hunk of
    /// another chapter than `home`; `None` for a hunk of its home chapter
    /// with no note.
    fn note_card(
        &self,
        story: &Story,
        hunk: &story::HunkNote,
        home: usize,
        ow: usize,
        nw: usize,
    ) -> Option<Vec<Line>> {
        let other = (hunk.chapter != home)
            .then(|| story.chapters.get(hunk.chapter.wrapping_sub(1)))
            .flatten();
        if hunk.note.is_empty() && other.is_none() {
            return None;
        }
        let theme = self.theme();
        let title = match other {
            // Numbered among the chapters told, as the headings count them;
            // the generated files' and what isn't committed by name alone.
            Some(chapter) if hunk.chapter <= self.told_chapters() => vec![
                Span::styled(
                    format!("{}/{}", hunk.chapter, self.told_chapters()),
                    Tone::Accent.bold(&theme),
                ),
                Span::styled(format!(" {}", chapter.title), Tone::Text.bold(&theme)),
            ],
            Some(chapter) => vec![Span::styled(chapter.title.clone(), Tone::Text.bold(&theme))],
            None => vec![
                kit::mark(kit::Mark::Note, 0, &theme),
                Span::styled(" Note", Tone::Text.bold(&theme)),
            ],
        };
        let cols = if ow > 0 && nw > 0 {
            ow + nw + 1
        } else {
            ow + nw
        };
        let inside: Vec<Vec<Span<'static>>> =
            switchboard_ui::draw::wrap(&hunk.note, cols.saturating_sub(4).max(10))
                .into_iter()
                .filter(|l| !l.is_empty())
                .map(|l| vec![Span::styled(l, Tone::Text.style(&theme))])
                .collect();
        let edge = kit::card_edge(false, None, &theme);
        let rows = kit::card(cols, inside.len() + 2, title, Vec::new(), &inside, edge);
        Some(rows.into_iter().map(Line::Card).collect())
    }

    /// Opens the shown file at its first change, the cursor on the first
    /// word that changed, in the middle of the screen.
    fn first_change(&mut self) {
        let height = self.body_height();
        let alone = self
            .current()
            .and_then(|e| self.files.get(&e.key()))
            .and_then(|s| s.alone);
        let rows_changed: Vec<bool> = self
            .current()
            .and_then(|e| self.files.get(&e.key()))
            .map(|s| s.rows.iter().map(|r| r.changes()).collect())
            .unwrap_or_default();
        let lines = self.screen_lines().to_vec();
        self.cur = 0;
        self.col = 0;
        self.top = 0;
        if alone.is_some() {
            return;
        }
        let found = lines.iter().position(
            |l| matches!(l, Line::Pair { row, .. } if rows_changed.get(*row) == Some(&true)),
        );
        if let Some(i) = found {
            self.cur = i;
            if let (
                Line::Pair {
                    row,
                    new: Cell::Text { range, .. },
                    ..
                },
                Some(entry),
            ) = (&lines[i], self.current())
                && let Some(s) = self.files.get(&entry.key())
                && let Some(w) = s.rows[*row].new_words.first()
                && w.start >= range.start
                && w.start <= range.end
            {
                let line = &s.new[s.rows[*row].new.unwrap_or(0)];
                self.col = width(&line[range.start..w.start]);
            }
            self.top = i.saturating_sub(height / 2);
        }
    }

    /// The line number (from 1) under the cursor on a side, the nearest
    /// line above (else below) where the side has none there.
    fn line_under(&mut self, new_side: bool) -> Option<usize> {
        self.line_at(new_side, self.cur)
    }

    /// The line number (from 1) at a screen line of the diff, on a side,
    /// as `line_under` finds it.
    fn line_at(&mut self, new_side: bool, cur: usize) -> Option<usize> {
        let lines = self.screen_lines().to_vec();
        let on = |l: &Line| match l {
            Line::Pair { old, new, .. } => match if new_side { new } else { old } {
                Cell::Text { line, .. } => Some(line + 1),
                Cell::Gap => None,
            },
            _ => None,
        };
        lines
            .get(..=cur.min(lines.len().saturating_sub(1)))
            .and_then(|before| before.iter().rev().find_map(on))
            .or_else(|| lines.iter().skip(cur).find_map(on))
    }

    // ── Keys ────────────────────────────────────────────────────────────────

    /// A key pressed, the files the view then reaches read (`read_view`),
    /// as are those after the mouse, an event or a resize.
    pub fn key(&mut self, key: KeyEvent) -> Act {
        let act = self.key_now(key);
        self.read_view();
        act
    }

    fn key_now(&mut self, key: KeyEvent) -> Act {
        if key.kind == KeyEventKind::Release {
            return Act::Same;
        }
        // A message closes on the next key, which goes on to do what it
        // does, as vim.notify's never held one up; Esc only closes it.
        let closed = self.history.is_none() && self.message.take().is_some();
        // The list's selection is gone once the focus has left the list,
        // however it left.
        if self.focus != Focus::List {
            self.list_visual = None;
        }
        // What the mouse selected: Esc clears it, y copies it again (and
        // goes on, so yp and yl still copy the place), any other key ends it.
        if self.selection.is_some() {
            match key.code {
                KeyCode::Esc => {
                    // With the y that copied it, were it the key before.
                    self.selection = None;
                    self.pending = None;
                    self.count = None;
                    self.yanked = false;
                    return Act::Redraw;
                }
                KeyCode::Char('y') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.copy_selection();
                    self.yanked = true;
                }
                _ => self.selection = None,
            }
        }
        if closed && key.code == KeyCode::Esc {
            return Act::Redraw;
        }
        let act = self.key_on(key);
        // A y that copied waits for its second key, or it was no pair's
        // first (typed into a box).
        if self.pending.is_none() {
            self.yanked = false;
        }
        match act {
            Act::Same if closed => Act::Redraw,
            act => act,
        }
    }

    fn key_on(&mut self, key: KeyEvent) -> Act {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // M-m, alone: the settings view, opened or closed. In the workspace
        // the key is the workspace's, and never taken here.
        let alt_m = key.modifiers == KeyModifiers::ALT && key.code == KeyCode::Char('m');
        // With nothing to show, or a change that couldn't be read, too;
        // over a question being typed, whose draft waits under the view.
        if alt_m && self.opts.alone && !self.in_history {
            self.toggle_settings();
            return Act::Redraw;
        }
        let reset = ctrl && matches!(key.code, KeyCode::Char('\\') | KeyCode::Char('4'));
        if (self.menu.is_some() || self.settings.is_some()) && reset {
            // C-\ C-n, which the palette's commands press first: the menu
            // and the settings view close, so the keys after reach the diff.
            self.menu = None;
            self.settings = None;
        } else if self.menu.is_some() || self.settings.is_some() {
            if ctrl && key.code == KeyCode::Char('c') {
                return Act::Quit;
            }
            return if self.menu.is_some() {
                self.menu_key(key)
            } else {
                self.settings_key(key)
            };
        }
        // A commit of the history has the keys, but C-c, which closes the
        // whole diff.
        if let Some(h) = &mut self.history {
            if ctrl && key.code == KeyCode::Char('c') {
                return Act::Quit;
            }
            let act = h.app.key(key);
            return self.after_history(act);
        }
        if ctrl && key.code == KeyCode::Char('c') && self.ask_box.is_none() {
            return Act::Quit;
        }
        // C-\ C-n, which the palette's commands press first: back to the
        // diff's own keys from wherever typing is.
        if ctrl && matches!(key.code, KeyCode::Char('\\') | KeyCode::Char('4')) {
            self.ask_box = None;
            self.pending = None;
            self.message = None;
            self.filtering = false;
            self.menu = None;
            return Act::Redraw;
        }
        if ctrl && key.code == KeyCode::Char('n') && self.ask_box.is_none() {
            return Act::Same;
        }
        // A refreshed map still being told moves no focus among files out
        // of sight.
        if alt_arrow(&key).is_some()
            && self.mode == Mode::Map
            && !self.mapped()
            && self.telling == Telling::Waiting
        {
            return Act::Same;
        }
        if let Some(dir) = alt_arrow(&key) {
            // Moving between windows ends typing the path filter, kept.
            self.filtering = false;
            // With pane_arrows off the workspace passes them on unasked, and
            // they do nothing here, as nvim/diff.lua fed them back.
            if self.opts.pane_arrows {
                let windows = self.loading.is_none()
                    && self.failed.is_none()
                    && self.ask_box.is_none()
                    && self.answer.is_none()
                    && !(self.cover.is_some() && self.telling == Telling::Waiting);
                if windows {
                    self.window_move(dir);
                } else {
                    self.beside(dir);
                }
            }
            return Act::Redraw;
        }
        // With nothing to show, alone, the menu's keys open the settings,
        // the one thing of its menu that works there: a setting may be why.
        let menu_key = (key.code == KeyCode::Char('m') && key.modifiers.is_empty())
            || (key.code == KeyCode::F(10) && key.modifiers == KeyModifiers::SHIFT);
        if (self.nothing.is_some() || self.failed.is_some()) && self.opts.alone && menu_key {
            self.toggle_settings();
            return Act::Redraw;
        }
        if self.failed.is_some() || self.nothing.is_some() {
            return Act::Quit;
        }
        if self.loading.is_some() {
            return match key.code {
                KeyCode::Char('q') | KeyCode::Esc => Act::Quit,
                // A commit of the history being read: c goes back too.
                KeyCode::Char('c') if self.in_history => Act::Quit,
                _ => Act::Same,
            };
        }
        if self.cover.is_some() && self.telling == Telling::Waiting {
            return match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.cover = None;
                    Act::Redraw
                }
                KeyCode::Char('t') => {
                    self.raw = !self.raw;
                    Act::Redraw
                }
                _ => Act::Same,
            };
        }
        if self.ask_box.is_some() {
            return self.ask_key(key);
        }
        if self.answer.is_some() {
            return self.answer_key(key);
        }
        if self.filtering {
            // S-F10 opens the menu from the filter too, the filter kept,
            // as a right click there does.
            if key.code == KeyCode::F(10) && key.modifiers == KeyModifiers::SHIFT {
                self.filtering = false;
                self.open_menu(None);
                return Act::Redraw;
            }
            return self.filter_key(key);
        }
        let before = self.focus;
        let act = self.normal_key(key);
        if self.focus != before {
            self.visual = None;
        }
        act
    }

    fn normal_key(&mut self, key: KeyEvent) -> Act {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // While a refreshed map is told, its place takes only closing and
        // g f, back to the files: nothing acts on files out of sight.
        if self.mode == Mode::Map && !self.mapped() && self.telling == Telling::Waiting {
            return match (self.pending.take(), key.code) {
                (Some('g'), KeyCode::Char('f')) => {
                    self.show_files(false);
                    Act::Redraw
                }
                (None, KeyCode::Char('g')) => {
                    self.pending = Some('g');
                    Act::Same
                }
                (None, KeyCode::Char('q') | KeyCode::Esc) => Act::Quit,
                _ => Act::Same,
            };
        }
        if let Some(first) = self.pending.take() {
            return self.second_key(first, key);
        }
        if let KeyCode::Char(c @ '0'..='9') = key.code
            && !ctrl
            && (c != '0' || self.count.is_some())
        {
            let n = c.to_digit(10).unwrap_or(0) as usize;
            self.count = Some(self.count.unwrap_or(0) * 10 + n);
            return Act::Same;
        }
        let count = self.count.take().unwrap_or(1).max(1);
        // The map has the keys while it is on the screen, and the open
        // chapter's own map while a thing of it is chosen.
        if self.on_map()
            && let Some(act) = self.map_key(key)
        {
            return act;
        }
        if !self.things_shown() {
            self.thing = None;
        }
        if self.in_chapter()
            && let Some(act) = self.thing_key(key)
        {
            return act;
        }
        let list = self.focus == Focus::List;
        let page = self.body_height().max(1);
        match key.code {
            KeyCode::Char('q') => return Act::Quit,
            KeyCode::Char(c @ ('y' | 'g' | ']' | '[' | 'z')) if !ctrl => {
                self.pending = Some(c);
                self.count = Some(count).filter(|n| *n > 1);
                // y on the lines selected copies their text.
                let selected = if list { self.list_visual } else { self.visual };
                if c == 'y' && selected.is_some() {
                    self.copy_visual();
                    return Act::Redraw;
                }
                return Act::Same;
            }
            KeyCode::Char('w') if ctrl => {
                self.pending = Some('w');
                return Act::Same;
            }
            KeyCode::Char('E') => self.widen(),
            KeyCode::Char('m') if list && key.modifiers.is_empty() => self.open_menu(None),
            // S-F10, anywhere: the menu a right click opens.
            KeyCode::F(10) if key.modifiers == KeyModifiers::SHIFT => self.open_menu(None),
            KeyCode::Char('/') if list && !ctrl => {
                self.filtering = true;
                self.list_visual = None;
            }
            KeyCode::Char('R') if !ctrl => self.refresh(),
            KeyCode::Char('c') if !ctrl && self.in_history => self.wants = Some(Wants::Leave),
            KeyCode::Char('c') if !ctrl => self.open_history(),
            // From an open chapter's list, Tab reaches its own map first.
            KeyCode::Tab if list && self.things_shown() => {
                self.thing = Some(0);
            }
            KeyCode::Tab => {
                self.focus = if list { self.main_side() } else { Focus::List };
            }
            KeyCode::BackTab => {
                self.focus = if list { self.main_side() } else { Focus::List };
            }
            KeyCode::Esc if list && self.list_visual.is_some() => self.list_visual = None,
            // From an open chapter, back to the map it was opened from.
            KeyCode::Esc if list && self.in_chapter() => self.show_map(),
            KeyCode::Esc if list => return Act::Quit,
            KeyCode::Esc if self.visual.is_some() => self.visual = None,
            KeyCode::Esc => self.focus = Focus::List,
            KeyCode::Char('v' | 'V') if list && !ctrl => {
                self.list_visual = match self.list_visual {
                    Some(_) => None,
                    None => Some(self.at),
                };
            }
            KeyCode::Char('v' | 'V') if !ctrl => {
                self.visual = match self.visual {
                    Some(_) => None,
                    None => Some(self.cur),
                };
            }
            KeyCode::Char('J') if list => self.scroll_chosen(SCROLL * count, true),
            KeyCode::Char('K') if list => self.scroll_chosen(SCROLL * count, false),
            KeyCode::Char('j') | KeyCode::Down if list => self.move_list(count as isize),
            KeyCode::Char('k') | KeyCode::Up if list => self.move_list(-(count as isize)),
            KeyCode::Char('d') if ctrl && list => self.move_list((page / 2) as isize),
            KeyCode::Char('u') if ctrl && list => self.move_list(-((page / 2) as isize)),
            // On the last file already, the list scrolls to its end, where
            // an open chapter names its links and what it uses.
            KeyCode::Char('G') | KeyCode::End
                if list && self.at + 1 >= self.order.len() && self.in_chapter() =>
            {
                self.list_top = self.list_rows().len().saturating_sub(self.list_height());
            }
            KeyCode::Char('G') | KeyCode::End if list => self.move_list(isize::MAX / 2),
            KeyCode::Home if list => self.move_list(isize::MIN / 2),
            KeyCode::Enter | KeyCode::Char('l' | 'o') | KeyCode::Right if list => {
                self.focus = self.main_side();
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(count as isize),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(-(count as isize)),
            KeyCode::Char('d') if ctrl => self.move_cursor((page / 2) as isize),
            KeyCode::Char('u') if ctrl => self.move_cursor(-((page / 2) as isize)),
            KeyCode::Char('f') if ctrl => self.move_cursor(page as isize),
            KeyCode::Char('b') if ctrl => self.move_cursor(-(page as isize)),
            KeyCode::PageDown => self.move_cursor(page as isize),
            KeyCode::PageUp => self.move_cursor(-(page as isize)),
            KeyCode::Char('e') if ctrl => self.scroll_diff(count, true),
            KeyCode::Char('y') if ctrl => self.scroll_diff(count, false),
            KeyCode::Char('G') | KeyCode::End => self.move_cursor(isize::MAX / 2),
            KeyCode::Home => self.move_cursor(isize::MIN / 2),
            KeyCode::Enter => match self.also_under() {
                Some((chapter, hunk)) => self.go_to_also(chapter, hunk),
                None => self.open_fold(),
            },
            KeyCode::Left => {
                self.focus = match self.focus {
                    Focus::New if self.side_widths().0 > 0 => Focus::Old,
                    _ => Focus::List,
                }
            }
            KeyCode::Right => {
                if self.focus == Focus::Old && self.side_widths().1 > 0 {
                    self.focus = Focus::New;
                }
            }
            _ => return Act::Same,
        }
        Act::Redraw
    }

    /// The second key of a pair: y, g, ], [, z and C-w's.
    fn second_key(&mut self, first: char, key: KeyEvent) -> Act {
        // After a y that copied the lines selected, only yp and yl go on
        // with the selection, which is still lit: Esc and v end it and do
        // nothing more, as they end any selection, and any other key ends
        // it and does its own work.
        let yanked = std::mem::take(&mut self.yanked);
        let ends = matches!(key.code, KeyCode::Esc | KeyCode::Char('v' | 'V'));
        if yanked && !matches!(key.code, KeyCode::Char('p' | 'l')) {
            self.visual = None;
            self.list_visual = None;
            if ends {
                return Act::Redraw;
            }
            if key.code != KeyCode::Char('S') {
                return self.normal_key(key);
            }
        }
        let KeyCode::Char(c) = key.code else {
            return Act::Redraw;
        };
        let count = self.count.take().unwrap_or(1);
        // The map has no line under a cursor to copy.
        if self.on_map() && first == 'y' && matches!(c, 'l' | 'p') {
            self.say("The map has no line to copy: g f shows the chosen chapter's file.");
            return Act::Redraw;
        }
        if self.on_map() && !Self::pair_on_map(first, c) {
            return Act::Redraw;
        }
        let in_map = self.mapped() && self.mode == Mode::Map;
        match (first, c) {
            ('g', 'm') => self.show_map(),
            ('g', 'f') => self.show_files(false),
            (']', 's') => self.story_step(1),
            ('[', 's') => self.story_step(-1),
            (']', 'c') if in_map && (self.page.is_none() || self.focus == Focus::List) => {
                self.map_step(1);
            }
            ('[', 'c') if in_map && (self.page.is_none() || self.focus == Focus::List) => {
                self.map_step(-1);
            }
            // The map is one pane, with no window of the diff to go to.
            ('w', 'w') if self.on_map() => {}
            // A window asked for takes the keys from the chapter's map.
            ('w', 'w') if self.thing.take().is_some() => self.focus = self.main_side(),
            ('y', 'p') => self.copy_path(),
            ('y', 'l') => self.copy_link(),
            ('y', 'S') => self.copy_story(),
            ('g', 'g') if self.focus == Focus::List => self.move_list(isize::MIN / 2),
            ('g', 'g') => self.move_cursor(isize::MIN / 2),
            ('g', 'a') if self.opts.ask => self.open_ask(),
            // A commit of the history asks nothing, which is said.
            ('g', 'a') if self.in_history => {
                self.say("Ask works in the diff, not in the history");
            }
            (']', 'c') if self.focus == Focus::List => self.chapter_jump(1),
            ('[', 'c') if self.focus == Focus::List => self.chapter_jump(-1),
            (']', 'c') => self.change_jump(count, true),
            ('[', 'c') => self.change_jump(count, false),
            (']', 'f') => self.file_jump(count as isize),
            ('[', 'f') => self.file_jump(-(count as isize)),
            (']', 'p') => self.turn_page(1),
            ('[', 'p') => self.turn_page(-1),
            ('z', 'o' | 'a' | 'v') => self.open_fold(),
            ('z', 'R') => self.all_folds(true),
            ('z', 'M') => self.all_folds(false),
            ('w', dir @ ('h' | 'j' | 'k' | 'l')) => self.window_move(dir),
            ('w', 'w') => {
                self.focus = match self.focus {
                    Focus::List => self.first_side(),
                    Focus::Old if self.side_widths().1 > 0 => Focus::New,
                    _ => Focus::List,
                }
            }
            _ => {}
        }
        Act::Redraw
    }

    /// C-w with h, j, k or l, and the M-arrows: the list or the side of
    /// the diff that way, else the workspace's pane there, as nvim/diff.lua
    /// moved between diffview's windows and handed off to tmux at the
    /// edges.
    fn window_move(&mut self, dir: char) {
        // A window asked for takes the keys from the chapter's own map.
        self.thing = None;
        // The map is one pane: every way from it is to a pane beside.
        if self.on_map() {
            return self.beside(dir);
        }
        let (old, new) = self.side_widths();
        self.focus = match (dir, self.focus) {
            ('h', Focus::New) if old > 0 => Focus::Old,
            ('h', Focus::New | Focus::Old) => Focus::List,
            ('l', Focus::List) => self.first_side(),
            ('l', Focus::Old) if new > 0 => Focus::New,
            _ => return self.beside(dir),
        };
    }

    /// The workspace's pane on a side of the diff's (`select-pane -L`),
    /// off the screen's thread; nothing outside the workspace.
    fn beside(&self, dir: char) {
        if !self.opts.server {
            return;
        }
        let side = match dir {
            'h' => "-L",
            'j' => "-D",
            'k' => "-U",
            _ => "-R",
        };
        std::thread::spawn(move || {
            let _ = host().command(&["select-pane", side]);
        });
    }

    /// The side a file's diff is read on: the new, or the old one alone.
    fn main_side(&self) -> Focus {
        if self.side_widths().1 > 0 {
            Focus::New
        } else {
            Focus::Old
        }
    }

    fn first_side(&self) -> Focus {
        if self.side_widths().0 > 0 {
            Focus::Old
        } else {
            Focus::New
        }
    }

    fn move_list(&mut self, by: isize) {
        if self.order.is_empty() {
            return;
        }
        // A key that moves it brings the cursor back in sight, where it
        // was or not.
        self.list_followed = None;
        let to = (self.at as isize + by).clamp(0, self.order.len() as isize - 1) as usize;
        if to != self.at {
            self.moved = true;
            self.show_at(to);
        }
    }

    /// ]f and [f: the next file or the one before, opened at its first
    /// change as the list opens it, from the list or a side of the diff.
    fn file_jump(&mut self, by: isize) {
        self.move_list(by);
        if self.focus != Focus::List {
            self.keep_side();
        }
    }

    /// The cursor moved `by` lines within the shown file, stopping at its
    /// first and last lines: while the files read as one scroll too, where
    /// another file is the list's to choose, or ]f and [f's.
    fn move_cursor(&mut self, by: isize) {
        self.adrift = false;
        let n = self.screen_lines().len();
        if n == 0 {
            return;
        }
        let to_an_end = by.unsigned_abs() >= (isize::MAX / 4) as usize;
        let stopped_at_the_end_with_a_file_after =
            by > 0 && !to_an_end && self.cur + 1 >= n && self.at + 1 < self.order.len();
        if stopped_at_the_end_with_a_file_after && self.one_scroll() && !self.said_end {
            self.said_end = true;
            let path = self.current().map(|e| e.path.clone()).unwrap_or_default();
            let name = path.rsplit('/').next().unwrap_or_default();
            self.say(format!("End of {name} · ]f for the next file"));
        }
        self.cur = (self.cur as isize + by).clamp(0, n as isize - 1) as usize;
        self.col = 0;
        self.moved = true;
        self.keep_cursor();
    }

    /// The view's file the shown one, but while the files read as one
    /// scroll and it is still a file before it, or one after it the view
    /// was scrolled to, off the cursor's file.
    fn fix_view(&mut self) {
        let past = self.view > self.at && !self.adrift;
        if !self.one_scroll() || past || self.view >= self.order.len() {
            self.view = self.at;
        }
    }

    /// The view scrolled so the cursor is in it, but one scrolled off the
    /// cursor's file, which stays until the cursor moves.
    fn keep_cursor(&mut self) {
        let h = self.body_height().max(1);
        self.fix_view();
        if self.one_scroll() {
            if self.adrift {
                return;
            }
            match self.rows_from_view(self.at, self.cur) {
                None => (self.view, self.top) = (self.at, self.cur),
                Some(d) if d >= h => {
                    (self.view, self.top) = self.stream_ahead(self.view, self.top, d + 1 - h);
                }
                Some(_) => {}
            }
            return;
        }
        if self.cur < self.top {
            self.top = self.cur;
        } else if self.cur >= self.top + h {
            self.top = self.cur + 1 - h;
        }
    }

    /// The cursor kept on the lines of its own file the view shows, after
    /// the view scrolled: on its file's first or last line in sight, never
    /// in another file, and left where it is once its file has scrolled out.
    fn cursor_into_view(&mut self, h: usize) {
        let n = self.lines_of(self.at).len();
        let first = match self.view.cmp(&self.at) {
            std::cmp::Ordering::Less => Some(0),
            std::cmp::Ordering::Equal => Some(self.top),
            std::cmp::Ordering::Greater => None,
        };
        let in_sight = first.filter(|&lo| lo < n).and_then(|lo| {
            let down = self.rows_from_view(self.at, lo)?;
            (down < h).then(|| (lo, (lo + h - 1 - down).min(n - 1)))
        });
        self.adrift = in_sight.is_none();
        if let Some((lo, hi)) = in_sight {
            self.cur = self.cur.clamp(lo, hi);
        }
    }

    /// How many rows the scroll has from row `top` of file `k` to its end,
    /// counted no further than `upto`.
    fn rows_left(&mut self, mut k: usize, top: usize, upto: usize) -> usize {
        let mut rows = self.stream_len(k).saturating_sub(top);
        while rows < upto && k + 1 < self.order.len() {
            k += 1;
            rows += self.stream_len(k);
        }
        rows
    }

    /// The list's `J` and `K`: the diff scrolled `by` lines within the
    /// list's chosen file, stopping with its first line at the top or its
    /// last at the bottom, so the files reading as one scroll never takes
    /// it on into another file.
    fn scroll_chosen(&mut self, by: usize, down: bool) {
        if !self.one_scroll() || self.order.is_empty() {
            self.scroll_diff(by, down);
            return;
        }
        let h = self.body_height().max(1);
        self.fix_view();
        let n = self.lines_of(self.at).len();
        let last = n.saturating_sub(h);
        let from = if self.view == self.at {
            self.top.min(last)
        } else {
            0
        };
        self.view = self.at;
        self.adrift = false;
        self.top = if down {
            (from + by).min(last)
        } else {
            from.saturating_sub(by)
        };
        let hi = (self.top + h - 1).min(n.saturating_sub(1));
        self.cur = self.cur.clamp(self.top.min(hi), hi);
        self.moved = true;
    }

    /// The rows the diff has in all and the first shown, for its scroll
    /// bar: the shown file's lines, or while the files read as one scroll,
    /// every file's, one not read yet counted by its changed lines, since
    /// the scroll never reads every file.
    fn diff_extent(&mut self) -> (usize, usize) {
        if !self.one_scroll() {
            let top = self.top;
            return (self.screen_lines().len(), top);
        }
        let (mut total, mut top) = (0, 0);
        for k in 0..self.order.len() {
            if k == self.view {
                top = total + self.top;
            }
            let entry = self.order.get(k).and_then(|&i| self.entry(i));
            let laid = entry.and_then(|e| self.lines.get(&e.key()).map(|(_, _, l)| l.len()));
            let guess = entry.map_or(1, |e| (e.additions + e.deletions).max(1));
            let gap = if k + 1 < self.order.len() { 2 } else { 0 };
            total += laid.unwrap_or(guess) + gap;
        }
        (total, top)
    }

    /// The diff scrolled so row `to` of `diff_extent` is at the top, as
    /// far as the scroll goes.
    fn scroll_diff_to(&mut self, to: usize) {
        let (_, top) = self.diff_extent();
        if to > top {
            self.scroll_diff(to - top, true);
        } else if to < top {
            self.scroll_diff(top - to, false);
        }
    }

    /// The list's rows in all and its width, for its scroll bar.
    fn list_extent(&self) -> (usize, u16) {
        (
            self.list_rows().len(),
            self.list_width.min(self.cols.saturating_sub(10)),
        )
    }

    /// Where a press on a scroll bar lands, the list's or the diff's, and
    /// the row in the bar.
    fn on_scroll_bar(&mut self, x: u16, y: u16) -> Option<(BarDrag, usize)> {
        if !self.opts.scroll_bars || self.ask_box.is_some() {
            return None;
        }
        if self.answer.is_some() {
            let (bx, by, body) = self.answer_bar?;
            let on = x == bx && y >= by && ((y - by) as usize) < body;
            return on.then(|| (BarDrag::Answer, (y - by) as usize));
        }
        let h = self.list_height();
        let pinned = self.list_pinned();
        let (rows, w) = self.list_extent();
        if w > 0 && x + 1 == w && y >= pinned && rows > h {
            return Some((BarDrag::List, (y - pinned) as usize));
        }
        let area = self.diff_area();
        let body = area.y + self.head_height() as u16;
        if x + 1 == area.right() && y >= body && y < area.bottom() {
            let (total, _) = self.diff_extent();
            if total > self.body_height() {
                return Some((BarDrag::Diff, (y - body) as usize));
            }
        }
        None
    }

    /// The first line a scroll bar's surface shows now.
    fn bar_top(&mut self, bar: BarDrag) -> usize {
        match bar {
            BarDrag::List => self.list_top,
            BarDrag::Diff => self.diff_extent().1,
            BarDrag::Answer => self.answer.as_ref().map_or(0, |a| a.top),
        }
    }

    /// The answer window's lines in all, its first shown and how many show.
    fn answer_extent(&self) -> (usize, usize, usize) {
        let total = self.answer_lines(self.answer_width()).len();
        let top = self.answer.as_ref().map_or(0, |a| a.top);
        (total, top, self.answer_bar.map_or(1, |(_, _, body)| body))
    }

    /// A press or a drag (`drag`, the thumb's row it was taken by) on a
    /// scroll bar's row `row`.
    fn scroll_bar_to(&mut self, bar: BarDrag, row: usize, drag: Option<kit::Grab>) {
        match bar {
            BarDrag::List => {
                // Reading the list: a story arriving keeps it.
                self.moved = true;
                let h = self.list_height();
                let (rows, _) = self.list_extent();
                self.list_top = if let Some(grab) = drag {
                    kit::scroll_drag(rows, h, h, row, grab)
                } else {
                    kit::scroll_press(rows, self.list_top, h, h, row).unwrap_or(self.list_top)
                };
            }
            BarDrag::Diff => {
                let h = self.body_height().max(1);
                let (total, top) = self.diff_extent();
                let to = if let Some(grab) = drag {
                    kit::scroll_drag(total, h, h, row, grab)
                } else {
                    kit::scroll_press(total, top, h, h, row).unwrap_or(top)
                };
                // The end as the bar counts it is the diff's end, wherever
                // reading its files would put it: the last file's last lines,
                // read alone, as G reads only the file it goes to.
                if to >= total.saturating_sub(h) {
                    self.scroll_diff_to_end(h);
                } else if to == 0 {
                    self.scroll_diff_to_start();
                } else {
                    self.scroll_diff_to(to);
                }
            }
            BarDrag::Answer => {
                let (total, top, body) = self.answer_extent();
                let to = if let Some(grab) = drag {
                    kit::scroll_drag(total, body, body, row, grab)
                } else {
                    kit::scroll_press(total, top, body, body, row).unwrap_or(top)
                };
                // The cursor brought into the view, as the wheel does.
                if let Some(a) = &mut self.answer {
                    a.top = to;
                    a.cursor = a
                        .cursor
                        .clamp(to, to + body - 1)
                        .min(total.saturating_sub(1));
                }
            }
        }
    }

    /// The diff scrolled to its start: in the one scroll of files, the first
    /// file's first line with the cursor there, whatever the files between
    /// were estimated at.
    fn scroll_diff_to_start(&mut self) {
        if !self.one_scroll() || self.order.is_empty() {
            self.scroll_diff_to(0);
            return;
        }
        if self.at != 0 {
            self.cross_to(0, 0);
        }
        self.cur = 0;
        self.adrift = false;
        (self.view, self.top) = (0, 0);
        self.moved = true;
    }

    /// The diff scrolled to its end, `h` rows showing: in the one scroll
    /// of files, the last file's last rows, with the cursor on its last
    /// line, reading that file alone rather than every file before it.
    fn scroll_diff_to_end(&mut self, h: usize) {
        if !self.one_scroll() || self.order.is_empty() {
            self.scroll_diff(usize::MAX / 2, true);
            return;
        }
        let k = self.order.len() - 1;
        let n = self.lines_of(k).len();
        let last = n.saturating_sub(1);
        if k != self.at {
            self.cross_to(k, last);
        }
        self.cur = last;
        self.adrift = false;
        (self.view, self.top) = (k, n.saturating_sub(h));
        self.moved = true;
    }

    /// The diff scrolled `by` lines, the cursor kept in view: within its
    /// own file while the files read as one scroll, where the view goes on
    /// into the next file and the cursor doesn't.
    fn scroll_diff(&mut self, by: usize, down: bool) {
        let h = self.body_height().max(1);
        if self.one_scroll() && !self.order.is_empty() {
            self.fix_view();
            (self.view, self.top) = if down {
                let left = self.rows_left(self.view, self.top, by + h);
                let by = by.min(left.saturating_sub(h));
                self.stream_ahead(self.view, self.top, by)
            } else {
                self.stream_back(self.view, self.top, by)
            };
            self.cursor_into_view(h);
            self.moved = true;
            return;
        }
        let n = self.screen_lines().len();
        let last = n.saturating_sub(h);
        self.top = if down {
            (self.top + by).min(last)
        } else {
            self.top.saturating_sub(by).min(last)
        };
        let hi = (self.top + h - 1).min(n.saturating_sub(1));
        self.cur = self.cur.clamp(self.top.min(hi), hi);
        self.moved = true;
    }

    /// ]c and [c in the diff: the start of the next change or the one before.
    /// While the files read as one scroll, one past the last change goes on
    /// to the next file's first, and one before the first to the last of
    /// the file before.
    fn change_jump(&mut self, count: usize, forward: bool) {
        self.adrift = false;
        for _ in 0..count {
            let starts = self.change_starts(self.at);
            let next = if forward {
                starts.iter().find(|&&i| i > self.cur).copied()
            } else {
                starts.iter().rev().find(|&&i| i < self.cur).copied()
            };
            match next {
                Some(i) => self.cur = i,
                None if self.one_scroll() => {
                    let mut found = None;
                    let mut k = self.at;
                    while found.is_none() {
                        k = match forward {
                            true if k + 1 < self.order.len() => k + 1,
                            false if k > 0 => k - 1,
                            _ => break,
                        };
                        let starts = self.change_starts(k);
                        found = if forward {
                            starts.first()
                        } else {
                            starts.last()
                        }
                        .map(|&i| (k, i));
                    }
                    match found {
                        Some((k, i)) => self.cross_to(k, i),
                        None => break,
                    }
                }
                None => break,
            }
        }
        self.col = 0;
        self.keep_cursor();
    }

    /// The screen lines a change starts at in the list's file `k`.
    fn change_starts(&mut self, k: usize) -> Vec<usize> {
        let lines = self.lines_of(k).to_vec();
        let sides = self
            .order
            .get(k)
            .and_then(|&i| self.entry(i))
            .and_then(|e| self.files.get(&e.key()));
        let rows = sides.map(|s| &s.rows);
        let changed: Vec<bool> = lines
            .iter()
            .map(|l| match (l, rows) {
                (Line::Pair { row, .. }, Some(rows)) => rows[*row].changes(),
                _ => false,
            })
            .collect();
        let starts: Vec<usize> = (0..lines.len())
            .filter(|&i| changed[i] && (i == 0 || !changed[i - 1]))
            .collect();
        // In the one scroll every file is a stop: one with no line marked
        // changed (added or deleted, shown on one side; binary; renamed
        // alone; empty) starts at its first line.
        if self.one_scroll() && starts.is_empty() && !lines.is_empty() {
            return vec![0];
        }
        starts
    }

    fn open_fold(&mut self) {
        let cur = self.cur;
        let Some(Line::Fold { first, .. }) = self.screen_lines().get(cur).cloned() else {
            return;
        };
        if let Some(path) = self.current().map(Entry::key)
            && let Some(s) = self.files.get_mut(&path)
        {
            s.opened.insert(first);
        }
        self.lines.clear();
    }

    fn all_folds(&mut self, open: bool) {
        if let Some(path) = self.current().map(Entry::key)
            && let Some(s) = self.files.get_mut(&path)
        {
            s.all_open = open;
            if !open {
                s.opened.clear();
            }
        }
        self.lines.clear();
        let n = self.screen_lines().len();
        self.cur = self.cur.min(n.saturating_sub(1));
        self.keep_cursor();
    }

    /// E: the list widened to fit its longest path, or narrowed again.
    fn widen(&mut self) {
        self.list_width = if self.list_width != LIST_WIDTH {
            LIST_WIDTH
        } else {
            self.fit_width()
        };
        self.lines.clear();
    }

    // ── The path filter ─────────────────────────────────────────────────────

    /// A key while the path filter is typed: text narrows the list, the
    /// arrows move in it, Enter keeps the filter and gives the keys back to
    /// the list, Esc clears it.
    fn filter_key(&mut self, key: KeyEvent) -> Act {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                self.filtering = false;
                self.path_filter.clear();
                self.refilter();
            }
            KeyCode::Enter => self.filtering = false,
            KeyCode::Down => self.move_list(1),
            KeyCode::Up => self.move_list(-1),
            KeyCode::Backspace => {
                self.path_filter.pop();
                self.refilter();
            }
            KeyCode::Char('u') if ctrl => {
                self.path_filter.clear();
                self.refilter();
            }
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                self.path_filter.push(c);
                self.refilter();
            }
            _ => return Act::Same,
        }
        Act::Redraw
    }

    /// The list narrowed to what the filter leaves, the cursor on its first
    /// file and that file shown.
    fn refilter(&mut self) {
        self.sort();
        self.list_top = 0;
        self.list_followed = None;
        self.moved = true;
        if self.order.is_empty() {
            self.at = 0;
            self.view = 0;
        } else {
            self.show_at(0);
        }
    }

    /// How many files the filter leaves of how many the list may show.
    fn filtered_count(&self) -> (usize, usize) {
        (self.order.len(), self.listable().len())
    }

    /// The rows over the list's scrolling ones: the filter's line, while it
    /// is typed or kept, pinned there so it shows wherever the list is.
    pub(super) fn list_pinned(&self) -> u16 {
        u16::from(self.filtering || !self.path_filter.is_empty())
    }

    /// How many of the list's rows show under what is pinned.
    pub(super) fn list_height(&self) -> usize {
        (self.diff_area().height - self.list_pinned().min(self.diff_area().height)) as usize
    }

    // ── The story's chapters and pages ──────────────────────────────────────

    /// Why there is no story to move through, said.
    fn no_story(&mut self) {
        let text = if self.in_history {
            "A commit of the history has no story of its own.\nc goes back to the whole branch's."
                .to_string()
        } else if !self.opts.story {
            "No story for this diff: diff stories are off in Settings (diff_story).".to_string()
        } else if self.telling == Telling::Waiting {
            "The story is still being told.\n]p and ]c move through its chapters once it is."
                .to_string()
        } else {
            let why = match &self.telling {
                Telling::Untold(why) => why.clone(),
                _ => "it couldn't be told".into(),
            };
            format!("No story for this diff: {why}.\nThe list is in the kinds' order.")
        };
        self.say(text);
    }

    /// The list's rows: its heading, the spinner while the story is told,
    /// each chapter's heading over its first file, and the files.
    fn list_rows(&self) -> Vec<ListRow> {
        let first = self
            .order
            .first()
            .and_then(|&i| self.entry(i))
            .map_or(Section::Committed, |e| e.section);
        let mut rows = vec![ListRow::Header(first)];
        if !self.entries().is_empty() {
            rows.extend([ListRow::Totals, ListRow::Blank]);
        }
        if self.telling == Telling::Waiting {
            rows.push(ListRow::Loading);
        }
        let cols = self.list_width.saturating_sub(2).max(8) as usize;
        if let Some((_, subject)) = &self.title {
            for line in switchboard_ui::draw::wrap(subject, cols) {
                rows.push(ListRow::Heading(line, StoryLook::Title));
            }
            rows.push(ListRow::Heading(
                "]p [p other commits · c back".into(),
                StoryLook::Comment,
            ));
        }
        let short = self.list_width < SHORT_WHY;
        let starts = self.starts();
        let apart = self.not_committed_apart();
        for (k, &i) in self.order.iter().enumerate() {
            // A working tree's staged changes under a heading of their own.
            if let Some(e) = self.entry(i)
                && e.section != first
                && k > 0
                && self
                    .order
                    .get(k - 1)
                    .and_then(|&j| self.entry(j))
                    .map(|p| p.section)
                    != Some(e.section)
            {
                rows.push(ListRow::Header(e.section));
            }
            // A chapter opened from the map heads its files once, the
            // ones it shows from another chapter among them.
            let opened = self.page.filter(|_| self.in_chapter());
            if let Some(page) = opened {
                if k == 0 && self.opts.ux2 {
                    rows.push(ListRow::Chapter(page));
                } else if k == 0
                    && let Some(story) = &self.story
                {
                    for (text, look) in story::heading(story, page, cols, short) {
                        rows.push(ListRow::Heading(text, look));
                    }
                }
            } else if let (Some(story), Some(path)) = (&self.story, self.entry(i).map(|e| &e.path))
                && starts[k]
                && let Some(&n) = story.chapter.get(path)
            {
                if self.opts.ux2 {
                    // A chapter's number and title, the why going over the
                    // diff; what isn't committed yet under a label of its
                    // own in the needs tone.
                    if k > 0 {
                        rows.push(ListRow::Blank);
                    }
                    if self.uncommitted_chapter() == Some(n) {
                        let files = self
                            .order
                            .iter()
                            .filter(|&&j| {
                                self.entry(j)
                                    .is_some_and(|e| story.chapter.get(&e.path) == Some(&n))
                            })
                            .count();
                        rows.push(ListRow::Label(
                            story::NOT_COMMITTED.into(),
                            Tone::Needs,
                            Some(files),
                        ));
                    } else if self.generated_chapter() == Some(n) {
                        rows.push(ListRow::Label(GENERATED_LAST.into(), Tone::Dim, None));
                    } else {
                        rows.push(ListRow::Chapter(n));
                    }
                } else {
                    for (text, look) in story::heading(story, n, cols, short) {
                        rows.push(ListRow::Heading(text, look));
                    }
                }
            }
            // Without a story, what only the working tree changes under a
            // label of its own in the needs tone, after the commits' files.
            if let Some(e) = self.entry(i)
                && apart.contains(e.path.as_str())
                && !(k > 0
                    && self
                        .order
                        .get(k - 1)
                        .and_then(|&j| self.entry(j))
                        .is_some_and(|p| apart.contains(p.path.as_str())))
            {
                if k > 0 {
                    rows.push(ListRow::Blank);
                }
                let files = self
                    .order
                    .iter()
                    .filter(|&&j| {
                        self.entry(j)
                            .is_some_and(|e| apart.contains(e.path.as_str()))
                    })
                    .count();
                rows.push(ListRow::Label(
                    story::NOT_COMMITTED.into(),
                    Tone::Needs,
                    Some(files),
                ));
            }
            // With UX 2.0, the generated files the order puts last under a
            // label saying so.
            if self.opts.ux2
                && let Some(e) = self.entry(i)
                && order::generated(&e.path)
                && self
                    .story
                    .as_ref()
                    .is_none_or(|s| !s.chapter.contains_key(&e.path))
                && !(k > 0
                    && self
                        .order
                        .get(k - 1)
                        .and_then(|&j| self.entry(j))
                        .is_some_and(|p| order::generated(&p.path)))
            {
                if k > 0 {
                    rows.push(ListRow::Blank);
                }
                rows.push(ListRow::Label(GENERATED_LAST.into(), Tone::Dim, None));
            }
            rows.push(ListRow::File(k));
        }
        // Under an open chapter's files: what it builds on, and what
        // needs it.
        for (label, lines) in self.chapter_links() {
            rows.push(ListRow::Blank);
            rows.push(ListRow::Label(label, Tone::Dim, None));
            rows.extend(lines.into_iter().map(ListRow::Hint));
        }
        // What the filter hides, under the list, and how to clear it.
        let hidden = self.listed_files.saturating_sub(self.left.files);
        if !self.path_filter.trim().is_empty() && hidden > 0 {
            let clears = if self.filtering {
                "Esc clears it"
            } else {
                "/ then Esc clears it"
            };
            rows.extend([
                ListRow::Blank,
                ListRow::Hint(format!(
                    "{hidden} file{} hidden by the filter",
                    if hidden == 1 { "" } else { "s" }
                )),
                ListRow::Hint(clears.into()),
            ]);
        }
        rows
    }

    /// The files only the working tree changes, which UX 2.0 lists last
    /// under "Not committed yet" while no story is told (a story puts them
    /// in a last chapter of that name); a file the commits change too
    /// stays among theirs, marked ●, and so does a generated file among
    /// the generated ones, last.
    pub(super) fn not_committed_apart(&self) -> HashSet<&str> {
        let Some(u) = self.change.as_ref().and_then(|c| c.uncommitted.as_ref()) else {
            return HashSet::new();
        };
        if !self.opts.ux2 || self.story.is_some() || self.title.is_some() {
            return HashSet::new();
        }
        let committed: HashSet<&str> = u
            .committed
            .entries
            .iter()
            .map(|e| e.path.as_str())
            .collect();
        u.dirty
            .iter()
            .map(String::as_str)
            .filter(|p| !committed.contains(p) && !order::generated(p))
            .collect()
    }

    /// The story's last chapter when it holds what isn't committed yet,
    /// which UX 2.0 sets apart from the chapters Haiku told.
    fn uncommitted_chapter(&self) -> Option<usize> {
        let story = self.story.as_ref()?;
        let last = story.chapters.last()?;
        (self.change.as_ref().is_some_and(Change::to_work_tree)
            && last.title == story::NOT_COMMITTED
            && last.why.is_empty())
        .then_some(story.chapters.len())
    }

    /// The chapter the story made of generated files, which UX 2.0 draws
    /// as their label rather than as a chapter told.
    fn generated_chapter(&self) -> Option<usize> {
        let story = self.story.as_ref()?;
        story
            .chapters
            .iter()
            .position(|c| c.title == story::GENERATED && c.why == story::GENERATED_WHY)
            .map(|i| i + 1)
    }

    /// The story's chapters but the ones of what isn't committed yet and of
    /// generated files.
    fn told_chapters(&self) -> usize {
        let all = self.story.as_ref().map_or(0, |s| s.chapters.len());
        all - usize::from(self.uncommitted_chapter().is_some())
            - usize::from(self.generated_chapter().is_some())
    }

    /// Which of the list's files start a chapter.
    fn starts(&self) -> Vec<bool> {
        let rows: Vec<story::Row> = self
            .order
            .iter()
            .map(|&i| story::Row {
                chapter: self
                    .story
                    .as_ref()
                    .and_then(|s| s.chapter.get(&self.entries()[i].path).copied()),
                section: format!("{:?}", self.entries()[i].section),
            })
            .collect();
        story::starts(&rows)
    }

    /// ]c and [c in the list: the next or the previous heading's file.
    fn chapter_jump(&mut self, step: i32) {
        if self.story.is_none() {
            return self.no_story();
        }
        if self.order.is_empty() {
            return self.say("The filter leaves no files.\n/ then esc clears it.");
        }
        let starts = self.starts();
        match story::jump(&starts, self.at + 1, step) {
            Some(to) => {
                self.moved = true;
                self.show_at(to - 1);
            }
            None if !self.path_filter.trim().is_empty() => {
                let way = if step > 0 { "after" } else { "before" };
                self.say(format!(
                    "No chapter {way} this one has a file the filter leaves.\n/ then esc clears it."
                ));
            }
            None => {}
        }
    }

    /// ]p and [p: the story a chapter at a time, as if each were a commit
    /// of its own, [p from the first going back to the whole diff.
    fn turn_page(&mut self, step: i32) {
        if self.in_history {
            self.wants = Some(Wants::Step(step));
            return;
        }
        if !self.opts.pages {
            return;
        }
        if self.mapped() && self.mode == Mode::Map {
            return self.map_page(step);
        }
        let Some(story) = &self.story else {
            return self.no_story();
        };
        let index = self.page.unwrap_or(0) as i32 + step;
        if index > story.chapters.len() as i32 {
            return;
        }
        if index < 1 {
            if self.page.take().is_some() {
                // Laid out for the page's chapter: again for the whole.
                self.lines.clear();
                self.sort();
                let back = self.whole_at.take();
                let at = back
                    .and_then(|p| {
                        self.order
                            .iter()
                            .position(|&i| self.entries()[i].key() == p)
                    })
                    .unwrap_or(0);
                self.show_at(at);
                self.page_opened();
            }
            return;
        }
        self.open_page(index as usize);
    }

    /// A chapter shown as a page of its own, on the first of its files
    /// someone wrote.
    fn open_page(&mut self, index: usize) {
        if self.page.is_none() {
            self.whole_at = self.current().map(Entry::key);
        }
        self.page = Some(index);
        // The list from its top, as a page of its own.
        self.list_top = 0;
        self.list_followed = None;
        // Each file's notes name chapters by the page's: laid out again.
        self.lines.clear();
        self.at = 0;
        self.sort();
        self.at = 0;
        let first = self.first_written();
        self.show_at(first);
        self.page_opened();
    }

    /// The chapter the line under the diff's cursor says its hunk is also
    /// in, when it is such a line.
    fn also_under(&mut self) -> Option<(usize, Option<String>)> {
        let cur = self.cur;
        match self.screen_lines().get(cur) {
            Some(Line::Also(chapter, hunk, _)) => Some((*chapter, hunk.clone())),
            _ => None,
        }
    }

    // ── The branch's commits one at a time ─────────────────────────────────

    /// `c`: the branch's commits, oldest first, the first shown.
    fn open_history(&mut self) {
        let Some(change) = &self.change else {
            return;
        };
        if change.working() {
            return self
                .say("These are the working tree's changes: there are no commits to go through.");
        }
        let commits = git::commits_between(&change.top, &change.left, &change.right);
        self.open_commits(commits);
    }

    /// The history of these commits (as `open_history` reads them), the
    /// first shown.
    fn open_commits(&mut self, commits: Vec<(String, String)>) {
        if commits.is_empty() {
            return self.say("No commits on this branch since its base.");
        }
        self.show_commit(commits, 0);
    }

    /// A commit of the history shown: a diff of its own, which reads the
    /// commit's change off the screen's thread and tells no story (a
    /// commit is the branch's story a page at a time already).
    fn show_commit(&mut self, commits: Vec<(String, String)>, at: usize) {
        let Some(top) = self.change.as_ref().map(|c| c.top.clone()) else {
            return;
        };
        let Some((hash, subject)) = commits.get(at).cloned() else {
            return;
        };
        self.histories += 1;
        let id = self.histories;
        let (tx, rx) = channel::<Msg>();
        let outer = self.tx.clone();
        std::thread::spawn(move || {
            for msg in rx {
                if outer.send(Msg::History(id, Box::new(msg))).is_err() {
                    break;
                }
            }
        });
        let opts = Options {
            story: false,
            map: false,
            pages: false,
            progress: false,
            notes: false,
            ask: false,
            haiku: false,
            ..self.opts
        };
        let mut app = App::new(opts, self.cols, self.rows, tx.clone());
        app.term = self.term;
        app.in_history = true;
        app.list_width = self.list_width;
        let short: String = hash.chars().take(7).collect();
        app.title = Some((
            format!(" Commit {} of {} · {short}", at + 1, commits.len()),
            subject,
        ));
        std::thread::spawn(move || {
            let read = git::read_commit(&top, &hash)
                .map(|c| (Box::new(c), None))
                .map_err(|e| e.to_string());
            let _ = tx.send(Msg::Loaded(read));
        });
        if let Some(mut old) = self.history.take() {
            old.app.stop();
        }
        self.history = Some(History {
            commits,
            at,
            id,
            app: Box::new(app),
        });
    }

    /// What a commit's diff asked for, after a key or a click there: the
    /// next commit or the one before, or back to the whole branch, as it
    /// is too when it would have closed.
    fn after_history(&mut self, act: Act) -> Act {
        let Some(h) = &mut self.history else {
            return act;
        };
        match h.app.wants.take() {
            Some(Wants::Step(by)) => {
                let to = h.at as i64 + i64::from(by);
                if to >= 0 && (to as usize) < h.commits.len() {
                    let commits = std::mem::take(&mut h.commits);
                    self.show_commit(commits, to as usize);
                } else {
                    h.app.say(if by > 0 {
                        "This is the branch's last commit; c goes back to the whole branch."
                    } else {
                        "This is the branch's first commit; c goes back to the whole branch."
                    });
                }
                Act::Redraw
            }
            Some(Wants::Leave) => {
                self.leave_history();
                Act::Redraw
            }
            Some(Wants::Settings) => {
                self.toggle_settings();
                Act::Redraw
            }
            None if act == Act::Quit => {
                self.leave_history();
                Act::Redraw
            }
            None => act,
        }
    }

    /// Back to the whole branch, where it was.
    fn leave_history(&mut self) {
        if let Some(mut h) = self.history.take() {
            h.app.stop();
        }
    }

    // ── Copying ─────────────────────────────────────────────────────────────

    /// Puts text on the clipboard: through the terminal (OSC 52), which the
    /// multiplexer passes on, and the desktop's own outside it.
    fn clip(&mut self, text: &str) {
        self.out.extend_from_slice(
            format!("\x1b]52;c;{}\x07", util::base64(text.as_bytes())).as_bytes(),
        );
        if self.opts.clipboard && std::env::var_os("SWITCHBOARD").is_none() {
            let text = text.to_string();
            std::thread::spawn(move || {
                host().copy(&text);
            });
        }
    }

    /// Where the cursor is: the file, which side, and the lines, the first
    /// and the last of those selected (the cursor's alone with none). A
    /// selection ends once it is copied, as Vim's `y` leaves Visual mode.
    fn here(&mut self) -> Option<(String, bool, usize, usize)> {
        let entry = self.current()?.clone();
        let new_side = match self.focus {
            Focus::Old => false,
            Focus::New => true,
            Focus::List => self.main_side() == Focus::New,
        };
        let line = self.line_under(new_side).unwrap_or(1);
        self.list_visual = None;
        let (first, last) = match self.visual.take().filter(|_| self.focus != Focus::List) {
            Some(from) => {
                let other = self.line_at(new_side, from).unwrap_or(line);
                (line.min(other), line.max(other))
            }
            None => (line, line),
        };
        let path = if new_side {
            entry.path.clone()
        } else {
            entry.old().to_string()
        };
        Some((path, new_side, first, last))
    }

    fn copy_path(&mut self) {
        let Some((path, _, first, last)) = self.here() else {
            return self.say("No file here");
        };
        let text = format!("{path}:{}", span(first, last, ""));
        self.clip(&text);
        self.say(format!("Copied {text}"));
    }

    fn copy_link(&mut self) {
        let Some((path, new_side, first, last)) = self.here() else {
            return self.say("No file here");
        };
        let line = span(first, last, "L");
        let (Some(change), Some(entry)) = (&self.change, self.current()) else {
            return;
        };
        // The working tree's side of a file with changes not yet committed
        // has no line on GitHub to link to yet.
        if new_side && change.dirty(&entry.path) {
            let text = format!(
                "{} has uncommitted changes, so this side has no GitHub link yet",
                entry.path
            );
            return self.say(text);
        }
        let Some(repo) = git::github_repo(&change.top) else {
            return self.say("origin isn't on GitHub");
        };
        let (left, right) = change.sides(entry);
        let commit = change
            .commit_of(if new_side { &right } else { &left })
            .to_string();
        let text = format!("https://github.com/{repo}/blob/{commit}/{path}#L{line}");
        self.clip(&text);
        self.say(format!("Copied {text}"));
    }

    fn copy_story(&mut self) {
        let Some(story) = &self.story else {
            return self.say("No story told for this diff yet");
        };
        let text = story::markdown(story);
        let n = story.chapters.len();
        self.clip(&text);
        self.say(format!("Copied the story as Markdown, {n} chapters"));
    }

    // ── Asking ──────────────────────────────────────────────────────────────

    /// Where the cursor is, for a question: the file and line of the diff
    /// side it is in, or the file under it in the list.
    fn ask_cursor(&mut self) -> ask::Cursor {
        // On the map no file is under a cursor: the file list's is out
        // of sight, and not what the question is about.
        if self.on_map() {
            return ask::Cursor::default();
        }
        let Some(entry) = self.current().cloned() else {
            return ask::Cursor::default();
        };
        if self.focus == Focus::List {
            return ask::Cursor {
                path: Some(entry.path),
                ..ask::Cursor::default()
            };
        }
        let new_side = self.focus == Focus::New && entry.status != 'D';
        let line = self.line_under(self.focus == Focus::New);
        let side = if new_side {
            ask::Side::New
        } else {
            ask::Side::Old
        };
        let hunk = line
            .and_then(|l| ask::hunk_at(&self.hunks, &entry.path, l, self.run_of(&entry), side))
            .map(|h| h.id.clone());
        ask::Cursor {
            path: Some(entry.path),
            line,
            old_side: !new_side,
            hunk,
        }
    }

    fn open_ask(&mut self) {
        // Never with Ask Claude about the diff off, turned off since an
        // answer opened included.
        if self.change.is_none() || !self.opts.ask {
            return;
        }
        let at = self.ask_cursor();
        self.ask_box = Some(AskBox {
            text: String::new(),
            pos: 0,
            at,
        });
    }

    fn ask_key(&mut self, key: KeyEvent) -> Act {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let Some(b) = &mut self.ask_box else {
            return Act::Same;
        };
        match key.code {
            KeyCode::Esc => self.ask_box = None,
            KeyCode::Char('c') if ctrl => self.ask_box = None,
            KeyCode::Enter if shift => b.insert("\n"),
            KeyCode::Char('j') if ctrl => b.insert("\n"),
            KeyCode::Enter => self.send_question(),
            KeyCode::Backspace => {
                if let Some(at) = b.char_before() {
                    b.text.replace_range(at..b.pos, "");
                    b.pos = at;
                }
            }
            KeyCode::Delete => {
                if let Some(end) = b.char_after() {
                    b.text.replace_range(b.pos..end, "");
                }
            }
            KeyCode::Left => b.pos = b.char_before().unwrap_or(b.pos),
            KeyCode::Right => b.pos = b.char_after().unwrap_or(b.pos),
            KeyCode::Home => b.pos = b.line_start(),
            KeyCode::Char('a') if ctrl => b.pos = b.line_start(),
            KeyCode::End => b.pos = b.line_end(),
            KeyCode::Char('e') if ctrl => b.pos = b.line_end(),
            KeyCode::Char('u') if ctrl => {
                b.text.clear();
                b.pos = 0;
            }
            KeyCode::Char(c) if !ctrl => b.insert(c.encode_utf8(&mut [0; 4])),
            _ => return Act::Same,
        }
        Act::Redraw
    }

    /// Text pasted: into the question being typed, line breaks and all
    /// (the terminal's bracketed paste), and nowhere else.
    pub fn paste(&mut self, text: &str) -> Act {
        // Pasted into the menu's search; nothing else is typed into under
        // the menu or the settings view.
        if self.menu.is_some() {
            return self.menu_paste(text);
        }
        if self.settings.is_some() {
            return Act::Same;
        }
        if let Some(h) = &mut self.history {
            let act = h.app.paste(text);
            return self.after_history(act);
        }
        if self.ask_box.is_none() && self.filtering {
            self.path_filter.extend(
                text.chars()
                    .map(|c| if c.is_whitespace() { ' ' } else { c })
                    .filter(|c| !c.is_control()),
            );
            self.refilter();
            self.read_view();
            return Act::Redraw;
        }
        let Some(b) = &mut self.ask_box else {
            return Act::Same;
        };
        b.insert(&text.replace("\r\n", "\n").replace('\r', "\n"));
        Act::Redraw
    }

    fn send_question(&mut self) {
        let Some(b) = self.ask_box.take() else {
            return;
        };
        // Never with Ask turned off since the box opened.
        if !self.opts.ask {
            return;
        }
        let question = b.text.trim().to_string();
        if question.is_empty() {
            return;
        }
        let Some(change) = &self.change else {
            return;
        };
        let story = self.story.as_ref();
        let question_for = ask::Question {
            top: change.top.clone(),
            session_input: ask::session_input(story, &self.input, &change.top, &b.at, &question),
            input: ask::input(story, Some(&self.input), &b.at, &question),
            session: self.session.clone(),
        };
        self.asks += 1;
        let id = self.asks;
        let asking = Asking::default();
        if let Some(old) = self.answer.take() {
            old.asking.stop();
        }
        self.answer = Some(Answer {
            id,
            question,
            body: String::new(),
            done: false,
            top: 0,
            cursor: 0,
            asking: asking.clone(),
            who: None,
            visual: None,
        });
        if !self.opts.haiku {
            return;
        }
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            ask::run(&question_for, &asking, &|said| {
                let _ = tx.send(Msg::Asked(id, said));
            });
        });
    }

    fn asked(&mut self, id: u64, said: Said) {
        let Some(a) = self.answer.as_mut().filter(|a| a.id == id) else {
            return;
        };
        match said {
            Said::Text(text) => a.body.push_str(&text),
            Said::Who(who) => a.who = Some(who),
            Said::Tool(line) => {
                if !a.body.is_empty() {
                    a.body.push_str("\n\n");
                }
                a.body.push_str(&line);
                a.body.push_str("\n\n");
            }
            Said::Done(Ok(text)) => {
                a.done = true;
                if a.body.trim().is_empty() {
                    a.body = text;
                }
            }
            // Claude Code missing is all there is to say, as diffview.lua
            // said it.
            Said::Done(Err(why)) if why == ask::NOT_INSTALLED => {
                a.done = true;
                a.body = why;
            }
            Said::Done(Err(why)) => {
                a.done = true;
                if !a.body.is_empty() {
                    a.body.push_str("\n\n");
                }
                let asked_the_session = a
                    .who
                    .as_ref()
                    .is_some_and(|w| w != ask::FELL_BACK && w != ask::ASKED_HAIKU);
                a.body.push_str(if asked_the_session {
                    "**The tab's session couldn't answer**"
                } else {
                    "**Haiku couldn't answer**"
                });
                if !why.is_empty() {
                    a.body.push_str(&format!(": {why}"));
                }
            }
        }
    }

    fn close_answer(&mut self) {
        if let Some(a) = self.answer.take() {
            a.asking.stop();
        }
    }

    fn answer_key(&mut self, key: KeyEvent) -> Act {
        if let Some(first) = self.pending.take() {
            // After a y that copied the lines selected, the selection ends:
            // Esc and v do nothing more, and a key that isn't y's does its
            // own work.
            if std::mem::take(&mut self.yanked) {
                if let Some(a) = &mut self.answer {
                    a.visual = None;
                }
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('v' | 'V')) {
                    return Act::Redraw;
                }
                if !matches!(key.code, KeyCode::Char('y' | 'S')) {
                    return self.answer_key(key);
                }
            }
            let KeyCode::Char(c) = key.code else {
                return Act::Redraw;
            };
            match (first, c) {
                ('g', 'g') => {
                    if let Some(a) = &mut self.answer {
                        a.cursor = 0;
                    }
                }
                ('g', 'a') => {
                    self.close_answer();
                    self.open_ask();
                }
                ('g', 'f') => self.go_to_cited(),
                ('y', 'y') => self.copy_answer(),
                ('y', 'S') => self.copy_story(),
                _ => {}
            }
            return Act::Redraw;
        }
        let lines = self.answer_lines(self.answer_width()).len();
        let half = (self.answer_body() / 2).max(1);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(a) = &mut self.answer else {
            return Act::Same;
        };
        let last = lines.saturating_sub(1);
        match key.code {
            KeyCode::Esc if a.visual.is_some() => a.visual = None,
            KeyCode::Char('q') | KeyCode::Esc => self.close_answer(),
            KeyCode::Char('d') if ctrl => a.cursor = (a.cursor + half).min(last),
            KeyCode::Char('u') if ctrl => a.cursor = a.cursor.saturating_sub(half),
            KeyCode::Char('v' | 'V') if !ctrl => {
                a.visual = match a.visual {
                    Some(_) => None,
                    None => Some(a.cursor),
                };
            }
            KeyCode::Char(c @ ('g' | 'y')) if !ctrl => {
                self.pending = Some(c);
                // y on the lines selected copies their text.
                if c == 'y' {
                    self.copy_answer_lines();
                }
            }
            KeyCode::Char('j') | KeyCode::Down => a.cursor = (a.cursor + 1).min(last),
            KeyCode::Char('k') | KeyCode::Up => a.cursor = a.cursor.saturating_sub(1),
            KeyCode::Char('G') | KeyCode::End => a.cursor = last,
            KeyCode::Home => a.cursor = 0,
            _ => return Act::Same,
        }
        Act::Redraw
    }

    fn copy_answer(&mut self) {
        let Some(body) = self.answer.as_ref().map(|a| a.body.trim().to_string()) else {
            return;
        };
        self.clip(&format!("{body}\n"));
        self.say("Copied the answer");
    }

    /// gf: the path:line under the answer's cursor, in the diff when the
    /// change has that file.
    fn go_to_cited(&mut self) {
        let lines = self.answer_lines(self.answer_width());
        let cursor = self.answer.as_ref().map_or(0, |a| a.cursor);
        let Some(line) = lines.get(cursor) else {
            return;
        };
        let Some((_, cited)) = ask::all_cited(&line.0).into_iter().next() else {
            return self.say("No path:line under the cursor");
        };
        self.cited(cited);
    }

    fn cited(&mut self, cited: ask::Cited) {
        let Some(top) = self
            .change
            .as_ref()
            .map(|c| c.top.to_string_lossy().into_owned())
        else {
            return;
        };
        let path = cited.path.strip_prefix("./").unwrap_or(&cited.path);
        let path = path
            .strip_prefix(&format!("{top}/"))
            .unwrap_or(path)
            .to_string();
        self.close_answer();
        // From the map or an open chapter, the line cited is in the files.
        // Only for a file the diff has: one outside it opens beside, the
        // map or the chapter left as it is.
        let changed = self.entries().iter().any(|e| e.path == path);
        if changed && self.mapped() && self.mode == Mode::Map {
            self.whole_files();
        }
        let found = self
            .order
            .iter()
            .position(|&i| self.entries()[i].path == path);
        let Some(at) = found else {
            let file = Path::new(&top).join(&path);
            let inside = file
                .canonicalize()
                .ok()
                .zip(Path::new(&top).canonicalize().ok())
                .is_some_and(|(f, t)| f.starts_with(t) && f.is_file());
            if !inside {
                return self.say(format!("{path} isn't in the repository"));
            }
            if !self.open_beside(&top, &file, cited.line) {
                self.say(format!("{path} isn't changed in this diff"));
            }
            return;
        };
        self.moved = true;
        self.show_at(at);
        self.focus = self.main_side();
        let new_side = self.focus == Focus::New;
        let lines = self.screen_lines().to_vec();
        let found = lines.iter().position(|l| match l {
            Line::Pair { old, new, .. } => matches!(if new_side { new } else { old }, Cell::Text { line, range } if *line + 1 == cited.line && range.start == 0),
            _ => false,
        });
        if let Some(i) = found {
            self.cur = i;
            self.col = 0;
            self.top = i.saturating_sub(self.body_height() / 2);
        }
    }

    /// The answer window's rows of text: the window less its edges and its
    /// row of buttons.
    fn answer_body(&self) -> usize {
        let h = self
            .rows
            .saturating_sub(2)
            .max(5)
            .min(self.rows.saturating_sub(1));
        h.saturating_sub(3) as usize
    }

    /// A file the diff doesn't change, opened in a pane below it at a line,
    /// as the answer's `gf` opened one in a split below the diff: in your
    /// `$VISUAL` or `$EDITOR` (`+<line>`, as Vim, Neovim, nano and Emacs
    /// take it), else read as plain text (`switchboard view +<line>`). False
    /// outside the workspace, which has no pane to open it in.
    fn open_beside(&self, top: &str, file: &Path, line: usize) -> bool {
        if !self.opts.server {
            return false;
        }
        let file = file.to_string_lossy().into_owned();
        let editor = ["VISUAL", "EDITOR"]
            .iter()
            .find_map(|v| std::env::var(v).ok().filter(|e| !e.trim().is_empty()));
        let mut args: Vec<String> = ["split", "-D", "-c", top, "--"]
            .iter()
            .map(|a| a.to_string())
            .collect();
        match editor {
            Some(editor) => args.extend([
                "sh".to_string(),
                "-c".into(),
                format!("exec {editor} \"+$1\" \"$2\""),
                "sh".into(),
                line.to_string(),
                file,
            ]),
            None => {
                let exe = util::exe()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| "switchboard".into());
                args.extend([exe, "view".into(), format!("+{line}"), file]);
            }
        }
        std::thread::spawn(move || {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let _ = host().command(&args);
        });
        true
    }

    fn answer_width(&self) -> u16 {
        (self.cols / 2)
            .saturating_sub(2)
            .max(40)
            .min(self.cols.saturating_sub(2))
    }

    /// The answer as lines for its window: its Markdown laid out as the
    /// PR view lays it out, with a spinner while it comes.
    fn answer_lines(&self, cols: u16) -> Vec<(String, Vec<(String, Style)>)> {
        let Some(a) = &self.answer else {
            return Vec::new();
        };
        let inner = cols.saturating_sub(4).max(10) as usize;
        let mut text = a
            .who
            .as_ref()
            .map(|who| format!("_{who}_\n\n"))
            .unwrap_or_default();
        text.push_str(&a.body);
        if !a.done {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(self.spinner());
            if a.body.is_empty() {
                text.push_str(" reading the diff");
            }
        }
        let doc = switchboard_ui::markdown::Markdown::parse(&text, "").layout(inner);
        doc.lines
            .iter()
            .map(|l| {
                let spans = l
                    .spans
                    .iter()
                    .map(|s| (s.text.clone(), s.style.ratatui()))
                    .collect();
                (l.plain(), spans)
            })
            .collect()
    }

    // ── The mouse ───────────────────────────────────────────────────────────

    pub fn mouse(&mut self, mut m: MouseEvent) -> Act {
        // The header's row, alone, takes no click; the rest is the diff's,
        // counted from under it.
        if self.has_header() {
            // A drag or a release over the header goes on as one at the
            // diff's first row, so a selection started under it ends.
            let held = matches!(m.kind, MouseEventKind::Drag(_) | MouseEventKind::Up(_));
            if m.row == 0 && !held {
                return Act::Same;
            }
            m.row = m.row.saturating_sub(1);
        }
        let act = self.mouse_now(m);
        self.read_view();
        act
    }

    /// The button drawn at a cell.
    fn hit(&self, x: u16, y: u16) -> Option<Button> {
        self.buttons
            .iter()
            .find(|(_, r)| x >= r.x && x < r.right() && y >= r.y && y < r.bottom())
            .map(|(b, _)| *b)
    }

    fn mouse_now(&mut self, m: MouseEvent) -> Act {
        if self.menu.is_some() {
            return self.menu_mouse(m);
        }
        if self.settings.is_some() {
            let hit = self.hit(m.column, m.row);
            if matches!(m.kind, MouseEventKind::Moved | MouseEventKind::Drag(_)) {
                if !self.opts.hover || hit == self.hover {
                    return Act::Same;
                }
                self.hover = hit;
                return Act::Redraw;
            }
            // A press closes a message; its release leaves the one the
            // press may have said (why a setting couldn't be written).
            let pressed = matches!(m.kind, MouseEventKind::Down(_));
            let closed = pressed && self.message.take().is_some();
            return match self.settings_mouse(m, hit) {
                Act::Same if closed => Act::Redraw,
                act => act,
            };
        }
        if let Some(h) = &mut self.history {
            let act = h.app.mouse(m);
            return self.after_history(act);
        }
        let (x, y) = (m.column, m.row);
        let hit = self.hit(x, y);
        match m.kind {
            MouseEventKind::Drag(MouseButton::Left) if self.bar_drag.is_some() => {
                let Some((bar, from, grab)) = self.bar_drag else {
                    return Act::Same;
                };
                let row = y.saturating_sub(from) as usize;
                self.scroll_bar_to(bar, row, Some(grab));
                return Act::Redraw;
            }
            MouseEventKind::Up(MouseButton::Left) if self.bar_drag.is_some() => {
                self.bar_drag = None;
                return Act::Same;
            }
            MouseEventKind::Drag(MouseButton::Left) if self.pressed.is_some() => {
                return self.drag_select(x, y);
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let open = self.open_on_up.take().filter(|_| self.selection.is_none());
                let also = self.also_on_up.take().filter(|_| self.selection.is_none());
                let act = self.release_select();
                if let Some((chapter, hunk)) = also {
                    self.go_to_also(chapter, hunk);
                    return Act::Redraw;
                }
                return match open {
                    Some(n) => {
                        self.open_chapter(n);
                        Act::Redraw
                    }
                    None => act,
                };
            }
            MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                let lit = self.hit_at(x, y);
                if !self.opts.hover || (hit == self.hover && lit == self.lit) {
                    return Act::Same;
                }
                self.hover = hit;
                self.lit = lit;
                return Act::Redraw;
            }
            MouseEventKind::Down(MouseButton::Left) => {}
            // The place of a refreshed map still being told takes no wheel.
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
                if self.mode == Mode::Map && !self.mapped() && self.telling == Telling::Waiting =>
            {
                return Act::Same;
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let down = m.kind == MouseEventKind::ScrollDown;
                // Over a chapter's own map, the wheel moves among its
                // things, which brings one cut off into sight.
                if self.answer.is_none() && x >= self.diff_area().x && self.over_things(y) {
                    let count = self
                        .story
                        .as_ref()
                        .zip(self.page)
                        .and_then(|(s, p)| s.chapters.get(p - 1))
                        .map_or(0, |c| c.things.len());
                    let at = self.thing.unwrap_or(0);
                    self.thing = Some(if down {
                        (at + 1).min(count.saturating_sub(1))
                    } else {
                        at.saturating_sub(1)
                    });
                    return Act::Redraw;
                }
                // Over the map's list, the wheel moves between the stories,
                // so one cut off the list and the bar is still reached.
                if self.on_map() && self.answer.is_none() && x < self.list_width {
                    self.story_step(if down { 1 } else { -1 });
                    return Act::Redraw;
                }
                // Over the map, the wheel reads on from the goal down.
                if self.on_map() && self.answer.is_none() && self.ask_box.is_none() {
                    self.map_step(if down { 1 } else { -1 });
                    return Act::Redraw;
                }
                if self.answer.is_some() {
                    // The view moved and the cursor brought into it, as
                    // the answer's window was a Neovim float's.
                    let lines = self.answer_lines(self.answer_width()).len();
                    let body = self.answer_body().max(1);
                    if let Some(a) = &mut self.answer {
                        let last = lines.saturating_sub(1);
                        a.top = if down {
                            (a.top + 3).min(lines.saturating_sub(body))
                        } else {
                            a.top.saturating_sub(3)
                        };
                        a.cursor = a.cursor.clamp(a.top, a.top + body - 1).min(last);
                    }
                } else if x < self.list_width {
                    // The list alone moves; the cursor stays on its file.
                    // Reading the list: a story arriving keeps it.
                    self.moved = true;
                    self.list_top = if down {
                        self.list_top + 3
                    } else {
                        self.list_top.saturating_sub(3)
                    };
                } else {
                    self.scroll_diff(3, down);
                }
                return Act::Redraw;
            }
            // A right click opens the diff's menu beside it, as `m` does.
            MouseEventKind::Down(MouseButton::Right) => {
                let shown = self.failed.is_none()
                    && self.nothing.is_none()
                    && self.loading.is_none()
                    && !(self.cover.is_some() && self.telling == Telling::Waiting)
                    && self.ask_box.is_none()
                    && self.answer.is_none();
                // Only where a click takes a menu's option (menu_mouse), so
                // none opens that the mouse couldn't close.
                if !shown || !switchboard_ui::settings::is_on("menu_mouse") {
                    return Act::Same;
                }
                self.message = None;
                self.filtering = false;
                self.open_menu(Some((x, y)));
                return Act::Redraw;
            }
            _ => return Act::Same,
        }
        // A click closes a message and goes on to do what it does; but on
        // a scroll bar, which scrolls under a selection, it ends what was
        // selected.
        self.pressed = None;
        self.pressed_cite = None;
        let closed = self.message.take().is_some();
        // A click anywhere but the filter's line ends typing in it, the
        // filter kept, so a button's keys or a click in the diff act as
        // they would; one on that line types into it again.
        let pinned = self.list_pinned();
        self.filtering = false;
        // The Settings button, wherever the row is drawn: with nothing to
        // show, while the change is read and under the story's cover.
        if hit == Some(Button::Settings) {
            return self.press(Button::Settings);
        }
        if self.failed.is_some() || self.nothing.is_some() {
            return Act::Quit;
        }
        // The cover, and the place of a refreshed map still being told,
        // take no click but the row's buttons: nothing out of sight is chosen.
        let map_told = self.mode == Mode::Map && !self.mapped() && self.telling == Telling::Waiting;
        if self.loading.is_some()
            || (self.cover.is_some() && self.telling == Telling::Waiting)
            || map_told
        {
            // A button of the row drawn there presses its keys, which do
            // there what they do from the keyboard: Close closes.
            if let Some(button @ Button::Bar(_)) = hit {
                return self.press(button);
            }
            return if closed { Act::Redraw } else { Act::Same };
        }
        if let Some(button) = hit {
            self.selection = None;
            self.visual = None;
            self.list_visual = None;
            return self.press(button);
        }
        // A press on something of the map does what it does, and a drag
        // from it still selects the map's text.
        if self.ask_box.is_none()
            && self.answer.is_none()
            && let Some(hit) = self.hit_at(x, y)
        {
            self.selection = None;
            self.pressed = Some((x, y));
            return self.map_click(hit);
        }
        // A click anywhere else takes the keys from the open chapter's
        // own map.
        self.thing = None;
        // Nothing else of the map takes a click.
        if self.on_map() && self.ask_box.is_none() && self.answer.is_none() {
            let had = self.selection.take().is_some();
            self.pressed = Some((x, y));
            return if closed || had {
                Act::Redraw
            } else {
                Act::Same
            };
        }
        if let Some((bar, row)) = self.on_scroll_bar(x, y) {
            // The press focuses the side its bar is on, as a press there does.
            match bar {
                BarDrag::List => self.focus = Focus::List,
                BarDrag::Diff if self.focus == Focus::List => self.focus = self.main_side(),
                BarDrag::Diff | BarDrag::Answer => {}
            }
            // The drag goes on from where the press left it, the diff's real
            // end included, which reading files on the way can move.
            self.scroll_bar_to(bar, row, None);
            let top = self.bar_top(bar);
            self.bar_drag = Some((bar, y - row as u16, (row, top)));
            return Act::Redraw;
        }
        self.selection = None;
        self.visual = None;
        self.list_visual = None;
        if let Some(a) = &mut self.answer {
            a.visual = None;
        }
        // A drag from here selects; a second press on the cell selects its
        // word and a third its line, in place of what a click does.
        self.pressed = Some((x, y));
        let clicks = self.count_click(x, y);
        if clicks > 1 && self.select_unit(x, y, clicks == 3) {
            return Act::Redraw;
        }
        // A click outside a box closes it.
        let inside = |r: Option<Rect>| {
            r.is_some_and(|r| x >= r.x && x < r.right() && y >= r.y && y < r.bottom())
        };
        if self.ask_box.is_some() {
            if !inside(self.ask_rect) {
                self.ask_box = None;
            }
            return Act::Redraw;
        }
        if self.answer.is_some() {
            if !inside(self.answer_rect) {
                self.close_answer();
                return Act::Redraw;
            }
            if let Some(r) = self.answer_rect {
                let row = (y.saturating_sub(r.y + 2)) as usize;
                let lines = self.answer_lines(r.width);
                let mut found = None;
                if let Some(a) = &mut self.answer {
                    let at = a.top + row;
                    a.cursor = at.min(lines.len().saturating_sub(1));
                    found = lines
                        .get(at)
                        .and_then(|line| ask::cited(&line.0, (x.saturating_sub(r.x + 2)) as usize));
                }
                // Gone to once the button is let go, unless it dragged.
                self.pressed_cite = found;
            }
            return Act::Redraw;
        }
        if y >= self.diff_area().bottom() {
            return Act::Same;
        }
        if x < self.list_width {
            self.focus = Focus::List;
            if y < pinned {
                self.filtering = true;
                return Act::Redraw;
            }
            let rows = self.list_rows();
            let at = self.list_top + (y - pinned) as usize;
            let file = rows.get(at..).and_then(|rest| {
                rest.iter().find_map(|r| match r {
                    ListRow::File(k) => Some(*k),
                    _ => None,
                })
            });
            if let Some(k) = file {
                self.moved = true;
                self.show_at(k);
            }
            return Act::Redraw;
        }
        let area = self.diff_area();
        let body = area.y + self.head_height() as u16;
        if y >= body && y < area.bottom() {
            // While the files read as one scroll, the file clicked in is
            // shown; a click on a rule or a bar between them does nothing.
            if self.one_scroll() && !self.order.is_empty() {
                self.fix_view();
                let row = (y - body) as usize;
                let (k, t) = self.stream_ahead(self.view, self.top, row);
                if self.rows_from_view(k, t) != Some(row) || t >= self.lines_of(k).len() {
                    return Act::Redraw;
                }
                self.adrift = false;
                if k != self.at {
                    self.cross_to(k, t);
                }
                self.cur = t;
            }
            let (ow, _) = self.side_widths();
            self.focus = if ow > 0 && x < area.x + ow {
                Focus::Old
            } else {
                self.main_side()
            };
            if !self.one_scroll() {
                let n = self.screen_lines().len();
                self.cur = (self.top + (y - body) as usize).min(n.saturating_sub(1));
            }
            let side_x = if self.focus == Focus::Old {
                area.x
            } else {
                area.x + if ow > 0 { ow + 1 } else { 0 }
            };
            let (ow, nw) = self.side_widths();
            let side_w = if self.focus == Focus::Old { ow } else { nw };
            let gutter = self
                .current()
                .and_then(|e| self.files.get(&e.key()))
                .map_or(0, |s| s.gutter(side_w as usize));
            self.col = (x.saturating_sub(side_x) as usize).saturating_sub(gutter);
            let cur = self.cur;
            if matches!(self.screen_lines().get(cur), Some(Line::Fold { .. })) {
                self.open_fold();
            }
            // Gone to as the mouse is let go, so a drag from it selects.
            if let Some(also) = self.also_under() {
                self.also_on_up = Some(also);
            }
        }
        Act::Redraw
    }

    /// A button pressed, as its keys would be.
    fn press(&mut self, button: Button) -> Act {
        // A button takes the keys from a chapter's own map.
        self.thing = None;
        match button {
            // While a question is typed, the row's buttons do nothing: its
            // keys would be typed into the draft.
            // While a question is typed, the row is Settings and Close
            // alone (`bar_buttons`), Close closing the question, draft and
            // all, as Esc does.
            Button::Bar(_) if self.ask_box.is_some() && self.settings.is_none() => {
                self.ask_box = None;
                return Act::Redraw;
            }
            Button::Bar(i) => {
                let keys = BUTTONS
                    .iter()
                    .filter(|(_, k)| self.opts.ask || *k != "ga")
                    .nth(i)
                    .map(|(_, k)| *k)
                    .unwrap_or("");
                // Over the settings view, a button goes back to the diff
                // and does its work there; Close only closes the view, and
                // so does any of them over a question's draft, which its
                // keys would be typed into.
                let over_view = self.settings.take().is_some();
                if over_view && (keys == "q" || self.ask_box.is_some()) {
                    return Act::Redraw;
                }
                // Over an answer, whose keys they aren't, the answer goes
                // first, so the button does its work in the diff.
                // Over an answer, whose keys they aren't, the answer goes
                // first, so the button does its work in the diff.
                if self.answer.is_some() {
                    self.close_answer();
                }
                self.focus = Focus::List;
                let mut act = Act::Same;
                for c in keys.chars() {
                    act = self.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
                }
                if act == Act::Quit {
                    return Act::Quit;
                }
            }
            Button::Settings => self.toggle_settings(),
            Button::Option(_) => {}
            Button::Mode(mode) => self.choose_mode(mode),
            Button::AskSend => self.send_question(),
            Button::AskCancel => self.ask_box = None,
            Button::AnswerClose => self.close_answer(),
            Button::AnswerAgain => {
                self.close_answer();
                self.open_ask();
            }
            Button::AnswerCopy => self.copy_answer(),
        }
        Act::Redraw
    }
}

/// The side an M-arrow moves to (h, j, k or l), for an arrow with Alt
/// alone.
fn alt_arrow(key: &KeyEvent) -> Option<char> {
    if key.modifiers != KeyModifiers::ALT {
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

/// A change's files as Haiku reads them: a working tree's file both staged
/// and changed since is one file, as it was in diffview's story.
fn story_entries(change: &Change) -> Vec<story::Entry> {
    let mut entries: Vec<story::Entry> = Vec::new();
    for e in &change.entries {
        match entries.iter_mut().find(|s| s.path == e.path) {
            Some(seen) => {
                seen.additions += e.additions;
                seen.deletions += e.deletions;
            }
            None => entries.push(story::Entry {
                path: e.path.clone(),
                additions: e.additions,
                deletions: e.deletions,
            }),
        }
    }
    entries
}

/// In a change running to the working tree, the files it lists with
/// changes not yet committed, and those its commits change, each in the
/// kinds' order, then A to Z (diffview.lua's `look_uncommitted`); none in
/// any other change.
pub fn uncommitted_orders(change: &Change) -> (Vec<String>, Vec<String>) {
    let Some(u) = &change.uncommitted else {
        return (Vec::new(), Vec::new());
    };
    let none = HashMap::new();
    let listed: HashSet<&str> = change.entries.iter().map(|e| e.path.as_str()).collect();
    let in_order = |paths: Vec<&String>| -> Vec<String> {
        let mut kept: Vec<String> = paths
            .into_iter()
            .filter(|p| listed.contains(p.as_str()))
            .cloned()
            .collect();
        kept.sort_by(|a, b| order::reading_order(a, b, None, &none));
        kept.dedup();
        kept
    };
    (
        in_order(u.dirty.iter().collect()),
        in_order(u.committed.entries.iter().map(|e| &e.path).collect()),
    )
}

/// Lines as `yp` and `yl` name them: one line alone, else the first and
/// the last, the last after `mark` (GitHub's `#L12-L20`).
fn span(first: usize, last: usize, mark: &str) -> String {
    if last > first {
        format!("{first}-{mark}{last}")
    } else {
        first.to_string()
    }
}

// ── Running it ──────────────────────────────────────────────────────────────

/// Reads the change in `dir` off the screen's thread: the base kept for
/// the branch's commit, else asked for and fetched (said on the screen
/// meanwhile), then the change against it, to the working tree with
/// `uncommitted` (the diff_uncommitted setting); with nothing to show, says
/// so (`Change::nothing_to_show`) rather than give an empty diff.
fn load(dir: PathBuf, base: Option<String>, working: bool, uncommitted: bool, tx: Sender<Msg>) {
    let result = (|| -> Result<(Box<Change>, Option<String>), String> {
        let facts = git::facts(&dir).map_err(|e| e.to_string())?;
        if working {
            let change = git::read_working(&facts.top).map_err(|e| e.to_string())?;
            return Ok((Box::new(change), None));
        }
        let mut said = None;
        let (base, why) = match base {
            Some(b) => (b, git::Why::Named),
            None => match git::cached_for_opening(&dir) {
                Some(kept) => kept,
                None => {
                    let _ = tx.send(Msg::Loading("Fetching the base branch...".into()));
                    let Some(found) = git::fetch_base(&dir) else {
                        return Err("Couldn't work out the branch's base".into());
                    };
                    if !found.fetched {
                        said = Some(format!(
                            "Could not fetch {}; comparing with it as last fetched.",
                            found.base
                        ));
                    }
                    (found.base, found.why)
                }
            },
        };
        let mut change = if uncommitted {
            git::read_to_work_tree(&facts.top, &base)
        } else {
            git::read_change(&facts.top, &base)
        }
        .map_err(|e| e.to_string())?;
        change.why = why;
        Ok((Box::new(change), said))
    })();
    let nothing = result
        .as_ref()
        .ok()
        .and_then(|(change, said)| Some((change.nothing_to_show()?, said)));
    let msg = match nothing {
        Some((text, Some(said))) => Msg::Nothing(format!("{text}\n\n{said}")),
        Some((text, None)) => Msg::Nothing(text),
        None => Msg::Loaded(result),
    };
    let _ = tx.send(msg);
}

/// The terminal as the diff wants it while it runs, and put back however
/// it ends: raw, on the alternate screen, clicks, drags and the wheel
/// reported, and every movement too with the hover on; text pasted
/// bracketed, so a question pasted into `ga`'s box keeps its line breaks;
/// and keys told apart (the kitty keyboard protocol's flag 1, which
/// crossterm reads), so Shift-Enter there adds a line, as Neovim asked
/// for them. In a pane the workspace sends keys so only while the diff
/// asks.
struct Term;

impl Term {
    fn enter(motion: bool) -> io::Result<Term> {
        terminal::enable_raw_mode()?;
        let mut out = io::stdout();
        out.write_all(b"\x1b[?1049h\x1b[?25l\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[?2004h\x1b[>1u")?;
        if motion {
            out.write_all(b"\x1b[?1003h")?;
        }
        out.flush()?;
        Ok(Term)
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        let mut out = io::stdout();
        let _ = out.write_all(
            b"\x1b[<u\x1b[?2004l\x1b[?1003l\x1b[?1006l\x1b[?1002l\x1b[?1000l\x1b[0m\x1b[?25h\x1b[?1049l",
        );
        let _ = out.flush();
        let _ = terminal::disable_raw_mode();
    }
}

/// Set by SIGHUP or SIGTERM: the program ends at the next turn of its loop.
static ENDED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    ENDED.store(true, Ordering::Relaxed);
}

/// Runs the diff on this terminal until it is closed: the change in `dir`
/// against `base` (worked out as M-d does when `None`), or with `working`
/// the working tree's changes.
pub fn run(
    dir: PathBuf,
    base: Option<String>,
    working: bool,
    session: Option<ask::Session>,
) -> ExitCode {
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let (tx, rx): (Sender<Msg>, Receiver<Msg>) = channel();
    let opts = Options::read();
    let mut app = App::new(opts, cols, rows, tx.clone());
    app.session = session.filter(|_| settings::is_on("diff_ask_session"));
    app.opened = Some((dir.clone(), base.clone(), working));
    app.round = host().draws("round");
    {
        let tx = tx.clone();
        let uncommitted = opts.uncommitted;
        std::thread::spawn(move || load(dir, base, working, uncommitted, tx));
    }
    // The pane closed under it (M-w), or the server ending: the loop ends
    // as it does on q, so the ask under way and its partial file go too.
    for signal in [libc::SIGHUP, libc::SIGTERM] {
        // SAFETY: a handler that only stores to an atomic.
        unsafe {
            libc::signal(signal, on_signal as *const () as libc::sighandler_t);
        }
    }
    // Every movement with the hover on: the menu (m, a right click) lights
    // its option under the mouse, whatever else is on.
    let term = match Term::enter(opts.hover) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("switchboard diff: can't take the terminal: {e}");
            return ExitCode::FAILURE;
        }
    };
    // The terminal's colours: its background for the tints and for the
    // `system` theme, which Neovim followed through Ghostty's, its text's
    // for the faint words and the buttons.
    let (colors, _) = switchboard_ui::terminal::ask(ASK_WAIT);
    if let Some(light) = colors.light() {
        switchboard_ui::theme::set_system(light);
    }
    app.opts.light = Options::read().light;
    app.term = colors;
    let mut screen = Screen::default();
    let mut dirty = true;
    let mut last_tick = Instant::now();
    // Alone, the look for a newer release, once the diff has drawn.
    let mut looks = crate::update::allowed();
    // Looked again each hour while the diff stays open: the day's look
    // asks GitHub once its answer is a day old, else reads what it kept.
    let mut looked = Instant::now();
    let looking = std::sync::Arc::new(AtomicBool::new(false));
    while !ENDED.load(Ordering::Relaxed) {
        while let Ok(msg) = rx.try_recv() {
            app.event(msg);
            dirty = true;
        }
        if app.closed() {
            break;
        }
        if last_tick.elapsed() >= Duration::from_millis(100) {
            last_tick = Instant::now();
            if app.tick() {
                dirty = true;
            }
        }
        if dirty {
            let (cols, rows) = terminal::size().unwrap_or((cols, rows));
            let mut buf = ratatui::buffer::Buffer::empty(Rect::new(0, 0, cols, rows));
            let cursor = app.draw(&mut buf);
            let mut bytes = screen.render(&buf, cursor);
            bytes.append(&mut app.out);
            let mut out = io::stdout().lock();
            let _ = out.write_all(&bytes);
            let _ = out.flush();
            dirty = false;
        }
        // The first look once the diff has drawn, then hourly.
        // The settings read anew each hour first, so one turned off since
        // (in Switchboard, or by hand) is followed: no look, and nothing
        // said of one found before.
        let hour = opts.alone && looked.elapsed() >= Duration::from_secs(3600);
        if hour && !looks {
            // The next hour, whatever comes of this one.
            looked = Instant::now();
            crate::host::reread_settings();
            // Every setting read anew is followed, not the look's alone.
            let before = app.opts;
            app.opts = app.opts.read_again();
            app.settings_changed(before);
            if app.opts != before {
                dirty = true;
            }
            if !crate::update::turned_on() && app.newer.take().is_some() {
                dirty = true;
            }
        }
        let hour_up = hour && !looks && !app.installed && crate::update::allowed();
        if ((looks && !dirty) || hour_up) && !looking.swap(true, Ordering::SeqCst) {
            looks = false;
            looked = Instant::now();
            let (tx, looking) = (tx.clone(), looking.clone());
            std::thread::spawn(move || {
                let _ = tx.send(Msg::Newer(crate::update::look(util::now_secs())));
                looking.store(false, Ordering::SeqCst);
            });
        }
        let act = match event::poll(Duration::from_millis(50)) {
            Ok(true) => match event::read() {
                Ok(TermEvent::Key(key)) => app.key(key),
                Ok(TermEvent::Mouse(m)) => app.mouse(m),
                Ok(TermEvent::Paste(text)) => app.paste(&text),
                Ok(TermEvent::Resize(c, r)) => {
                    app.resize(c, r);
                    screen.invalidate();
                    Act::Redraw
                }
                Ok(_) => Act::Same,
                Err(_) => Act::Quit,
            },
            Ok(false) => Act::Same,
            Err(_) => Act::Quit,
        };
        match act {
            Act::Quit => break,
            Act::Redraw => dirty = true,
            Act::Same => {}
        }
    }
    app.stop();
    drop(term);
    // An update started from the settings view is finished, not left half
    // done: said on the terminal given back, with what came of it.
    if app.updating {
        // The terminal is given back, so C-c ends it here: the old binary
        // stays whole until the new one is renamed over it in one step.
        println!("Finishing the update of switchboard-diff… (Ctrl-C leaves it as it was)");
        // The terminal closed (SIGHUP) ends it too, the worker with the
        // process, before anything is put in the binary's place.
        while !ENDED.load(Ordering::Relaxed) {
            let Ok(msg) = rx.recv_timeout(Duration::from_millis(200)) else {
                continue;
            };
            if let Msg::Updated(outcome) = msg {
                match outcome {
                    Ok(crate::update::Outcome::Installed(version)) => {
                        println!("Updated to switchboard-diff {version}.");
                    }
                    Ok(crate::update::Outcome::UpToDate) => {
                        println!("switchboard-diff is up to date.");
                    }
                    Ok(crate::update::Outcome::Withdrawn(version)) => {
                        println!("switchboard-diff {version} is no longer offered.");
                    }
                    Ok(crate::update::Outcome::Replaced) => {
                        println!(
                            "switchboard-diff was replaced since this one started; nothing was changed."
                        );
                    }
                    Ok(crate::update::Outcome::Out(text)) => println!("{text}"),
                    Err(why) => eprintln!("switchboard-diff update: {why}"),
                }
                break;
            }
        }
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use switchboard_ui::kit::Surface;

    fn opts() -> Options {
        Options {
            order: true,
            story: true,
            pages: true,
            progress: true,
            notes: true,
            list_wide: false,
            uncommitted: true,
            ask: true,
            easy: false,
            key_help: true,
            ux2: false,
            hover: true,
            light: false,
            pane_arrows: true,
            haiku: false,
            server: false,
            clipboard: false,
            scroll_bars: false,
            alone: false,
            map: true,
            keep: false,
            menu_mouse: true,
        }
    }

    fn entry(path: &str, status: char, additions: usize, deletions: usize) -> Entry {
        Entry {
            path: path.into(),
            old_path: None,
            status,
            additions,
            deletions,
            binary: false,
            section: Section::Committed,
        }
    }

    /// A change of three files: a README line changed, a function's body
    /// changed, and a file added, with their texts on each side.
    fn app() -> App {
        let (tx, _rx) = channel();
        let mut app = App::new(opts(), 100, 16, tx);
        let header = |p: &str| format!("diff --git a/{p} b/{p}\n--- a/{p}\n+++ b/{p}\n");
        let diff = format!(
            "{}@@ -1,2 +1,2 @@\n # Title\n-Old words here.\n+New words here.\n{}@@ -1,3 +1,3 @@\n function f() {{\n-  return 1;\n+  return 2;\n }}\n{}@@ -0,0 +1 @@\n+export const x = 1;\n",
            header("README.md"),
            header("src/a.js"),
            header("src/new.js"),
        );
        app.give_text("L", "README.md", Some("# Title\nOld words here.\n".into()));
        app.give_text("R", "README.md", Some("# Title\nNew words here.\n".into()));
        app.give_text(
            "L",
            "src/a.js",
            Some("function f() {\n  return 1;\n}\n".into()),
        );
        app.give_text(
            "R",
            "src/a.js",
            Some("function f() {\n  return 2;\n}\n".into()),
        );
        app.give_text("R", "src/new.js", Some("export const x = 1;\n".into()));
        app.loaded(Change {
            top: PathBuf::from("/repo"),
            base: "origin/main".into(),
            why: git::Why::default(),
            left: "L".into(),
            right: "R".into(),
            entries: vec![
                entry("src/a.js", 'M', 1, 1),
                entry("README.md", 'M', 1, 1),
                entry("src/new.js", 'A', 1, 0),
            ],
            diff,
            staged: None,
            raw_commits: String::new(),
            uncommitted: None,
        });
        app
    }

    fn draw(app: &mut App) -> Buffer {
        // The whole screen: the diff, under its header when it runs alone.
        let rows = app.rows + u16::from(app.has_header());
        let mut buf = Buffer::empty(Rect::new(0, 0, app.cols, rows));
        app.draw(&mut buf);
        buf
    }

    fn screen(app: &mut App) -> Vec<String> {
        let buf = draw(app);
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    fn press(app: &mut App, keys: &str) {
        for c in keys.chars() {
            app.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }

    /// The first line holding `text`.
    fn line_with(lines: &[String], text: &str) -> usize {
        lines
            .iter()
            .position(|l| l.contains(text))
            .unwrap_or_else(|| panic!("no {text:?} in {lines:?}"))
    }

    /// The list's part of each line.
    fn list(shown: &[String]) -> Vec<String> {
        shown.iter().map(|l| l.chars().take(30).collect()).collect()
    }

    /// A change of the files given, with no text to show.
    fn app_of(entries: Vec<Entry>, ux2: bool) -> App {
        let (tx, _rx) = channel();
        let mut app = App::new(Options { ux2, ..opts() }, 100, 16, tx);
        app.loaded(Change {
            top: PathBuf::from("/repo"),
            base: "origin/main".into(),
            why: git::Why::default(),
            left: "L".into(),
            right: "R".into(),
            entries,
            diff: String::new(),
            staged: None,
            raw_commits: String::new(),
            uncommitted: None,
        });
        app
    }

    #[test]
    fn the_list_heads_with_the_whole_changes_files_lines_and_bar() {
        for ux2 in [false, true] {
            let mut app = app_of(
                vec![entry("src/a.rs", 'M', 10, 2), entry("src/b.rs", 'A', 3, 0)],
                ux2,
            );
            let shown = screen(&mut app);
            let at = line_with(&shown, "2 files  +13 −2");
            assert!(shown[at].contains("■■■■■"), "{shown:?}");
            assert!(at < line_with(&list(&shown), "a.rs"), "{shown:?}");
            let row = app.totals_row(40);
            let cells = |tone: Tone| {
                row.iter()
                    .filter(|s| s.style == tone.style(&app.theme()))
                    .map(|s| s.content.chars().filter(|c| *c == '■').count())
                    .sum::<usize>()
            };
            assert_eq!((cells(Tone::Done), cells(Tone::Failed)), (4, 1));
        }
    }

    #[test]
    fn a_binary_file_adds_a_file_to_the_totals_and_no_lines() {
        let mut bin = entry("logo.png", 'A', 0, 0);
        bin.binary = true;
        let mut app = app_of(vec![entry("src/a.rs", 'M', 10, 2), bin], true);
        line_with(&screen(&mut app), "2 files  +10 −2");
    }

    #[test]
    fn the_totals_count_only_the_files_the_path_filter_leaves() {
        let mut app = app_of(
            vec![
                entry("src/a.rs", 'M', 10, 2),
                entry("src/b.rs", 'A', 3, 0),
                entry("tests/c.rs", 'A', 7, 1),
            ],
            true,
        );
        line_with(&screen(&mut app), "3 files  +20 −3");
        press(&mut app, "/src");
        let shown = list(&screen(&mut app));
        line_with(&shown, "2 of 3 files  +13 −2");
        line_with(&shown, "1 file hidden by the filter");
        line_with(&shown, "Esc clears it");
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        line_with(&list(&screen(&mut app)), "/ then Esc clears it");
        press(&mut app, "/");
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let shown = list(&screen(&mut app));
        line_with(&shown, "3 files  +20 −3");
        assert!(!shown.iter().any(|l| l.contains("hidden")), "{shown:?}");
    }

    #[test]
    fn a_file_both_staged_and_changed_since_counts_once_filtered_too() {
        let mut staged = entry("a.txt", 'M', 2, 1);
        staged.section = Section::Staged;
        let mut unstaged = entry("a.txt", 'M', 1, 0);
        unstaged.section = Section::Unstaged;
        let mut app = app_of(vec![staged, unstaged, entry("b.txt", 'M', 4, 0)], true);
        line_with(&screen(&mut app), "2 files  +7 −1");
        press(&mut app, "/a.txt");
        let shown = list(&screen(&mut app));
        line_with(&shown, "1 of 2 files  +3 −1");
        line_with(&shown, "1 file hidden by the filter");
    }

    #[test]
    fn totals_too_wide_for_the_list_close_up_before_cutting_the_bar() {
        let mut app = app_of(
            (0..10)
                .map(|n| entry(&format!("src/f{n}.rs"), 'M', 18, 4))
                .collect(),
            true,
        );
        app.path_filter = "f1".into();
        app.sort();
        let row: String = app
            .totals_row(28)
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(row.trim_end(), " 1 of 10 files +18 −4 ■■■■■");
        // Wider still, the words are cut and the bar, its deleted cell
        // last, stays whole.
        let mut app = app_of(
            (0..20)
                .map(|n| entry(&format!("f{n}.rs"), 'M', 184, 36))
                .collect(),
            true,
        );
        app.path_filter = "f1".into();
        app.sort();
        let row = app.totals_row(30);
        let text: String = row.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(width(&text), 30, "{text:?}");
        assert!(text.ends_with(" ■■■■■"), "{text:?}");
        let last = row.last().unwrap();
        assert_eq!(last.content, "", "no faint cells");
        assert_eq!(row[row.len() - 2].content, "■", "the deleted cell kept");
    }

    #[test]
    fn easy_mode_puts_a_button_for_each_key_at_the_bottom_and_a_click_presses_it() {
        let mut app = app();
        app.opts.easy = true;
        // Wide enough for every button and the keys named after them.
        app.resize(220, 16);
        let shown = screen(&mut app);
        let bar = shown.last().unwrap();
        for label in [
            "◀ Chapter",
            "Chapter ▶",
            "◀ Page",
            "Page ▶",
            "Widen",
            "Copy link",
            "Refresh",
            "Ask",
            "Close",
        ] {
            assert!(bar.contains(label), "{label} in {bar:?}");
        }
        assert!(bar.contains("R refresh"), "{bar:?}");
        assert!(
            bar.contains("choose a file"),
            "the keys named after them: {bar:?}"
        );
        app.opts.key_help = false;
        assert!(!screen(&mut app).last().unwrap().contains("choose a file"));
        // A click on Chapter ▶ presses ]c, which says why there is no story.
        let (_, at) = *app
            .buttons
            .iter()
            .find(|(b, _)| *b == Button::Bar(1))
            .unwrap();
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: at.x + 1,
            row: at.y,
            modifiers: KeyModifiers::NONE,
        };
        app.mouse(click);
        let said = app.message.take().map(|(t, _)| t).unwrap_or_default();
        assert!(
            said.starts_with("No story for this diff: Haiku isn't asked here."),
            "{said:?}"
        );
        let (_, close) = *app
            .buttons
            .iter()
            .find(|(b, _)| *b == Button::Bar(8))
            .unwrap();
        let click = MouseEvent {
            column: close.x + 1,
            row: close.y,
            ..click
        };
        assert_eq!(app.mouse(click), Act::Quit);
    }

    /// The diff run alone, as `switchboard-diff`.
    fn alone() -> App {
        let mut app = app();
        app.opts.alone = true;
        app.resize(200, 60);
        app
    }

    fn alt_m() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('m'), KeyModifiers::ALT)
    }

    fn click_on(app: &mut App, button: Button) -> Act {
        let (_, at) = *app.buttons.iter().find(|(b, _)| *b == button).unwrap();
        // Where it was drawn, on the screen: a row down under the header.
        let down = u16::from(app.has_header());
        app.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: at.x + 1,
            row: at.y + down,
            modifiers: KeyModifiers::NONE,
        })
    }

    #[test]
    fn alone_it_heads_its_screen_and_its_row_ends_with_settings_before_close() {
        let mut app = alone();
        let shown = screen(&mut app);
        // Its own header: its name, what it compares, and its version.
        assert!(shown[0].starts_with(" switchboard-diff   /repo · against origin/main"));
        assert!(
            shown[0].ends_with(env!("CARGO_PKG_VERSION")),
            "{:?}",
            shown[0]
        );
        // The diff under it, a row down.
        assert!(shown[1].contains("Changes (3)"), "{:?}", shown[1]);
        // The row of buttons, with no easy mode, Settings before Close.
        let bar = shown.last().unwrap();
        let (settings, close) = (bar.find("Settings").unwrap(), bar.find("Close").unwrap());
        assert!(settings < close, "{bar:?}");
        assert!(
            bar.contains("Refresh") && bar.ends_with("M-m settings"),
            "{bar:?}"
        );

        // Drawn as the kit draws a button: bold in the accent on the panel
        // surface, on the chosen surface under the mouse.
        let theme = app.theme();
        let (_, at) = *app
            .buttons
            .iter()
            .find(|(b, _)| *b == Button::Settings)
            .unwrap();
        let cell = |app: &mut App| draw(app)[(at.x + 1, at.y + 1)].clone();
        let resting = cell(&mut app);
        assert_eq!(resting.bg, Surface::Panel.color(&theme));
        assert_eq!(resting.fg, Tone::Accent.fg(&theme));
        assert!(resting.modifier.contains(ratatui::style::Modifier::BOLD));
        app.mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: at.x + 1,
            row: at.y + 1,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(cell(&mut app).bg, Surface::Chosen.color(&theme));

        // In the workspace: no header, no row without easy mode, and with
        // it no Settings button.
        let mut embedded = super::tests::app();
        embedded.resize(200, 30);
        let shown = screen(&mut embedded);
        assert!(!shown[0].contains("switchboard-diff"), "{:?}", shown[0]);
        assert!(!shown.last().unwrap().contains("Close"));
        embedded.opts.easy = true;
        let bar = screen(&mut embedded).pop().unwrap();
        assert!(
            bar.contains("Close") && !bar.contains("Settings"),
            "{bar:?}"
        );
    }

    #[test]
    fn alone_m_m_and_the_settings_button_open_its_settings_view() {
        let mut app = alone();
        assert_eq!(app.key(alt_m()), Act::Redraw);
        assert!(app.in_settings());
        let shown = screen(&mut app);
        let text = shown.join("\n");
        // The header stays, the view under it: the diff's own options as
        // its manifest names them, the file they are kept in, and the look
        // for a newer release.
        assert!(shown[0].starts_with(" switchboard-diff"), "{:?}", shown[0]);
        assert!(text.contains("  Settings"), "{text}");
        assert!(text.contains("settings.json · none changed"), "{text}");
        for (label, key) in [
            ("Files in reading order", "diff_order"),
            ("Diff told as a story", "diff_story"),
            ("Ask Claude about the diff", "diff_ask"),
            ("Check for a newer switchboard-diff", "diff_update_check"),
        ] {
            let row = shown.iter().find(|l| l.contains(label)).unwrap();
            assert!(row.contains(key) && row.contains(" on"), "{row:?}");
        }
        assert!(
            text.contains("OPTIONS") && text.contains("UPDATES"),
            "{text}"
        );
        // What only the workspace has a use for is none of its rows.
        assert!(!text.contains("diff_ask_session") && !text.contains("diff_beside_share"));
        // The chosen row is opened to its default and what Enter does.
        assert!(text.contains("default on  ·  Enter: on or off"), "{text}");
        // The row of buttons stays, Settings lit, and says how to go back.
        let bar = shown.last().unwrap();
        assert!(bar.ends_with("Esc back to the diff"), "{bar:?}");
        let (_, at) = *app
            .buttons
            .iter()
            .find(|(b, _)| *b == Button::Settings)
            .unwrap();
        let lit = draw(&mut app)[(at.x + 1, at.y + 1)].clone();
        assert_ne!(lit.bg, Surface::Panel.color(&app.theme()));

        // Tab and Shift-Tab reach each row, coming round.
        app.key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert!(screen(&mut app).join("\n").contains("▌ Check for a newer"));
        app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        // j and k move through the rows; Esc goes back to the diff.
        press(&mut app, "j");
        let shown = screen(&mut app).join("\n");
        assert!(shown.contains("each with a heading"), "{shown}");
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.in_settings());
        assert!(screen(&mut app).join("\n").contains("README.md"));

        // A click on the Settings button opens it, and M-m closes it.
        screen(&mut app);
        assert_eq!(click_on(&mut app, Button::Settings), Act::Redraw);
        assert!(app.in_settings());
        app.key(alt_m());
        assert!(!app.in_settings());

        // A newer release shows at the header's right end and as a row.
        app.event(Msg::Newer(Some(crate::update::Newer {
            version: "9.9.0".into(),
            published: None,
        })));
        let shown = screen(&mut app);
        assert!(
            shown[0].ends_with("9.9.0 is out · switchboard-diff update"),
            "{:?}",
            shown[0]
        );
        app.key(alt_m());
        let text = screen(&mut app).join("\n");
        assert!(text.contains("Update to 9.9.0"), "{text}");
        assert!(text.contains("Runs switchboard-diff update"), "{text}");

        // A button pressed over the view goes back to the diff: Close only
        // closes the view, and the diff stays open.
        let close = Button::Bar(BUTTONS.len() - 1);
        assert_eq!(click_on(&mut app, close), Act::Redraw);
        assert!(!app.in_settings());

        // On a screen too narrow for the row, Settings and Close stay.
        app.resize(80, 30);
        let bar = screen(&mut app).pop().unwrap();
        assert!(bar.contains("Settings") && bar.contains("Close"), "{bar:?}");
        assert!(!bar.contains("Refresh") && !bar.contains("Ask"), "{bar:?}");

        // With nothing to show, the view still opens, and a change read
        // after takes the screen back.
        app.nothing = Some("Nothing here".into());
        app.key(alt_m());
        assert!(app.in_settings());
        app.key(alt_m());
        let change = app.change.clone().unwrap();
        // So does m, the menu's key.
        press(&mut app, "m");
        assert!(app.in_settings());
        app.key(alt_m());
        // The row stays on that screen, and its Settings button opens the
        // view rather than closing the diff.
        let bar = screen(&mut app).pop().unwrap();
        assert!(bar.contains("Settings") && bar.contains("Close"), "{bar:?}");
        assert!(!bar.contains("Chapter"), "only what works there: {bar:?}");
        assert_eq!(click_on(&mut app, Button::Settings), Act::Redraw);
        assert!(app.in_settings());
        app.key(alt_m());
        app.event(Msg::Loaded(Ok((Box::new(change), None))));
        assert!(app.nothing.is_none());

        // From a commit of the history, the Settings button opens the
        // branch's diff's view, never one of the commit's own.
        let mut inner = alone();
        inner.in_history = true;
        inner.toggle_settings();
        assert!(!inner.in_settings());
        assert_eq!(inner.wants, Some(Wants::Settings));
    }

    #[test]
    fn in_the_workspace_m_m_is_not_taken_and_the_menu_points_at_switchboards_settings() {
        let mut app = app();
        app.resize(200, 30);
        assert_eq!(app.key(alt_m()), Act::Same);
        assert!(!app.in_settings());
        // m in the file list opens the diff's menu, which says where the
        // settings are.
        press(&mut app, "m");
        let text = screen(&mut app).join("\n");
        assert!(
            text.contains("Settings are in Switchboard's Settings (Diff)"),
            "{text}"
        );
        assert!(text.contains("Refresh") && text.contains("Close"), "{text}");
        // Chosen with no server to open them, it says where they are.
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!app.in_settings());
        let said = app.message.take().map(|(t, _)| t).unwrap_or_default();
        assert!(
            said.contains("Switchboard's Settings, under Diff"),
            "{said:?}"
        );
        // The palette's reset closes it, so the keys it sends after reach
        // the diff.
        press(&mut app, "m");
        app.key(KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::CONTROL));
        assert!(app.menu.is_none());
        // Esc closes the menu and leaves the diff open.
        press(&mut app, "m");
        assert_eq!(
            app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Act::Redraw
        );
        assert!(app.menu.is_none());

        // Alone, its first option is the settings view.
        let mut app = alone();
        press(&mut app, "m");
        let text = screen(&mut app).join("\n");
        assert!(!text.contains("Switchboard's Settings"), "{text}");
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.in_settings());
    }

    #[test]
    fn a_story_turned_off_in_the_settings_stops_being_told_and_is_dropped() {
        let mut app = app();
        app.show_story(Some(commits_story()));
        app.telling = Telling::Waiting;
        app.cover = Some(Instant::now());
        assert!(app.story.is_some());
        let before = app.opts;
        app.opts.story = false;
        app.settings_changed(before);
        assert!(app.story.is_none() && app.told.is_none());
        assert_ne!(app.telling, Telling::Waiting);
        assert!(app.cover.is_none());
        // The list is the files alone again.
        let listed = screen(&mut app).join("\n");
        assert!(listed.contains("README.md"), "{listed}");

        // A chapter open as a page, with pages turned off, goes back to the
        // whole diff.
        app.page = Some(1);
        let before = app.opts;
        app.opts.pages = false;
        app.settings_changed(before);
        assert_eq!(app.page, None);

        // The file list follows its width's setting at once.
        let before = app.opts;
        app.opts.list_wide = !before.list_wide;
        app.list_width = 0;
        app.settings_changed(before);
        assert_eq!(app.list_width, app.fit_width());
    }

    /// A change running to the working tree, its commits' part changing
    /// src/a.js and src/b.js, `b` the line src/b.js has on disk now, and
    /// `dirty` the files not committed: src/new.js not at all.
    fn to_work_tree(b: &str, dirty: &[&str], extra: &[&str]) -> Change {
        let header = |p: &str| format!("diff --git a/{p} b/{p}\n--- a/{p}\n+++ b/{p}\n");
        let committed_diff = format!(
            "{}@@ -1 +1 @@\n-a = 1\n+a = 2\n{}@@ -1 +1 @@\n-b = 1\n+b = 2\n",
            header("src/a.js"),
            header("src/b.js")
        );
        let mut diff = format!(
            "{}@@ -1 +1 @@\n-a = 1\n+a = 2\n{}@@ -1 +1 @@\n-b = 1\n+{b}\n{}@@ -0,0 +1 @@\n+fresh\n",
            header("src/a.js"),
            header("src/b.js"),
            header("src/new.js")
        );
        let mut entries = vec![
            entry("src/a.js", 'M', 1, 1),
            entry("src/b.js", 'M', 1, 1),
            entry("src/new.js", 'A', 1, 0),
        ];
        for path in extra {
            diff.push_str(&format!("{}@@ -0,0 +1 @@\n+more\n", header(path)));
            entries.push(entry(path, 'A', 1, 0));
        }
        let committed = Change {
            top: PathBuf::from("/repo"),
            base: "origin/main".into(),
            left: "L".into(),
            right: "H".into(),
            entries: vec![entry("src/a.js", 'M', 1, 1), entry("src/b.js", 'M', 1, 1)],
            diff: committed_diff,
            ..Change::default()
        };
        Change {
            top: PathBuf::from("/repo"),
            base: "origin/main".into(),
            left: "L".into(),
            right: "H".into(),
            entries,
            diff,
            uncommitted: Some(git::Uncommitted {
                dirty: dirty.iter().map(|p| p.to_string()).collect(),
                committed: Box::new(committed),
            }),
            ..Change::default()
        }
    }

    /// The diff of `to_work_tree`, with each file's sides.
    fn working_app() -> App {
        working_app_with(opts())
    }

    fn working_app_with(opts: Options) -> App {
        let (tx, _rx) = channel();
        let mut app = App::new(opts, 100, 16, tx);
        app.give_text("L", "src/a.js", Some("a = 1\n".into()));
        app.give_text(git::WORK_TREE, "src/a.js", Some("a = 2\n".into()));
        app.give_text("L", "src/b.js", Some("b = 1\n".into()));
        app.give_text(git::WORK_TREE, "src/b.js", Some("b = 3\n".into()));
        app.give_text(git::WORK_TREE, "src/new.js", Some("fresh\n".into()));
        app.loaded(to_work_tree("b = 3", &["src/b.js", "src/new.js"], &[]));
        app
    }

    /// The story of the commits' part alone, as Haiku tells it.
    fn commits_story() -> Story {
        let change = to_work_tree("b = 3", &[], &[]);
        let committed = change.told_part();
        let (_, hunks) = story::split(&committed.runs());
        let answer = r#"{"chapters": [
            {"title": "Bump a", "why": "a moves.", "hunks": [{"id": "src/a.js#1", "note": "Check a."}]},
            {"title": "Bump b", "why": "b follows.", "hunks": [{"id": "src/b.js#1", "note": "Check b."}]}
        ]}"#;
        let (_, paths) = story::key_of(committed);
        story::parse(answer, &paths, order::generated, &hunks).unwrap()
    }

    /// With UX 2.0, a change adding a file of `lines` lines at each of
    /// `paths`, the list on the second.
    fn added_files(paths: &[&str], lines: usize) -> App {
        let (tx, _rx) = channel();
        let mut app = App::new(
            Options {
                ux2: true,
                ..opts()
            },
            120,
            24,
            tx,
        );
        let mut diff = String::new();
        let mut entries = Vec::new();
        for p in paths {
            let text: String = (1..=lines).map(|i| format!("{p} line {i}\n")).collect();
            diff.push_str(&format!(
                "diff --git a/{p} b/{p}\n--- /dev/null\n+++ b/{p}\n@@ -0,0 +1,{lines} @@\n"
            ));
            for line in text.lines() {
                diff.push_str(&format!("+{line}\n"));
            }
            app.give_text("R", p, Some(text));
            entries.push(entry(p, 'A', lines, 0));
        }
        app.loaded(Change {
            top: PathBuf::from("/repo"),
            base: "origin/main".into(),
            why: git::Why::default(),
            left: "L".into(),
            right: "R".into(),
            entries,
            diff,
            staged: None,
            raw_commits: String::new(),
            uncommitted: None,
        });
        app.show_at(1);
        app
    }

    #[test]
    fn the_file_is_never_greyed_and_the_list_is_while_the_file_has_the_keys() {
        let mut app = added_files(&["src/one.js", "src/two.js"], 3);
        let file = app.diff_area();
        // The colors of the text in the list and in the file, and whether
        // it is drawn faint, as text in the terminal's own color is.
        let texts = |app: &mut App| {
            let buf = draw(app);
            let of = |x0: u16, x1: u16| -> Vec<(Color, bool)> {
                (0..file.bottom())
                    .flat_map(|y| (x0..x1).map(move |x| (x, y)))
                    .filter(|&at| buf[at].symbol().trim() != "")
                    .map(|at| {
                        (
                            buf[at].fg,
                            buf[at].modifier.contains(ratatui::style::Modifier::DIM),
                        )
                    })
                    .collect()
            };
            (of(0, file.x - 1), of(file.x, file.right()))
        };
        // How much of one pane's text changed color between two draws.
        let changed = |a: &[(Color, bool)], b: &[(Color, bool)]| {
            assert_eq!(a.len(), b.len());
            a.iter().zip(b).filter(|(a, b)| a != b).count() * 2 > a.len()
        };
        // With the keys in the list, the file it shows is as it is with
        // the keys in it: no cell of it greyed.
        let (list, file_from_list) = texts(&mut app);
        app.focus = app.main_side();
        let (greyed_list, shown) = texts(&mut app);
        assert!(changed(&list, &greyed_list), "the list had the keys");
        assert_eq!(shown, file_from_list, "the file is lit from the list");
        // And nothing of the screen is greyed while the list has them.
        app.focus = Focus::List;
        let from_list = draw(&mut app);
        let mut greyed = from_list.clone();
        kit::unfocused(&mut greyed, file, &app.theme());
        let differ = |a: &Buffer, b: &Buffer| {
            (0..file.bottom())
                .flat_map(|y| (file.x..file.right()).map(move |x| (x, y)))
                .any(|at| a[at] != b[at])
        };
        assert!(differ(&from_list, &greyed), "greying the file would show");
    }

    #[test]
    fn with_ux2_an_added_files_lines_are_numbered_and_tinted_as_added_and_a_footer_names_the_keys()
    {
        let mut app = added_files(&["src/one.js", "src/two.js"], 3);
        // With the keys in the file.
        app.focus = app.main_side();
        let buf = draw(&mut app);
        app.focus = Focus::List;
        let shown = screen(&mut app);
        let first = line_with(&shown, "src/two.js line 1");
        assert!(
            shown[first].contains("   1 src/two.js line 1"),
            "{:?}",
            shown[first]
        );
        assert!(shown[first + 2].contains("   3 src/two.js line 3"));
        // Tinted green from the number to the side's right edge.
        let added = app.palette().2;
        let x = app.diff_area().x;
        assert_eq!(buf[(x + 1, first as u16)].bg, added);
        assert_eq!(buf[(x + 40, first as u16)].bg, added);
        // The footer on the last row, the list's keys while it has the
        // focus, the diff's once a side has it.
        assert_eq!(shown[23].trim(), "↑↓ move  ↵ read  esc close");
        press(&mut app, "l");
        assert!(screen(&mut app)[23].contains("←→ sides  esc list"));
        // A click on the footer chooses nothing in the list.
        let at_before = app.at;
        app.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row: 23,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.at, at_before);
    }

    #[test]
    fn with_ux2_and_no_story_what_only_the_working_tree_changes_is_listed_last_under_not_committed_yet()
     {
        let mut app = working_app_with(Options {
            ux2: true,
            story: false,
            ..opts()
        });
        let names = list(&screen(&mut app));
        let heading = line_with(&names, "NOT COMMITTED YET");
        assert!(names[heading].trim_end().ends_with('1'), "{names:?}");
        assert!(names[0].trim_end().ends_with('2'), "{names:?}");
        // b.js, changed by the commits too, stays among theirs, marked.
        assert!(line_with(&names, "a.js") < heading && line_with(&names, "b.js") < heading);
        assert!(line_with(&names, "new.js") > heading, "{names:?}");
    }

    /// With UX 2.0, a change to the working tree whose commits change a.js
    /// and Cargo.lock, and whose working tree alone new.js and
    /// package-lock.json; told as a story with `story`.
    fn generated_and_apart(story: bool) -> App {
        let (tx, _rx) = channel();
        let mut app = App::new(
            Options {
                ux2: true,
                story,
                ..opts()
            },
            100,
            16,
            tx,
        );
        for (side, text) in [("L", "x = 1\n"), (git::WORK_TREE, "x = 2\n")] {
            for p in ["src/a.js", "Cargo.lock", "src/new.js", "package-lock.json"] {
                app.give_text(side, p, Some(text.into()));
            }
        }
        let mut change = to_work_tree("b = 3", &["src/new.js", "package-lock.json"], &[]);
        change.entries = ["src/a.js", "Cargo.lock", "src/new.js", "package-lock.json"]
            .iter()
            .map(|p| entry(p, 'M', 1, 1))
            .collect();
        if let Some(u) = &mut change.uncommitted {
            u.committed.entries =
                vec![entry("src/a.js", 'M', 1, 1), entry("Cargo.lock", 'M', 1, 1)];
        }
        app.loaded(change);
        app
    }

    #[test]
    fn generated_files_stay_last_after_whats_not_committed_yet() {
        let mut app = generated_and_apart(false);
        let names = list(&screen(&mut app));
        let apart = line_with(&names, "NOT COMMITTED YET");
        let generated = line_with(&names, "GENERATED, LAST");
        assert!(line_with(&names, "a.js") < apart, "{names:?}");
        assert!(apart < line_with(&names, "new.js"), "{names:?}");
        assert!(line_with(&names, "new.js") < generated, "{names:?}");
        assert!(generated < line_with(&names, "Cargo.lock"), "{names:?}");
        assert!(generated < line_with(&names, "package-lock"), "{names:?}");
        assert_eq!(
            names.iter().filter(|l| l.contains("GENERATED")).count(),
            1,
            "{names:?}"
        );
    }

    #[test]
    fn told_as_a_story_the_chapters_place_generated_files_each_heading_once() {
        let mut app = generated_and_apart(true);
        let names = list(&screen(&mut app));
        for heading in ["Committed", "NOT COMMITTED YET"] {
            assert_eq!(
                names.iter().filter(|l| l.contains(heading)).count(),
                1,
                "{heading}: {names:?}"
            );
        }
        assert!(line_with(&names, "Cargo.lock") < line_with(&names, "NOT COMMITTED YET"));
        assert!(line_with(&names, "NOT COMMITTED YET") < line_with(&names, "package-lock"));
    }

    #[test]
    fn with_ux2_the_lists_shift_j_and_k_scroll_the_chosen_file_and_stop_at_its_ends() {
        let mut app = added_files(&["a.js", "b.js", "c.js"], 40);
        let h = app.body_height();
        assert!(h < 40, "{h}");
        let n = app.lines_of(1).len();
        // J held stops with the second file's last line at the bottom.
        for _ in 0..10 {
            press(&mut app, "J");
        }
        assert_eq!((app.at, app.view, app.top), (1, 1, n - h));
        assert!(screen(&mut app).iter().any(|l| l.contains("b.js line 40")));
        // K held stops with its first line at the top.
        for _ in 0..10 {
            press(&mut app, "K");
        }
        assert_eq!((app.at, app.view, app.top), (1, 1, 0));
        // The diff's own j stops at the file's last line too.
        press(&mut app, "l");
        for _ in 0..n {
            press(&mut app, "j");
        }
        assert_eq!((app.at, app.cur), (1, n - 1));
        // A file shorter than the diff's area doesn't move.
        let mut short = added_files(&["a.js", "b.js", "c.js"], 3);
        press(&mut short, "J");
        assert_eq!((short.at, short.view, short.top), (1, 1, 0));
    }

    #[test]
    fn with_ux2_the_cursor_stays_in_its_file_and_the_view_scrolls_on_past_it() {
        let mut app = added_files(&["a.js", "b.js", "c.js"], 40);
        let h = app.body_height();
        let n = app.lines_of(1).len();
        let ctrl = |app: &mut App, c: char| {
            app.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
        };
        press(&mut app, "l");
        // k at the file's first line stays there.
        press(&mut app, "ggk");
        assert_eq!((app.at, app.cur), (1, 0));
        // C-d near the end stops at the last line, and so does a count.
        app.cur = n - 3;
        ctrl(&mut app, 'd');
        assert_eq!((app.at, app.cur), (1, n - 1));
        assert!(app.message.is_none());
        press(&mut app, "gg99j");
        assert_eq!((app.at, app.cur), (1, n - 1));
        // j there stays, saying once how to go on.
        press(&mut app, "j");
        assert_eq!((app.at, app.cur), (1, n - 1));
        let said = app.message.clone().map(|(t, _)| t);
        assert_eq!(said.as_deref(), Some("End of b.js · ]f for the next file"));
        press(&mut app, "j");
        press(&mut app, "j");
        assert_eq!((app.at, app.cur), (1, n - 1));
        assert!(app.message.is_none());
        // ]f goes to the next file's first change and [f back, the focus
        // still in the diff.
        press(&mut app, "]f");
        assert_eq!((app.at, app.cur, app.focus), (2, 0, Focus::New));
        press(&mut app, "]f");
        assert_eq!(app.at, 2);
        press(&mut app, "[f");
        assert_eq!((app.at, app.cur), (1, 0));
        press(&mut app, "2[f");
        assert_eq!(app.at, 0);
        press(&mut app, "]f");
        // The wheel scrolls into the next file, the cursor on its file's
        // last line in sight and the list's file unchanged.
        app.scroll_diff(n - 3, true);
        assert_eq!((app.at, app.view, app.top, app.cur), (1, 1, n - 3, n - 3));
        app.scroll_diff(2, true);
        assert_eq!((app.at, app.cur), (1, n - 1));
        let shown = screen(&mut app);
        assert!(shown.iter().any(|l| l.contains("c.js line 1")), "{shown:?}");
        // Its file scrolled out, the cursor is left where it is.
        for _ in 0..4 {
            app.scroll_diff(h, true);
        }
        assert_eq!((app.at, app.view, app.cur), (1, 2, n - 1));
        let shown = screen(&mut app);
        assert!(
            shown.iter().any(|l| l.contains("c.js line 40")),
            "{shown:?}"
        );
        assert_eq!((app.at, app.view), (1, 2));
        // Scrolled back, it is on its file's first line in sight again.
        app.scroll_diff(h, false);
        assert_eq!(app.at, 1);
        assert!(app.rows_from_view(1, app.cur).is_some_and(|d| d < h));
        // A key that moves it brings the view back to it.
        for _ in 0..4 {
            app.scroll_diff(h, true);
        }
        press(&mut app, "k");
        assert_eq!(app.at, 1);
        assert!(app.rows_from_view(1, app.cur).is_some_and(|d| d < h));
    }

    #[test]
    fn with_ux2_the_list_sets_whats_not_committed_apart_the_why_heads_the_diff_and_the_files_read_as_one_scroll()
     {
        let (tx, _rx) = channel();
        let mut app = App::new(
            Options {
                ux2: true,
                ..opts()
            },
            160,
            30,
            tx,
        );
        app.give_text("L", "src/a.js", Some("a = 1\n".into()));
        app.give_text(git::WORK_TREE, "src/a.js", Some("a = 2\n".into()));
        app.give_text("L", "src/b.js", Some("b = 1\n".into()));
        app.give_text(git::WORK_TREE, "src/b.js", Some("b = 3\n".into()));
        app.give_text(git::WORK_TREE, "src/new.js", Some("fresh\n".into()));
        app.give_text(git::WORK_TREE, "Cargo.lock", Some("more\n".into()));
        app.loaded(to_work_tree(
            "b = 3",
            &["src/b.js", "src/new.js"],
            &["Cargo.lock"],
        ));
        app.told(commits_story());
        let shown = screen(&mut app);
        let names = list(&shown);
        let at = |names: &[String], text: &str| {
            names
                .iter()
                .position(|l| l.contains(text))
                .unwrap_or_else(|| panic!("{text} in {names:?}"))
        };
        // The list: the story's two chapters counted, what isn't committed
        // under a label of its own, the generated file last under its own.
        assert!(names[0].contains("STORY") && names[0].trim_end().ends_with('2'));
        assert!(
            at(&names, "1 Bump a") < at(&names, "a.js")
                && at(&names, "2 Bump b") < at(&names, "b.js")
                && at(&names, "b.js") < at(&names, "NOT COMMITTED YET")
                && at(&names, "NOT COMMITTED YET") < at(&names, "new.js")
                && at(&names, "new.js") < at(&names, "GENERATED, LAST")
                && at(&names, "GENERATED, LAST") < at(&names, "Cargo.lock"),
            "{names:?}"
        );
        assert!(
            names[at(&names, "NOT COMMITTED YET")]
                .trim_end()
                .ends_with('1')
        );
        assert!(names[at(&names, "b.js")].contains('●'));
        assert!(names[at(&names, "new.js")].contains('●'));
        assert!(!names[at(&names, "a.js")].contains('●'));
        assert!(!shown.iter().any(|l| l.contains("1/2")), "{shown:?}");
        // Over the diff, the chapter, its why and a rule; one bar.
        assert!(
            shown[0].contains("Chapter 1 of 2  Bump a"),
            "{:?}",
            shown[0]
        );
        assert!(shown[0].contains("against origin/main"), "{:?}", shown[0]);
        assert!(shown[1].contains("a moves."), "{:?}", shown[1]);
        assert!(shown[2].contains("────"), "{:?}", shown[2]);
        assert!(shown[3].contains("src/a.js   +1 −1"), "{:?}", shown[3]);
        assert!(shown[3].contains("origin/main  ⟷  HEAD"), "{:?}", shown[3]);
        // The hunk's note a card under it.
        let changed = at(&shown, "a = 2");
        let card = at(&shown, "╭ ✎ Note");
        assert!(changed < card, "{shown:?}");
        assert!(shown[card + 1].contains("│ Check a."), "{shown:?}");
        // The next file follows, its bar under a rule.
        let next = at(&shown, "src/b.js   +1 −1");
        assert!(card < next && shown[next - 1].contains("────"), "{shown:?}");
        assert!(shown[next].contains("origin/main  ⟷  working tree"));
        // A bar too narrow for a long path cuts the path, never its counts
        // or what happened to the file.
        let long = entry("src/components/settings/placeholder.png", 'A', 0, 0);
        let bar: String = app
            .bar_ux2(&long, 40)
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert!(bar.contains("placeholder.png   +0 −0  added"), "{bar:?}");
        // An added file says so on its bar.
        assert!(
            shown
                .iter()
                .any(|l| l.contains("src/new.js   +1 −0  added")),
            "{shown:?}"
        );
        // Moving on past a.js's last line stays in a.js; ]f goes into
        // b.js, its chapter now over the diff.
        press(&mut app, "l");
        for _ in 0..8 {
            press(&mut app, "j");
        }
        assert_eq!(app.current().unwrap().path, "src/a.js");
        press(&mut app, "]f");
        assert_eq!(app.current().unwrap().path, "src/b.js");
        let shown = screen(&mut app);
        assert!(
            shown[0].contains("Chapter 2 of 2  Bump b"),
            "{:?}",
            shown[0]
        );
        // And back into a.js.
        press(&mut app, "[f");
        assert_eq!(app.current().unwrap().path, "src/a.js");
        // The wheel scrolls through the files as one, as far as the last
        // file's end at the bottom, the cursor left in its file.
        app.resize(160, 12);
        press(&mut app, "gg");
        for _ in 0..10 {
            app.scroll_diff(3, true);
        }
        assert_eq!(app.current().unwrap().path, "src/a.js");
        let shown = screen(&mut app);
        assert!(shown[10].contains("more"), "{shown:?}");
        assert!(
            shown[11].contains("↑↓ move"),
            "the footer under it: {shown:?}"
        );
        // The uncommitted chapter over its file, in the needs tone's words.
        let fresh = app
            .order
            .iter()
            .position(|&i| app.entries()[i].path == "src/new.js")
            .unwrap();
        app.show_at(fresh);
        let shown = screen(&mut app);
        assert!(
            shown[0].contains("Not committed yet") && !shown[0].contains("Chapter"),
            "{:?}",
            shown[0]
        );
        // Read again with a file more, the view past the end of what its
        // file now holds: kept within it, the shown file still shown.
        app.view = 0;
        app.top = 500;
        app.give_text(git::WORK_TREE, "src/c.js", Some("more\n".into()));
        app.refreshed(to_work_tree(
            "b = 3",
            &["src/b.js", "src/new.js"],
            &["Cargo.lock", "src/c.js"],
        ));
        assert_eq!(app.current().unwrap().path, "src/new.js");
        let _ = screen(&mut app);
        press(&mut app, "j");
        // A story that places a generated file makes a chapter of it, which
        // is drawn as the generated files' label and not counted as told.
        let change = to_work_tree("b = 3", &[], &[]);
        let committed = change.told_part();
        let (_, hunks) = story::split(&committed.runs());
        let (_, mut paths) = story::key_of(committed);
        paths.push("Cargo.lock".into());
        let answer = r#"{"chapters": [
            {"title": "Bump a", "why": "a moves.", "hunks": [{"id": "src/a.js#1", "note": "Check a."}]},
            {"title": "Bump b", "why": "b follows.", "hunks": [{"id": "src/b.js#1", "note": "Check b."}]}
        ]}"#;
        app.told(story::parse(answer, &paths, order::generated, &hunks).unwrap());
        app.resize(160, 30);
        let names = list(&screen(&mut app));
        assert!(names[0].contains("STORY") && names[0].trim_end().ends_with('2'));
        assert!(
            at(&names, "GENERATED, LAST") < at(&names, "Cargo.lock"),
            "{names:?}"
        );
        assert!(
            !names.iter().any(|l| l.contains("Generated files")),
            "{names:?}"
        );
        let lock = app
            .order
            .iter()
            .position(|&i| app.entries()[i].path == "Cargo.lock")
            .unwrap();
        app.show_at(lock);
        let shown = screen(&mut app);
        assert!(
            shown[0].contains("Generated, last") && !shown[0].contains("Chapter"),
            "{:?}",
            shown[0]
        );
    }

    #[test]
    fn whats_not_committed_is_marked_and_listed_in_a_last_chapter_after_the_commits_story() {
        let mut app = working_app();
        // Wide enough for a side's bar to say it whole.
        app.resize(160, 16);
        // No story of the commits (Haiku isn't asked here): they go first,
        // in a chapter of their own.
        let names = list(&screen(&mut app));
        let at = |names: &[String], text: &str| {
            names
                .iter()
                .position(|l| l.contains(text))
                .unwrap_or_else(|| panic!("{text} in {names:?}"))
        };
        assert!(
            at(&names, "1/2 Committed") < at(&names, "a.js")
                && at(&names, "b.js") < at(&names, "2/2 Not committed yet")
                && at(&names, "2/2 Not committed yet") < at(&names, "new.js"),
            "{names:?}"
        );
        // The story told of the commits: its chapters, then the files only
        // uncommitted, one in both kept in its committed chapter.
        app.told(commits_story());
        let shown = screen(&mut app);
        let names = list(&shown);
        assert!(
            at(&names, "1/3 Bump a") < at(&names, "a.js")
                && at(&names, "2/3 Bump b") < at(&names, "b.js")
                && at(&names, "b.js") < at(&names, "3/3 Not committed yet")
                && at(&names, "3/3 Not committed yet") < at(&names, "new.js"),
            "{names:?}"
        );
        let marked: Vec<&String> = names.iter().filter(|l| l.contains('●')).collect();
        assert_eq!(marked.len(), 2, "b.js and new.js marked: {names:?}");
        assert!(!names[at(&names, "a.js")].contains('●'));
        assert_eq!(app.current().unwrap().path, "src/a.js");
        assert!(!shown[0].contains("uncommitted"), "{:?}", shown[0]);
        assert!(
            shown.iter().any(|l| l.contains("▎ Check a.")),
            "a file changed only in its commits keeps its notes"
        );
        press(&mut app, "j");
        let shown = screen(&mut app);
        assert_eq!(app.current().unwrap().path, "src/b.js");
        assert!(shown[0].contains("● uncommitted"), "{:?}", shown[0]);
        assert!(
            !shown.iter().any(|l| l.contains("Check b.")),
            "no note at the committed lines of a file changed since: {shown:?}"
        );
        assert!(
            shown.iter().any(|l| l.contains("b = 3")),
            "its new side the file on disk"
        );
        // yl gives no link to a line not committed.
        press(&mut app, "jyl");
        assert_eq!(
            app.message.take().unwrap().0,
            "src/new.js has uncommitted changes, so this side has no GitHub link yet"
        );
        press(&mut app, "ggyl");
        assert_eq!(app.message.take().unwrap().0, "origin isn't on GitHub");
        press(&mut app, "yS");
        assert!(app.message.take().unwrap().0.contains("3 chapters"));
    }

    #[test]
    fn read_again_the_commits_story_is_kept_and_whats_not_committed_comes_in() {
        let mut app = working_app();
        app.told(commits_story());
        press(&mut app, "jj");
        assert_eq!(app.current().unwrap().path, "src/new.js");
        // A file written since, while the commits stay as they were.
        app.refreshed(to_work_tree(
            "b = 3",
            &["src/b.js", "src/c.js", "src/new.js"],
            &["src/c.js"],
        ));
        assert_eq!(app.telling, Telling::Told, "the story kept");
        let titles: Vec<String> = app
            .story
            .as_ref()
            .unwrap()
            .chapters
            .iter()
            .map(|c| c.title.clone())
            .collect();
        assert_eq!(titles, ["Bump a", "Bump b", "Not committed yet"]);
        assert_eq!(
            app.story.as_ref().unwrap().chapters[2].files,
            ["src/c.js", "src/new.js"]
        );
        assert_eq!(
            app.current().unwrap().path,
            "src/new.js",
            "the cursor kept on its file"
        );
        // A new commit: the story told is of commits no longer the
        // branch's, so it goes, the commits in a chapter of their own until
        // theirs is told.
        let mut moved = to_work_tree("b = 3", &["src/new.js"], &[]);
        if let Some(u) = &mut moved.uncommitted {
            u.committed.diff = u.committed.diff.replace("+b = 2", "+b = 3");
        }
        // What the base is to the branch stays the open change's.
        if let Some(open) = &mut app.change {
            open.why = git::Why::NoPr;
        }
        app.refreshed(moved);
        assert_eq!(
            app.change.as_ref().unwrap().against(),
            "against origin/main (no PR)"
        );
        assert_eq!(
            app.telling,
            Telling::Untold("Haiku isn't asked here".into())
        );
        let titles: Vec<String> = app
            .story
            .as_ref()
            .unwrap()
            .chapters
            .iter()
            .map(|c| c.title.clone())
            .collect();
        assert_eq!(titles, ["Committed", "Not committed yet"]);
        // An answer to the story asked before is dropped.
        let before = app.tells;
        app.event(Msg::Heard(
            before + 1,
            Heard::Told(Box::new(commits_story())),
        ));
        assert_eq!(
            app.telling,
            Telling::Untold("Haiku isn't asked here".into())
        );
    }

    #[test]
    fn with_nothing_committed_the_uncommitted_chapter_stands_alone_and_nothing_at_all_is_said() {
        let (tx, _rx) = channel();
        let mut app = App::new(opts(), 100, 16, tx);
        let mut change = to_work_tree("b = 3", &["src/new.js"], &[]);
        change.entries.retain(|e| e.path == "src/new.js");
        if let Some(u) = &mut change.uncommitted {
            u.committed.entries.clear();
            u.committed.diff.clear();
        }
        app.loaded(change);
        assert_eq!(
            app.telling,
            Telling::Untold("nothing is committed yet".into())
        );
        assert_eq!(app.cover, None, "no story to wait for under a cover");
        let names = list(&screen(&mut app));
        assert!(
            names.iter().any(|l| l.contains("1/1 Not committed yet")),
            "{names:?}"
        );
        // Nothing to show: outside the workspace, the diff's own screen
        // says so, and any key closes it.
        let (tx, _rx) = channel();
        let mut app = App::new(opts(), 100, 16, tx);
        app.event(Msg::Nothing(
            "Nothing to show: no commits past origin/main and no uncommitted changes".into(),
        ));
        assert!(!app.closed());
        let shown = screen(&mut app);
        assert!(
            shown
                .iter()
                .any(|l| l.contains("no commits past origin/main and no uncommitted changes")),
            "{shown:?}"
        );
        assert_eq!(
            app.key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
            Act::Quit
        );
    }

    #[test]
    fn v_selects_lines_on_a_side_and_yp_copies_them_as_a_range() {
        let mut app = app();
        press(&mut app, "j");
        assert_eq!(app.current().unwrap().path, "src/a.js");
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.focus, Focus::New);
        press(&mut app, "ggvjj");
        let shown = draw(&mut app);
        let (ow, _) = app.side_widths();
        let x = app.diff_area().x + ow + 2;
        assert!(
            shown[(x, 1)]
                .modifier
                .contains(ratatui::style::Modifier::REVERSED),
            "the lines selected lit"
        );
        press(&mut app, "yp");
        assert_eq!(app.message.as_ref().unwrap().0, "Copied src/a.js:1-3");
        assert_eq!(app.visual, None, "the selection ends once copied");
        app.message = None;
        press(&mut app, "yp");
        assert_eq!(app.message.as_ref().unwrap().0, "Copied src/a.js:3");
        // Esc ends a selection before it leaves the side.
        app.message = None;
        press(&mut app, "v");
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!((app.visual, app.focus), (None, Focus::New));
        assert_eq!(span(12, 20, "L"), "12-L20");
        assert_eq!(span(7, 7, "L"), "7");
    }

    #[test]
    fn the_list_reads_docs_first_and_the_diff_is_side_by_side() {
        let mut app = app();
        let shown = screen(&mut app);
        let names = list(&shown);
        let at = |name: &str| names.iter().position(|l| l.contains(name)).unwrap();
        assert!(names[0].contains("Changes (3)"), "{names:?}");
        assert!(
            at("README.md") < at("a.js") && at("a.js") < at("new.js"),
            "{names:?}"
        );
        // README.md is shown first: its old line on the left, its new on
        // the right, on the same row.
        let y = shown
            .iter()
            .position(|l| l.contains("Old words here."))
            .expect("the old line");
        let row = &shown[y];
        assert!(row.contains("New words here."), "side by side: {row:?}");
        assert!(
            row.find("Old").unwrap() < row.find("New").unwrap(),
            "old on the left: {row:?}"
        );
        assert!(
            shown[0].contains("README.md"),
            "the path over the diff: {:?}",
            shown[0]
        );
        // The changed word in the stronger shade, the rest of the line in
        // the fainter one.
        // With the keys in the file.
        app.focus = app.main_side();
        let buf = draw(&mut app);
        app.focus = Focus::List;
        let x = row[..row.find("Old").unwrap()].chars().count() as u16;
        let y = y as u16;
        assert_eq!(
            buf[(x, y)].bg,
            blend(REMOVED, 0x1d1f21, 0.55),
            "the word changed"
        );
        assert_eq!(
            buf[(x + 4, y)].bg,
            blend(REMOVED, 0x1d1f21, 0.14),
            "the line around it"
        );
        assert_eq!(app.cur, y as usize - 1, "opened on the first change");
    }

    #[test]
    fn a_working_trees_changes_not_staged_come_first_and_its_staged_ones_after() {
        let (tx, _rx) = channel();
        let mut app = App::new(opts(), 100, 16, tx);
        let header = |p: &str| format!("diff --git a/{p} b/{p}\n--- a/{p}\n+++ b/{p}\n");
        let staged = format!("{}@@ -1 +1 @@\n-a = 1\n+a = 2\n", header("a.txt"));
        let unstaged = format!("{}@@ -1 +1 @@\n-a = 2\n+a = 3\n", header("a.txt"));
        app.give_text("H", "a.txt", Some("a = 1\n".into()));
        app.give_text(git::INDEX, "a.txt", Some("a = 2\n".into()));
        app.give_text(git::WORK_TREE, "a.txt", Some("a = 3\n".into()));
        let in_section = |section| Entry {
            section,
            ..entry("a.txt", 'M', 1, 1)
        };
        app.loaded(Change {
            top: PathBuf::from("/repo"),
            left: "H".into(),
            right: "H".into(),
            entries: vec![in_section(Section::Staged), in_section(Section::Unstaged)],
            diff: unstaged,
            staged: Some(staged),
            ..Change::default()
        });
        let shown = screen(&mut app);
        let names = list(&shown);
        let at = |text: &str| names.iter().position(|l| l.contains(text)).unwrap();
        assert!(at("Changes (1)") < at("Staged changes (1)"), "{names:?}");
        assert_eq!(names.iter().filter(|l| l.contains("a.txt")).count(), 2);
        let row = shown
            .iter()
            .find(|l| l.contains("a = 3"))
            .expect("the file");
        assert!(row.contains("a = 2"), "the index beside the file: {row:?}");
        press(&mut app, "j");
        assert_eq!(app.current().unwrap().section, Section::Staged);
        let row = screen(&mut app)
            .into_iter()
            .find(|l| l.contains("a = 2"))
            .expect("the staged change");
        assert!(row.contains("a = 1"), "HEAD beside the index: {row:?}");
    }

    #[test]
    fn c_goes_through_the_branchs_commits_one_at_a_time_and_back() {
        let mut app = app();
        let commits = vec![
            ("a1".repeat(20), "Return two".to_string()),
            ("b2".repeat(20), "Say so in the README".to_string()),
        ];
        app.open_commits(commits);
        let h = app.history.as_mut().expect("the history");
        assert_eq!(h.at, 0);
        h.app.give_text(
            "P",
            "src/a.js",
            Some("function f() {\n  return 1;\n}\n".into()),
        );
        h.app.give_text(
            "C",
            "src/a.js",
            Some("function f() {\n  return 2;\n}\n".into()),
        );
        h.app.loaded(Change {
            top: PathBuf::from("/repo"),
            left: "P".into(),
            right: "C".into(),
            entries: vec![entry("src/a.js", 'M', 1, 1)],
            diff: "diff --git a/src/a.js b/src/a.js\n@@ -2 +2 @@\n-  return 1;\n+  return 2;\n"
                .into(),
            ..Change::default()
        });
        let shown = screen(&mut app);
        let names = list(&shown);
        assert!(names[0].contains("Commit 1 of 2 · a1a1a1a"), "{names:?}");
        assert!(names.iter().any(|l| l.contains("Return two")), "{names:?}");
        assert!(
            shown
                .iter()
                .any(|l| l.contains("return 1;") && l.contains("return 2;")),
            "the commit's own change: {shown:?}"
        );
        press(&mut app, "]p");
        assert_eq!(app.history.as_ref().unwrap().at, 1);
        let title = app.history.as_ref().unwrap().app.title.clone().unwrap();
        assert_eq!(title.0, " Commit 2 of 2 · b2b2b2b");
        assert!(
            screen(&mut app)
                .iter()
                .any(|l| l.contains("Reading the change")),
            "read off the screen's thread"
        );
        // Read, it takes the keys: none after the last commit.
        app.history.as_mut().unwrap().app.loaded(Change::default());
        press(&mut app, "]p");
        assert_eq!(
            app.history.as_ref().unwrap().at,
            1,
            "no commit after the last"
        );
        let h = app.history.as_mut().unwrap();
        assert!(h.app.message.take().unwrap().0.contains("last commit"));
        press(&mut app, "[p");
        assert_eq!(app.history.as_ref().unwrap().at, 0);
        // c goes back to the whole branch, and so does q, rather than close.
        press(&mut app, "c");
        assert!(app.history.is_none());
        assert!(list(&screen(&mut app))[0].contains("Changes (3)"));
        app.open_commits(vec![("c3".repeat(20), "One".into())]);
        assert_eq!(
            app.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
            Act::Redraw
        );
        assert!(app.history.is_none());
        app.open_commits(Vec::new());
        assert!(app.history.is_none());
        assert_eq!(
            app.message.as_ref().unwrap().0,
            "No commits on this branch since its base."
        );
    }

    #[test]
    fn an_added_file_is_shown_alone_and_the_keys_move_through_the_list() {
        let mut app = app();
        press(&mut app, "jj");
        assert_eq!(app.current().unwrap().path, "src/new.js");
        let shown = screen(&mut app);
        let row = shown
            .iter()
            .find(|l| l.contains("export const x"))
            .expect("the new file");
        assert_eq!(
            row.matches("export").count(),
            1,
            "alone, not beside an empty half"
        );
        press(&mut app, "k");
        assert_eq!(app.current().unwrap().path, "src/a.js");
        press(&mut app, "gg");
        assert_eq!(app.current().unwrap().path, "README.md");
        assert_eq!(
            app.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
            Act::Quit
        );
    }

    #[test]
    fn c_w_and_the_m_arrows_move_between_the_list_and_the_sides() {
        let mut app = app();
        screen(&mut app);
        let ctrl_w = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
        app.key(ctrl_w);
        press(&mut app, "l");
        assert_eq!(app.focus, Focus::Old);
        app.key(ctrl_w);
        press(&mut app, "l");
        assert_eq!(app.focus, Focus::New);
        let alt = |code| KeyEvent::new(code, KeyModifiers::ALT);
        app.key(alt(KeyCode::Left));
        assert_eq!(app.focus, Focus::Old);
        app.key(alt(KeyCode::Left));
        assert_eq!(app.focus, Focus::List);
        // At the edge the workspace's pane is asked for (not here, with no
        // server), and the focus stays.
        app.key(alt(KeyCode::Left));
        assert_eq!(app.focus, Focus::List);
        // With pane_arrows off they do nothing, not even move as an arrow.
        app.opts.pane_arrows = false;
        app.key(alt(KeyCode::Right));
        assert_eq!(app.focus, Focus::List);
    }

    #[test]
    fn a_story_heads_its_chapters_in_the_list_and_c_moves_between_them() {
        let mut app = app();
        let answer = r#"{"chapters": [
            {"title": "Return two", "why": "The function returns 2 now.", "hunks": [{"id": "src/a.js#1", "note": "Check the callers."}]},
            {"title": "Say so", "why": "The README says it.", "hunks": ["README.md#1", "src/new.js#1"]}
        ]}"#;
        let paths: Vec<String> = vec!["README.md".into(), "src/a.js".into(), "src/new.js".into()];
        let told = story::parse(answer, &paths, order::generated, &app.hunks).unwrap();
        app.told(told);
        let shown = screen(&mut app);
        let names = list(&shown);
        let at = |name: &str| names.iter().position(|l| l.contains(name)).unwrap();
        assert!(at("1/2 Return two") < at("a.js"), "{names:?}");
        assert!(
            at("a.js") < at("2/2 Say so") && at("2/2 Say so") < at("README.md"),
            "{names:?}"
        );
        assert_eq!(
            app.current().unwrap().path,
            "src/a.js",
            "the story's first file shown"
        );
        assert!(
            shown.iter().any(|l| l.contains("▎ Check the callers.")),
            "the hunk's note over it: {shown:?}"
        );
        assert!(
            shown[0].contains("1/2 Return two · "),
            "the chapter in the bar: {:?}",
            shown[0]
        );
        press(&mut app, "]c");
        assert_eq!(app.current().unwrap().path, "README.md");
        press(&mut app, "[c");
        assert_eq!(app.current().unwrap().path, "src/a.js");
        // A page of the second chapter shows its files alone.
        press(&mut app, "]p]p");
        assert_eq!(app.page, Some(2));
        let page: Vec<&str> = app
            .order
            .iter()
            .map(|&i| app.entries()[i].path.as_str())
            .collect();
        assert_eq!(page, ["README.md", "src/new.js"]);
        press(&mut app, "[p[p");
        assert_eq!(app.page, None, "back to the whole diff");
        assert_eq!(app.order.len(), 3);
        press(&mut app, "yS");
        let out = String::from_utf8_lossy(&app.out).into_owned();
        assert!(out.starts_with("\x1b]52;c;"), "copied through the terminal");
        assert!(app.message.as_ref().unwrap().0.contains("2 chapters"));
    }

    #[test]
    fn the_cover_is_blank_at_first_then_says_what_is_read() {
        let mut app = app();
        app.telling = Telling::Waiting;
        app.cover = Some(Instant::now());
        assert!(
            screen(&mut app).iter().all(|l| l.is_empty()),
            "blank at first"
        );
        app.cover = Some(Instant::now() - Duration::from_secs(1));
        app.progress = Some(Progress {
            text:
                r#"{"chapters": [{"title": "Return two", "why": "x", "hunks": []}, {"title": "Sa"#
                    .into(),
            started: util::now_secs(),
        });
        let shown = screen(&mut app);
        assert!(
            shown
                .iter()
                .any(|l| l.contains("Telling the change as a story"))
        );
        assert!(
            shown.iter().any(|l| l.contains("Read 3 files (+3 −2)")),
            "{shown:?}"
        );
        assert!(shown.iter().any(|l| l.contains("✓ 1. Return two")));
        assert!(shown.iter().any(|l| l.contains("Esc shows the diff now")));
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(
            screen(&mut app).iter().any(|l| l.contains("README.md")),
            "Esc shows the diff"
        );
    }

    #[test]
    fn the_list_past_the_screen_has_a_scroll_bar_that_pages_and_drags() {
        let (tx, _rx) = channel();
        let mut app = App::new(
            Options {
                scroll_bars: true,
                ..opts()
            },
            100,
            16,
            tx,
        );
        let paths: Vec<String> = (0..40).map(|i| format!("f{i:02}.txt")).collect();
        let mut diff = String::new();
        for p in &paths {
            diff.push_str(&format!(
                "diff --git a/{p} b/{p}\n--- a/{p}\n+++ b/{p}\n@@ -1 +1 @@\n-a\n+b\n"
            ));
            app.give_text("L", p, Some("a\n".into()));
            app.give_text("R", p, Some("b\n".into()));
        }
        app.loaded(Change {
            top: PathBuf::from("/repo"),
            base: "origin/main".into(),
            why: git::Why::default(),
            left: "L".into(),
            right: "R".into(),
            entries: paths.iter().map(|p| entry(p, 'M', 1, 1)).collect(),
            diff,
            staged: None,
            raw_commits: String::new(),
            uncommitted: None,
        });
        let shown = screen(&mut app);
        let bar_col = |shown: &[String]| -> Vec<char> {
            shown
                .iter()
                .take(16)
                .map(|l| l.chars().nth(29).unwrap_or(' '))
                .collect()
        };
        let col = bar_col(&shown);
        assert!(col.contains(&'┃') && col.contains(&'│'), "{shown:#?}");
        let click = |app: &mut App, kind: MouseEventKind, row: u16| {
            app.mouse(MouseEvent {
                kind,
                column: 29,
                row,
                modifiers: KeyModifiers::NONE,
            })
        };
        click(&mut app, MouseEventKind::Down(MouseButton::Left), 15);
        assert!(app.list_top > 0, "a press below the thumb pages down");
        click(&mut app, MouseEventKind::Drag(MouseButton::Left), 0);
        assert_eq!(app.list_top, 0, "dragged back to the top");
        click(&mut app, MouseEventKind::Up(MouseButton::Left), 0);
        assert!(app.bar_drag.is_none());
        app.opts.scroll_bars = false;
        assert!(!bar_col(&screen(&mut app)).contains(&'┃'));
    }

    #[test]
    fn the_wheel_scrolls_the_list_and_the_answer_past_their_cursors() {
        let mut app = app();
        screen(&mut app);
        let wheel = |app: &mut App, column: u16, down: bool| {
            app.mouse(MouseEvent {
                kind: if down {
                    MouseEventKind::ScrollDown
                } else {
                    MouseEventKind::ScrollUp
                },
                column,
                row: 3,
                modifiers: KeyModifiers::NONE,
            })
        };
        // Past the totals and the blank under them, then past README.md.
        wheel(&mut app, 2, true);
        wheel(&mut app, 2, true);
        let shown = list(&screen(&mut app));
        assert_eq!(app.list_top, 5, "{shown:?}");
        assert!(!shown.iter().any(|l| l.contains("README.md")), "{shown:?}");
        assert_eq!(app.current().unwrap().path, "README.md", "still chosen");
        press(&mut app, "j");
        screen(&mut app);
        assert_eq!(app.list_top, 4, "a key brings the cursor back in sight");
        // The answer's window: the view moves, the cursor along with it.
        press(&mut app, "ga");
        press(&mut app, "Why?");
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let id = app.answer.as_ref().unwrap().id;
        let long: String = (1..=60).map(|i| format!("Point {i}.\n\n")).collect();
        app.event(Msg::Asked(id, Said::Done(Ok(long))));
        screen(&mut app);
        for _ in 0..4 {
            wheel(&mut app, 90, true);
        }
        screen(&mut app);
        let a = app.answer.as_ref().unwrap();
        assert_eq!(a.top, 12);
        assert!(a.cursor >= a.top, "{} {}", a.cursor, a.top);
        press(&mut app, "gg");
        screen(&mut app);
        assert_eq!(app.answer.as_ref().unwrap().top, 0);
    }

    #[test]
    fn a_merge_conflict_unresolved_has_no_story_and_ga_in_a_commit_says_so() {
        let (tx, _rx) = channel();
        let mut app = App::new(opts(), 100, 16, tx);
        app.loaded(Change {
            top: PathBuf::from("/repo"),
            entries: vec![entry("src/a.js", 'U', 1, 1), entry("src/b.js", 'M', 1, 0)],
            ..Change::default()
        });
        assert_eq!(
            app.telling,
            Telling::Untold("a merge conflict is unresolved".into())
        );
        app.in_history = true;
        app.opts.ask = false;
        press(&mut app, "ga");
        let said = app.message.take().map(|(t, _)| t);
        assert_eq!(
            said.as_deref(),
            Some("Ask works in the diff, not in the history")
        );
    }

    #[test]
    fn a_message_closes_on_the_next_key_which_goes_on_to_do_its_work() {
        let mut app = app();
        screen(&mut app);
        press(&mut app, "yp");
        assert!(app.message.is_some(), "Copied said");
        press(&mut app, "j");
        assert!(app.message.is_none());
        assert_eq!(app.current().unwrap().path, "src/a.js", "and j moved");
        // Esc only closes it, rather than closing the diff from the list.
        press(&mut app, "yp");
        let esc = app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(esc, Act::Redraw);
        assert!(app.message.is_none());
    }

    #[test]
    fn the_tints_are_mixed_over_the_terminals_own_background() {
        let mut app = app();
        assert_eq!(app.palette().0, blend(REMOVED, 0x1d1f21, 0.14), "unsaid");
        let mut said = switchboard_ui::terminal::Colors::default();
        said.take(b"\x1b]11;rgb:28/2a/36\x07");
        said.take(b"\x1b]10;rgb:f8/f8/f2\x07");
        app.term = said;
        assert_eq!(app.palette().0, blend(REMOVED, 0x282a36, 0.14));
        assert_eq!(app.palette().3, blend(ADDED, 0x282a36, 0.55));
        // A dark background under the light theme (set by hand) isn't mixed
        // over: the theme's own is.
        app.opts.light = true;
        assert_eq!(app.palette().0, blend(REMOVED, 0xffffff, 0.14));
    }

    #[test]
    fn a_question_takes_a_paste_whole_and_shift_enter_adds_a_line() {
        let mut app = app();
        press(&mut app, "ga");
        app.paste("Why this?\r\nAnd that?");
        assert!(app.ask_box.is_some(), "a pasted line break doesn't send it");
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        press(&mut app, "More");
        for _ in 0..4 {
            app.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        }
        press(&mut app, "Even ");
        app.key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        press(&mut app, "!");
        let text = app.ask_box.as_ref().unwrap().text.clone();
        assert_eq!(text, "Why this?\nAnd that?\nEven More!");
        let shown = screen(&mut app);
        for line in ["Why this?", "And that?", "Even More!"] {
            assert!(
                shown.iter().any(|l| l.contains(line)),
                "{line} in {shown:?}"
            );
        }
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.answer.as_ref().unwrap().question, text);
    }

    /// A key as the Keys view writes it, as the terminal sends it.
    fn events(token: &str) -> Vec<KeyEvent> {
        let plain = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        match token {
            "esc" => vec![KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)],
            "↵" => vec![KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)],
            "⇧↵" => vec![KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)],
            "S-F10" => vec![KeyEvent::new(KeyCode::F(10), KeyModifiers::SHIFT)],
            t if t.starts_with('^') => t[1..]
                .chars()
                .map(|c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
                .collect(),
            t => t.chars().map(plain).collect(),
        }
    }

    /// A change of five files, each one line changed: two under `src/`,
    /// a test, a README and a doc.
    fn five() -> App {
        let (tx, _rx) = channel();
        let mut app = App::new(opts(), 100, 16, tx);
        let paths = [
            "src/a.rs",
            "src/b.rs",
            "tests/a.rs",
            "README.md",
            "docs/guide.md",
        ];
        let mut diff = String::new();
        for p in paths {
            diff.push_str(&format!(
                "diff --git a/{p} b/{p}\n--- a/{p}\n+++ b/{p}\n@@ -1 +1 @@\n-old\n+new\n"
            ));
            app.give_text("L", p, Some("old\n".into()));
            app.give_text("R", p, Some("new\n".into()));
        }
        app.loaded(Change {
            top: PathBuf::from("/repo"),
            base: "origin/main".into(),
            why: git::Why::default(),
            left: "L".into(),
            right: "R".into(),
            entries: paths.iter().map(|p| entry(p, 'M', 1, 1)).collect(),
            diff,
            staged: None,
            raw_commits: String::new(),
            uncommitted: None,
        });
        app
    }

    fn listed(app: &App) -> Vec<String> {
        app.order
            .iter()
            .map(|&i| app.entries()[i].path.clone())
            .collect()
    }

    fn special(app: &mut App, code: KeyCode) {
        app.key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn a_path_filter_matches_every_word_and_leaves_out_a_bang_word() {
        assert!(path_matches("", "src/a.rs"));
        assert!(path_matches("SRC a.rs", "src/a.rs"));
        assert!(!path_matches("src b.rs", "src/a.rs"));
        assert!(!path_matches("!tests/", "tests/a.rs"));
        assert!(path_matches("!tests/ !", "src/a.rs"));
    }

    #[test]
    fn slash_filters_the_file_list_by_path_and_esc_clears_it() {
        let mut app = five();
        press(&mut app, "/src");
        assert_eq!(listed(&app), ["src/a.rs", "src/b.rs"]);
        assert_eq!(app.at, 0, "the cursor on the first file left");
        let shown = screen(&mut app);
        line_with(&shown, "2 of 5 files");
        // Enter keeps it, and j moves among the files left.
        special(&mut app, KeyCode::Enter);
        press(&mut app, "jj");
        assert_eq!(listed(&app).len(), 2);
        assert_eq!(app.current().unwrap().path, "src/b.rs");
        // / reopens it, Esc clears it.
        press(&mut app, "/");
        special(&mut app, KeyCode::Esc);
        assert_eq!(listed(&app).len(), 5);
        assert!(!screen(&mut app).iter().any(|l| l.contains("of 5 files")));
        // Esc again, in the list, closes the diff.
        assert_eq!(
            app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Act::Quit
        );
    }

    #[test]
    fn a_bang_word_leaves_paths_out_and_nothing_left_says_so() {
        let mut app = five();
        press(&mut app, "/!tests/");
        assert!(!listed(&app).contains(&"tests/a.rs".to_string()));
        assert_eq!(listed(&app).len(), 4);
        special(&mut app, KeyCode::Esc);
        press(&mut app, "/nothing");
        assert!(listed(&app).is_empty());
        line_with(&screen(&mut app), "No file's path matches");
    }

    #[test]
    fn the_filter_line_stays_in_sight_down_a_long_list() {
        let mut app = five();
        app.resize(100, 6);
        press(&mut app, "GG");
        press(&mut app, "/");
        let shown = screen(&mut app);
        assert!(shown[0].contains("5 of 5 files"), "{shown:?}");
        press(&mut app, "s");
        assert!(screen(&mut app)[0].contains("of 5 files"));
    }

    #[test]
    fn the_palettes_reset_ends_typing_the_filter() {
        let mut app = five();
        press(&mut app, "/src");
        app.key(KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::CONTROL));
        assert!(!app.filtering);
        assert_eq!(app.path_filter, "src", "the filter kept");
        press(&mut app, "j");
        assert_eq!(app.current().unwrap().path, "src/b.rs");
    }

    #[test]
    fn a_click_on_a_button_ends_typing_the_filter() {
        let mut app = five();
        app.opts.easy = true;
        app.resize(220, 16);
        press(&mut app, "/src");
        screen(&mut app);
        let close = BUTTONS.iter().position(|(_, k)| *k == "q").unwrap();
        let (_, at) = *app
            .buttons
            .iter()
            .find(|(b, _)| *b == Button::Bar(close))
            .unwrap();
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: at.x + 1,
            row: at.y,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(app.mouse(click), Act::Quit);
        assert_eq!(app.path_filter, "src");
    }

    #[test]
    fn every_key_is_safe_while_the_filter_leaves_no_files() {
        for (_, rows) in KEYS {
            for row in *rows {
                for token in row.keys.split(' ') {
                    let mut app = five();
                    let paths = [
                        "src/a.rs",
                        "src/b.rs",
                        "tests/a.rs",
                        "README.md",
                        "docs/guide.md",
                    ];
                    app.told(story::Story {
                        chapters: (0..2)
                            .map(|n| story::Chapter {
                                title: format!("Part {n}"),
                                why: "Why.".into(),
                                watch: String::new(),
                                files: paths[n * 2..n * 2 + 2]
                                    .iter()
                                    .map(|p| p.to_string())
                                    .collect(),
                                touches: paths[n * 2..n * 2 + 2]
                                    .iter()
                                    .map(|p| p.to_string())
                                    .collect(),
                                ..Default::default()
                            })
                            .collect(),
                        place: HashMap::new(),
                        chapter: paths
                            .iter()
                            .enumerate()
                            .map(|(i, p)| (p.to_string(), (i / 2).min(1) + 1))
                            .collect(),
                        hunks: HashMap::new(),
                        missed: None,
                        ..Default::default()
                    });
                    press(&mut app, "/nothing");
                    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                    for start in ["", "\t"] {
                        if start == "\t" {
                            app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
                        }
                        for event in events(token) {
                            app.key(event);
                        }
                        for keys in ["[c", "]c", "[p", "]p", "jkGJK"] {
                            press(&mut app, keys);
                        }
                        screen(&mut app);
                    }
                }
            }
        }
    }

    #[test]
    fn a_click_on_the_empty_filter_line_keeps_typing() {
        let mut app = five();
        press(&mut app, "/");
        screen(&mut app);
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        app.mouse(click);
        assert!(app.filtering);
        press(&mut app, "q");
        assert_eq!(app.path_filter, "q");
    }

    #[test]
    fn an_alt_arrow_to_the_diff_ends_typing_the_filter() {
        let mut app = five();
        press(&mut app, "/src");
        app.key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
        assert!(!app.filtering);
        assert_ne!(app.focus, Focus::List);
        assert_eq!(app.path_filter, "src");
    }

    #[test]
    fn the_filter_stays_through_the_change_read_again() {
        let mut app = five();
        press(&mut app, "/src");
        special(&mut app, KeyCode::Enter);
        let change = app.change.clone().unwrap();
        app.refreshed(change);
        assert_eq!(listed(&app), ["src/a.rs", "src/b.rs"]);
    }

    #[test]
    fn the_keys_view_lists_keys_the_diff_takes() {
        // Each place a group is listed for, made from a fresh diff.
        let place = |title: &str| -> App {
            let mut app = app();
            match title {
                "M-d's diff" => {}
                "M-d: the story's map" => app = map_app(),
                "M-d: the cover while the story is told" => {
                    app.telling = Telling::Waiting;
                    app.cover = Some(Instant::now());
                }
                "M-d: asking a question (ga)" => press(&mut app, "ga"),
                "M-d: an answer (ga)" => {
                    press(&mut app, "gaWhy?");
                    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                    assert!(app.answer.is_some());
                }
                other => panic!("no place for {other}"),
            }
            app
        };
        for (title, rows) in KEYS {
            for row in *rows {
                for token in row.keys.split(' ') {
                    let mut app = place(title);
                    // Selecting is in a side of the diff.
                    if token == "v" || token == "V" {
                        app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
                    }
                    let mut act = Act::Same;
                    for event in events(token) {
                        act = app.key(event);
                    }
                    assert_ne!(act, Act::Same, "{token} in {title} does nothing");
                }
            }
        }
        // ga and its boxes only with diff_ask.
        let without: Vec<String> = key_groups(false)
            .into_iter()
            .flat_map(|(_, rows)| rows.into_iter().map(|(k, _)| k))
            .collect();
        assert!(!without.contains(&"ga".to_string()));
        assert_eq!(key_groups(false).len(), 3);
        assert_eq!(key_groups(true).len(), 5);
    }

    #[test]
    fn ga_opens_a_box_and_a_question_opens_its_answer_beside_the_diff() {
        let mut app = app();
        press(&mut app, "ga");
        assert!(
            screen(&mut app)
                .iter()
                .any(|l| l.contains("Ask about this diff"))
        );
        press(&mut app, "Why?");
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let id = app.answer.as_ref().unwrap().id;
        app.event(Msg::Asked(id, Said::Text("It returns ".into())));
        app.event(Msg::Asked(id, Said::Tool("_Read src/b.js_".into())));
        app.event(Msg::Asked(id, Said::Text("2, see src/a.js:2.".into())));
        app.event(Msg::Asked(id, Said::Done(Ok("whole".into()))));
        let shown = screen(&mut app);
        assert!(
            shown.iter().any(|l| l.contains("Why?")),
            "titled with the question"
        );
        assert!(
            shown.iter().any(|l| l.contains("Read src/b.js")),
            "{shown:?}"
        );
        assert!(
            shown
                .iter()
                .any(|l| l.contains("Close") && l.contains("Ask again"))
        );
        // gf on the cited line opens src/a.js at line 2.
        let lines = app.answer_lines(app.answer_width());
        let at = lines
            .iter()
            .position(|l| l.0.contains("src/a.js:2"))
            .unwrap();
        app.answer.as_mut().unwrap().cursor = at;
        press(&mut app, "gf");
        assert!(app.answer.is_none());
        assert_eq!(app.current().unwrap().path, "src/a.js");
        assert_eq!(app.line_under(true), Some(2));
    }

    fn mouse(app: &mut App, kind: MouseEventKind, x: u16, y: u16) -> Act {
        app.mouse(MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        })
    }

    const DOWN: MouseEventKind = MouseEventKind::Down(MouseButton::Left);
    const DRAG: MouseEventKind = MouseEventKind::Drag(MouseButton::Left);
    const UP: MouseEventKind = MouseEventKind::Up(MouseButton::Left);

    /// Where `text` is on the screen: its column and row.
    fn place_of(shown: &[String], text: &str) -> (u16, u16) {
        let y = line_with(shown, text);
        let before = &shown[y][..shown[y].find(text).unwrap()];
        (before.chars().count() as u16, y as u16)
    }

    /// What was last put on the clipboard, read back from the escape sent
    /// for it (OSC 52), which is taken.
    fn clipboard(app: &mut App) -> String {
        let sent = String::from_utf8(std::mem::take(&mut app.out)).unwrap();
        let Some(at) = sent.rfind("\x1b]52;c;") else {
            return String::new();
        };
        let coded = sent[at + 7..].trim_end_matches('\x07');
        let value = |c: u8| match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            _ => 63,
        };
        let mut bytes = Vec::new();
        for four in coded.trim_end_matches('=').as_bytes().chunks(4) {
            let n = four
                .iter()
                .fold(0u32, |n, c| (n << 6) | u32::from(value(*c)))
                << (6 * (4 - four.len()));
            bytes.extend_from_slice(&n.to_be_bytes()[1..four.len()]);
        }
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn a_drag_in_a_side_copies_its_text_without_line_numbers_or_the_other_side() {
        let mut app = app();
        press(&mut app, "j");
        let shown = screen(&mut app);
        let (x, y) = place_of(&shown, "function f() {");
        let (x, y) = {
            // The new side's copy of the line, right of the old side's.
            let (ow, _) = app.side_widths();
            let from = (app.diff_area().x + ow) as usize;
            let rest: String = shown[y as usize].chars().skip(from).collect();
            let _ = x;
            (from as u16 + place_of(&[rest], "function f() {").0, y)
        };
        // Three whole lines, from the first's start to the last's end.
        mouse(&mut app, DOWN, x, y);
        assert_eq!(mouse(&mut app, DRAG, x + 40, y + 2), Act::Redraw);
        mouse(&mut app, UP, x + 40, y + 2);
        assert_eq!(clipboard(&mut app), "function f() {\n  return 2;\n}");
        assert_eq!(app.message.as_ref().unwrap().0, "Copied 3 lines");
        // What is selected is lit, and stays until a key or a click.
        app.message = None;
        let drawn = draw(&mut app);
        assert_eq!(drawn[(x, y + 1)].bg, Color::Indexed(239));
        assert_ne!(
            drawn[(x - 2, y + 1)].bg,
            Color::Indexed(239),
            "not the numbers"
        );
        // y copies it again; Esc clears it before anything else.
        press(&mut app, "y");
        assert_eq!(clipboard(&mut app), "function f() {\n  return 2;\n}");
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.selection.is_none() && app.message.is_none());
        assert_eq!(app.focus, Focus::New, "Esc cleared the selection alone");
        // The y before it waits for no second key: the next key is its own.
        let at = app.cur;
        press(&mut app, "j");
        assert_eq!(app.cur, at + 1);
        // And so is the key after y, when it isn't yp's or yl's.
        draw(&mut app);
        mouse(&mut app, DOWN, x, y);
        mouse(&mut app, DRAG, x + 3, y);
        mouse(&mut app, UP, x + 3, y);
        let at = app.cur;
        press(&mut app, "yj");
        assert!(app.selection.is_none());
        assert_eq!(app.cur, at + 1);
        // By characters: from inside one line to inside the next, backwards.
        draw(&mut app);
        mouse(&mut app, DOWN, x + 7, y + 1);
        mouse(&mut app, DRAG, x + 9, y);
        mouse(&mut app, UP, x + 9, y);
        assert_eq!(clipboard(&mut app), "f() {\n  return");
        assert_eq!(app.message.as_ref().unwrap().0, "Copied 2 lines");
    }

    #[test]
    fn a_double_click_copies_a_word_and_a_triple_click_its_line() {
        let mut app = app();
        press(&mut app, "j");
        let shown = screen(&mut app);
        let (x, y) = place_of(&shown, "return 2;");
        mouse(&mut app, DOWN, x + 2, y);
        mouse(&mut app, UP, x + 2, y);
        assert!(app.out.is_empty(), "a click copies nothing");
        mouse(&mut app, DOWN, x + 2, y);
        mouse(&mut app, UP, x + 2, y);
        assert_eq!(clipboard(&mut app), "return");
        assert_eq!(app.message.as_ref().unwrap().0, "Copied 1 line");
        mouse(&mut app, DOWN, x + 2, y);
        mouse(&mut app, UP, x + 2, y);
        assert_eq!(clipboard(&mut app), "  return 2;");
    }

    #[test]
    fn v_then_y_copies_the_lines_text_and_yp_still_their_place() {
        let mut app = app();
        press(&mut app, "j");
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        press(&mut app, "ggVjy");
        assert_eq!(clipboard(&mut app), "function f() {\n  return 2;");
        assert_eq!(app.message.as_ref().unwrap().0, "Copied 2 lines");
        press(&mut app, "p");
        assert_eq!(app.message.as_ref().unwrap().0, "Copied src/a.js:1-2");
        assert_eq!(app.visual, None);
        // The file's own text, a tab a tab, though the screen shows spaces.
        app.give_text("R", "src/a.js", Some("if (x) {\n\treturn 2;\n}\n".into()));
        app.files.clear();
        app.lines.clear();
        press(&mut app, "ggVjy");
        assert_eq!(clipboard(&mut app), "if (x) {\n\treturn 2;");
        press(&mut app, "j");
        // Any other key after y ends the selection and does its own work.
        press(&mut app, "ggVjy");
        let at = app.cur;
        press(&mut app, "j");
        assert_eq!((app.visual, app.cur), (None, at + 1));
        // An arrow too, which is no character.
        press(&mut app, "ggVy");
        app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!((app.visual, app.cur), (None, 1));
        // In the list, v selects files and y copies their paths.
        app.focus = Focus::List;
        press(&mut app, "ggvjy");
        assert_eq!(clipboard(&mut app), "README.md\nsrc/a.js");
        // Esc closes the message, then ends the selection, still lit after
        // y, before it would close the diff.
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.key(esc), Act::Redraw);
        assert!(app.list_visual.is_some());
        assert_eq!(app.key(esc), Act::Redraw);
        assert_eq!(app.list_visual, None);
        assert_eq!(app.key(esc), Act::Quit);
        // A side's selection is none of the list's: easy mode's buttons
        // move to the list with it left behind.
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        press(&mut app, "v");
        app.press(Button::Bar(0));
        assert_eq!((app.focus, app.list_visual), (Focus::List, None));
    }

    #[test]
    fn a_drag_in_the_file_list_copies_its_rows_and_a_click_still_chooses_a_file() {
        let mut app = app();
        let shown = screen(&mut app);
        let (_, first) = place_of(&list(&shown), "README.md");
        let (_, second) = place_of(&list(&shown), "a.js");
        mouse(&mut app, DOWN, 0, first);
        mouse(&mut app, DRAG, 29, second);
        mouse(&mut app, UP, 29, second);
        let rows: Vec<String> = [first, second]
            .iter()
            .map(|y| list(&shown)[*y as usize].trim_end().to_string())
            .collect();
        assert!(rows[0].contains("README.md") && rows[1].contains("a.js"));
        assert_eq!(clipboard(&mut app), rows.join("\n"));
        assert_eq!(app.message.as_ref().unwrap().0, "Copied 2 lines");
        // A press and a release on one cell selects nothing.
        app.message = None;
        let shown = screen(&mut app);
        let (x, y) = place_of(&list(&shown), "new.js");
        mouse(&mut app, DOWN, x, y);
        mouse(&mut app, DRAG, x, y);
        mouse(&mut app, UP, x, y);
        assert_eq!(app.current().unwrap().path, "src/new.js");
        assert!(app.selection.is_none() && app.out.is_empty() && app.message.is_none());
        // A drag from the list's empty rows selects nothing of the diff
        // beside them.
        draw(&mut app);
        mouse(&mut app, DOWN, 5, 12);
        mouse(&mut app, DRAG, 20, 13);
        mouse(&mut app, UP, 20, 13);
        assert!(app.selection.is_none() && app.out.is_empty());
    }

    #[test]
    fn a_selection_stays_on_its_text_as_the_wheel_scrolls_and_copies_what_scrolled_away() {
        let mut app = added_files(&["a.rs", "b.rs"], 60);
        let shown = screen(&mut app);
        let path = app.current().unwrap().path.clone();
        let (x, y) = place_of(&shown, &format!("{path} line 6"));
        mouse(&mut app, DOWN, x, y);
        draw(&mut app);
        mouse(&mut app, DRAG, x + 4, y + 1);
        // The wheel under the held mouse: the end follows the text there.
        for _ in 0..4 {
            mouse(&mut app, MouseEventKind::ScrollDown, x + 4, y + 1);
            draw(&mut app);
        }
        let shown = screen(&mut app);
        assert!(!shown.iter().any(|l| l.ends_with(&format!("{path} line 6"))));
        mouse(&mut app, UP, x + 4, y + 1);
        // To the fifth column of line 19, now under the mouse.
        let lines: Vec<String> = (6..=18).map(|i| format!("{path} line {i}")).collect();
        assert_eq!(clipboard(&mut app), format!("{}\n{path}", lines.join("\n")));
        assert_eq!(app.message.as_ref().unwrap().0, "Copied 14 lines");
        // A press on the scroll bar scrolls under the selection, kept.
        app.opts.scroll_bars = true;
        app.message = None;
        draw(&mut app);
        let bar = app.diff_area().right() - 1;
        let row = app.diff_area().y + app.head_height() as u16 + 1;
        mouse(&mut app, DOWN, bar, row);
        mouse(&mut app, UP, bar, row);
        assert!(app.selection.is_some());
    }

    #[test]
    fn a_turn_of_the_wheel_under_a_held_drag_copies_the_rows_it_took_off_the_screen() {
        let mut app = added_files(&["a.rs", "b.rs"], 60);
        let shown = screen(&mut app);
        let path = app.current().unwrap().path.clone();
        // A drag along the first row in sight, then the wheel, which takes
        // that row and the two after it off the screen at once.
        let (x, y) = place_of(&shown, &format!("{path} line 1"));
        mouse(&mut app, DOWN, x, y);
        draw(&mut app);
        mouse(&mut app, DRAG, x + 3, y);
        mouse(&mut app, MouseEventKind::ScrollDown, x + 3, y);
        draw(&mut app);
        mouse(&mut app, UP, x + 3, y);
        assert_eq!(
            clipboard(&mut app),
            format!("{path} line 1\n{path} line 2\n{path} line 3\n{path}")
        );
    }

    #[test]
    fn a_line_wrapped_is_copied_as_one_with_nothing_added_where_it_wraps() {
        let (tx, _rx) = channel();
        let mut app = App::new(opts(), 100, 16, tx);
        let long = "let answer = compute(first_argument, second_argument, third_argument);";
        app.give_text("R", "src/long.rs", Some(format!("{long}\nshort\n")));
        app.loaded(Change {
            top: PathBuf::from("/repo"),
            base: "origin/main".into(),
            why: git::Why::default(),
            left: "L".into(),
            right: "R".into(),
            entries: vec![entry("src/long.rs", 'A', 2, 0)],
            diff: format!(
                "diff --git a/src/long.rs b/src/long.rs\n--- /dev/null\n+++ b/src/long.rs\n@@ -0,0 +1,2 @@\n+{long}\n+short\n"
            ),
            staged: None,
            raw_commits: String::new(),
            uncommitted: None,
        });
        let shown = screen(&mut app);
        let (x, y) = place_of(&shown, "let answer");
        let (_, last) = place_of(&shown, "short");
        assert!(
            last > y + 1,
            "the long line takes more than a row: {shown:?}"
        );
        mouse(&mut app, DOWN, x, y);
        mouse(&mut app, DRAG, x + 60, last);
        mouse(&mut app, UP, x + 60, last);
        assert_eq!(clipboard(&mut app), format!("{long}\nshort"));
        // A triple click on its second row takes the whole line.
        app.message = None;
        draw(&mut app);
        for _ in 0..3 {
            mouse(&mut app, DOWN, x + 2, y + 1);
            mouse(&mut app, UP, x + 2, y + 1);
        }
        assert_eq!(clipboard(&mut app), long);
    }

    #[test]
    fn a_drag_over_an_answers_path_selects_it_and_a_click_still_goes_there() {
        let mut app = app();
        press(&mut app, "gaWhy?");
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let id = app.answer.as_ref().unwrap().id;
        app.event(Msg::Asked(id, Said::Text("See src/a.js:2 there.".into())));
        app.event(Msg::Asked(id, Said::Done(Ok("whole".into()))));
        let shown = screen(&mut app);
        let (x, y) = place_of(&shown, "src/a.js:2");
        mouse(&mut app, DOWN, x, y);
        mouse(&mut app, DRAG, x + 9, y);
        mouse(&mut app, UP, x + 9, y);
        assert_eq!(clipboard(&mut app), "src/a.js:2");
        assert!(app.answer.is_some(), "the answer stays under a drag");
        // A press and a release on it goes to the line.
        app.message = None;
        draw(&mut app);
        mouse(&mut app, DOWN, x + 1, y);
        assert!(app.answer.is_some());
        mouse(&mut app, UP, x + 1, y);
        assert!(app.answer.is_none());
        assert_eq!(app.current().unwrap().path, "src/a.js");
    }

    #[test]
    fn v_in_an_answer_selects_lines_and_y_copies_them() {
        let mut app = app();
        press(&mut app, "gaWhy?");
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let id = app.answer.as_ref().unwrap().id;
        app.event(Msg::Asked(id, Said::Text("One.\n\nTwo.\n\nThree.".into())));
        app.event(Msg::Asked(id, Said::Done(Ok("whole".into()))));
        let lines = app.answer_lines(app.answer_width());
        let one = lines.iter().position(|l| l.0.contains("One.")).unwrap();
        app.answer.as_mut().unwrap().cursor = one;
        press(&mut app, "vjjy");
        assert_eq!(clipboard(&mut app), "One.\n\nTwo.");
        // A drag in the answer selects its words.
        press(&mut app, "j");
        assert_eq!(app.answer.as_ref().unwrap().visual, None);
        app.message = None;
        let shown = screen(&mut app);
        let (x, y) = place_of(&shown, "Three.");
        mouse(&mut app, DOWN, x, y);
        mouse(&mut app, DRAG, x + 3, y);
        mouse(&mut app, UP, x + 3, y);
        assert_eq!(clipboard(&mut app), "Thre");
        assert!(app.answer.is_some());
    }

    /// The answer `map_app` is told: a main story of four chapters and
    /// the side work. Wiring the page (the goal) builds on counting and on
    /// the topics; the topics build on the store and show the count's hunk
    /// again; counting builds on the store.
    const MAP_ANSWER: &str = r#"{"stories": [
        {"story": "Signups map", "kind": "main", "chapters": [
            {"id": "c1", "title": "Store each outline", "why": "The map needs outlines kept.", "watch": "", "goal": false, "needs": [],
             "hunks": [{"id": "src/store.rs#1", "note": "The store."}], "files": [],
             "things": [{"name": "store_outline", "kind": "function", "hunk": "src/store.rs#1", "new": true, "uses": []}]},
            {"id": "c2", "title": "Count the signups", "why": "The map shows counts. They are folded.", "watch": "", "goal": false, "needs": ["c1"],
             "hunks": [{"id": "src/count.rs#1", "note": "The count."}], "files": [],
             "things": [{"name": "count_signups", "kind": "function", "hunk": "src/count.rs#1", "new": true, "uses": ["fold_small"]},
                        {"name": "fold_small", "kind": "function", "hunk": "src/count.rs#1", "new": true, "uses": []},
                        {"name": "never_there", "kind": "function", "hunk": "src/count.rs#1", "new": true, "uses": []}]},
            {"id": "c3", "title": "Draw the topics", "why": "Topics get a callout.", "watch": "", "goal": false, "needs": ["c1"],
             "hunks": [{"id": "src/topics.rs#1", "note": ""}, {"id": "src/count.rs#1", "note": "", "repeat": true}], "files": [], "things": []},
            {"id": "c4", "title": "Wire the page", "why": "The page shows both.", "watch": "", "goal": true, "needs": ["c2", "c3"],
             "hunks": [{"id": "src/page.rs#1", "note": ""}], "files": [], "things": []}
        ]},
        {"story": "Side work", "kind": "side", "chapters": [
            {"id": "c5", "title": "Format the helpers", "why": "Spacing only.", "watch": "", "goal": true, "needs": [],
             "hunks": [{"id": "src/fmt.rs#1", "note": ""}], "files": [], "things": []}
        ]}
    ]}"#;

    /// A change of five files told as a map (`MAP_ANSWER`), on a screen
    /// large enough for the map, with UX 2.0.
    fn map_app_with(opts: Options) -> App {
        let (tx, _rx) = channel();
        let mut app = App::new(opts, 120, 40, tx);
        let added = [
            (
                "src/store.rs",
                "pub fn store_outline(zip: &str) {\n    keep(zip);\n}\n",
            ),
            (
                "src/count.rs",
                "pub fn count_signups() -> usize {\n    store_outline(\"x\");\n    fold_small(0)\n}\nfn fold_small(n: usize) -> usize {\n    n\n}\n",
            ),
            ("src/topics.rs", "pub fn topics() -> usize {\n    3\n}\n"),
            (
                "src/page.rs",
                "pub fn page() {\n    count_signups();\n    topics();\n}\n",
            ),
        ];
        let mut diff = String::new();
        let mut entries = Vec::new();
        for (path, text) in added {
            let lines = text.lines().count();
            diff.push_str(&format!(
                "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{lines} @@\n"
            ));
            for line in text.lines() {
                diff.push_str(&format!("+{line}\n"));
            }
            app.give_text("R", path, Some(text.into()));
            entries.push(entry(path, 'A', lines, 0));
        }
        diff.push_str(
            "diff --git a/src/fmt.rs b/src/fmt.rs\n--- a/src/fmt.rs\n+++ b/src/fmt.rs\n@@ -1 +1 @@\n-fn  spaced() {}\n+fn spaced() {}\n",
        );
        app.give_text("L", "src/fmt.rs", Some("fn  spaced() {}\n".into()));
        app.give_text("R", "src/fmt.rs", Some("fn spaced() {}\n".into()));
        entries.push(entry("src/fmt.rs", 'M', 1, 1));
        let change = Change {
            top: PathBuf::from("/repo"),
            base: "origin/main".into(),
            why: git::Why::default(),
            left: "L".into(),
            right: "R".into(),
            entries,
            diff,
            staged: None,
            raw_commits: String::new(),
            uncommitted: None,
        };
        let (_, hunks) = story::split(&change.runs());
        let (_, paths) = story::key_of(&change);
        let told = story::parse(MAP_ANSWER, &paths, order::generated, &hunks).unwrap();
        app.loaded(change);
        app.told(told);
        app
    }

    fn map_app() -> App {
        map_app_with(Options {
            ux2: true,
            ..opts()
        })
    }

    #[test]
    fn a_story_told_as_a_map_opens_on_its_map_the_goal_at_the_top() {
        let mut app = map_app();
        assert!(app.on_map(), "Map the first time, once a story is told");
        let shown = screen(&mut app);
        let row = |text: &str| line_with(&shown, text);
        // The goal first, each chapter under what builds on it.
        assert!(row("THE GOAL") < row("│     4 Wire the page"));
        assert!(row("│     4 Wire the page") < row("│ 3 Draw the topics"));
        assert!(row("│     4 Wire the page") < row("│ 2 Count the signups"));
        assert!(row("│ 2 Count the signups") < row("│ 1 Store each outline"));
        assert!(row("│ 3 Draw the topics") < row("│ 1 Store each outline"));
        // Every link as Haiku gave it, none dotted.
        assert!(!shown.iter().any(|l| l.contains('┆')), "{shown:#?}");
        assert!(shown[row("▼ builds on, as Haiku tells it")].contains("✓ read"));
        // The list reads from the goal down, and names the other story.
        let list: Vec<String> = shown
            .iter()
            .map(|l| l.chars().take(30).collect::<String>().trim().to_string())
            .collect();
        let at = |text: &str| list.iter().position(|l| l.contains(text)).unwrap();
        assert!(at("4 Wire the page") < at("2 Count the signups"));
        assert!(at("2 Count the signups") < at("1 Store each outline"));
        assert!(at("1 Store each outline") < at("3 Draw the topics"));
        assert!(list[at("READ")].ends_with("0 of 4"));
        assert!(at("OTHER STORIES") < at("Side work"));
        assert!(shown[0].contains("Signups map 4") && shown[0].contains("Side work 1"));
    }

    #[test]
    fn the_maps_cursor_moves_along_the_arrows_and_c_reads_from_the_goal_down() {
        let mut app = map_app();
        assert_eq!(app.map_at, 4);
        press(&mut app, "j");
        assert_eq!(
            app.map_at, 2,
            "down to the first chapter the goal builds on"
        );
        press(&mut app, "j");
        assert_eq!(app.map_at, 1);
        press(&mut app, "k");
        assert_eq!(app.map_at, 2, "up to a chapter that needs it");
        // From the goal, what it builds on before its sibling.
        press(&mut app, "kk");
        let mut read = vec![app.map_at];
        for _ in 0..4 {
            press(&mut app, "]c");
            read.push(app.map_at);
        }
        assert_eq!(read, [4, 2, 1, 3, 3]);
        press(&mut app, "[c");
        assert_eq!(app.map_at, 1, "[c goes back the way it came");
        // ]s and [s move between the stories.
        press(&mut app, "]s");
        assert_eq!((app.thread, app.map_at), (1, 5));
        assert!(screen(&mut app)[0].contains("SIDE WORK"));
        press(&mut app, "[s");
        assert_eq!((app.thread, app.map_at), (0, 4));
    }

    #[test]
    fn enter_opens_a_chapter_on_its_own_map_and_a_thing_goes_to_its_hunk() {
        let mut app = map_app();
        press(&mut app, "j");
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.in_chapter());
        assert_eq!(app.page, Some(2));
        let shown = screen(&mut app);
        assert!(shown[0].contains("‹ Signups map"));
        assert!(shown[0].contains("✓ 1 of 4 read · g m the map"));
        let label = line_with(&shown, "WHAT IT ADDS OR CHANGES");
        assert!(shown[label].contains("3 things"), "each thing Haiku named");
        assert!(line_with(&shown, "count_signups") < line_with(&shown, "fold_small"));
        assert!(line_with(&shown, "BUILDS ON") < line_with(&shown, "NEEDED BY"));
        assert!(shown[line_with(&shown, "BUILDS ON") + 1].contains("1 Store each outline"));
        // Tab reaches the chapter's own map; Enter on a thing goes to its hunk.
        app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.thing, Some(0));
        press(&mut app, "j");
        assert_eq!(app.thing, Some(1));
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.thing, None);
        assert_eq!(app.focus, Focus::New);
        assert_eq!(app.current().map(|e| e.path.as_str()), Some("src/count.rs"));
        // Esc from the list goes back to the map, the chapter read.
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.on_map());
        let shown = screen(&mut app);
        assert!(shown.iter().any(|l| l.contains("✓ 2 Count the signups")));
        assert!(shown[line_with(&shown, "READ")].contains("1 of 4"));
        // ]c in an open chapter opens the next from the goal down.
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        press(&mut app, "]c");
        assert_eq!(app.page, Some(1));
    }

    #[test]
    fn a_hunk_in_two_chapters_is_shown_in_both_under_its_also_line() {
        let mut app = map_app();
        app.open_chapter(3);
        let shown = screen(&mut app);
        let also = line_with(&shown, "also in chapter 2: Count the signups");
        assert!(shown[also + 1].contains("pub fn count_signups"));
        assert!(shown.iter().any(|l| l.contains("src/count.rs  also 2")));
        assert!(shown.iter().any(|l| l.contains("src/topics.rs")));
        assert!(shown.iter().any(|l| l.contains("1 Store each outline")));
        // Enter on the line goes to the hunk's home.
        let at = app
            .order
            .iter()
            .position(|&i| app.entries()[i].path == "src/count.rs")
            .unwrap();
        app.show_at(at);
        app.focus = Focus::New;
        app.cur = 0;
        assert_eq!(app.also_under().map(|(n, _)| n), Some(2));
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.page, Some(2));
        // In its home the hunk has its note and no such line.
        let shown = screen(&mut app);
        assert!(!shown.iter().any(|l| l.contains("also in chapter")));
        assert!(shown.iter().any(|l| l.contains("The count.")));
    }

    #[test]
    fn each_mode_keeps_its_place_and_files_mode_draws_as_it_did() {
        let mut app = map_app();
        // g f: the chosen chapter's file, in Files mode.
        press(&mut app, "jgf");
        assert_eq!(app.mode, Mode::Files);
        assert_eq!(app.current().map(|e| e.path.as_str()), Some("src/count.rs"));
        // Files mode is the diff as it is with no map at all.
        let mut plain = map_app_with(Options {
            ux2: true,
            map: false,
            ..opts()
        });
        assert!(!plain.mapped());
        let at = app.at;
        plain.show_at(at);
        assert_eq!(screen(&mut app), screen(&mut plain));
        // The menu switches, each mode back where it was left.
        press(&mut app, "j");
        let left = app.current().map(|e| e.path.clone());
        press(&mut app, "m");
        assert!(screen(&mut app).iter().any(|l| l.contains("› Files")));
        press(&mut app, "k");
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.on_map());
        assert_eq!(app.map_at, 2, "the map where it was left");
        press(&mut app, "mj");
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Files);
        assert_eq!(app.current().map(|e| e.path.clone()), left);
        // g m comes back to the map from anywhere, the diff's side included.
        app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        press(&mut app, "gm");
        assert!(app.on_map());
    }

    #[test]
    fn easy_mode_leads_its_buttons_with_the_map_and_files_switch() {
        let mut app = map_app_with(Options {
            ux2: true,
            easy: true,
            ..opts()
        });
        let shown = screen(&mut app);
        let last = shown.last().unwrap();
        assert!(last.find("Map").unwrap() < last.find("Files").unwrap());
        assert!(last.find("Files").unwrap() < last.find("◀ Chapter").unwrap());
        let (_, files) = app
            .buttons
            .iter()
            .find(|(b, _)| *b == Button::Mode(Mode::Files))
            .copied()
            .unwrap();
        app.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: files.x,
            row: files.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.mode, Mode::Files);
        // A click on a box chooses its chapter, and a second opens it.
        press(&mut app, "gm");
        screen(&mut app);
        let (_, at) = app
            .hits
            .iter()
            .rev()
            .find(|(hit, _)| *hit == map::Hit::Chapter(1))
            .copied()
            .unwrap();
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: at.x + 1,
            row: at.y + 1,
            modifiers: KeyModifiers::NONE,
        };
        let up = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            ..click
        };
        app.mouse(click);
        app.mouse(up);
        assert_eq!((app.map_at, app.page), (1, None));
        app.mouse(click);
        assert_eq!(app.page, None, "it opens as the mouse is let go");
        app.mouse(up);
        assert_eq!(app.page, Some(1));
    }

    #[test]
    fn with_the_setting_off_there_is_no_map_mode() {
        let mut app = map_app_with(Options {
            ux2: true,
            map: false,
            ..opts()
        });
        assert_eq!(app.mode, Mode::Files);
        assert!(!app.on_map());
        press(&mut app, "gm");
        assert!(!app.on_map());
        assert!(
            app.message
                .as_ref()
                .is_some_and(|(text, _)| text.contains("diff_story_map"))
        );
        // A story told as a list has none either, whatever the setting.
        let mut list = five();
        list.told(Story {
            chapters: vec![story::Chapter {
                title: "All of it".into(),
                files: vec!["src/a.rs".into()],
                touches: vec!["src/a.rs".into()],
                ..Default::default()
            }],
            place: HashMap::from([("src/a.rs".to_string(), 1)]),
            chapter: HashMap::from([("src/a.rs".to_string(), 1)]),
            ..Default::default()
        });
        assert!(!list.mapped());
        // The menu has no Map or Files with no map.
        press(&mut list, "m");
        assert!(list.menu.is_some());
        assert!(!screen(&mut list).iter().any(|l| l.contains("Files")));
    }

    #[test]
    fn the_diff_opens_in_the_mode_used_last() {
        let kept = Options {
            ux2: true,
            keep: true,
            ..opts()
        };
        let _ = std::fs::remove_file(crate::paths::state_dir().join("diff-mode"));
        let mut app = map_app_with(kept);
        assert_eq!(app.mode, Mode::Map, "Map the first time");
        press(&mut app, "gf");
        assert_eq!(map_app_with(kept).mode, Mode::Files);
        press(&mut app, "gm");
        assert_eq!(map_app_with(kept).mode, Mode::Map);
        let _ = std::fs::remove_file(crate::paths::state_dir().join("diff-mode"));
    }

    #[test]
    fn the_pages_keys_work_from_the_map_and_the_palettes_reset_closes_the_menu() {
        let mut app = map_app();
        press(&mut app, "j]p");
        assert_eq!(app.page, Some(2), "]p opens the chosen chapter");
        press(&mut app, "]p");
        assert_eq!(app.page, Some(1), "then the next read from the goal down");
        press(&mut app, "[p[p[p");
        assert!(app.on_map(), "[p from the first goes back to the map");
        // The palette's commands press C-\ C-n first, then their keys.
        press(&mut app, "m");
        assert!(app.menu.is_some());
        app.key(KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::CONTROL));
        assert!(app.menu.is_none());
        press(&mut app, "yS");
        assert!(
            app.message
                .as_ref()
                .is_some_and(|(text, _)| text.contains("Copied the story"))
        );
        // A right click opens the menu too, and the map keeps the keys.
        press(&mut app, "gm");
        app.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: 60,
            row: 20,
            modifiers: KeyModifiers::NONE,
        });
        assert!(app.menu.is_some());
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        press(&mut app, "w");
        assert_eq!(app.focus, Focus::List);
        let before = app.map_at;
        press(&mut app, "]c");
        assert_ne!(app.map_at, before);
        // / searches the menu, as it searches every menu.
        press(&mut app, "m/fil");
        let shown = screen(&mut app);
        assert!(shown.iter().any(|l| l.contains("› Files")));
        assert!(!shown.iter().any(|l| l.contains("   Map ")), "{shown:#?}");
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Files);
    }
}
