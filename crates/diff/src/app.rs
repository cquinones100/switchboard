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
//! then saying what is read and each page as Haiku names it (`t` shows
//! the answer itself); Esc shows the diff now. Each page heads its first
//! file in the list with its number, title and why, each hunk worth it
//! carries a note over it in the diff, ]c and [c move between the pages,
//! ]p and [p open them a page at a time, and yS copies the story as
//! Markdown. Without a story, the files keep the kinds' order and Haiku's
//! order of the code by its paths (order.rs).
//!
//! A story told in chapters (the diff_story_map setting) opens on what
//! changes, read down one column beside an outline of its pages, a view
//! beside the file view (app/reading.rs): `g m` and `g f` switch between
//! the two, and `m` asks which.
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
//! uncommitted in a last "Not committed yet" page drawn without asking
//! Haiku (story.rs's `with_uncommitted`), the commits in a "Committed"
//! page before it when they have no story, and with nothing committed
//! that page alone, under no cover; a file changed since its commits
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
//! The list's width is yours to set (width.rs): a drag of its right edge or
//! the pane resize keys while it has the keys, kept for the next diff, the
//! menu's "Use the file list's default width" going back to the default.
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
use switchboard_ui::pointer::Pointer;
use switchboard_ui::screen::Screen;

use crate::host::host;
use crate::{settings, util};

mod draw;
mod map;
mod own;
mod page;
mod reading;
mod select;
mod trail;
mod width;

pub use page::story_page;

pub use reading::Mode;
use reading::{Hit, Place_, Reader};
pub use width::ResizeKey;

/// The list's narrow width, and how wide a list must be to head a page
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
    /// The Story view's column.
    Story,
}

/// The settings the diff follows, read as it opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Files in reading order (diff_order); off, A to Z.
    pub order: bool,
    /// The diff told as a story (diff_story, with diff_order).
    pub story: bool,
    /// ]p and [p open pages a page at a time (diff_story_pages).
    pub pages_alone: bool,
    /// The cover shows what is read and each page (diff_story_progress).
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
    /// UX 2.0 (ux2): the list under the kit's labels, the page's why over
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
    /// The story told in chapters, with a Story view beside the files
    /// (diff_story_map, with diff_story).
    pub map: bool,
    /// The mode used last and the pages read are kept between runs;
    /// off in the tests.
    pub keep: bool,
    /// A click takes an option of a menu, and a right click opens one
    /// (menu_mouse).
    pub menu_mouse: bool,
    /// Alone in a terminal that takes the mouse pointer's shapes (Ghostty,
    /// kitty), the diff sets the pointer itself (with hover_highlight); in
    /// a pane the workspace does, told the shape; off in the tests.
    pub pointer: bool,
    /// The list's width is yours: a drag of its edge and the pane resize
    /// keys set it, and the width kept is the one it opens at
    /// (diff_list_resize); off, it is `LIST_WIDTH`.
    pub resize: bool,
}

impl Options {
    pub fn read() -> Options {
        let on = settings::is_on;
        let order = on("diff_order");
        let story = order && on("diff_story");
        Options {
            order,
            story,
            pages_alone: story && on("diff_story_pages"),
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
            resize: on("diff_list_resize"),
            haiku: true,
            server: host().in_pane(),
            clipboard: true,
            pointer: !host().in_pane() && switchboard_ui::pointer::here(),
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
    /// The Story column's hunks colored, by the hash of each hunk's
    /// path and lines (`reading::hunk_key`).
    HunksColored(Vec<(u64, Option<reading::HunkColors>)>),
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
    /// The line over a hunk a page shows from another, across both
    /// sides: that page's number and the hunk's id (none for the line at
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
    /// With UX 2.0: a page's number and title, a section's label in its
    /// tone with its count, and the blank row between pages.
    Page(usize),
    /// In a story told in chapters, with UX 2.0: a chapter's number and name
    /// over its pages, as board 136 lists them.
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
    /// Back where the reader was before the last jump, and forward again
    /// (trail.rs).
    Back,
    Forward,
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

/// The label over the pages no chapter holds, in the strip's chip and
/// Files mode's list.
const FILES_ONLY: &str = "In the files only";

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
            key_row(
                "]c [c",
                "next, last chapter of the story in the file list (a story told as a list: its next, last page)",
            ),
            key_row(
                "]p [p",
                "next page as a page of its own, the one before (from the first, the whole diff)",
            ),
            key_row("]f [f", "next, last file, opened at its first change"),
            key_row("J K", "scroll the chosen file down, up from the file list"),
            key_row("E", "widen the file list, or narrow it again"),
            key_row(
                "C-M-S-h C-M-S-l",
                "narrow, widen the file list, kept for the next diff (a drag of its edge too)",
            ),
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
                "gs gm gf",
                "the story view, the story's map, and the page being read in the file view",
            ),
            key_row(
                "m",
                "the view's menu: the story, the map or the files, Tell it again",
            ),
            ask_row("ga", "ask Claude about the diff"),
            key_row(
                "m",
                "the diff's menu, in the file list: its settings, and what its buttons do",
            ),
            key_row("S-F10", "the diff's menu, from the list or the diff"),
            key_row(
                "H L",
                "back to where you were before a jump, and forward again",
            ),
            key_row("q esc", "close the diff (esc in the file list)"),
            key_row("^c", "close the diff and go back to the pane"),
        ],
    ),
    (
        "M-d: the story view",
        &[
            key_row("j k", "move in the outline, or down and up the column"),
            key_row(
                "J K",
                "scroll the column down, up, from the outline or the column",
            ),
            key_row(
                "↵",
                "in the outline, read from there (In the files only: their files); in the column, a change's menu, or a file of tests unfolded",
            ),
            key_row("] [", "the next page's section, the one before"),
            key_row("tab", "between the outline and the column"),
            key_row("h l", "along What runs, from the column"),
            key_row(
                "v V",
                "select lines of the column: y then copies their text",
            ),
            key_row("gf", "the page being read in the file view"),
        ],
    ),
    (
        "M-d: the story's map",
        &[
            key_row(
                "j k h l",
                "down a level to what the page builds on, up to what needs it, across a level",
            ),
            key_row("↵", "read the chosen page in the story view"),
            key_row("] [", "the next chapter's map, the one before"),
            key_row(
                "tab",
                "the chapters' strip: h l between the chapters, ↵ on In the files only opens their files",
            ),
        ],
    ),
    (
        "M-d: the cover while the story is told",
        &[
            key_row("esc q", "show the diff now, without waiting for the story"),
            key_row("t", "show Haiku's answer so far, or the pages again"),
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
    /// The width you gave the list (width.rs), which it opens at and `E`
    /// narrows it to; none for `LIST_WIDTH`. Whether `E` (or
    /// diff_list_wide) has the list as wide as its longest path, which it
    /// stays as the screen changes size; whether a drag, a key or the
    /// menu has set the width here, which a commit's diff gives back to
    /// the branch's; the file it is kept in; the column a press on the
    /// list's edge landed on and whether the drag after it has moved the
    /// edge; whether the mouse is over the edge; and the keys that move a
    /// pane's divider left or right.
    set_width: Option<u16>,
    list_widened: bool,
    width_touched: bool,
    width_file: Option<PathBuf>,
    edge_drag: Option<(u16, bool)>,
    edge_hover: bool,
    resize_keys: Vec<ResizeKey>,
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
    /// committed yet the story shown adds in a last page).
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
    /// The page shown as a page, and the file the whole diff showed.
    alone: Option<usize>,
    whole_at: Option<String>,
    /// With a story told in chapters (reading.rs): which of the two
    /// views is shown, the Story view's own state, and the sections read
    /// (0 what changes, else a page).
    mode: Mode,
    reader: Reader,
    /// The map's own state (map.rs).
    mapper: map::Mapper,
    read: HashSet<usize>,
    /// The story asked next is Tell it again's, asked anew.
    retelling: bool,
    /// Where each thing of the Story view a click lands on was last
    /// drawn, and the one under the mouse.
    hits: Vec<(Hit, Rect)>,
    lit: Option<Hit>,
    /// What of the Story view a press landed on that was already chosen,
    /// done once the mouse is let go with nothing dragged over, and
    /// where.
    open_on_up: Option<Hit>,
    up_at: (u16, u16),
    /// The "also in page" line a press landed on, gone to once the
    /// mouse is let go with nothing dragged over.
    also_on_up: Option<(usize, Option<String>)>,
    /// Where Files mode was left, for the way back from the Story view.
    files_place: Option<Place_>,
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
    /// Where the mouse was last, and the pointer's shape last said for it
    /// (`point`), none until one is.
    mouse_at: Option<(u16, u16)>,
    pointer: Option<Pointer>,
    /// Where each button was last drawn.
    buttons: Vec<(Button, Rect)>,
    /// The row of buttons over the diff, where each was drawn on the
    /// screen (header and all).
    bar: Vec<(Button, Rect)>,
    /// Where the reader was before each jump, and where back left
    /// (trail.rs).
    back: Vec<trail::Spot>,
    ahead: Vec<trail::Spot>,
    /// Where the boxes were last drawn, for clicks outside them.
    ask_rect: Option<Rect>,
    answer_rect: Option<Rect>,
    /// The list as last drawn: how many rows it has, and how many of them
    /// run down to its last file (none with no file), which a click shows
    /// a file from; the pointer's shape is read from these, so a movement
    /// of the mouse never builds the list's rows again.
    list_drawn: (usize, Option<usize>),
    /// The diff's scroll bar where it was last drawn: its column, first
    /// row and height, for the pointer's shape over it.
    diff_bar: Option<(u16, u16, usize)>,
    /// The row the diff's text started on as last drawn, under its
    /// heading, for the pointer's shape over it.
    body_top: Option<u16>,
    /// Who tells the workspace the pointer's shape, off the thread that
    /// draws and reads the keys, started with the first shape (`tell`).
    pointer_tx: Option<(Sender<Option<&'static str>>, std::thread::JoinHandle<()>)>,
    /// The rows of the answer last drawn, as text, from its window's first:
    /// the pointer's shape over them is read from these, so a movement of
    /// the mouse never lays the whole answer out again.
    answer_shown: Vec<String>,
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
            reading: Reading::default(),
            order: Vec::new(),
            at: 0,
            path_filter: String::new(),
            filtering: false,
            list_top: 0,
            list_width: LIST_WIDTH,
            set_width: None,
            list_widened: false,
            width_touched: false,
            width_file: None,
            edge_drag: None,
            edge_hover: false,
            resize_keys: width::default_keys(),
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
            alone: None,
            whole_at: None,
            mode: Mode::Files,
            reader: Reader::default(),
            mapper: map::Mapper::default(),
            read: HashSet::new(),
            retelling: false,
            hits: Vec::new(),
            lit: None,
            open_on_up: None,
            up_at: (0, 0),
            also_on_up: None,
            files_place: None,
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
            mouse_at: None,
            pointer: None,
            buttons: Vec::new(),
            bar: Vec::new(),
            back: Vec::new(),
            ahead: Vec::new(),
            ask_rect: None,
            answer_rect: None,
            list_drawn: (0, None),
            diff_bar: None,
            body_top: None,
            pointer_tx: None,
            answer_shown: Vec::new(),
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
        // The pane gets its beam back; alone, the terminal does as it is
        // put back (`Term`).
        // Once what waits to be told has been, this is asked on this
        // thread, which has nothing left to draw: the diff may have been
        // run from a shell that stays in the pane, which must not keep the
        // diff's shape.
        if self.opts.server && self.pointer.take().is_some() {
            if let Some((tx, told)) = self.pointer_tx.take() {
                drop(tx);
                let _ = told.join();
            }
            host().pointer(None);
        }
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
        let rows = rows.saturating_sub(self.lead());
        self.screen_now(cols, rows);
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
                | Msg::HunksColored(_)
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
            Msg::Heard(_, Heard::Progress(p)) => {
                // The model that tells it, where another program asked
                // first and this one follows its telling.
                if !p.model.is_empty() {
                    self.reading.model = story::model_by(Some(&p.model), 0, 0, false);
                }
                self.progress = Some(p);
            }
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
            Msg::HunksColored(colored) => self.hunks_colored(colored),
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
        // A width of your own is the one it opens at, whatever
        // diff_list_wide says; E still widens it.
        // A commit's diff the branch's widened list opened is fitted to
        // its own paths too.
        if (self.opts.list_wide && self.kept_width().is_none())
            || (self.opts.resize && self.in_history && self.list_widened)
        {
            self.list_width = self.fit_width();
            // Wide whatever the screen leaves it now, so a wider screen
            // fits it to its longest path.
            self.list_widened = true;
        }
        self.pick_first();
        self.ask_story(true);
    }

    /// The story's context: the tab's plan note, with the diff_story_plan
    /// setting on and the diff in a pane of the workspace, asked of the
    /// workspace for each telling, so a plan turned off since is never sent.
    fn story_context(&mut self) -> String {
        if self.opts.server {
            story::plan_context(None)
        } else {
            String::new()
        }
    }

    /// What Haiku is given of the whole change for a question (`ga`), and
    /// its hunks, which a question's cursor is found in: the whole diff
    /// shown, what isn't committed yet included.
    fn read_input(&mut self, change: &Change) {
        let (diffs, hunks) = story::split(&change.runs());
        let commits = story::commits(&change.raw_commits);
        // Never the tab's plan: diff_story_plan gives it to the story alone.
        self.input = story::input(&story_entries(change), &diffs, &hunks, &commits, "");
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
    /// files in a last page of their own (`story::with_uncommitted`),
    /// the commits in a "Committed" page before it when they have no
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
        // The column is laid out from the story: again for this one.
        self.reader.target = None;
        self.relay();
        if self
            .alone
            .is_some_and(|p| self.story.as_ref().is_none_or(|s| p > s.pages.len()))
        {
            self.alone = None;
            self.whole_at = None;
            self.lines.clear();
        }
    }

    /// No story in chapters, and none coming (the change read again has no
    /// story, or one that is told as a list): the files as they were left
    /// before the Story view took their place. One still being told keeps
    /// the view.
    fn settle_mode(&mut self) -> bool {
        let falls = !self.mapped() && self.mode != Mode::Files && self.telling != Telling::Waiting;
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
    /// committed put in its last page again; else asked for, with no
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
        // In the shape the open story was asked in, so a setting changed
        // since reads no unchanged story as another.
        let (key, _) = story::key_of_shaped(change.told_part(), self.opts.map);
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
            // The story to come is another one, whose pages aren't
            // this one's by number: the Story view starts again on it.
            if self.mode != Mode::Files {
                self.alone = None;
                self.whole_at = None;
                self.reader = Reader::default();
            }
            self.forget_alone();
            self.show_story(None);
            self.ask_story(false);
            // The files where they were left: nothing of the page's
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
        let page: Option<HashSet<&str>> = self.alone.and_then(|p| {
            self.story
                .as_ref()
                .and_then(|s| s.pages.get(p - 1))
                .map(|c| c.touches.iter().map(String::as_str).collect())
        });
        // A file listed twice (staged, and changed since) is on the page
        // by the diff its hunks there are from; one with no hunk of the
        // story's (binary, renamed) by its path alone.
        let of_alone = |entry: &Entry| -> bool {
            // A story told as a list lists a page's files by their path,
            // as it did.
            if !self.mapped() {
                return true;
            }
            let (Some(page), Some(story), Some(run)) =
                (self.alone, &self.story, self.run_of(entry))
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
                    h.page == page || story.repeats.get(&h.id).is_some_and(|c| c.contains(&page))
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
                    .is_none_or(|p| p.contains(entries[i].path.as_str()) && of_alone(&entries[i]))
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
            // a story, nothing is apart and its pages place them.
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
        self.narrow_width().max((longest + 14).min(self.cols / 2))
    }

    /// The story asked for, under the cover (with `cover`), unless there is
    /// none to tell: two files of someone's at least, since generated files
    /// go last whatever is said. In a change running to the working tree it
    /// is its commits' story, the one told ahead, found under the same key;
    /// with nothing committed there is none to wait for, only the files not
    /// committed yet in their page.
    fn ask_story(&mut self, cover: bool) {
        let context = self.story_context();
        let Some(change) = &self.change else {
            return;
        };
        let told = change.told_part();
        // One shape for the key, the prompt, the parse and the view that
        // shows what comes: the diff's own, as it opened or as Tell it
        // again last read it.
        let map = self.opts.map;
        let (key, paths) = story::key_of_shaped(told, map);
        let entries = story_entries(told);
        let (diffs, hunks) = story::split(&told.runs());
        // Made from the told part each time, so a retell's plan read anew
        // goes to the story alone, never into what a question sends.
        let commits = story::commits(&told.raw_commits);
        let input = story::input(&entries, &diffs, &hunks, &commits, &context);
        let nothing_committed = change.to_work_tree() && told.entries.is_empty();
        let conflict = change.entries.iter().any(|e| e.status == 'U');
        // What the model is given, and which model, for the cover's
        // first lines.
        let (additions, deletions) = (
            entries.iter().map(|e| e.additions).sum::<usize>(),
            entries.iter().map(|e| e.deletions).sum::<usize>(),
        );
        let model = story::model(entries.len(), additions + deletions, !context.is_empty());
        self.reading = Reading {
            files: entries.len(),
            additions,
            deletions,
            estimate: Some(story::estimate(story::count(&hunks))),
            model,
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
        // A telling before this one said nothing of this one.
        self.progress = None;
        if cover {
            self.cover = Some(Instant::now());
        }
        let asked = Asked {
            key,
            input,
            paths,
            hunks,
            model: model.into(),
            plan: !context.is_empty(),
            fresh: std::mem::take(&mut self.retelling),
            map,
        };
        let teller = self.teller.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            teller.tell(&asked, &|heard| {
                let _ = tx.send(Msg::Heard(id, heard));
            });
        });
    }

    /// The menu's Tell it again: the story kept forgotten and told anew
    /// under the cover, by the model the diff_story_model setting names
    /// now, the pages read before forgotten with it. The settings and the
    /// tab's plan are read again, and what is sent made anew from them, so
    /// a model or a plan turned off since the diff opened counts.
    pub(super) fn tell_again(&mut self) {
        let Some(key) = self.story_key.clone() else {
            return self.no_story();
        };
        if self.telling == Telling::Waiting {
            return;
        }
        // Read fresh: a story turned off since the diff opened sends nothing.
        crate::host::reread_settings();
        if !(settings::is_on("diff_order") && settings::is_on("diff_story")) {
            return self.say(
                "No story is told: Settings' \"Diff told as a story\" (diff_story) or \"Files in reading order\" (diff_order) is off now.",
            );
        }
        // The shape it is told in, from the settings read fresh above, so
        // the view showing it agrees with what is asked.
        self.opts.map = self.opts.story && settings::is_on("diff_story_map");
        // Both ways it may be told, so a setting changed since the story
        // was told finds no other kept in its place.
        tell::forget(&key);
        if let Some(change) = &self.change {
            for kept in story::keys_of(change.told_part()) {
                tell::forget(&kept);
            }
        }
        self.retelling = true;
        self.read.clear();
        self.reader = Reader::default();
        self.forget_alone();
        self.show_story(None);
        // A page shown alone is let go: the whole list again meanwhile.
        self.lines.clear();
        self.sort();
        let at = self.at.min(self.order.len().saturating_sub(1));
        self.show_at(at);
        self.ask_story(true);
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

    /// The story is told: the list in its order, headed by its pages.
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

    /// The columns the list is drawn in: its width, or fewer in a pane too
    /// narrow to leave the diff ten beside it. A click and the pointer's
    /// shape take the list to end where it is drawn.
    fn list_cols(&self) -> u16 {
        self.list_width.min(self.cols.saturating_sub(10))
    }

    /// The diff area's columns, and each side's.
    fn diff_area(&self) -> Rect {
        // The Story view's outline is where the list is; the map takes
        // the whole width.
        let x = if self.on_map() {
            0
        } else {
            self.list_cols() + 1
        };
        let bottom = u16::from(self.has_footer());
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

    /// The rows over the diff's text: the page's heading with UX 2.0,
    /// then the bar.
    fn head_height(&self) -> usize {
        self.page_head(self.diff_area().width as usize).len() + 1
    }

    /// The rows the diff's text takes, below the page's heading and the
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

    /// The page a file is shown under: the page's, else its own.
    fn home(&self, path: &str) -> Option<usize> {
        self.alone
            .or_else(|| self.story.as_ref().and_then(|s| s.page.get(path).copied()))
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
        // working tree may have moved: one it shows from another page
        // is said at the file's head, where it can still be gone to.
        let mut head: Vec<Line> = Vec::new();
        if self.opts.notes
            && self.changed_since(entry)
            && let Some(story) = &self.story
        {
            for home in self.also_any(&entry.path, self.run_of(entry)) {
                head.push(self.also_line(story, &entry.path, home, None));
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
                // A hunk shown here from another page, which this one
                // can't be read without: a line over it saying where its
                // home is, its note left there.
                if hunk.page != home
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
                        &entry.path,
                        hunk.page,
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
    /// "✎ Note", or with the page's number and title for a hunk of
    /// another page than `home`; `None` for a hunk of its home page
    /// with no note.
    fn note_card(
        &self,
        story: &Story,
        hunk: &story::HunkNote,
        home: usize,
        ow: usize,
        nw: usize,
    ) -> Option<Vec<Line>> {
        let other = (hunk.page != home)
            .then(|| story.pages.get(hunk.page.wrapping_sub(1)))
            .flatten();
        if hunk.note.is_empty() && other.is_none() {
            return None;
        }
        let theme = self.theme();
        let title = match other {
            // Numbered among the pages told, as the headings count them;
            // the generated files' and what isn't committed by name alone.
            Some(page) if hunk.page <= self.told_pages() => vec![
                Span::styled(
                    if story.chapters.is_empty() {
                        format!("{}/{}", hunk.page, self.told_pages())
                    } else {
                        story::page_of(story, hunk.page, story::chapter_of(story, home))
                    },
                    Tone::Accent.bold(&theme),
                ),
                Span::styled(format!(" {}", page.title), Tone::Text.bold(&theme)),
            ],
            Some(page) => vec![Span::styled(page.title.clone(), Tone::Text.bold(&theme))],
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
        if key.kind != KeyEventKind::Release
            && let Some(forward) = self.trail_key(&key)
        {
            self.message = None;
            self.go_along(forward);
            self.read_view();
            return Act::Redraw;
        }
        let before = self.before_key(&key);
        let act = self.key_now(key);
        self.after(before);
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
        // goes on, so yp and yl still copy the place), any other key ends it,
        // but a pane resize key, which lets it go itself when the list's
        // width changes.
        if self.selection.is_some() && self.resize_step(&key).is_none() {
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
        // A pane resize key, whatever else is open: the list's width, or
        // handed back to the workspace (width.rs); a commit's diff takes it
        // for its own list.
        // Not over a change that couldn't be read or shows nothing, where
        // any key closes the diff.
        let closes = self.failed.is_some() || self.nothing.is_some();
        if let Some(by) = self.resize_step(&key).filter(|_| !closes) {
            if let Some(h) = &mut self.history
                && self.menu.is_none()
                && self.settings.is_none()
            {
                let act = h.app.key(key);
                return self.after_history(act);
            }
            return self.resize_key(by);
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
        // A refreshed story still being told moves no focus among files out
        // of sight.
        if alt_arrow(&key).is_some()
            && self.mode != Mode::Files
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
                    // Told again from the Story view: the files now, as the
                    // cover says, while the story is told.
                    if self.mode != Mode::Files && !self.mapped() {
                        self.files_again();
                    }
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
        // While a refreshed story is told, its place takes only closing and
        // g f, back to the files: nothing acts on files out of sight.
        if self.mode != Mode::Files && !self.mapped() && self.telling == Telling::Waiting {
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
        // The map has the keys while it is on the screen.
        if self.on_map()
            && let Some(act) = self.map_key(key)
        {
            return act;
        }
        // The Story view has the keys while it is on the screen.
        if self.on_story()
            && let Some(act) = self.story_key(key, count)
        {
            self.read_if_reading();
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
            KeyCode::Tab => {
                self.focus = if list { self.main_side() } else { Focus::List };
            }
            KeyCode::BackTab => {
                self.focus = if list { self.main_side() } else { Focus::List };
            }
            KeyCode::Esc if list && self.list_visual.is_some() => self.list_visual = None,
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
                Some((page, hunk)) => self.go_to_also(page, hunk),
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
        // The map has no file or line under a cursor: its pairs are the
        // views', the story's copy and a question.
        if self.on_map() {
            match (first, c) {
                ('g', 'f') => self.show_files(false),
                ('g', 's') => self.open_story_view(),
                ('y', 'S') => self.copy_story(),
                ('g', 'a') if self.opts.ask => self.open_ask(),
                ('w', dir @ ('h' | 'j' | 'k' | 'l')) => self.beside(dir),
                _ => {}
            }
            return Act::Redraw;
        }
        // The Story view has pairs of its own, and no file under a cursor.
        if self.on_story() {
            self.story_pair(first, c);
            self.read_if_reading();
            return Act::Redraw;
        }
        match (first, c) {
            ('g', 'm') => self.open_map_view(),
            ('g', 's') => self.open_story_view(),
            ('g', 'f') => self.show_files(false),
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
            (']', 'c') if self.focus == Focus::List && self.mapped() => self.chapter_jump(1),
            ('[', 'c') if self.focus == Focus::List && self.mapped() => self.chapter_jump(-1),
            (']', 'c') if self.focus == Focus::List => self.page_jump(1),
            ('[', 'c') if self.focus == Focus::List => self.page_jump(-1),
            (']', 'c') => self.change_jump(count, true),
            ('[', 'c') => self.change_jump(count, false),
            (']', 'f') => self.file_jump(count as isize),
            ('[', 'f') => self.file_jump(-(count as isize)),
            (']', 'p') => self.turn_alone(1),
            ('[', 'p') => self.turn_alone(-1),
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
        // The Story view's two panes: the outline and the column, then
        // the panes beside.
        if self.on_story() {
            self.focus = match (dir, self.focus) {
                ('h', Focus::Old | Focus::New) => Focus::List,
                ('l', Focus::List) => Focus::New,
                _ => return self.beside(dir),
            };
            return self.read_if_reading();
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
        // The Story view's column, at its right edge while it runs past.
        if self.on_story() {
            let rect = self.column_rect();
            let total = self.column(self.column_cols()).len();
            let on = x + 1 == rect.right() && y >= rect.y && y < rect.bottom();
            return (on && total > rect.height as usize)
                .then(|| (BarDrag::Story, (y - rect.y) as usize));
        }
        // The list's bar is there while the list last drawn ran past the
        // screen, so nothing is built to find it.
        let pinned = self.list_pinned();
        let w = self.list_width.min(self.cols.saturating_sub(10));
        let h = self.list_height();
        let on_track = y >= pinned && usize::from(y - pinned) < h;
        if w > 0 && x + 1 == w && on_track && self.list_drawn.0 > h {
            return Some((BarDrag::List, (y - pinned) as usize));
        }
        let area = self.diff_area();
        if x + 1 != area.right() {
            return None;
        }
        let body = area.y + self.head_height() as u16;
        if y >= body && y < area.bottom() {
            let (total, _) = self.diff_extent();
            if total > self.body_height() {
                return Some((BarDrag::Diff, (y - body) as usize));
            }
        }
        None
    }

    /// Whether the mouse is over a scroll bar a press would take
    /// (`on_scroll_bar`), found from what was last drawn, so a movement of
    /// the mouse measures nothing: the answer's and the diff's by where
    /// they were drawn, the list's by its rows then, with the filter's
    /// typing ended as a press ends it before it looks.
    fn over_scroll_bar(&mut self, x: u16, y: u16) -> bool {
        let at = |bar: Option<(u16, u16, usize)>| {
            bar.is_some_and(|(bx, by, h)| x == bx && y >= by && usize::from(y - by) < h)
        };
        if !self.opts.scroll_bars || self.ask_box.is_some() {
            return false;
        }
        if self.answer.is_some() {
            return at(self.answer_bar);
        }
        let pinned = u16::from(!self.path_filter.is_empty());
        let h = usize::from(self.diff_area().height.saturating_sub(pinned));
        let w = self.list_width.min(self.cols.saturating_sub(10));
        let on_track = y >= pinned && usize::from(y - pinned) < h;
        (w > 0 && x + 1 == w && on_track && self.list_drawn.0 > h) || at(self.diff_bar)
    }

    /// The first line a scroll bar's surface shows now.
    fn bar_top(&mut self, bar: BarDrag) -> usize {
        match bar {
            BarDrag::List => self.list_top,
            BarDrag::Diff => self.diff_extent().1,
            BarDrag::Answer => self.answer.as_ref().map_or(0, |a| a.top),
            BarDrag::Story => self.reader.top,
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
            BarDrag::Story => self.scroll_story_to(row, drag),
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

    /// E: the list widened to fit its longest path, or narrowed again, to
    /// the width you gave it when you gave it one; the cursor kept on what
    /// it was on as the lines wrap anew.
    fn widen(&mut self) {
        // A wide list whose paths changed since (R listing a longer one)
        // fits them first.
        let refit = self.fit_width();
        if self.opts.resize && self.list_widened && self.list_width != refit {
            self.relay_out(refit);
            return;
        }
        // By the mode rather than the width shown, which the screen's
        // half may cap at the narrow width too; with diff_list_resize off,
        // whose list keeps its width as the screen changes, by the width
        // shown, as before.
        self.list_widened = if self.opts.resize {
            !self.list_widened
        } else {
            self.list_width == LIST_WIDTH
        };
        let to = if self.list_widened {
            self.fit_width()
        } else {
            self.narrow_width()
        };
        self.relay_out(to);
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

    // ── The story's pages ──────────────────────────────────────────────

    /// Why there is no story to move through, said.
    fn no_story(&mut self) {
        let text = if self.in_history {
            "A commit of the history has no story of its own.\nc goes back to the whole branch's."
                .to_string()
        } else if !self.opts.story {
            "No story for this diff: diff stories are off in Settings (diff_story).".to_string()
        } else if self.telling == Telling::Waiting {
            "The story is still being told.\n]p and ]c move through its pages once it is."
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
    /// each page's heading over its first file, and the files.
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
        // The chapter whose pages are listed last, in a story told in chapters.
        // A chapter's staged copy is a group of its own, as ]c walks them.
        let mut listed_chapter: Option<(Section, Option<usize>)> = None;
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
            if let (Some(story), Some(path)) = (&self.story, self.entry(i).map(|e| &e.path))
                && starts[k]
                && let Some(&n) = story.page.get(path)
            {
                // Each chapter's pages under the chapter, and the pages no
                // chapter holds under In the files only, in either layout.
                let chapter_row = self.told_map().and_then(|map| {
                    let chapter = story::chapter_of(map, n);
                    let section = self.entry(i).map(|e| e.section);
                    let group = section.map(|s| (s, chapter));
                    (listed_chapter != group).then_some((map, chapter, group))
                });
                if !self.opts.ux2
                    && let Some((map, chapter, group)) = chapter_row
                {
                    listed_chapter = group;
                    if k > 0 {
                        rows.push(ListRow::Blank);
                    }
                    rows.push(match chapter {
                        Some(c) => ListRow::Chapter(c),
                        None => ListRow::Label(
                            FILES_ONLY.into(),
                            Tone::Dim,
                            Some(self.files_only(map).1),
                        ),
                    });
                }
                if self.opts.ux2 {
                    // A page's number and title, the why going over the
                    // diff; what isn't committed yet under a label of its
                    // own in the needs tone.
                    if k > 0 {
                        rows.push(ListRow::Blank);
                    }
                    if let Some((map, chapter, group)) = chapter_row {
                        listed_chapter = group;
                        rows.push(match chapter {
                            Some(c) => ListRow::Chapter(c),
                            None => ListRow::Label(
                                FILES_ONLY.into(),
                                Tone::Dim,
                                Some(self.files_only(map).1),
                            ),
                        });
                    }
                    if self.uncommitted_page() == Some(n) {
                        let files = self
                            .order
                            .iter()
                            .filter(|&&j| {
                                self.entry(j)
                                    .is_some_and(|e| story.page.get(&e.path) == Some(&n))
                            })
                            .count();
                        rows.push(ListRow::Label(
                            story::NOT_COMMITTED.into(),
                            Tone::Needs,
                            Some(files),
                        ));
                    } else if self.generated_page() == Some(n) {
                        rows.push(ListRow::Label(GENERATED_LAST.into(), Tone::Dim, None));
                    } else {
                        rows.push(ListRow::Page(n));
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
                    .is_none_or(|s| !s.page.contains_key(&e.path))
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
    /// in a last page of that name); a file the commits change too
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

    /// The story's last page when it holds what isn't committed yet,
    /// which UX 2.0 sets apart from the pages Haiku told.
    fn uncommitted_page(&self) -> Option<usize> {
        let story = self.story.as_ref()?;
        let last = story.pages.last()?;
        (self.change.as_ref().is_some_and(Change::to_work_tree)
            && last.title == story::NOT_COMMITTED
            && last.why.is_empty())
        .then_some(story.pages.len())
    }

    /// The page the story made of generated files, which UX 2.0 draws
    /// as their label rather than as a page told.
    fn generated_page(&self) -> Option<usize> {
        let story = self.story.as_ref()?;
        story
            .pages
            .iter()
            .position(|c| c.title == story::GENERATED && c.why == story::GENERATED_WHY)
            .map(|i| i + 1)
    }

    /// The story's pages but the ones of what isn't committed yet and of
    /// generated files.
    fn told_pages(&self) -> usize {
        let all = self.story.as_ref().map_or(0, |s| s.pages.len());
        all - usize::from(self.uncommitted_page().is_some())
            - usize::from(self.generated_page().is_some())
    }

    /// Which of the list's files start a page.
    fn starts(&self) -> Vec<bool> {
        let rows: Vec<story::Row> = self
            .order
            .iter()
            .map(|&i| story::Row {
                page: self
                    .story
                    .as_ref()
                    .and_then(|s| s.page.get(&self.entries()[i].path).copied()),
                section: format!("{:?}", self.entries()[i].section),
            })
            .collect();
        story::starts(&rows)
    }

    /// ]c and [c in the list: the next or the previous heading's file.
    fn page_jump(&mut self, step: i32) {
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
                    "No page {way} this one has a file the filter leaves.\n/ then esc clears it."
                ));
            }
            None => {}
        }
    }

    /// ]p and [p: the story a page at a time, as if each were a commit
    /// of its own, [p from the first going back to the whole diff.
    fn turn_alone(&mut self, step: i32) {
        if self.in_history {
            self.wants = Some(Wants::Step(step));
            return;
        }
        if !self.opts.pages_alone {
            return;
        }
        let Some(story) = &self.story else {
            return self.no_story();
        };
        let index = self.alone.unwrap_or(0) as i32 + step;
        if index > story.pages.len() as i32 {
            return;
        }
        if index < 1 {
            if self.alone.take().is_some() {
                // Laid out for the page alone: again for the whole.
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
            }
            return;
        }
        self.open_alone(index as usize);
    }

    /// A page shown as a page of its own, on the first of its files
    /// someone wrote.
    fn open_alone(&mut self, index: usize) {
        if self.alone.is_none() {
            self.whole_at = self.current().map(Entry::key);
        }
        self.alone = Some(index);
        // The list from its top, as a page of its own.
        self.list_top = 0;
        self.list_followed = None;
        // Each file's notes name pages by the page's: laid out again.
        self.lines.clear();
        self.at = 0;
        self.sort();
        self.at = 0;
        let first = self.first_written();
        self.show_at(first);
    }

    /// The page the line under the diff's cursor says its hunk is also
    /// in, when it is such a line.
    fn also_under(&mut self) -> Option<(usize, Option<String>)> {
        let cur = self.cur;
        match self.screen_lines().get(cur) {
            Some(Line::Also(page, hunk, _)) => Some((*page, hunk.clone())),
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
            pages_alone: false,
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
        app.list_widened = self.list_widened;
        app.set_width = self.set_width;
        app.width_file = self.width_file.clone();
        app.resize_keys = self.resize_keys.clone();
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
        // A width the list was given in a commit's diff is the branch's
        // too; one the commit's diff only opened at, or E widened it to,
        // leaves the branch's list as it was.
        let given = self.history.as_mut().and_then(|h| {
            std::mem::take(&mut h.app.width_touched).then_some((
                h.app.set_width,
                h.app.list_widened,
                h.app.list_width,
            ))
        });
        // The default width asked for there is the branch's own default,
        // from its own paths.
        match given {
            // Its file already taken away there.
            Some((None, ..)) => self.default_width_here(),
            Some((set_width, widened, width)) => {
                self.set_width = set_width;
                self.list_widened = widened;
                self.relay_out(width);
            }
            None => {}
        }
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
        let n = story.pages.len();
        self.clip(&text);
        self.say(format!("Copied the story as Markdown, {n} pages"));
    }

    // ── Asking ──────────────────────────────────────────────────────────────

    /// Where the cursor is, for a question: the file and line of the diff
    /// side it is in, or the file under it in the list.
    fn ask_cursor(&mut self) -> ask::Cursor {
        // In the Story view, the change under the column's cursor; the
        // file list's is out of sight, and not what the question is about.
        if self.on_story() {
            return self.reading_cursor();
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
        // A question gets only a story told from the commits and the diff alone: one that
        // read the tab's plan (or may have, kept before that was known)
        // carries its words, which diff_story_plan gives the story alone.
        let story = self.story.as_ref().filter(|s| s.from_plan == Some(false));
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
        // A key typed in the answer, or a copy made in it, was the
        // answer's, never the first of a pair in the diff.
        self.pending = None;
        self.count = None;
        self.yanked = false;
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
        // From the Story view, the line cited is in the files.
        // Only for a file the diff has: one outside it opens beside, the
        // Story view left as it is.
        let changed = self.entries().iter().any(|e| e.path == path);
        if changed && self.mapped() && self.mode != Mode::Files {
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
        // Where the mouse is on the screen, header and all, for the
        // pointer's shape (`shape_at`).
        self.mouse_at = Some((m.column, m.row));
        // The header's row, alone, takes no click, and the row of buttons
        // under it its own; the rest is the diff's, counted from under
        // them.
        let lead = self.lead();
        if lead > 0 {
            // A drag or a release over them goes on as one at the diff's
            // first row, so a selection started under them ends.
            let held = matches!(m.kind, MouseEventKind::Drag(_) | MouseEventKind::Up(_));
            if m.row < lead && !held {
                if self.has_buttons() && m.row == u16::from(self.has_header()) {
                    let act = self.bar_mouse(m);
                    self.read_view();
                    return act;
                }
                return Act::Same;
            }
            m.row = m.row.saturating_sub(lead);
        }
        let before = self.before_click(&m);
        let act = self.mouse_now(m);
        self.after(before);
        self.read_view();
        act
    }

    /// The rows over the diff: the header, alone, and the row of buttons.
    pub(super) fn lead(&self) -> u16 {
        u16::from(self.has_header()) + u16::from(self.has_buttons())
    }

    /// The mouse on the row of buttons: the one under it lit, and pressed
    /// by a click.
    fn bar_mouse(&mut self, m: MouseEvent) -> Act {
        let hit = self
            .bar
            .iter()
            .find(|(_, r)| m.column >= r.x && m.column < r.right())
            .map(|(b, _)| *b);
        match m.kind {
            MouseEventKind::Moved => {
                if !self.opts.hover || hit == self.hover {
                    return Act::Same;
                }
                self.hover = hit;
                Act::Redraw
            }
            MouseEventKind::Down(MouseButton::Left) => {
                // A click ends typing the filter, the filter kept, as one
                // anywhere does.
                self.menu = None;
                self.filtering = false;
                let Some(button) = hit else {
                    return Act::Redraw;
                };
                let before = self.before_click(&m);
                let act = self.press(button);
                if !matches!(button, Button::Back | Button::Forward) {
                    self.after(before);
                }
                act
            }
            _ => Act::Same,
        }
    }

    /// The button drawn at a cell.
    fn hit(&self, x: u16, y: u16) -> Option<Button> {
        self.buttons
            .iter()
            .find(|(_, r)| x >= r.x && x < r.right() && y >= r.y && y < r.bottom())
            .map(|(b, _)| *b)
    }

    /// Says the mouse pointer's shape for where the mouse is, when it has
    /// changed since it was last said (crates/ui/src/pointer.rs): in a
    /// pane to the workspace, which shows it over the pane, and alone as
    /// what to write to a terminal that takes it. Called once the screen
    /// is drawn and after the mouse, so the shape follows what is under
    /// the mouse now; nothing with hover_highlight off.
    pub fn point(&mut self) -> Vec<u8> {
        // Nobody to show it: nothing is looked up.
        if !self.opts.server && !self.opts.pointer {
            return Vec::new();
        }
        // hover_highlight turned off while it runs: the beam once.
        if !self.opts.hover && self.pointer.take().is_some() {
            if self.opts.server {
                self.tell(Pointer::Text.name());
                return Vec::new();
            }
            return Pointer::Text.osc();
        }
        let Some(shape) = self.point_now() else {
            return Vec::new();
        };
        if self.opts.server {
            self.tell(shape.name());
        } else if self.opts.pointer {
            return shape.osc();
        }
        Vec::new()
    }

    /// Tells the workspace the shape over the diff's pane from a thread of
    /// its own, so the thread that draws and reads
    /// the keys never waits on the server; of several waiting, only the
    /// last is told.
    fn tell(&mut self, shape: &'static str) {
        let (tx, _) = self.pointer_tx.get_or_insert_with(|| {
            let (tx, rx) = channel::<Option<&'static str>>();
            let told = std::thread::spawn(move || {
                while let Ok(mut shape) = rx.recv() {
                    while let Ok(newer) = rx.try_recv() {
                        shape = newer;
                    }
                    host().pointer(shape);
                }
            });
            (tx, told)
        });
        let _ = tx.send(Some(shape));
    }

    /// The shape to say now: the one under the mouse, when it differs from
    /// the one last said.
    fn point_now(&mut self) -> Option<Pointer> {
        if !self.opts.hover {
            return None;
        }
        let (x, y) = self.mouse_at?;
        let shape = self.shape_at(x, y);
        (self.pointer != Some(shape)).then(|| {
            self.pointer = Some(shape);
            shape
        })
    }

    /// The pointer's shape over a cell, by what a click there does
    /// (`mouse_now`): the hand over a button, the list's rows down to its
    /// last file (a click on a row over a file shows that file), a fold in
    /// the diff and a `path:line` in an answer;
    /// the text beam over the diff's text, an answer's, and where a click
    /// types (the filter's line, the question's box); the arrow over the
    /// rest, a scroll bar included, as the workspace's own have it.
    fn shape_at(&mut self, x: u16, mut y: u16) -> Pointer {
        // The header's row takes no click, the row of buttons its own; the
        // rest is counted from under them, as the mouse is (`mouse`).
        let lead = self.lead();
        if y < lead {
            let on_bar = self.has_buttons() && y == u16::from(self.has_header());
            let button = self.bar.iter().any(|(_, r)| x >= r.x && x < r.right());
            return if on_bar && button {
                Pointer::Hand
            } else {
                Pointer::Arrow
            };
        }
        y -= lead;
        // Nothing is read for it, as nothing is while the screen is drawn:
        // a file not read yet is no row of text, in a commit's diff of the
        // history too.
        let drawing = std::mem::replace(&mut self.drawing, true);
        let shape = self.shape_under_header(x, y);
        self.drawing = drawing;
        shape
    }

    fn shape_under_header(&mut self, x: u16, y: u16) -> Pointer {
        // The menu (m, a right click) and the settings view take every
        // click while they are up, over a commit's diff of the history too.
        if let Some(shape) = self.menu_pointer(x, y) {
            return shape;
        }
        if self.settings.is_some() {
            return if self.hit(x, y).is_some() {
                Pointer::Hand
            } else {
                Pointer::Arrow
            };
        }
        if let Some(h) = &mut self.history {
            // A commit's diff reads nothing for it either.
            let drawing = std::mem::replace(&mut h.app.drawing, true);
            let shape = h.app.shape_under_header(x, y);
            h.app.drawing = drawing;
            return shape;
        }
        // A scroll bar's thumb being dragged has the mouse until it is let
        // go, over the row's buttons too.
        if self.bar_drag.is_some() {
            return Pointer::Arrow;
        }
        // The row's buttons press wherever it is drawn, the change being
        // read and under the story's cover too; while a refreshed story is
        // told, whose place takes only closing, Close alone of its keys.
        let map_told =
            self.mode != Mode::Files && !self.mapped() && self.telling == Telling::Waiting;
        if let Some(button) = self.hit(x, y) {
            let inert = map_told && matches!(button, Button::Bar(i) if self.bar_keys(i) != "q");
            return if inert { Pointer::Arrow } else { Pointer::Hand };
        }
        let inside = |r: Option<Rect>| {
            r.is_some_and(|r| x >= r.x && x < r.right() && y >= r.y && y < r.bottom())
        };
        // As a click: the cover, and a refreshed story still being told,
        // take nothing but the row's buttons.
        let waiting = self.cover.is_some() && self.telling == Telling::Waiting;
        // With nothing to show, or the change unread, a click anywhere
        // closes the diff, as any key does.
        if self.failed.is_some() || self.nothing.is_some() {
            return Pointer::Hand;
        }
        if self.bar_drag.is_some() || self.loading.is_some() || waiting || map_told {
            return Pointer::Arrow;
        }
        // The Story view: a hand over an entry of the outline, a line of
        // the column that does something and a step of What runs, which a
        // click takes; the text's shape over the rest of the column.
        if self.ask_box.is_none() && self.answer.is_none() {
            if self.hit_at(x, y).is_some() {
                return Pointer::Hand;
            }
            if self.on_story() {
                return if x > self.list_cols() && y < self.diff_area().bottom() {
                    Pointer::Text
                } else {
                    Pointer::Arrow
                };
            }
        }
        if self.over_scroll_bar(x, y) {
            return Pointer::Arrow;
        }
        if self.ask_box.is_some() {
            return if inside(self.ask_rect) {
                Pointer::Text
            } else {
                Pointer::Arrow
            };
        }
        if self.answer.is_some() {
            let Some(r) = self.answer_rect.filter(|r| inside(Some(*r))) else {
                return Pointer::Arrow;
            };
            let Some((row, col)) = self.answer_cell(r, x, y) else {
                return Pointer::Arrow;
            };
            let cells = usize::from(r.width.saturating_sub(4));
            let cited = self
                .answer_shown
                .get(row)
                .and_then(|line| Self::cited_shown(line, col, cells))
                .is_some();
            return if cited { Pointer::Hand } else { Pointer::Text };
        }
        let area = self.diff_area();
        if y >= area.bottom() {
            return Pointer::Arrow;
        }
        if x < self.list_cols() {
            let pinned = self.list_pinned();
            if y < pinned {
                return Pointer::Text;
            }
            // A click shows the file at the row or the next one under it,
            // so every row down to the last file is pressed; those under
            // it are not.
            let from = self.list_top + (y - pinned) as usize;
            let pressed = self.list_drawn.1.is_some_and(|files| from < files);
            return if pressed {
                Pointer::Hand
            } else {
                Pointer::Arrow
            };
        }
        // Where the diff's rows start, as last drawn: the heading over them
        // (a chapter's why) isn't laid out again.
        let Some(body) = self.body_top else {
            return Pointer::Arrow;
        };
        // The rule beside the list is the diff's body to a click.
        if y < body || self.order.is_empty() {
            return Pointer::Arrow;
        }
        let row = (y - body) as usize;
        let line = if self.one_scroll() {
            // A rule or a bar between two files of the scroll is no text.
            let view = if self.view < self.order.len() {
                self.view
            } else {
                self.at
            };
            let (k, t) = self.stream_ahead(view, self.top, row);
            let on_a_line = self.view == view && self.rows_from_view(k, t) == Some(row);
            self.lines_of(k).get(t).filter(|_| on_a_line).cloned()
        } else if self.shown_binary() {
            // A binary file shows only a line saying so.
            None
        } else {
            // A click under the file's last line goes to that line, and
            // opens it when it is a fold, so it has that line's shape.
            let at = self.top + row;
            let lines = self.screen_lines();
            lines.get(at).or(lines.last()).cloned()
        };
        match line {
            // A fold opens, and an "also in chapter" line goes there.
            Some(Line::Fold { .. } | Line::Also(..)) => Pointer::Hand,
            Some(_) => Pointer::Text,
            None => Pointer::Arrow,
        }
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
            MouseEventKind::Drag(MouseButton::Left) if self.edge_drag.is_some() => {
                return self.drag_edge(x);
            }
            MouseEventKind::Up(MouseButton::Left) if self.edge_drag.is_some() => {
                return self.release_edge(Some((x, y)));
            }
            MouseEventKind::Drag(MouseButton::Left) if self.pressed.is_some() => {
                return self.drag_select(x, y);
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let open = self.open_on_up.take().filter(|_| self.selection.is_none());
                let also = self.also_on_up.take().filter(|_| self.selection.is_none());
                let act = self.release_select();
                if let Some((page, hunk)) = also {
                    self.go_to_also(page, hunk);
                    return Act::Redraw;
                }
                return match open {
                    Some(hit) => {
                        self.story_release(hit, self.up_at);
                        Act::Redraw
                    }
                    None => act,
                };
            }
            MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                let lit = self.hit_at(x, y);
                let edge = hit.is_none() && self.on_edge(x, y);
                if !self.opts.hover
                    || (hit == self.hover && lit == self.lit && edge == self.edge_hover)
                {
                    return Act::Same;
                }
                self.hover = hit;
                self.lit = lit;
                self.edge_hover = edge;
                return Act::Redraw;
            }
            MouseEventKind::Down(MouseButton::Left) => {}
            // The place of a refreshed story still being told takes no wheel.
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
                if self.mode != Mode::Files
                    && !self.mapped()
                    && self.telling == Telling::Waiting =>
            {
                return Act::Same;
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let down = m.kind == MouseEventKind::ScrollDown;
                // Over the map, the wheel reads on from the goal down.
                if self.on_map() && self.answer.is_none() && self.ask_box.is_none() {
                    self.map_step(if down { 1 } else { -1 });
                    return Act::Redraw;
                }
                // Over the Story view, the outline moves and the column scrolls.
                if self.on_story() && self.answer.is_none() && self.ask_box.is_none() {
                    self.story_wheel(x, down);
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
                } else if x < self.list_cols() {
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
        let typing = std::mem::take(&mut self.filtering);
        // The Settings button, wherever the row is drawn: with nothing to
        // show, while the change is read and under the story's cover.
        if hit == Some(Button::Settings) {
            return self.press(Button::Settings);
        }
        if self.failed.is_some() || self.nothing.is_some() {
            return Act::Quit;
        }
        // The cover, and the place of a refreshed story still being told,
        // take no click but the row's buttons: nothing out of sight is chosen.
        let map_told =
            self.mode != Mode::Files && !self.mapped() && self.telling == Telling::Waiting;
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
        // A press on the list's edge takes it: the drag after it sets the
        // list's width (width.rs).
        if self.on_edge(x, y) {
            return self.press_edge(x);
        }
        // A press on something of the Story view does what it does, and a
        // drag from it still selects its text.
        if self.ask_box.is_none()
            && self.answer.is_none()
            && let Some(hit) = self.hit_at(x, y)
        {
            self.selection = None;
            self.pressed = Some((x, y));
            self.up_at = (x, y);
            return self.story_press(hit);
        }
        // Nothing else of the Story view takes a click but the pane it is
        // in, which takes the keys.
        if self.on_story()
            && self.ask_box.is_none()
            && self.answer.is_none()
            && self.on_scroll_bar(x, y).is_none()
        {
            let column = self.column_rect();
            if y < self.diff_area().bottom() {
                self.focus = if x < self.list_cols() {
                    Focus::List
                } else {
                    Focus::New
                };
            }
            // A line of the column with nothing to do takes the cursor,
            // and the page it is in is read, as a line that does does.
            if self.focus != Focus::List && y >= column.y && y < column.bottom() {
                self.click_reading_row((y - column.y) as usize);
            }
            // The outline taking the keys ends the column's selection.
            self.read_if_reading();
            self.selection = None;
            self.pressed = Some((x, y));
            return Act::Redraw;
        }
        if let Some((bar, row)) = self.on_scroll_bar(x, y) {
            // The press focuses the side its bar is on, as a press there does.
            match bar {
                BarDrag::List => self.focus = Focus::List,
                BarDrag::Diff if self.focus == Focus::List => self.focus = self.main_side(),
                BarDrag::Story => self.focus = Focus::New,
                BarDrag::Diff | BarDrag::Answer => {}
            }
            // The drag goes on from where the press left it, the diff's real
            // end included, which reading files on the way can move.
            self.scroll_bar_to(bar, row, None);
            let top = self.bar_top(bar);
            self.bar_drag = Some((bar, y - row as u16, (row, top)));
            return Act::Redraw;
        }
        let selected =
            self.selection.is_some() || self.visual.is_some() || self.list_visual.is_some();
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
            if let Some(r) = self.answer_rect
                && let Some((row, col)) = self.answer_cell(r, x, y)
            {
                let lines = self.answer_lines(r.width);
                let mut found = None;
                if let Some(a) = &mut self.answer {
                    let at = a.top + row;
                    a.cursor = at.min(lines.len().saturating_sub(1));
                    let cells = usize::from(r.width.saturating_sub(4));
                    found = lines
                        .get(at)
                        .and_then(|line| Self::cited_shown(&line.0, col, cells));
                }
                // Gone to once the button is let go, unless it dragged.
                self.pressed_cite = found;
            }
            return Act::Redraw;
        }
        if y >= self.diff_area().bottom() {
            // Drawn again when the click closed a message, ended a
            // selection or the filter's typing, so they are gone from the
            // screen too.
            return if closed || selected || typing {
                Act::Redraw
            } else {
                Act::Same
            };
        }
        if x < self.list_cols() {
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
            // A binary file shown alone shows no lines, so no fold of it
            // is opened.
            if !self.binary_alone()
                && matches!(self.screen_lines().get(cur), Some(Line::Fold { .. }))
            {
                self.open_fold();
            }
            // Gone to as the mouse is let go, so a drag from it selects;
            // a binary file shown alone shows no such line.
            if !self.binary_alone()
                && let Some(also) = self.also_under()
            {
                self.also_on_up = Some(also);
            }
        }
        Act::Redraw
    }

    /// The line of an answer's body and the column of its text a screen
    /// cell is, in its window `r`: none on the window's edges, its
    /// padding, its scroll bar's column and its heading, which hold none
    /// of the answer's text, for a click and for the pointer's shape alike.
    fn answer_cell(&self, r: Rect, x: u16, y: u16) -> Option<(usize, usize)> {
        let row = usize::from(y.checked_sub(r.y + 2)?);
        let col = usize::from(x.checked_sub(r.x + 2)?);
        (row < self.answer_body() && x + 2 < r.right()).then_some((row, col))
    }

    /// The `path:line` drawn at column `col` of an answer's line shown in
    /// `cells` columns: only one the window shows whole, since a line cut
    /// at its right edge (a row of code, which doesn't wrap) may show
    /// `a.rs:12` of `a.rs:1234`, which names another line.
    fn cited_shown(line: &str, col: usize, cells: usize) -> Option<ask::Cited> {
        let shown = line
            .char_indices()
            .map(|(i, c)| i + c.len_utf8())
            .take_while(|end| width(&line[..*end]) <= cells)
            .last()
            .map_or("", |end| &line[..end]);
        // What is shown is looked in first, so a movement over a row with
        // no path:line under the mouse reads no more than the window shows;
        // then the line on to the end of the word cut at the window's edge,
        // whose path:line must be the same.
        let seen = Self::cited_at(shown, col)?;
        let end = line[shown.len()..]
            .find(char::is_whitespace)
            .map_or(line.len(), |i| shown.len() + i);
        (Self::cited_at(&line[..end], col).as_ref() == Some(&seen)).then_some(seen)
    }

    /// The `path:line` drawn at column `col` of an answer's line, the
    /// column counted in cells as the line is drawn, whatever bytes its
    /// characters before it take.
    fn cited_at(line: &str, col: usize) -> Option<ask::Cited> {
        // The text up to and with each character is measured whole, as it
        // is drawn, so characters drawn as one (an emoji of several) count
        // the cells they take together.
        let (byte, _) = line
            .char_indices()
            .find(|(i, c)| width(&line[..i + c.len_utf8()]) > col)?;
        ask::cited(line, byte)
    }

    /// The keys easy mode's `i`th button presses, as the row is drawn.
    fn bar_keys(&self, i: usize) -> &'static str {
        BUTTONS
            .iter()
            .filter(|(_, k)| self.opts.ask || *k != "ga")
            .nth(i)
            .map(|(_, k)| *k)
            .unwrap_or("")
    }

    /// Whether the file shown is binary and shown alone, a line saying
    /// so in place of its rows (`draw_diff_body`); in the one scroll of
    /// UX 2.0 its rows are drawn as built.
    fn binary_alone(&self) -> bool {
        !self.one_scroll() && self.shown_binary()
    }

    /// Whether the file shown is binary.
    fn shown_binary(&self) -> bool {
        self.current()
            .and_then(|e| self.files.get(&e.key()))
            .is_some_and(|s| s.binary)
    }

    /// The button at a cell, as a click and the pointer's shape find it:
    /// while the question's box or an answer is open, only its own, since
    /// the row's keys would go to the box (typed into the question, or not
    /// an answer's keys); a click on another closes the box, as a click
    /// anywhere outside it does.
    /// A button pressed, as its keys would be.
    fn press(&mut self, button: Button) -> Act {
        // A button does its own work, never finishing a pair, a count or
        // a copy made before the click.
        self.pending = None;
        self.count = None;
        self.yanked = false;
        match button {
            Button::Back | Button::Forward => {
                self.settings = None;
                self.go_along(button == Button::Forward);
                return Act::Redraw;
            }
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
                let keys = self.bar_keys(i);
                // Over the settings view, a button goes back to the diff
                // and does its work there; Close only closes the view, and
                // so does any of them over a question's draft, which its
                // keys would be typed into.
                let over_view = self.settings.take().is_some();
                if over_view && (keys == "q" || self.ask_box.is_some()) {
                    return Act::Redraw;
                }
                // Its keys may go on to a commit's diff of the history,
                // which they reach as its own: no pair, count or copy of
                // its own finishes them, and typing in its filter ends,
                // the filter kept, as a click ends it here.
                if let Some(h) = &mut self.history {
                    h.app.pending = None;
                    h.app.count = None;
                    h.app.yanked = false;
                    h.app.filtering = false;
                }
                // Over an answer, whose keys they aren't, the answer goes
                // first, so the button does its work in the diff.
                // Over an answer, whose keys they aren't, the answer goes
                // first, so the button does its work in the diff.
                if self.answer.is_some() {
                    self.close_answer();
                }
                self.focus = Focus::List;
                // In the Story view ] and [ alone turn the pages: the page
                // and chapter buttons do what their keys mean there.
                if self.on_story() && matches!(keys, "]c" | "[c" | "]p" | "[p") {
                    match keys {
                        "]c" => self.chapter_step(1),
                        "[c" => self.chapter_step(-1),
                        "]p" if self.opts.pages_alone => self.page_step(1),
                        "[p" if self.opts.pages_alone => self.page_step(-1),
                        _ => {}
                    }
                    self.read_if_reading();
                    return Act::Redraw;
                }
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
struct Term {
    /// Whether the diff set the mouse pointer's shape, so the terminal's
    /// text beam is put back, as the workspace's client puts it back.
    pointer: bool,
}

impl Term {
    fn enter(motion: bool, pointer: bool) -> io::Result<Term> {
        terminal::enable_raw_mode()?;
        let mut out = io::stdout();
        out.write_all(b"\x1b[?1049h\x1b[?25l\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[?2004h\x1b[>1u")?;
        if motion {
            out.write_all(b"\x1b[?1003h")?;
        }
        out.flush()?;
        Ok(Term { pointer })
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        let mut out = io::stdout();
        if self.pointer {
            let _ = out.write_all(&Pointer::Text.osc());
        }
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
    resize_keys: Vec<ResizeKey>,
) -> ExitCode {
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let (tx, rx): (Sender<Msg>, Receiver<Msg>) = channel();
    let opts = Options::read();
    let mut app = App::new(opts, cols, rows, tx.clone());
    app.session = session.filter(|_| settings::is_on("diff_ask_session"));
    app.opened = Some((dir.clone(), base.clone(), working));
    app.round = host().draws("round");
    app.keep_width_in(width::kept_file());
    // In a pane, only the keys the workspace named (none when keys.json
    // binds none, or for a diff typed there by hand, whose keys the
    // workspace keeps); alone, its defaults.
    if opts.server || !resize_keys.is_empty() {
        app.resize_keys = resize_keys;
    }
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
    // its option under the mouse, whatever else is on, and the pointer's
    // shape follows what is under it.
    let mut term = match Term::enter(opts.hover, opts.hover && opts.pointer) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("switchboard diff: can't take the terminal: {e}");
            return ExitCode::FAILURE;
        }
    };
    // The workspace passes the pane resize keys on only to a pane marked
    // with the keys it takes (width.rs), which a diff from an older build
    // never is, nor one with diff_list_resize off, whose keys stay the
    // workspace's.
    // A mark an earlier program left on the pane (one restored with the
    // layout) is taken away when this one takes none.
    if opts.server && opts.resize {
        let keys = format!("keys:{}", width::notations(&app.resize_keys));
        let _ = host().command(&["set", "@resize_keys", &keys]);
    } else if opts.server {
        let _ = host().command(&["set", "-u", "@resize_keys"]);
    }
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
    let mut point = false;
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
            point = true;
        }
        // The pointer's shape, once what is under the mouse is drawn.
        if std::mem::take(&mut point) {
            let shape = app.point();
            if !shape.is_empty() {
                let mut out = io::stdout().lock();
                let _ = out.write_all(&shape);
                let _ = out.flush();
            }
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
                Ok(TermEvent::Mouse(m)) => {
                    point = true;
                    app.mouse(m)
                }
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
    // The terminal's beam is put back only when the diff changed it.
    term.pointer = app.opts.pointer && app.pointer.is_some();
    if opts.server {
        let _ = host().command(&["set", "-u", "@resize_keys"]);
    }
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
            pages_alone: true,
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
            pointer: false,
            resize: true,
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
        let rows = app.rows + app.lead();
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
    fn the_pointer_is_a_hand_over_what_a_click_presses_the_beam_over_text_and_the_arrow_elsewhere()
    {
        let mut app = app();
        app.opts.easy = true;
        app.resize(220, 16);
        screen(&mut app);
        // On the screen, the row of buttons at its top, the diff under it.
        let on_screen = |app: &mut App, x: u16, y: u16| {
            app.mouse(MouseEvent {
                kind: MouseEventKind::Moved,
                column: x,
                row: y,
                modifiers: KeyModifiers::NONE,
            });
            screen(app);
            app.point_now()
        };
        let over = |app: &mut App, x: u16, y: u16| {
            let lead = app.lead();
            on_screen(app, x, y + lead)
        };
        assert_eq!(
            app.point_now(),
            None,
            "nothing until the mouse is somewhere"
        );
        // A button, said once however far the mouse moves on it.
        let (_, at) = *app.bar.iter().find(|(b, _)| *b == Button::Bar(1)).unwrap();
        assert_eq!(on_screen(&mut app, at.x + 1, at.y), Some(Pointer::Hand));
        assert_eq!(on_screen(&mut app, at.x + 2, at.y), None);
        // The diff's text, and the bar over it.
        let area = app.diff_area();
        let body = area.y + app.head_height() as u16;
        assert_eq!(over(&mut app, area.x + 8, body), Some(Pointer::Text));
        assert_eq!(over(&mut app, area.x + 9, body), None);
        assert_eq!(over(&mut app, area.x + 8, area.y), Some(Pointer::Arrow));
        // A file's row in the list and the rows over it, which a click
        // shows the file from, then the rows under its last file.
        let rows = app.list_rows();
        let file = rows
            .iter()
            .position(|r| matches!(r, ListRow::File(_)))
            .unwrap();
        let y = (file - app.list_top) as u16 + app.list_pinned();
        assert_eq!(over(&mut app, 3, y), Some(Pointer::Hand));
        assert!(matches!(rows[0], ListRow::Header(_)));
        assert_eq!(over(&mut app, 3, 0), None, "a heading shows its first file");
        assert_eq!(
            over(&mut app, 3, rows.len() as u16 + 1),
            Some(Pointer::Arrow)
        );
        // The filter's line takes typing, the question's box too; outside
        // the box a click only closes it.
        app.key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert_eq!(over(&mut app, 3, 0), Some(Pointer::Text));
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(over(&mut app, 3, 0), Some(Pointer::Hand));
        for c in "ga".chars() {
            app.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        screen(&mut app);
        let ask = app.ask_rect.expect("the question's box");
        let send = app
            .buttons
            .iter()
            .find(|(b, _)| *b == Button::AskSend)
            .map(|(_, r)| *r)
            .expect("the box's Send");
        assert_ne!(send.y, ask.y + 2, "the typing is on a row of its own");
        assert_eq!(over(&mut app, ask.x + 2, ask.y + 2), Some(Pointer::Text));
        assert_eq!(over(&mut app, send.x, send.y), Some(Pointer::Hand));
        // Outside the box, over the list, the arrow: a click there only
        // closes the box.
        assert_eq!(over(&mut app, 3, y), Some(Pointer::Arrow));
        // While a question is typed the row is Close alone, which closes
        // it: a hand.
        let (_, close) = *app
            .bar
            .iter()
            .find(|(b, _)| matches!(b, Button::Bar(_)))
            .unwrap();
        assert_eq!(on_screen(&mut app, close.x, close.y), Some(Pointer::Hand));
        app.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: close.x,
            row: close.y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(app.ask_box.is_none(), "the click closed the box");
    }

    #[test]
    fn a_citation_is_found_by_the_cell_it_is_drawn_at() {
        let line = "See ‘src/a.rs:12’.";
        let at = |col| App::cited_at(line, col).map(|c| c.line);
        // The quote before it and the quote and the full stop after it are
        // no part of it, though the quotes take three bytes each.
        assert_eq!(at(4), None);
        assert_eq!(at(5), Some(12));
        assert_eq!(at(15), Some(12));
        assert_eq!(at(16), None);
        assert_eq!(at(17), None);
        assert_eq!(at(40), None);
        // Cut at the window's edge, a citation that shows another line is
        // none; one shown whole is.
        assert_eq!(App::cited_shown("src/a.rs:1234", 2, 11), None);
        assert_eq!(
            App::cited_shown("src/a.rs:1234", 2, 13).map(|c| c.line),
            Some(1234)
        );
        // An emoji of several characters is drawn as one, two cells wide.
        let line = "👩‍💻 src/a.rs:12.";
        let at = |col| App::cited_at(line, col).map(|c| c.line);
        assert_eq!(at(2), None);
        assert_eq!(at(3), Some(12));
        assert_eq!(at(13), Some(12));
        assert_eq!(at(14), None);
    }

    #[test]
    fn alone_the_shape_is_written_to_a_terminal_that_takes_it_and_never_with_hover_off() {
        let moved = |app: &mut App, x: u16, y: u16| {
            app.mouse(MouseEvent {
                kind: MouseEventKind::Moved,
                column: x,
                row: y,
                modifiers: KeyModifiers::NONE,
            });
            screen(app);
            app.point()
        };
        let mut app = app();
        screen(&mut app);
        let area = app.diff_area();
        let body = area.y + app.head_height() as u16;
        // A terminal that doesn't take the shapes is sent none.
        assert_eq!(moved(&mut app, area.x + 8, body), b"");
        assert_eq!(moved(&mut app, area.x + 8, area.y), b"");
        app.opts.pointer = true;
        assert_eq!(moved(&mut app, area.x + 8, body), Pointer::Text.osc());
        assert_eq!(moved(&mut app, area.x + 9, body), b"", "once per change");
        assert_eq!(moved(&mut app, area.x + 8, area.y), Pointer::Arrow.osc());
        // With hover_highlight turned off the beam is given back once, and
        // then nothing follows the mouse.
        app.opts.hover = false;
        assert_eq!(moved(&mut app, area.x + 8, body), Pointer::Text.osc());
        assert_eq!(moved(&mut app, area.x + 8, area.y), b"");
    }

    #[test]
    fn easy_mode_puts_a_button_for_each_key_over_the_diff_and_a_click_presses_it() {
        let mut app = app();
        app.opts.easy = true;
        // Wide enough for every button and the keys named after them.
        app.resize(220, 16);
        let shown = screen(&mut app);
        // Over the diff.
        let bar = &shown[0];
        for label in [
            "◀ Page",
            "Page ▶",
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
        assert!(!screen(&mut app)[0].contains("choose a file"));
        // A click on Page ▶ presses ]c, which says why there is no story.
        let (_, at) = *app.bar.iter().find(|(b, _)| *b == Button::Bar(1)).unwrap();
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
        let (_, close) = *app.bar.iter().find(|(b, _)| *b == Button::Bar(8)).unwrap();
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
        // The row of buttons is drawn where it is on the screen; anything
        // else under the header and the row.
        let on_bar = app.bar.iter().find(|(b, _)| *b == button).copied();
        let (down, (_, at)) = match on_bar {
            Some(found) => (0, found),
            None => (
                app.lead(),
                *app.buttons.iter().find(|(b, _)| *b == button).unwrap(),
            ),
        };
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
        // The row of buttons under it, with no easy mode, back and forward
        // first and Settings before Close.
        let bar = &shown[1];
        assert!(bar.starts_with("  ←   →"), "{bar:?}");
        // The diff under them.
        assert!(shown[2].contains("Changes (3)"), "{:?}", shown[2]);
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
            .bar
            .iter()
            .find(|(b, _)| *b == Button::Settings)
            .unwrap();
        let cell = |app: &mut App| draw(app)[(at.x + 1, at.y)].clone();
        let resting = cell(&mut app);
        assert_eq!(resting.bg, Surface::Panel.color(&theme));
        assert_eq!(resting.fg, Tone::Accent.fg(&theme));
        assert!(resting.modifier.contains(ratatui::style::Modifier::BOLD));
        app.mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: at.x + 1,
            row: at.y,
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
        let bar = screen(&mut embedded).remove(0);
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
        let bar = &shown[1];
        assert!(bar.ends_with("Esc back to the diff"), "{bar:?}");
        let (_, at) = *app
            .bar
            .iter()
            .find(|(b, _)| *b == Button::Settings)
            .unwrap();
        let lit = draw(&mut app)[(at.x + 1, at.y)].clone();
        assert_ne!(lit.bg, Surface::Panel.color(&app.theme()));

        // Tab and Shift-Tab reach each row, coming round.
        app.key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert!(screen(&mut app).join("\n").contains("▌ Check for a newer"));
        app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        // j and k move through the rows; Esc goes back to the diff.
        press(&mut app, "j");
        let shown = screen(&mut app).join("\n");
        assert!(shown.contains("pages Claude names"), "{shown}");
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
        let bar = screen(&mut app).remove(1);
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
        let bar = screen(&mut app).remove(1);
        assert!(bar.contains("Settings") && bar.contains("Close"), "{bar:?}");
        assert!(!bar.contains("Page"), "only what works there: {bar:?}");
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

        // A page open as a page, with pages turned off, goes back to the
        // whole diff.
        app.alone = Some(1);
        let before = app.opts;
        app.opts.pages_alone = false;
        app.settings_changed(before);
        assert_eq!(app.alone, None);

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
        let answer = r#"{"pages": [
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
    fn neither_the_file_nor_the_list_is_greyed_wherever_the_keys_are() {
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
        assert_eq!(list, greyed_list, "the list is never greyed");
        assert!(!changed(&list, &greyed_list), "the list stays lit");
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
    fn told_as_a_story_the_pages_place_generated_files_each_heading_once() {
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
        // The list: the story's two pages counted, what isn't committed
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
        // Over the diff, the page, its why and a rule; one bar.
        assert!(shown[0].contains("Page 1 of 2  Bump a"), "{:?}", shown[0]);
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
        // b.js, its page now over the diff.
        press(&mut app, "l");
        for _ in 0..8 {
            press(&mut app, "j");
        }
        assert_eq!(app.current().unwrap().path, "src/a.js");
        press(&mut app, "]f");
        assert_eq!(app.current().unwrap().path, "src/b.js");
        let shown = screen(&mut app);
        assert!(shown[0].contains("Page 2 of 2  Bump b"), "{:?}", shown[0]);
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
        // The uncommitted page over its file, in the needs tone's words.
        let fresh = app
            .order
            .iter()
            .position(|&i| app.entries()[i].path == "src/new.js")
            .unwrap();
        app.show_at(fresh);
        let shown = screen(&mut app);
        assert!(
            shown[0].contains("Not committed yet") && !shown[0].contains("Page"),
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
        // A story that places a generated file makes a page of it, which
        // is drawn as the generated files' label and not counted as told.
        let change = to_work_tree("b = 3", &[], &[]);
        let committed = change.told_part();
        let (_, hunks) = story::split(&committed.runs());
        let (_, mut paths) = story::key_of(committed);
        paths.push("Cargo.lock".into());
        let answer = r#"{"pages": [
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
            shown[0].contains("Generated, last") && !shown[0].contains("Page"),
            "{:?}",
            shown[0]
        );
    }

    #[test]
    fn whats_not_committed_is_marked_and_listed_in_a_last_page_after_the_commits_story() {
        let mut app = working_app();
        // Wide enough for a side's bar to say it whole.
        app.resize(160, 16);
        // No story of the commits (Haiku isn't asked here): they go first,
        // in a page of their own.
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
        // The story told of the commits: its pages, then the files only
        // uncommitted, one in both kept in its committed page.
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
        assert!(app.message.take().unwrap().0.contains("3 pages"));
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
            .pages
            .iter()
            .map(|c| c.title.clone())
            .collect();
        assert_eq!(titles, ["Bump a", "Bump b", "Not committed yet"]);
        assert_eq!(
            app.story.as_ref().unwrap().pages[2].files,
            ["src/c.js", "src/new.js"]
        );
        assert_eq!(
            app.current().unwrap().path,
            "src/new.js",
            "the cursor kept on its file"
        );
        // A new commit: the story told is of commits no longer the
        // branch's, so it goes, the commits in a page of their own until
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
            .pages
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
    fn with_nothing_committed_the_uncommitted_page_stands_alone_and_nothing_at_all_is_said() {
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
    fn a_story_heads_its_pages_in_the_list_and_c_moves_between_them() {
        let mut app = app();
        let answer = r#"{"pages": [
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
            "the page in the bar: {:?}",
            shown[0]
        );
        press(&mut app, "]c");
        assert_eq!(app.current().unwrap().path, "README.md");
        press(&mut app, "[c");
        assert_eq!(app.current().unwrap().path, "src/a.js");
        // A page of the second page shows its files alone.
        press(&mut app, "]p]p");
        assert_eq!(app.alone, Some(2));
        let page: Vec<&str> = app
            .order
            .iter()
            .map(|&i| app.entries()[i].path.as_str())
            .collect();
        assert_eq!(page, ["README.md", "src/new.js"]);
        press(&mut app, "[p[p");
        assert_eq!(app.alone, None, "back to the whole diff");
        assert_eq!(app.order.len(), 3);
        press(&mut app, "yS");
        let out = String::from_utf8_lossy(&app.out).into_owned();
        assert!(out.starts_with("\x1b]52;c;"), "copied through the terminal");
        assert!(app.message.as_ref().unwrap().0.contains("2 pages"));
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
            model: String::new(),
            text: r#"{"pages": [{"title": "Return two", "why": "x", "hunks": []}, {"title": "Sa"#
                .into(),
            started: util::now_secs(),
        });
        let shown = screen(&mut app);
        assert!(
            shown
                .iter()
                .any(|l| l.contains("elling the change as a story"))
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
            "tab" => vec![KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)],
            "↵" => vec![KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)],
            "⇧↵" => vec![KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)],
            "S-F10" => vec![KeyEvent::new(KeyCode::F(10), KeyModifiers::SHIFT)],
            t if t.starts_with("C-M-S-") => t[6..]
                .chars()
                .map(|c| {
                    let mods = KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT;
                    KeyEvent::new(KeyCode::Char(c), mods)
                })
                .collect(),
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
            .bar
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
                        pages: (0..2)
                            .map(|n| story::Page {
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
                        page: paths
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
                "M-d: the story's map" => {
                    app = map_app();
                    press(&mut app, "gm");
                }
                "M-d: the story view" => app = map_app(),
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
                    // Selecting is in a side of the diff, and h goes along
                    // What runs from the Story view's column.
                    let in_column = *title == "M-d: the story view" && token == "h";
                    if token == "v" || token == "V" || in_column {
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
        assert_eq!(key_groups(false).len(), 4);
        assert_eq!(key_groups(true).len(), 6);
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

    /// The answer `map_app` is told: what changes, a first chapter of four
    /// pages and the side work, and what runs. Wiring the page (the goal)
    /// builds on counting and on the topics; the topics build on the store
    /// and show the count's hunk again; counting builds on the store and
    /// has the count's tests.
    const MAP_ANSWER: &str = r#"{"before": "The event page lists signups as a table.", "after": "A map shows signups by ZIP code.",
      "chapters": [
        {"chapter": "Signups map", "kind": "main", "pages": [
            {"id": "c1", "title": "Store each outline", "claim": "Outlines are stored once, not fetched on each view.",
             "how": "store_outline keeps each ZIP's outline; the page reads it back.", "why": "The map needs outlines kept. Each is simplified.", "decided": "Store each outline once.",
             "instead": "of fetching it on each view: too slow.", "watch": "An outline of no points.", "goal": false, "needs": [],
             "hunks": [{"id": "src/store.rs#1", "note": "The store."}], "files": []},
            {"id": "c2", "title": "Count the signups", "why": "The map shows counts. They are folded.", "watch": "", "goal": false, "needs": ["c1"],
             "hunks": [{"id": "src/count.rs#1", "note": "The count."}, {"id": "src/count.test.rs#1", "note": ""}], "files": []},
            {"id": "c3", "title": "Draw the topics", "why": "Topics get a callout.", "watch": "", "goal": false, "needs": ["c1"],
             "hunks": [{"id": "src/topics.rs#1", "note": ""}, {"id": "src/count.rs#1", "note": "", "repeat": true}], "files": []},
            {"id": "c4", "title": "Wire the page", "why": "The page shows both.", "watch": "", "goal": true, "needs": ["c2", "c3"],
             "hunks": [{"id": "src/page.rs#1", "note": ""}], "files": []}
        ]},
        {"chapter": "Side work", "kind": "side", "pages": [
            {"id": "c5", "title": "Format the helpers", "why": "Spacing only.", "watch": "", "goal": true, "needs": [],
             "hunks": [{"id": "src/fmt.rs#1", "note": ""}], "files": []}
        ]}
      ],
      "runs": [{"step": "page()", "page": "c4"}, {"step": "count_signups", "page": "c2"}, {"step": "store_outline", "page": "c1"}]}"#;

    /// A change of six files told in chapters (`MAP_ANSWER`), on a screen
    /// large enough for the Story view, with UX 2.0.
    fn map_app_with(opts: Options) -> App {
        map_app_told(opts, MAP_ANSWER)
    }

    fn map_app_told(opts: Options, answer: &str) -> App {
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
            (
                "src/count.test.rs",
                "fn counts() {\n    assert_eq!(count_signups(), 0);\n}\n",
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
        let mut told = story::parse(answer, &paths, order::generated, &hunks).unwrap();
        told.model = "sonnet".into();
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

    fn enter(app: &mut App) {
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    }

    #[test]
    fn a_story_in_chapters_opens_on_what_changes_with_the_outline_beside_it() {
        let mut app = map_app();
        assert!(app.on_story(), "the Story view the first time");
        // Told from the plan, as kept with the story.
        app.story.as_mut().unwrap().from_plan = Some(true);
        app.relay();
        let shown = screen(&mut app);
        let row = |text: &str| line_with(&shown, text);
        // What runs over it all, the column under it.
        assert!(shown[0].contains("WHAT RUNS"));
        // Over the column, the story's name and size and what it compares with.
        assert!(shown[6].contains("Signups map  a story in 2 chapters, 5 pages · +"));
        assert!(shown[6].contains("against origin/main"), "{:?}", shown[6]);
        // Before and After side by side, After's arrow between them.
        let sides = row("╭─ Before");
        assert!(shown[sides].contains("╭─ After"));
        assert!(shown[row("The event page lists signups")].contains("──▶"));
        assert!(shown[row("The event page lists signups")].contains("A map shows signups"));
        // How it gets there: the first chapter's pages in reading order,
        // each with a How row and a Why row of its why's first sentence.
        assert!(row("WHAT CHANGES") < row("HOW IT GETS THERE"));
        assert!(row("1  Store each outline") < row("2  Count the signups"));
        assert!(row("3  Draw the topics") < row("4  Wire the page"));
        assert_eq!(
            row("   How  store_outline keeps each ZIP's outline"),
            row("1  Store each outline") + 1
        );
        assert_eq!(
            row("   Why  The map needs outlines kept."),
            row("1  Store each outline") + 2
        );
        assert!(
            !shown.iter().any(|l| l.contains("Tested")),
            "no page only tests"
        );
        assert!(!shown.iter().any(|l| l.contains("Each is simplified")));
        assert!(shown[row("Also in this change: Side work")].contains("format the helpers"));
        assert!(
            row("From the plan note and the commits. Told by Sonnet.") < row("Enter reads from 1.")
        );
        // The outline where the file list is: the chapters, What changes
        // and the pages under them, then the Read meter.
        let outline = list(&shown);
        let at = |text: &str| line_with(&outline, text);
        assert!(at("SIGNUPS MAP") < at("1 Signups map"));
        assert!(at("1 Signups map") < at("What changes"));
        assert!(at("What changes") < at("1 Store each outline"));
        assert!(at("4 Wire the page") < at("2 Side work"));
        assert!(at("2 Side work") < at("1 Format the helpers"));
        assert!(
            outline[at("READ")].trim_end().ends_with("0 of 5"),
            "{:?}",
            outline[at("READ")]
        );
        assert!(
            outline[at("What changes")].contains("✓"),
            "what changes is read"
        );
        assert_eq!(
            app.footer_keys()[..2],
            [("↑↓", "outline"), ("↵", "read from here")]
        );
    }

    #[test]
    fn the_column_reads_page_by_page_and_the_outline_follows_it() {
        let mut app = map_app();
        // Enter on What changes reads from page 1: the column has the keys.
        enter(&mut app);
        assert_eq!(app.focus, Focus::New);
        assert_eq!(app.section(), 1);
        let shown = screen(&mut app);
        // The page's heading at the top of the column, where it is at the right.
        assert!(shown[6].contains("1  Store each outline"));
        assert!(
            shown[6].contains("page 1 of 4 · chapter 1"),
            "{:?}",
            shown[6]
        );
        assert!(shown[7].contains("Outlines are stored once, not fetched on each view."));
        assert!(
            shown[8]
                .contains("How  store_outline keeps each ZIP's outline; the page reads it back.")
        );
        let row = |text: &str| line_with(&shown, text);
        assert!(row("Decided  Store each outline once.") < row("Instead  of fetching it"));
        assert!(row("Instead  of fetching it") < row("Watch    An outline of no points."));
        // Then its file, its changes inline with their numbers.
        assert!(shown[row("src/store.rs   +3 -0")].contains("new"));
        assert!(shown[row("The store.")].contains("‣"));
        line_with(&shown, "1 + pub fn store_outline(zip: &str) {");
        // The outline follows: the page chosen and read, the meter counting it.
        let outline = list(&shown);
        assert!(outline[line_with(&outline, "1 Store each outline")].contains("✓"));
        assert!(
            outline[line_with(&outline, "READ")]
                .trim_end()
                .ends_with("1 of 5")
        );
        assert_eq!(app.reader.item, 1);
        // ]p on to the next page and [p back; from the first, what changes.
        press(&mut app, "]");
        assert_eq!(app.section(), 2);
        // [p from below a page's heading goes to the page before, not its own.
        press(&mut app, "jjj");
        assert_eq!(app.section(), 2);
        press(&mut app, "[");
        assert_eq!(app.section(), 1);
        press(&mut app, "[");
        assert_eq!(app.section(), 0);
        // A wider outline lays the column out again on the same page.
        press(&mut app, "jjj");
        let before = app.section();
        app.list_width += 20;
        screen(&mut app);
        assert_eq!(app.section(), before);
        press(&mut app, "[");
        // v selects lines of the column, y copies their text.
        press(&mut app, "vjy");
        assert!(
            app.message
                .as_ref()
                .is_some_and(|(t, _)| t.contains("Copied 2 lines"))
        );
        // The Chapter ▶ button reaches the next chapter's first page; a c
        // typed after ] from the habit of ]c adds nothing.
        app.chapter_step(1);
        assert_eq!(app.section(), 5);
        press(&mut app, "[c");
        assert_eq!(app.section(), 4);
        assert!(app.history.is_none());
        // yl copies no path for a link: the files have the link.
        press(&mut app, "yl");
        assert!(
            app.message
                .as_ref()
                .is_some_and(|(t, _)| t.contains("Show it in the files"))
        );
        // j and k move the outline while it has the keys, the column with it.
        app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focus, Focus::List);
        press(&mut app, "k");
        assert_eq!(app.section(), 3);
        // J and K from the outline scroll the column, the outline keeping
        // the keys.
        let top = app.reader.top;
        press(&mut app, "J");
        assert!(app.reader.top > top);
        assert_eq!(app.focus, Focus::List);
        press(&mut app, "K");
        assert_eq!(app.reader.top, top);
        assert_eq!(
            app.footer_keys()[..2],
            [("↑↓", "outline"), ("↵", "read from here")]
        );
    }

    #[test]
    fn with_pages_a_time_off_the_page_keys_do_nothing_here_too() {
        let mut app = map_app_with(Options {
            ux2: true,
            pages_alone: false,
            ..opts()
        });
        press(&mut app, "]");
        assert_eq!(app.section(), 0);
    }

    #[test]
    fn a_long_outline_scrolls_to_its_chosen_page_the_meter_kept_at_its_foot() {
        let mut app = map_app();
        app.rows = 12;
        app.relay();
        // The side work's page, past what twelve rows show.
        let last = app.items().len() - 1;
        press(&mut app, &"j".repeat(last));
        let shown = list(&screen(&mut app));
        assert!(
            shown.iter().any(|l| l.contains("1 Format the helpers")),
            "{shown:#?}"
        );
        assert!(shown.iter().any(|l| l.contains("READ")));
        assert_eq!(
            app.read_count().0,
            0,
            "moving through the outline reads nothing"
        );
        enter(&mut app);
        assert_eq!(app.read_count().0, 1, "Enter reads it");
        assert!(
            !shown.iter().any(|l| l.contains("SIGNUPS MAP")),
            "scrolled past the top"
        );
    }

    #[test]
    fn the_map_shows_what_builds_on_what_and_enter_reads_a_page() {
        let mut app = map_app();
        press(&mut app, "gm");
        assert!(app.on_map());
        let shown = screen(&mut app);
        let row = |text: &str| line_with(&shown, text);
        // The goal first, each page under what builds on it.
        assert!(row("THE GOAL") < row("4 Wire the page"));
        assert!(row("4 Wire the page") < row("2 Count the signups"));
        assert!(row("2 Count the signups") < row("1 Store each outline"));
        assert!(shown[0].contains("1 Signups map"));
        // j down to what the goal builds on, Enter reads it in the column.
        assert_eq!(app.mapper.at, 4);
        press(&mut app, "j");
        assert_eq!(app.mapper.at, 2);
        enter(&mut app);
        assert!(app.on_story());
        assert_eq!((app.section(), app.focus), (2, Focus::New));
        // Back on the map, on the page being read; ] shows the next chapter.
        press(&mut app, "gm");
        assert_eq!(app.mapper.at, 2);
        press(&mut app, "]c");
        assert_eq!(app.mapper.chapter, 1);
        assert!(app.history.is_none(), "the c after ] adds nothing");
    }

    #[test]
    fn a_change_is_side_by_side_where_the_column_has_room() {
        let mut app = map_app();
        app.cols = 170;
        app.relay();
        app.go_to_section(5);
        let shown = screen(&mut app);
        // The old line at the left, the new beside it, each side 60
        // columns or more.
        let row = line_with(&shown, "fn  spaced() {}");
        assert!(shown[row].contains("│") && shown[row].contains("fn spaced() {}"));
        // A narrow column shows it removed over added.
        app.cols = 120;
        app.relay();
        let shown = screen(&mut app);
        let row = line_with(&shown, "fn  spaced() {}");
        assert!(!shown[row].contains("fn spaced() {}"));
        assert!(shown[row].contains("1 - fn  spaced"), "{}", shown[row]);
        assert!(shown[row + 1].contains("1 + fn spaced"));
        assert!(shown[row + 1].contains("fn spaced() {}"));
    }

    #[test]
    fn the_story_column_s_changes_are_in_their_syntax_s_colors() {
        let (tx, rx) = channel();
        let mut app = map_app();
        app.tx = tx;
        app.relay();
        app.go_to_section(5);
        // The color of the first cell of `text` on the row holding `row`.
        let color_at = |app: &mut App, row: &str, text: &str| {
            let shown = screen(app);
            let y = line_with(&shown, row);
            let x = shown[y].find(text).unwrap();
            let x = shown[y][..x].chars().count() as u16;
            draw(app)[(x, y as u16)].fg
        };
        for cols in [170, 120] {
            app.cols = cols;
            app.relay();
            // Uncolored until the colors come, off the screen's thread,
            // and kept for the next width.
            if cols == 170 {
                let before = color_at(&mut app, "fn  spaced", "fn  spaced");
                assert_ne!(before, Color::Indexed(5));
                let msg = rx.recv_timeout(Duration::from_secs(10)).unwrap();
                assert!(matches!(msg, Msg::HunksColored(_)));
                app.event(msg);
            }
            // The keyword in the keywords' color, the name in the functions',
            // on each side.
            assert_eq!(
                color_at(&mut app, "fn  spaced", "fn  spaced"),
                Color::Indexed(5),
                "{cols}"
            );
            assert_eq!(
                color_at(&mut app, "fn  spaced", "spaced"),
                Color::Indexed(4),
                "{cols}"
            );
            assert_eq!(
                color_at(&mut app, "fn spaced()", "fn spaced()"),
                Color::Indexed(5),
                "{cols}"
            );
        }
    }

    #[test]
    fn a_file_of_tests_is_folded_last_and_enter_unfolds_it() {
        let mut app = map_app();
        app.go_to_section(2);
        app.focus = Focus::New;
        let shown = screen(&mut app);
        let row = |text: &str| line_with(&shown, text);
        assert!(row("src/count.rs") < row("src/count.test.rs"));
        assert!(shown[row("src/count.test.rs")].contains("▸ folded: tests last"));
        assert!(
            !shown
                .iter()
                .any(|l| l.contains("assert_eq!(count_signups()"))
        );
        // The cursor down to the folded file, Enter unfolds it.
        while !app
            .row_at_cur()
            .is_some_and(|r| matches!(r.does, reading::Does::Folded { .. }))
        {
            press(&mut app, "j");
        }
        enter(&mut app);
        let shown = screen(&mut app);
        assert!(
            shown
                .iter()
                .any(|l| l.contains("assert_eq!(count_signups(), 0);"))
        );
    }

    #[test]
    fn enter_on_a_change_opens_its_menu_which_shows_it_in_the_files() {
        let mut app = map_app();
        app.go_to_section(1);
        app.focus = Focus::New;
        while !app
            .row_at_cur()
            .is_some_and(|r| matches!(r.does, reading::Does::Change { .. }))
        {
            press(&mut app, "j");
        }
        enter(&mut app);
        let shown = screen(&mut app);
        assert!(shown.iter().any(|l| l.contains("Show it in the files")));
        enter(&mut app);
        assert_eq!(app.mode, Mode::Files);
        assert_eq!(app.current().map(|e| e.path.as_str()), Some("src/store.rs"));
        assert_eq!(app.focus, Focus::New);
        // g s comes back to the Story view where it was left.
        press(&mut app, "gs");
        assert!(app.on_story());
        assert_eq!(app.section(), 1);
    }

    #[test]
    fn the_story_view_as_a_page_holds_the_whole_column_in_both_themes() {
        let app = map_app();
        let (change, told) = (app.change.clone().unwrap(), app.story.clone().unwrap());
        let page = story_page(opts(), 170, change, told, "Signups <map>").unwrap();
        assert!(page.contains("<title>Signups &lt;map&gt;</title>"));
        // Drawn dark and light, each shown as the browser asks.
        assert_eq!(page.matches("<pre class=").count(), 2);
        assert!(page.contains("prefers-color-scheme: dark"));
        // What runs, the overview and every page's hunks, the file of
        // tests unfolded.
        assert!(page.contains("WHAT RUNS"));
        assert!(page.contains("Format the helpers"));
        assert!(page.contains("assert_eq!(count_signups(), 0);"));
        assert!(page.contains("fold_small"));
        // Code is escaped, and the row of buttons left out.
        assert!(!page.contains("<str"));
        assert!(!page.contains("Settings ["));
    }

    #[test]
    fn switching_views_keeps_the_chapter_and_back_finds_each_view_as_it_was() {
        let mut app = map_app();
        let chapter = |app: &mut App| app.chapter_now().map(|(c, _)| c);
        // A view chosen in the menu or the switch, as Enter there does.
        let switch = |app: &mut App, mode: Mode| {
            let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
            let before = app.before_key(&enter);
            app.choose_mode(mode);
            app.after(before);
        };
        // Story on chapter 1, the files left on it.
        app.go_to_section(1);
        press(&mut app, "gf");
        assert_eq!(chapter(&mut app), Some(0));
        // The map moved on to chapter 2: the Story view and the files
        // follow it there, rather than staying where they were left.
        press(&mut app, "gm]");
        assert_eq!(chapter(&mut app), Some(1));
        switch(&mut app, Mode::Story);
        assert_eq!((app.mode, chapter(&mut app)), (Mode::Story, Some(1)));
        switch(&mut app, Mode::Files);
        assert_eq!((app.mode, chapter(&mut app)), (Mode::Files, Some(1)));
        // And back goes through each view on the chapter it was on.
        press(&mut app, "H");
        assert_eq!((app.mode, chapter(&mut app)), (Mode::Story, Some(1)));
        press(&mut app, "H");
        assert_eq!((app.mode, chapter(&mut app)), (Mode::Map, Some(1)));
        press(&mut app, "H");
        assert_eq!((app.mode, chapter(&mut app)), (Mode::Map, Some(0)));
    }

    #[test]
    fn h_and_l_go_back_and_forward_through_the_jumps_not_the_steps() {
        let mut app = map_app();
        assert_eq!(app.section(), 0);
        // Stepping down the outline is no jump; Enter reading from page 1 is.
        press(&mut app, "j");
        enter(&mut app);
        assert_eq!((app.section(), app.focus), (1, Focus::New));
        press(&mut app, "]");
        assert_eq!(app.section(), 2);
        // A view switched is a jump too.
        press(&mut app, "gf");
        assert_eq!(app.mode, Mode::Files);
        press(&mut app, "H");
        assert_eq!((app.mode, app.section()), (Mode::Story, 2));
        press(&mut app, "H");
        assert_eq!(app.section(), 1);
        press(&mut app, "H");
        assert_eq!((app.section(), app.focus), (1, Focus::List));
        // Forward again, to the files.
        press(&mut app, "LLL");
        assert_eq!(app.mode, Mode::Files);
        press(&mut app, "L");
        assert!(
            app.message
                .take()
                .is_some_and(|(t, _)| t.contains("Nothing to go forward"))
        );
        // The row of buttons leads with back and forward, which a click
        // presses.
        app.opts.easy = true;
        let shown = screen(&mut app);
        assert!(shown[0].starts_with("  ←   →"), "{:?}", shown[0]);
        let (_, back) = *app.bar.iter().find(|(b, _)| *b == Button::Back).unwrap();
        app.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: back.x + 1,
            row: back.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!((app.mode, app.section()), (Mode::Story, 2));
        // A jump after going back leaves nothing forward.
        press(&mut app, "[");
        press(&mut app, "L");
        assert!(app.message.take().is_some());
    }

    #[test]
    fn what_runs_lists_its_steps_in_order_with_the_page_being_read_lit() {
        let mut app = map_app();
        app.go_to_section(2);
        // What runs is always over the column.
        let shown = screen(&mut app);
        let row = |text: &str| line_with(&shown, text);
        assert!(shown[0].contains("WHAT RUNS"));
        let boxes = row("page()");
        assert!(shown[boxes].find("page()") < shown[boxes].find("count_signups"));
        assert!(shown[boxes].find("count_signups") < shown[boxes].find("store_outline"));
        assert!(shown[boxes + 1].contains("page 4"));
        // The step of the page being read is the one lit.
        assert_eq!(app.reader.run, Some(1));
        // l goes along the steps, each showing its page.
        app.focus = Focus::New;
        press(&mut app, "l");
        assert_eq!((app.reader.run, app.section()), (Some(2), 1));
        press(&mut app, "hh");
        assert_eq!((app.reader.run, app.section()), (Some(0), 4));
        // A page with no step lights none, and its boxes name a later
        // chapter's page with its chapter.
        app.go_to_section(3);
        assert_eq!(app.reader.run, None);
        // A click on a step goes to its page.
        let (_, at) = app
            .hits
            .iter()
            .find(|(hit, _)| *hit == Hit::Run(1))
            .copied()
            .unwrap();
        app.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: at.x + 1,
            row: at.y + 1,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.section(), 2);
        // In a narrow column the steps out of sight are a click away.
        app.cols = 64;
        app.relay();
        app.go_to_run(0);
        screen(&mut app);
        let (_, more) = app
            .hits
            .iter()
            .find(|(hit, r)| matches!(hit, Hit::Run(_)) && r.width == 1)
            .copied()
            .unwrap();
        app.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: more.x,
            row: more.y + 1,
            modifiers: KeyModifiers::NONE,
        });
        assert!(app.reader.run.is_some_and(|r| r > 0));
        // J and K scroll the column with What runs shown, from the column.
        app.focus = Focus::New;
        let top = app.reader.top;
        press(&mut app, "J");
        assert!(app.reader.top > top);
        press(&mut app, "K");
        assert_eq!(app.reader.top, top);
        // From the outline, l gives the column the keys.
        app.focus = Focus::List;
        press(&mut app, "l");
        assert_eq!(app.focus, Focus::New);
    }

    #[test]
    fn a_story_told_before_what_changes_was_asked_for_still_opens() {
        let old = MAP_ANSWER
            .replace(r#""before": "The event page lists signups as a table.", "after": "A map shows signups by ZIP code.","#, "")
            .replace(r#""how": "store_outline keeps each ZIP's outline; the page reads it back.", "#, "");
        let old = &old[..old
            .find(
                r#",
      "runs""#,
            )
            .unwrap()];
        let old = format!("{old}}}");
        let mut app = map_app_told(
            Options {
                ux2: true,
                ..opts()
            },
            &old,
        );
        assert!(app.on_story());
        let shown = screen(&mut app);
        assert!(
            shown
                .iter()
                .any(|l| l.contains("told before the diff said what changes"))
        );
        assert!(!shown.iter().any(|l| l.contains("WHAT RUNS")));
        // A page told without a how shows its claim as its How.
        let how = line_with(&shown, "   How  Outlines are stored once");
        assert_eq!(how, line_with(&shown, "1  Store each outline") + 1);
        assert!(!shown.iter().any(|l| l.contains("store_outline keeps")));
        // A page with no claim reads its why in its place.
        app.go_to_section(2);
        let shown = screen(&mut app);
        assert!(shown[1].contains("The map shows counts."), "{shown:#?}");
    }

    #[test]
    fn the_pages_that_only_test_fold_into_one_tested_row() {
        let answer = MAP_ANSWER
            .replace(
                r#"{"id": "src/count.rs#1", "note": "The count."}, {"id": "src/count.test.rs#1", "note": ""}"#,
                r#"{"id": "src/count.rs#1", "note": "The count."}"#,
            )
            .replace(
                r#"{"id": "c3", "title": "Draw the topics""#,
                r#"{"id": "c6", "title": "Test the count", "how": "counts() asserts it.", "why": "Tests check it.", "watch": "", "goal": false, "needs": ["c2"],
             "hunks": [{"id": "src/count.test.rs#1", "note": ""}], "files": []},
            {"id": "c3", "title": "Draw the topics""#,
            );
        let mut app = map_app_told(
            Options {
                ux2: true,
                ..opts()
            },
            &answer,
        );
        let shown = screen(&mut app);
        let row = |text: &str| line_with(&shown, text);
        // No step of its own, the outline still listing it.
        assert!(
            !shown
                .iter()
                .any(|l| l.contains("3  Test the count") || l.contains("counts() asserts"))
        );
        assert!(row("5  Wire the page") < row("Tested  the count (3)"));
        assert!(row("Tested  the count (3)") < row("Also in this change: Side work"));
    }

    #[test]
    fn a_hunk_of_another_chapter_is_named_by_its_chapter_and_page() {
        let app = map_app();
        let story = app.story.as_ref().unwrap();
        let hunk = story::HunkNote {
            id: "src/fmt.rs#1".into(),
            run: 1,
            old_start: 1,
            old_count: 1,
            new_start: 1,
            new_count: 1,
            page: 5,
            note: String::new(),
        };
        let lines = story::note_lines(story, &hunk, 2, 80).unwrap();
        assert!(
            lines[0].contains("chapter 2, 1/1 Format the helpers"),
            "{lines:?}"
        );
    }

    #[test]
    fn a_hunk_in_two_pages_is_in_both_the_column_and_a_page_of_the_files() {
        let mut app = map_app();
        // In the column, under its also line in the page that shows it again.
        app.go_to_section(3);
        let shown = screen(&mut app);
        let also = line_with(&shown, "also in page 2: Count the signups");
        assert!(shown[also + 1].contains("pub fn count_signups"));
        // A page of the files at a time shows it too, Enter on its line
        // going to its home.
        press(&mut app, "gf");
        app.open_alone(3);
        let shown = screen(&mut app);
        assert!(shown.iter().any(|l| l.contains("src/count.rs  also 2")));
        let at = app
            .order
            .iter()
            .position(|&i| app.entries()[i].path == "src/count.rs")
            .unwrap();
        app.show_at(at);
        app.focus = Focus::New;
        app.cur = 0;
        assert_eq!(app.also_under().map(|(n, _)| n), Some(2));
        enter(&mut app);
        assert_eq!(app.alone, Some(2));
    }

    #[test]
    fn each_view_keeps_its_place_and_files_mode_draws_as_it_did() {
        let mut app = map_app();
        // g f: the page being read, in Files mode.
        app.go_to_section(2);
        press(&mut app, "gf");
        assert_eq!(app.mode, Mode::Files);
        assert_eq!(app.current().map(|e| e.path.as_str()), Some("src/count.rs"));
        // Files mode lists the pages under their chapters, each counted in
        // its chapter, and heads the diff by both.
        let shown = screen(&mut app);
        let row = |text: &str| line_with(&shown, text);
        assert!(row(" 1 Signups map") < row(" 1 Store each outline"));
        assert!(row(" 4 Wire the page") < row(" 2 Side work"));
        assert!(row(" 2 Side work") < row(" 1 Format the helpers"));
        assert!(shown[0].contains("Chapter 1 · page 2 of 4  Count the signups"));
        // The menu switches, each view back where it was left.
        press(&mut app, "j");
        let left = app.current().map(|e| e.path.clone());
        press(&mut app, "m");
        assert!(screen(&mut app).iter().any(|l| l.contains("› Files")));
        press(&mut app, "kk");
        enter(&mut app);
        assert!(app.on_story());
        assert_eq!(app.section(), 2, "the column where it was left");
        press(&mut app, "mjj");
        enter(&mut app);
        assert_eq!(app.mode, Mode::Files);
        assert_eq!(app.current().map(|e| e.path.clone()), left);
        // g s comes back from anywhere, the diff's side included, and g m
        // shows the map.
        app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        press(&mut app, "gs");
        assert!(app.on_story());
        press(&mut app, "gm");
        assert!(app.on_map());
    }

    #[test]
    fn easy_mode_leads_its_buttons_with_the_story_and_files_switch() {
        let mut app = map_app_with(Options {
            ux2: true,
            easy: true,
            ..opts()
        });
        let shown = screen(&mut app);
        let last = &shown[0];
        assert!(last.find("Story").unwrap() < last.find("Files").unwrap());
        assert!(last.find("Files").unwrap() < last.find("◀ Page").unwrap());
        let (_, files) = app
            .bar
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
        // A click on an entry of the outline chooses it, and a second
        // reads from it as the mouse is let go.
        press(&mut app, "gs");
        screen(&mut app);
        let (_, at) = app
            .hits
            .iter()
            .find(|(hit, _)| *hit == Hit::Item(2))
            .copied()
            .unwrap();
        // On the screen, under the row of buttons.
        let y = at.y + app.lead();
        assert_eq!(app.shape_at(at.x + 1, y), Pointer::Hand);
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: at.x + 1,
            row: y,
            modifiers: KeyModifiers::NONE,
        };
        let up = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            ..click
        };
        app.mouse(click);
        app.mouse(up);
        assert_eq!(
            (app.reader.item, app.section(), app.focus),
            (2, 2, Focus::List)
        );
        app.mouse(click);
        assert_eq!(app.focus, Focus::List, "it reads as the mouse is let go");
        app.mouse(up);
        assert_eq!(app.focus, Focus::New);
    }

    #[test]
    fn with_the_setting_off_there_is_no_story_view() {
        let mut app = map_app_with(Options {
            ux2: true,
            map: false,
            ..opts()
        });
        assert_eq!(app.mode, Mode::Files);
        assert!(!app.on_story());
        press(&mut app, "gm");
        assert!(!app.on_story());
        assert!(
            app.message
                .as_ref()
                .is_some_and(|(text, _)| text.contains("diff_story_map"))
        );
        // A story told as a list has none either, whatever the setting.
        let mut list = five();
        list.told(Story {
            pages: vec![story::Page {
                title: "All of it".into(),
                files: vec!["src/a.rs".into()],
                touches: vec!["src/a.rs".into()],
                ..Default::default()
            }],
            place: HashMap::from([("src/a.rs".to_string(), 1)]),
            page: HashMap::from([("src/a.rs".to_string(), 1)]),
            ..Default::default()
        });
        assert!(!list.mapped());
        // The menu has no Story or Files with no chapters.
        press(&mut list, "m");
        assert!(list.menu.is_some());
        assert!(!screen(&mut list).iter().any(|l| l.contains("Files")));
    }

    #[test]
    fn the_diff_opens_in_the_view_used_last() {
        let kept = Options {
            ux2: true,
            keep: true,
            ..opts()
        };
        let _ = std::fs::remove_file(crate::paths::state_dir().join("diff-mode"));
        let mut app = map_app_with(kept);
        assert_eq!(app.mode, Mode::Story, "the Story view the first time");
        press(&mut app, "gf");
        assert_eq!(map_app_with(kept).mode, Mode::Files);
        press(&mut app, "gm");
        assert_eq!(map_app_with(kept).mode, Mode::Map);
        press(&mut app, "gs");
        assert_eq!(map_app_with(kept).mode, Mode::Story);
        let _ = std::fs::remove_file(crate::paths::state_dir().join("diff-mode"));
    }

    #[test]
    fn the_story_views_menu_and_pairs_and_the_palettes_reset() {
        let mut app = map_app();
        // The palette's commands press C-\ C-n first, then their keys.
        press(&mut app, "m");
        assert!(app.menu.is_some());
        assert!(screen(&mut app).iter().any(|l| l.contains("Tell it again")));
        app.key(KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::CONTROL));
        assert!(app.menu.is_none());
        press(&mut app, "yS");
        assert!(
            app.message
                .as_ref()
                .is_some_and(|(text, _)| text.contains("Copied the story"))
        );
        // A right click opens the menu too, and the view keeps the keys.
        app.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: 60,
            row: 20,
            modifiers: KeyModifiers::NONE,
        });
        assert!(app.menu.is_some());
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        // C-w w moves between the outline and the column.
        app.key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        press(&mut app, "w");
        assert_eq!(app.focus, Focus::New);
        app.key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        press(&mut app, "h");
        assert_eq!(app.focus, Focus::List);
        // / searches the menu, as it searches every menu.
        press(&mut app, "m/fil");
        let shown = screen(&mut app);
        assert!(shown.iter().any(|l| l.contains("› Files")));
        assert!(!shown.iter().any(|l| l.contains("   Story ")), "{shown:#?}");
        enter(&mut app);
        assert_eq!(app.mode, Mode::Files);
    }

    /// A file of a test's own for the list's width, in a folder made new
    /// for the test, so nothing already in the temporary directory is
    /// written through.
    fn width_file(test: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "switchboard-diff-{test}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir(&dir).expect("a folder of the test's own");
        dir.join("diff-list-width")
    }

    fn remove_width_file(file: &Path) {
        if let Some(dir) = file.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// The diff opened with the list's width kept in `file`.
    fn app_keeping(file: &Path) -> App {
        let mut app = app();
        app.keep_width_in(file.to_path_buf());
        app
    }

    const RESIZE: KeyModifiers = KeyModifiers::CONTROL
        .union(KeyModifiers::ALT)
        .union(KeyModifiers::SHIFT);

    #[test]
    fn a_drag_of_the_list_s_edge_sets_its_width_which_the_next_diff_opens_at() {
        let file = width_file("drag");
        let mut app = app_keeping(&file);
        assert_eq!(app.list_width, LIST_WIDTH, "no file is the default");
        // The edge is the column after the list's last, lit under the mouse.
        assert_eq!(mouse(&mut app, MouseEventKind::Moved, 30, 3), Act::Redraw);
        assert_eq!(draw(&mut app)[(30, 3)].symbol(), "│");
        assert_eq!(mouse(&mut app, MouseEventKind::Moved, 40, 3), Act::Redraw);
        assert_ne!(draw(&mut app)[(30, 3)].symbol(), "│");
        mouse(&mut app, DOWN, 30, 3);
        assert_eq!(mouse(&mut app, DRAG, 42, 5), Act::Redraw);
        assert_eq!(app.list_width, 42);
        assert_eq!(app.diff_area().x, 43);
        assert_eq!(draw(&mut app)[(42, 3)].symbol(), "│", "lit while dragged");
        assert!(!file.exists(), "nothing is kept as the mouse moves");
        // No wider than half the screen, no narrower than 24 columns.
        mouse(&mut app, DRAG, 90, 5);
        assert_eq!(app.list_width, 50);
        mouse(&mut app, DRAG, 3, 5);
        assert_eq!(app.list_width, 24);
        mouse(&mut app, DRAG, 42, 5);
        mouse(&mut app, UP, 42, 5);
        assert_eq!(std::fs::read_to_string(&file).unwrap().trim(), "42");
        assert!(app.selection.is_none(), "the drag selected nothing");
        // The next diff opens at it, diff_list_wide or not.
        assert_eq!(app_keeping(&file).list_width, 42);
        let (tx, _rx) = channel();
        let wide = Options {
            list_wide: true,
            ..opts()
        };
        let mut next = App::new(wide, 100, 16, tx);
        next.keep_width_in(file.clone());
        next.loaded(Change {
            top: PathBuf::from("/repo"),
            base: "origin/main".into(),
            why: git::Why::default(),
            left: "L".into(),
            right: "R".into(),
            entries: vec![entry("src/a.js", 'M', 1, 1)],
            diff: String::new(),
            staged: None,
            raw_commits: String::new(),
            uncommitted: None,
        });
        assert_eq!(next.list_width, 42);
        remove_width_file(&file);
    }

    #[test]
    fn the_pane_resize_keys_set_the_list_s_width_while_the_list_has_the_keys() {
        let file = width_file("keys");
        let mut app = app_keeping(&file);
        assert_eq!(app.focus, Focus::List);
        assert_eq!(
            app.key(KeyEvent::new(KeyCode::Char('l'), RESIZE)),
            Act::Redraw
        );
        assert_eq!(app.list_width, 35);
        assert_eq!(std::fs::read_to_string(&file).unwrap().trim(), "35");
        // The chord without its Shift is no resize; with the letter in
        // its upper case it is the same key.
        let unshifted = KeyModifiers::CONTROL | KeyModifiers::ALT;
        app.key(KeyEvent::new(KeyCode::Char('h'), unshifted));
        assert_eq!(app.list_width, 35);
        app.key(KeyEvent::new(KeyCode::Char('H'), RESIZE));
        assert_eq!(app.list_width, 30);
        app.key(KeyEvent::new(KeyCode::Char('h'), RESIZE));
        app.key(KeyEvent::new(KeyCode::Char('h'), RESIZE));
        assert_eq!(app.list_width, 24, "no narrower than 24 columns");
        assert_eq!(std::fs::read_to_string(&file).unwrap().trim(), "24");
        // In a side of the diff the keys are the workspace's.
        app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(
            app.key(KeyEvent::new(KeyCode::Char('l'), RESIZE)),
            Act::Same
        );
        assert_eq!(app.list_width, 24);
        // So they are with the menu open, which keeps the keys it had.
        app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        press(&mut app, "m");
        assert!(app.menu.is_some());
        app.key(KeyEvent::new(KeyCode::Char('l'), RESIZE));
        assert_eq!(app.list_width, 24);
        assert!(app.menu.is_some());
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        // The keys the workspace names are the ones taken.
        app.resize_keys = vec![ResizeKey::parse("M-Right", "3").unwrap()];
        app.opts.pane_arrows = false;
        app.key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
        assert_eq!(app.list_width, 27);
        app.key(KeyEvent::new(KeyCode::Char('l'), RESIZE));
        assert_eq!(app.list_width, 27);
        assert_eq!(ResizeKey::parse("C-M-S-h", "0"), None);
        assert_eq!(ResizeKey::parse("not a key", "5"), None);
        // A key that keeps its own meaning in the diff is no resize key.
        assert_eq!(ResizeKey::parse("Escape", "5"), None);
        assert_eq!(ResizeKey::parse("Enter", "-5"), None);
        remove_width_file(&file);
    }

    #[test]
    fn e_goes_between_the_width_set_and_the_longest_path() {
        let file = width_file("widen");
        let mut app = app_keeping(&file);
        // A path of 26 columns, which with its status and counts (14 more)
        // fits in 40.
        if let Some(change) = app.change.as_mut() {
            change.entries[0].path = "a/longer/path/to/a/file.rs".into();
        }
        mouse(&mut app, DOWN, 30, 3);
        mouse(&mut app, DRAG, 26, 3);
        mouse(&mut app, UP, 26, 3);
        assert_eq!(app.list_width, 26);
        press(&mut app, "E");
        assert_eq!(app.list_width, 40);
        // A screen of another width keeps the width E gave it.
        app.resize(90, 16);
        assert_eq!(app.list_width, 40);
        press(&mut app, "E");
        assert_eq!(app.list_width, 26);
        // One at the width set takes it as a narrow screen lets.
        app.resize(50, 16);
        assert_eq!(app.list_width, 25);
        app.resize(100, 16);
        assert_eq!(app.list_width, 26);
        remove_width_file(&file);
    }

    #[test]
    fn use_the_file_list_s_default_width_in_the_menu_forgets_the_width_kept() {
        let file = width_file("default");
        let mut app = app_keeping(&file);
        // Its last option, past the menu's first screen: up from the first.
        let offered = |app: &mut App| {
            app.key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
            screen(app)
                .iter()
                .any(|l| l.contains("Use the file list's default width"))
        };
        // With no width of its own the menu doesn't offer it.
        press(&mut app, "m");
        assert!(!offered(&mut app));
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Char('l'), RESIZE));
        assert_eq!(app.list_width, 35);
        // A right click on the list opens the menu, which offers it last.
        let right = MouseEventKind::Down(MouseButton::Right);
        assert_eq!(mouse(&mut app, right, 4, 3), Act::Redraw);
        assert!(offered(&mut app));
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.menu.is_none());
        assert_eq!(app.list_width, LIST_WIDTH);
        assert_eq!(app.set_width, None);
        assert!(!file.exists(), "no file is the default");
        assert_eq!(app_keeping(&file).list_width, LIST_WIDTH);
        remove_width_file(&file);
    }

    #[test]
    fn with_diff_list_resize_off_the_list_keeps_its_fixed_width() {
        let file = width_file("off");
        std::fs::write(&file, "42\n").unwrap();
        let mut app = app();
        app.opts.resize = false;
        app.keep_width_in(file.clone());
        assert_eq!(app.list_width, LIST_WIDTH, "the width kept is passed over");
        // Its edge takes no drag and doesn't light.
        assert_eq!(mouse(&mut app, MouseEventKind::Moved, 30, 3), Act::Same);
        mouse(&mut app, DOWN, 30, 3);
        mouse(&mut app, DRAG, 42, 3);
        mouse(&mut app, UP, 42, 3);
        assert_eq!(app.list_width, LIST_WIDTH);
        // The resize keys aren't the list's there.
        assert_eq!(
            app.key(KeyEvent::new(KeyCode::Char('l'), RESIZE)),
            Act::Same
        );
        assert_eq!(app.list_width, LIST_WIDTH);
        press(&mut app, "m");
        assert!(
            !screen(&mut app)
                .iter()
                .any(|l| l.contains("Use the file list's default width"))
        );
        assert_eq!(
            std::fs::read_to_string(&file).unwrap().trim(),
            "42",
            "kept for when the setting is on again"
        );
        remove_width_file(&file);
    }

    /// The diff with one file whose first lines are long enough to wrap,
    /// and a short last line.
    fn app_wrapping() -> App {
        let (tx, _rx) = channel();
        let mut app = App::new(opts(), 100, 16, tx);
        let long = |w: &str| format!("{} {w}", "word ".repeat(40).trim_end());
        let old: String = (0..6)
            .map(|i| format!("{}\n", long(&format!("old{i}"))))
            .collect();
        let new: String = (0..6)
            .map(|i| format!("{}\n", long(&format!("new{i}"))))
            .collect();
        let old = format!("{old}last\n");
        let new = format!("{new}last\n");
        let mut diff =
            "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1,7 +1,7 @@\n".to_string();
        for i in 0..6 {
            diff.push_str(&format!("-{}\n", long(&format!("old{i}"))));
        }
        for i in 0..6 {
            diff.push_str(&format!("+{}\n", long(&format!("new{i}"))));
        }
        diff.push_str(" last\n");
        app.give_text("L", "f.txt", Some(old));
        app.give_text("R", "f.txt", Some(new));
        app.loaded(Change {
            top: PathBuf::from("/repo"),
            base: "origin/main".into(),
            why: git::Why::default(),
            left: "L".into(),
            right: "R".into(),
            entries: vec![entry("f.txt", 'M', 6, 6)],
            diff,
            staged: None,
            raw_commits: String::new(),
            uncommitted: None,
        });
        app
    }

    /// The row of the diff the cursor's screen line is part of.
    fn cursor_row(app: &mut App) -> Option<usize> {
        let cur = app.cur;
        match app.screen_lines().get(cur) {
            Some(Line::Pair { row, .. }) => Some(*row),
            _ => None,
        }
    }

    #[test]
    fn the_cursor_stays_on_its_row_as_every_change_of_width_wraps_the_lines_anew() {
        let file = width_file("anchor");
        let mut app = app_wrapping();
        app.keep_width_in(file.clone());
        app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        // The cursor on the last line of the file, past the wrapped ones.
        press(&mut app, "G");
        let row = cursor_row(&mut app);
        assert!(row.is_some());
        let before = app.screen_lines().len();
        // A wider list: the sides narrower, the long lines on more rows.
        while app.focus != Focus::List {
            app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        }
        for _ in 0..4 {
            app.key(KeyEvent::new(KeyCode::Char('l'), RESIZE));
        }
        assert_eq!(app.list_width, 50);
        assert!(app.screen_lines().len() > before, "the lines wrapped anew");
        assert_eq!(cursor_row(&mut app), row, "a key");
        // A drag back, E, and the pane resized each keep it there too.
        mouse(&mut app, DOWN, 50, 3);
        mouse(&mut app, DRAG, 26, 3);
        mouse(&mut app, UP, 26, 3);
        assert_eq!(cursor_row(&mut app), row, "a drag");
        press(&mut app, "E");
        assert_eq!(cursor_row(&mut app), row, "E");
        app.resize(70, 16);
        assert_eq!(cursor_row(&mut app), row, "the pane resized");
        // In sight in the view.
        assert!(app.cur >= app.top && app.cur < app.top + app.body_height());
        remove_width_file(&file);
    }

    #[test]
    fn a_selection_by_v_keeps_its_rows_through_a_new_width() {
        let file = width_file("visual");
        let mut app = app_wrapping();
        app.keep_width_in(file.clone());
        app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        press(&mut app, "G");
        press(&mut app, "V");
        let start = app.visual.unwrap();
        let start_row = match app.screen_lines().get(start) {
            Some(Line::Pair { row, .. }) => *row,
            _ => panic!("v started on a row"),
        };
        app.resize(70, 16);
        let now = app.visual.unwrap();
        assert!(
            matches!(app.screen_lines().get(now), Some(Line::Pair { row, .. }) if *row == start_row)
        );
        remove_width_file(&file);
    }
}
