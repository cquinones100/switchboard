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
//! `ga` asks Haiku a question about the change (ask.rs): a box to type it
//! in, then a window on the right half with the answer streamed in.
//!
//! `yp` copies the cursor's `path:line` and `yl` a GitHub link to it; `v`
//! or `V` in a side of the diff selects lines, as Visual mode did in
//! diffview's buffers, and then `yp` and `yl` copy the lines selected
//! (`path:12-20`, `#L12-L20`).
//!
//! The screen is drawn into a cell buffer and only what changed is sent,
//! through the multiplexer's own renderer (`mux::compose::Screen`), so the
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
use ratatui::style::{Color, Modifier, Style};

use super::ask::{self, Asking, Said};
use super::git::{self, Change, Entry};
use super::highlight::{self, Hl};
use super::order;
use super::sides::{self, Item};
use super::story::{self, Look as StoryLook, Progress, Reading, Story};
use super::tell::{self, Asked, Heard, Teller};
use crate::mux::compose::{Frame, Screen};
use crate::ui::draw::width;
use crate::{settings, util};

mod draw;

/// The list's narrow width, and how wide a list must be to head a chapter
/// with its whole why (narrower, its first sentence, the rest in the bar).
const LIST_WIDTH: u16 = 30;
const SHORT_WHY: u16 = 48;
/// Lines J and K scroll the diff from the list.
const SCROLL: usize = 5;
/// How long the cover stays blank, and how long a message stays.
const COVER_BLANK: Duration = Duration::from_millis(300);
const MESSAGE_FOR: Duration = Duration::from_secs(5);
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// git's own colours, mixed over the terminal's background, as GitHub
/// shows a diff.
const ADDED: u32 = 0x2ea043;
const REMOVED: u32 = 0xf85149;
const ACCENT: Color = Color::Indexed(81);

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
    /// ga asks Haiku about the diff (diff_ask).
    pub ask: bool,
    /// A row of buttons for the list's keys (easy_keys), with the keys
    /// named after them (key_help_line).
    pub easy: bool,
    pub key_help: bool,
    /// The button under the mouse lit (hover_highlight).
    pub hover: bool,
    /// The light theme's colours (theme).
    pub light: bool,
    /// Whether Haiku may be asked at all; off in the tests.
    pub haiku: bool,
}

impl Options {
    pub fn read() -> Options {
        let all = settings::read_settings();
        let on = |key: &str| {
            all.iter()
                .find(|s| s.key == key)
                .is_none_or(|s| s.value != serde_json::Value::Bool(false))
        };
        let order = on("diff_order");
        let story = order && on("diff_story");
        Options {
            order,
            story,
            pages: story && on("diff_story_pages"),
            progress: story && on("diff_story_progress"),
            notes: story && on("diff_story_notes"),
            list_wide: on("diff_list_wide"),
            ask: on("diff_ask"),
            easy: on("easy_keys"),
            key_help: on("key_help_line"),
            hover: on("hover_highlight"),
            light: all.iter().any(|s| s.key == "theme" && s.value == "light"),
            haiku: true,
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
    Heard(Heard),
    /// Haiku's order of the code, by path.
    Order(Vec<String>),
    /// A side of a file coloured.
    Coloured {
        path: String,
        new_side: bool,
        lines: Vec<Vec<Hl>>,
    },
    /// What a question's answer says, by the question's number.
    Asked(u64, Said),
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
}

/// One row of the list.
#[derive(Clone, Debug, PartialEq)]
enum ListRow {
    Header,
    Loading,
    Heading(String, StoryLook),
    File(usize),
}

/// The story's state.
#[derive(Clone, Debug, PartialEq)]
enum Telling {
    Off,
    Waiting,
    Told,
    Untold(String),
}

/// The box a question is typed in.
struct AskBox {
    text: String,
    at: ask::Cursor,
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
}

/// A button of easy mode's row, or of a box's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Button {
    Bar(usize),
    AskSend,
    AskCancel,
    AnswerClose,
    AnswerAgain,
    AnswerCopy,
}

/// easy mode's buttons: each presses its keys in the list.
const BUTTONS: [(&str, &str); 8] = [
    ("◀ Chapter", "[c"),
    ("Chapter ▶", "]c"),
    ("◀ Page", "[p"),
    ("Page ▶", "]p"),
    ("Widen", "E"),
    ("Copy link", "yl"),
    ("Ask", "ga"),
    ("Close", "q"),
];

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
    change: Option<Change>,
    /// The diff as the story reads it: its hunks and what Haiku is given.
    hunks: story::Hunks,
    input: String,
    reading: Reading,
    /// Indexes into the change's files, in the order shown (a page's alone).
    order: Vec<usize>,
    /// The list's cursor (an index into `order`), the file it shows.
    at: usize,
    /// The list's first row on the screen, and its width.
    list_top: usize,
    list_width: u16,
    focus: Focus,
    /// The user has moved through the list, so a story arriving keeps the
    /// file shown.
    moved: bool,
    /// Each file's two sides once read, and each text read, by commit and path.
    files: HashMap<String, Sides>,
    texts: HashMap<(String, String), Option<String>>,
    /// The shown file's screen lines, for the sides' widths they were laid
    /// out for.
    lines: Option<(String, u16, u16, Vec<Line>)>,
    /// The diff's cursor (a screen line and a column) and its first line
    /// on the screen.
    cur: usize,
    col: usize,
    top: usize,
    /// Haiku's order of the code by its paths, when no story is told.
    smart: HashMap<String, usize>,
    story: Option<Story>,
    telling: Telling,
    /// The answer so far while the story is told, and when the cover went
    /// up (none once it is down); `raw` shows the answer itself on it.
    progress: Option<Progress>,
    cover: Option<Instant>,
    raw: bool,
    /// The chapter shown as a page, and the file the whole diff showed.
    page: Option<usize>,
    whole_at: Option<String>,
    ask_box: Option<AskBox>,
    answer: Option<Answer>,
    asks: u64,
    /// A message in a box over the screen, and when it was said.
    message: Option<(String, Instant)>,
    /// The first key of a pair (y, g, ], [, z, C-w as w), and a count typed.
    pending: Option<char>,
    /// Where a selection of lines started (`v`, `V`), as a screen line of
    /// the diff, on the side with the focus.
    visual: Option<usize>,
    count: Option<usize>,
    spin: usize,
    hover: Option<Button>,
    /// Where each button was last drawn.
    buttons: Vec<(Button, Rect)>,
    /// Where the boxes were last drawn, for clicks outside them.
    ask_rect: Option<Rect>,
    answer_rect: Option<Rect>,
    /// Escapes to write after the frame: the clipboard's.
    pub out: Vec<u8>,
    teller: Teller,
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
            change: None,
            hunks: Default::default(),
            input: String::new(),
            reading: Reading::default(),
            order: Vec::new(),
            at: 0,
            list_top: 0,
            list_width: LIST_WIDTH,
            focus: Focus::List,
            moved: false,
            files: HashMap::new(),
            texts: HashMap::new(),
            lines: None,
            cur: 0,
            col: 0,
            top: 0,
            smart: HashMap::new(),
            story: None,
            telling: Telling::Off,
            progress: None,
            cover: None,
            raw: false,
            page: None,
            whole_at: None,
            ask_box: None,
            answer: None,
            asks: 0,
            message: None,
            pending: None,
            visual: None,
            count: None,
            spin: 0,
            hover: None,
            buttons: Vec::new(),
            ask_rect: None,
            answer_rect: None,
            out: Vec::new(),
            teller: Teller::default(),
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
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols.max(20);
        self.rows = rows.max(5);
        self.lines = None;
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
        self.ticking() || changed
    }

    fn spinner(&self) -> &'static str {
        SPINNER[self.spin % SPINNER.len()]
    }

    // ── The change ──────────────────────────────────────────────────────────

    pub fn event(&mut self, msg: Msg) {
        match msg {
            Msg::Loading(text) => self.loading = Some(text),
            Msg::Loaded(Err(why)) => {
                self.loading = None;
                self.failed = Some(why);
            }
            Msg::Loaded(Ok((change, said))) => {
                self.loading = None;
                self.loaded(*change);
                if let Some(said) = said {
                    self.say(said);
                }
            }
            Msg::Heard(Heard::Progress(p)) => self.progress = Some(p),
            Msg::Heard(Heard::Told(told)) => self.told(*told),
            Msg::Heard(Heard::Untold(why)) => {
                self.telling = Telling::Untold(why);
                self.cover = None;
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
        }
    }

    /// The change is read: its files listed, the first one someone wrote
    /// shown, and its story asked for.
    pub fn loaded(&mut self, change: Change) {
        self.loading = None;
        let texts: Vec<&str> = vec![change.diff.as_str()];
        let (diffs, hunks) = story::split(&texts);
        let entries: Vec<story::Entry> = change
            .entries
            .iter()
            .map(|e| story::Entry {
                path: e.path.clone(),
                additions: e.additions,
                deletions: e.deletions,
            })
            .collect();
        let commits = story::commits(&change.raw_commits);
        self.input = story::input(&entries, &diffs, &hunks, &commits);
        self.reading = Reading {
            files: entries.len(),
            additions: entries.iter().map(|e| e.additions).sum(),
            deletions: entries.iter().map(|e| e.deletions).sum(),
            estimate: Some(story::estimate(story::count(&hunks))),
        };
        self.hunks = hunks;
        let mut paths: Vec<String> = change.entries.iter().map(|e| e.path.clone()).collect();
        paths.sort();
        let key = story::key(
            &change.top.to_string_lossy(),
            &paths,
            &change.diff,
            &commits,
        );
        self.change = Some(change);
        self.order = (0..self.entries().len()).collect();
        self.sort();
        if self.opts.list_wide {
            self.list_width = self.fit_width();
        }
        self.pick_first();
        self.ask_story(key, paths);
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

    /// The files in the order known now, the cursor kept on its file.
    fn sort(&mut self) {
        let keep = self.current().map(|e| e.path.clone());
        let entries = self.entries().to_vec();
        let story = self.story.as_ref();
        let smart = &self.smart;
        let page: Option<HashSet<&str>> = self.page.and_then(|p| {
            story
                .and_then(|s| s.chapters.get(p - 1))
                .map(|c| c.touches.iter().map(String::as_str).collect())
        });
        let mut order: Vec<usize> = (0..entries.len())
            .filter(|&i| {
                page.as_ref()
                    .is_none_or(|p| p.contains(entries[i].path.as_str()))
            })
            .collect();
        let by_order = self.opts.order;
        order.sort_by(|&a, &b| {
            let (pa, pb) = (&entries[a].path, &entries[b].path);
            if by_order {
                order::reading_order(pa, pb, story, smart)
            } else {
                order::alphabetical(pa, pb)
            }
        });
        self.order = order;
        if let Some(keep) = keep
            && let Some(at) = self.order.iter().position(|&i| entries[i].path == keep)
        {
            self.at = at;
        }
        self.at = self.at.min(self.order.len().saturating_sub(1));
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

    /// The story asked for, under the cover, unless there is none to tell:
    /// two files of someone's at least, since generated files go last
    /// whatever is said.
    fn ask_story(&mut self, key: String, paths: Vec<String>) {
        if !self.opts.story {
            self.ask_order();
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
        if !self.opts.haiku {
            self.telling = Telling::Untold("Haiku isn't asked here".into());
            return;
        }
        if tell::claude_command().is_none() {
            self.telling = Telling::Untold("claude isn't installed".into());
            return;
        }
        self.telling = Telling::Waiting;
        self.cover = Some(Instant::now());
        let asked = Asked {
            key,
            input: self.input.clone(),
            paths,
            hunks: self.hunks.clone(),
        };
        let teller = self.teller.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            teller.tell(&asked, &|heard| {
                let _ = tx.send(Msg::Heard(heard));
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
        let files: Vec<order::Ordered> = change
            .entries
            .iter()
            .map(|e| order::Ordered {
                path: e.path.clone(),
                additions: e.additions,
                deletions: e.deletions,
            })
            .collect();
        let files = order::to_order(&files);
        if files.is_empty() {
            return;
        }
        let (top, left, right) = (
            change.top.clone(),
            change.left.clone(),
            change.right.clone(),
        );
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            if let Some(order) = order::ask_order(&top, &left, &right, &files) {
                let _ = tx.send(Msg::Order(order));
            }
        });
    }

    /// The story is told: the list in its order, headed by its chapters.
    pub fn told(&mut self, story: Story) {
        if let Some(missed) = story::missed_text(&story) {
            self.say(format!("{missed}."));
        }
        self.story = Some(story);
        self.telling = Telling::Told;
        self.cover = None;
        self.sort();
        if !self.moved {
            self.pick_first();
        }
        self.lines = None;
    }

    // ── The file shown ──────────────────────────────────────────────────────

    /// Shows the list's file at `at`, opened at its first change.
    fn show_at(&mut self, at: usize) {
        self.visual = None;
        if self.order.is_empty() {
            return;
        }
        self.at = at.min(self.order.len() - 1);
        self.load_shown();
        self.lines = None;
        self.first_change();
    }

    /// Reads the shown file's two sides, once, and asks for their colours.
    fn load_shown(&mut self) {
        let Some(entry) = self.current().cloned() else {
            return;
        };
        if self.files.contains_key(&entry.path) {
            return;
        }
        let Some(change) = &self.change else {
            return;
        };
        let (left, right) = (change.left.clone(), change.right.clone());
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
        let light = self.opts.light;
        for (new_side, lines) in [(false, old.clone()), (true, new.clone())] {
            if lines.is_empty() {
                continue;
            }
            let tx = self.tx.clone();
            let path = entry.path.clone();
            let named = if new_side {
                entry.path.clone()
            } else {
                entry.old().to_string()
            };
            std::thread::spawn(move || {
                if let Some(lines) = highlight::highlight(&named, &lines, light) {
                    let _ = tx.send(Msg::Coloured {
                        path,
                        new_side,
                        lines,
                    });
                }
            });
        }
        self.files.insert(
            entry.path.clone(),
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
        let bottom = if self.opts.easy { 1 } else { 0 };
        Rect::new(
            x,
            0,
            self.cols.saturating_sub(x),
            self.rows.saturating_sub(bottom),
        )
    }

    /// The columns of the old side and the new; one of them 0 for a file
    /// shown alone.
    fn side_widths(&self) -> (u16, u16) {
        let w = self.diff_area().width;
        match self
            .current()
            .and_then(|e| self.files.get(&e.path))
            .and_then(|s| s.alone)
        {
            Some(true) => (0, w),
            Some(false) => (w, 0),
            None => {
                let old = w.saturating_sub(1) / 2;
                (old, w.saturating_sub(old + 1))
            }
        }
    }

    /// The rows the diff's text takes, below each side's bar.
    fn body_height(&self) -> usize {
        self.diff_area().height.saturating_sub(1) as usize
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
        let Some(entry) = self.current().cloned() else {
            return &[];
        };
        let (ow, nw) = self.side_widths();
        let fresh = self
            .lines
            .as_ref()
            .is_some_and(|(p, o, n, _)| *p == entry.path && *o == ow && *n == nw);
        if !fresh {
            let lines = self.build_lines(&entry, ow as usize, nw as usize);
            self.lines = Some((entry.path.clone(), ow, nw, lines));
        }
        &self.lines.as_ref().expect("built above").3
    }

    fn build_lines(&self, entry: &Entry, ow: usize, nw: usize) -> Vec<Line> {
        let Some(s) = self.files.get(&entry.path) else {
            return Vec::new();
        };
        // Each note goes over the line its hunk starts at, on the side it
        // changes: the new, or the old for lines only taken out.
        let mut notes: HashMap<(bool, usize), Vec<String>> = HashMap::new();
        if self.opts.notes
            && let Some(story) = &self.story
            && let Some(home) = self.home(&entry.path)
        {
            for hunk in story
                .hunks
                .get(&entry.path)
                .map(Vec::as_slice)
                .unwrap_or(&[])
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
                let line = (if count == 0 { first + 1 } else { first }).max(1);
                let cols = if new_side { nw } else { ow }.max(10);
                if let Some(text) = story::note_lines(story, hunk, home, cols) {
                    notes.entry((new_side, line - 1)).or_default().extend(text);
                }
            }
        }
        let mut out = Vec::new();
        let put_notes = |out: &mut Vec<Line>,
                         key: (bool, usize),
                         notes: &mut HashMap<(bool, usize), Vec<String>>| {
            if let Some(texts) = notes.remove(&key) {
                for text in texts {
                    out.push(Line::Note {
                        new_side: key.0,
                        text,
                    });
                }
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
                        .map(|o| sides::wrap(&s.old[o], ow.max(1)))
                        .unwrap_or_default();
                    let new_pieces = row
                        .new
                        .map(|n| sides::wrap(&s.new[n], nw.max(1)))
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
                }
            }
        }
        out
    }

    /// Opens the shown file at its first change, the cursor on the first
    /// word that changed, in the middle of the screen.
    fn first_change(&mut self) {
        let height = self.body_height();
        let alone = self
            .current()
            .and_then(|e| self.files.get(&e.path))
            .and_then(|s| s.alone);
        let rows_changed: Vec<bool> = self
            .current()
            .and_then(|e| self.files.get(&e.path))
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
                && let Some(s) = self.files.get(&entry.path)
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

    pub fn key(&mut self, key: KeyEvent) -> Act {
        if key.kind == KeyEventKind::Release {
            return Act::Same;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') && self.ask_box.is_none() {
            return Act::Quit;
        }
        // C-\ C-n, which the palette's commands press first: back to the
        // diff's own keys from wherever typing is.
        if ctrl && matches!(key.code, KeyCode::Char('\\') | KeyCode::Char('4')) {
            self.ask_box = None;
            self.pending = None;
            self.message = None;
            return Act::Redraw;
        }
        if ctrl && key.code == KeyCode::Char('n') && self.ask_box.is_none() {
            return Act::Same;
        }
        if self.message.take().is_some() {
            return Act::Redraw;
        }
        if self.failed.is_some() {
            return Act::Quit;
        }
        if self.loading.is_some() {
            return match key.code {
                KeyCode::Char('q') | KeyCode::Esc => Act::Quit,
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
        let before = self.focus;
        let act = self.normal_key(key);
        if self.focus != before {
            self.visual = None;
        }
        act
    }

    fn normal_key(&mut self, key: KeyEvent) -> Act {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
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
        let list = self.focus == Focus::List;
        let page = self.body_height().max(1);
        match key.code {
            KeyCode::Char('q') => return Act::Quit,
            KeyCode::Char(c @ ('y' | 'g' | ']' | '[' | 'z')) if !ctrl => {
                self.pending = Some(c);
                self.count = Some(count).filter(|n| *n > 1);
                return Act::Same;
            }
            KeyCode::Char('w') if ctrl => {
                self.pending = Some('w');
                return Act::Same;
            }
            KeyCode::Char('E') => self.widen(),
            KeyCode::Tab => {
                self.focus = if list { self.main_side() } else { Focus::List };
            }
            KeyCode::BackTab => {
                self.focus = if list { self.main_side() } else { Focus::List };
            }
            KeyCode::Esc if list => return Act::Quit,
            KeyCode::Esc if self.visual.is_some() => self.visual = None,
            KeyCode::Esc => self.focus = Focus::List,
            KeyCode::Char('v' | 'V') if !list && !ctrl => {
                self.visual = match self.visual {
                    Some(_) => None,
                    None => Some(self.cur),
                };
            }
            KeyCode::Char('J') if list => self.scroll_diff(SCROLL * count, true),
            KeyCode::Char('K') if list => self.scroll_diff(SCROLL * count, false),
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
            KeyCode::Enter => self.open_fold(),
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
        let KeyCode::Char(c) = key.code else {
            return Act::Redraw;
        };
        let count = self.count.take().unwrap_or(1);
        match (first, c) {
            ('y', 'p') => self.copy_path(),
            ('y', 'l') => self.copy_link(),
            ('y', 'S') => self.copy_story(),
            ('g', 'g') if self.focus == Focus::List => self.move_list(isize::MIN / 2),
            ('g', 'g') => self.move_cursor(isize::MIN / 2),
            ('g', 'a') if self.opts.ask => self.open_ask(),
            (']', 'c') if self.focus == Focus::List => self.chapter_jump(1),
            ('[', 'c') if self.focus == Focus::List => self.chapter_jump(-1),
            (']', 'c') => self.change_jump(count, true),
            ('[', 'c') => self.change_jump(count, false),
            (']', 'p') => self.turn_page(1),
            ('[', 'p') => self.turn_page(-1),
            ('z', 'o' | 'a' | 'v') => self.open_fold(),
            ('z', 'R') => self.all_folds(true),
            ('z', 'M') => self.all_folds(false),
            ('w', 'h') => {
                self.focus = match self.focus {
                    Focus::New if self.side_widths().0 > 0 => Focus::Old,
                    _ => Focus::List,
                }
            }
            ('w', 'l') => {
                self.focus = match self.focus {
                    Focus::List => self.first_side(),
                    _ if self.side_widths().1 > 0 => Focus::New,
                    other => other,
                }
            }
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
        let to = (self.at as isize + by).clamp(0, self.order.len() as isize - 1) as usize;
        if to != self.at {
            self.moved = true;
            self.show_at(to);
        }
    }

    fn move_cursor(&mut self, by: isize) {
        let n = self.screen_lines().len();
        if n == 0 {
            return;
        }
        self.cur = (self.cur as isize + by).clamp(0, n as isize - 1) as usize;
        self.col = 0;
        self.moved = true;
        self.keep_cursor();
    }

    /// The view scrolled so the cursor is in it.
    fn keep_cursor(&mut self) {
        let h = self.body_height().max(1);
        if self.cur < self.top {
            self.top = self.cur;
        } else if self.cur >= self.top + h {
            self.top = self.cur + 1 - h;
        }
    }

    /// The diff scrolled `by` lines, the cursor kept in view.
    fn scroll_diff(&mut self, by: usize, down: bool) {
        let n = self.screen_lines().len();
        let h = self.body_height().max(1);
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
    fn change_jump(&mut self, count: usize, forward: bool) {
        let lines = self.screen_lines().to_vec();
        let changed: Vec<bool> = {
            let rows = self
                .current()
                .and_then(|e| self.files.get(&e.path))
                .map(|s| &s.rows);
            lines
                .iter()
                .map(|l| match (l, rows) {
                    (Line::Pair { row, .. }, Some(rows)) => rows[*row].changes(),
                    _ => false,
                })
                .collect()
        };
        let starts: Vec<usize> = (0..lines.len())
            .filter(|&i| changed[i] && (i == 0 || !changed[i - 1]))
            .collect();
        for _ in 0..count {
            let next = if forward {
                starts.iter().find(|&&i| i > self.cur).copied()
            } else {
                starts.iter().rev().find(|&&i| i < self.cur).copied()
            };
            match next {
                Some(i) => self.cur = i,
                None => break,
            }
        }
        self.col = 0;
        self.keep_cursor();
    }

    fn open_fold(&mut self) {
        let cur = self.cur;
        let Some(Line::Fold { first, .. }) = self.screen_lines().get(cur).cloned() else {
            return;
        };
        if let Some(path) = self.current().map(|e| e.path.clone())
            && let Some(s) = self.files.get_mut(&path)
        {
            s.opened.insert(first);
        }
        self.lines = None;
    }

    fn all_folds(&mut self, open: bool) {
        if let Some(path) = self.current().map(|e| e.path.clone())
            && let Some(s) = self.files.get_mut(&path)
        {
            s.all_open = open;
            if !open {
                s.opened.clear();
            }
        }
        self.lines = None;
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
        self.lines = None;
    }

    // ── The story's chapters and pages ──────────────────────────────────────

    /// Why there is no story to move through, said.
    fn no_story(&mut self) {
        let text = if !self.opts.story {
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
        let mut rows = vec![ListRow::Header];
        if self.telling == Telling::Waiting {
            rows.push(ListRow::Loading);
        }
        let cols = self.list_width.saturating_sub(2).max(8) as usize;
        let short = self.list_width < SHORT_WHY;
        let starts = self.starts();
        for (k, &i) in self.order.iter().enumerate() {
            if let (Some(story), Some(path)) = (&self.story, self.entry(i).map(|e| &e.path))
                && starts[k]
                && let Some(&n) = story.chapter.get(path)
            {
                for (text, look) in story::heading(story, n, cols, short) {
                    rows.push(ListRow::Heading(text, look));
                }
            }
            rows.push(ListRow::File(k));
        }
        rows
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
                section: "changes".into(),
            })
            .collect();
        story::starts(&rows)
    }

    /// ]c and [c in the list: the next or the previous heading's file.
    fn chapter_jump(&mut self, step: i32) {
        if self.story.is_none() {
            return self.no_story();
        }
        let starts = self.starts();
        if let Some(to) = story::jump(&starts, self.at + 1, step) {
            self.moved = true;
            self.show_at(to - 1);
        }
    }

    /// ]p and [p: the story a chapter at a time, as if each were a commit
    /// of its own, [p from the first going back to the whole diff.
    fn turn_page(&mut self, step: i32) {
        if !self.opts.pages {
            return;
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
                self.sort();
                let back = self.whole_at.take();
                let at = back
                    .and_then(|p| self.order.iter().position(|&i| self.entries()[i].path == p))
                    .unwrap_or(0);
                self.show_at(at);
            }
            return;
        }
        if self.page.is_none() {
            self.whole_at = self.current().map(|e| e.path.clone());
        }
        self.page = Some(index as usize);
        self.at = 0;
        self.sort();
        self.at = 0;
        // The first of the page's files someone wrote, else its first.
        let first = self
            .order
            .iter()
            .position(|&i| !order::generated(&self.entries()[i].path))
            .unwrap_or(0);
        self.show_at(first);
    }

    // ── Copying ─────────────────────────────────────────────────────────────

    /// Puts text on the clipboard: through the terminal (OSC 52), which the
    /// multiplexer passes on, and the desktop's own outside it.
    fn clip(&mut self, text: &str) {
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
        let Some(change) = &self.change else {
            return;
        };
        let Some(repo) = git::github_repo(&change.top) else {
            return self.say("origin isn't on GitHub");
        };
        let commit = if new_side {
            &change.right
        } else {
            &change.left
        };
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
            .and_then(|l| ask::hunk_at(&self.hunks, &entry.path, l, None, side))
            .map(|h| h.id.clone());
        ask::Cursor {
            path: Some(entry.path),
            line,
            old_side: !new_side,
            hunk,
        }
    }

    fn open_ask(&mut self) {
        if self.change.is_none() {
            return;
        }
        let at = self.ask_cursor();
        self.ask_box = Some(AskBox {
            text: String::new(),
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
            KeyCode::Enter if shift => b.text.push('\n'),
            KeyCode::Char('j') if ctrl => b.text.push('\n'),
            KeyCode::Enter => self.send_question(),
            KeyCode::Backspace => {
                b.text.pop();
            }
            KeyCode::Char('u') if ctrl => b.text.clear(),
            KeyCode::Char(c) if !ctrl => b.text.push(c),
            _ => return Act::Same,
        }
        Act::Redraw
    }

    fn send_question(&mut self) {
        let Some(b) = self.ask_box.take() else {
            return;
        };
        let question = b.text.trim().to_string();
        if question.is_empty() {
            return;
        }
        let Some(change) = &self.change else {
            return;
        };
        let story = self.story.as_ref();
        let input = ask::input(story, Some(&self.input), &b.at, &question);
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
        });
        if !self.opts.haiku {
            return;
        }
        let top = change.top.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            ask::run(&top, input, &asking, &|said| {
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
            Said::Done(Err(why)) => {
                a.done = true;
                if !a.body.is_empty() {
                    a.body.push_str("\n\n");
                }
                a.body.push_str("**Haiku couldn't answer**");
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
            let KeyCode::Char(c) = key.code else {
                return Act::Redraw;
            };
            match (first, c) {
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
        let Some(a) = &mut self.answer else {
            return Act::Same;
        };
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.close_answer(),
            KeyCode::Char(c @ ('g' | 'y')) => self.pending = Some(c),
            KeyCode::Char('j') | KeyCode::Down => {
                a.cursor = (a.cursor + 1).min(lines.saturating_sub(1))
            }
            KeyCode::Char('k') | KeyCode::Up => a.cursor = a.cursor.saturating_sub(1),
            KeyCode::Char('G') => a.cursor = lines.saturating_sub(1),
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
        let found = self
            .order
            .iter()
            .position(|&i| self.entries()[i].path == path);
        let Some(at) = found else {
            let there = Path::new(&top).join(&path).exists();
            return self.say(if there {
                format!("{path} isn't changed in this diff")
            } else {
                format!("{path} isn't in the repository")
            });
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
        let mut text = a.body.clone();
        if !a.done {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(self.spinner());
            if a.body.is_empty() {
                text.push_str(" reading the diff");
            }
        }
        let doc = crate::views::markdown::Markdown::parse(&text, "").layout(inner);
        doc.lines
            .iter()
            .map(|l| {
                let spans = l
                    .spans
                    .iter()
                    .map(|s| (s.text.clone(), md_style(&s.style)))
                    .collect();
                (l.plain(), spans)
            })
            .collect()
    }

    // ── The mouse ───────────────────────────────────────────────────────────

    pub fn mouse(&mut self, m: MouseEvent) -> Act {
        let (x, y) = (m.column, m.row);
        let hit = self
            .buttons
            .iter()
            .find(|(_, r)| x >= r.x && x < r.right() && y >= r.y && y < r.bottom())
            .map(|(b, _)| *b);
        match m.kind {
            MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                if !self.opts.hover || hit == self.hover {
                    return Act::Same;
                }
                self.hover = hit;
                return Act::Redraw;
            }
            MouseEventKind::Down(MouseButton::Left) => {}
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let down = m.kind == MouseEventKind::ScrollDown;
                if self.answer.is_some() {
                    if let Some(a) = &mut self.answer {
                        a.top = if down {
                            a.top + 3
                        } else {
                            a.top.saturating_sub(3)
                        };
                    }
                } else if x < self.list_width {
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
            _ => return Act::Same,
        }
        if self.message.take().is_some() {
            return Act::Redraw;
        }
        if self.failed.is_some() {
            return Act::Quit;
        }
        if self.loading.is_some() || (self.cover.is_some() && self.telling == Telling::Waiting) {
            return Act::Same;
        }
        if let Some(button) = hit {
            return self.press(button);
        }
        self.visual = None;
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
                if let Some(found) = found {
                    self.cited(found);
                }
            }
            return Act::Redraw;
        }
        if x < self.list_width {
            self.focus = Focus::List;
            let rows = self.list_rows();
            let at = self.list_top + y as usize;
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
        if y > area.y && y < area.bottom() {
            let (ow, _) = self.side_widths();
            self.focus = if ow > 0 && x < area.x + ow {
                Focus::Old
            } else {
                self.main_side()
            };
            let n = self.screen_lines().len();
            self.cur = (self.top + (y - area.y - 1) as usize).min(n.saturating_sub(1));
            let side_x = if self.focus == Focus::Old {
                area.x
            } else {
                area.x + if ow > 0 { ow + 1 } else { 0 }
            };
            self.col = x.saturating_sub(side_x) as usize;
            let cur = self.cur;
            if matches!(self.screen_lines().get(cur), Some(Line::Fold { .. })) {
                self.open_fold();
            }
        }
        Act::Redraw
    }

    /// A button pressed, as its keys would be.
    fn press(&mut self, button: Button) -> Act {
        match button {
            Button::Bar(i) => {
                let keys = BUTTONS
                    .iter()
                    .filter(|(_, k)| self.opts.ask || *k != "ga")
                    .nth(i)
                    .map(|(_, k)| *k)
                    .unwrap_or("");
                self.focus = Focus::List;
                let mut act = Act::Same;
                for c in keys.chars() {
                    act = self.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
                }
                if act == Act::Quit {
                    return Act::Quit;
                }
            }
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

/// Lines as `yp` and `yl` name them: one line alone, else the first and
/// the last, the last after `mark` (GitHub's `#L12-L20`).
fn span(first: usize, last: usize, mark: &str) -> String {
    if last > first {
        format!("{first}-{mark}{last}")
    } else {
        first.to_string()
    }
}

/// A style of the PR view's Markdown, as a cell's.
fn md_style(s: &crate::views::markdown::Style) -> Style {
    let mut style = Style::new();
    if let Some(fg) = s.fg {
        style = style.fg(Color::Indexed(fg));
    }
    if let Some(bg) = s.bg {
        style = style.bg(Color::Indexed(bg));
    }
    for (on, m) in [
        (s.bold, Modifier::BOLD),
        (s.dim, Modifier::DIM),
        (s.italic, Modifier::ITALIC),
        (s.underline, Modifier::UNDERLINED),
        (s.crossed, Modifier::CROSSED_OUT),
        (s.reverse, Modifier::REVERSED),
    ] {
        if on {
            style = style.add_modifier(m);
        }
    }
    style
}

// ── Running it ──────────────────────────────────────────────────────────────

/// Reads the change in `dir` off the screen's thread: the base kept for
/// the branch's commit, else asked for and fetched (said on the screen
/// meanwhile), then the change against it.
fn load(dir: PathBuf, base: Option<String>, tx: Sender<Msg>) {
    let result = (|| -> Result<(Box<Change>, Option<String>), String> {
        let facts = git::facts(&dir).map_err(|e| e.to_string())?;
        let mut said = None;
        let base = match base {
            Some(b) => b,
            None => match git::cached_base(&dir) {
                Some(b) => b,
                None => {
                    let _ = tx.send(Msg::Loading("Fetching the PR base branch...".into()));
                    let Some(found) = git::fetch_base(&dir) else {
                        return Err("Couldn't work out the PR's base branch".into());
                    };
                    if !found.fetched {
                        said = Some(format!(
                            "Could not fetch {}; comparing with it as last fetched.",
                            found.base
                        ));
                    }
                    found.base
                }
            },
        };
        let change = git::read_change(&facts.top, &base).map_err(|e| e.to_string())?;
        Ok((Box::new(change), said))
    })();
    let _ = tx.send(Msg::Loaded(result));
}

/// The terminal as the diff wants it while it runs, and put back however
/// it ends: raw, on the alternate screen, clicks, drags and the wheel
/// reported, and every movement too with the hover on.
struct Term;

impl Term {
    fn enter(motion: bool) -> io::Result<Term> {
        terminal::enable_raw_mode()?;
        let mut out = io::stdout();
        out.write_all(b"\x1b[?1049h\x1b[?25l\x1b[?1000h\x1b[?1002h\x1b[?1006h")?;
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
        let _ = out
            .write_all(b"\x1b[?1003l\x1b[?1006l\x1b[?1002l\x1b[?1000l\x1b[0m\x1b[?25h\x1b[?1049l");
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
/// against `base` (worked out as M-d does when `None`).
pub fn run(dir: PathBuf, base: Option<String>) -> ExitCode {
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let (tx, rx): (Sender<Msg>, Receiver<Msg>) = channel();
    let opts = Options::read();
    let mut app = App::new(opts, cols, rows, tx.clone());
    {
        let tx = tx.clone();
        std::thread::spawn(move || load(dir, base, tx));
    }
    // The pane closed under it (M-w), or the server ending: the loop ends
    // as it does on q, so the ask under way and its partial file go too.
    for signal in [libc::SIGHUP, libc::SIGTERM] {
        // SAFETY: a handler that only stores to an atomic.
        unsafe {
            libc::signal(signal, on_signal as *const () as libc::sighandler_t);
        }
    }
    let term = match Term::enter(opts.hover && (opts.easy || opts.ask)) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("switchboard diff: can't take the terminal: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut screen = Screen::default();
    let mut dirty = true;
    let mut last_tick = Instant::now();
    while !ENDED.load(Ordering::Relaxed) {
        while let Ok(msg) = rx.try_recv() {
            app.event(msg);
            dirty = true;
        }
        if last_tick.elapsed() >= Duration::from_millis(100) {
            last_tick = Instant::now();
            if app.tick() {
                dirty = true;
            }
        }
        if dirty {
            let (cols, rows) = terminal::size().unwrap_or((cols, rows));
            let mut frame = Frame::new(cols, rows);
            frame.cursor = app.draw(&mut frame.buf);
            let mut bytes = screen.render(&frame);
            bytes.append(&mut app.out);
            let mut out = io::stdout().lock();
            let _ = out.write_all(&bytes);
            let _ = out.flush();
            dirty = false;
        }
        let act = match event::poll(Duration::from_millis(50)) {
            Ok(true) => match event::read() {
                Ok(TermEvent::Key(key)) => app.key(key),
                Ok(TermEvent::Mouse(m)) => app.mouse(m),
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
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;

    fn opts() -> Options {
        Options {
            order: true,
            story: true,
            pages: true,
            progress: true,
            notes: true,
            list_wide: false,
            ask: true,
            easy: false,
            key_help: true,
            hover: true,
            light: false,
            haiku: false,
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
            left: "L".into(),
            right: "R".into(),
            entries: vec![
                entry("src/a.js", 'M', 1, 1),
                entry("README.md", 'M', 1, 1),
                entry("src/new.js", 'A', 1, 0),
            ],
            diff,
            raw_commits: String::new(),
        });
        app
    }

    fn draw(app: &mut App) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, app.cols, app.rows));
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

    /// The list's part of each line.
    fn list(shown: &[String]) -> Vec<String> {
        shown.iter().map(|l| l.chars().take(30).collect()).collect()
    }

    #[test]
    fn easy_mode_puts_a_button_for_each_key_at_the_bottom_and_a_click_presses_it() {
        let mut app = app();
        app.opts.easy = true;
        let shown = screen(&mut app);
        let bar = shown.last().unwrap();
        for label in [
            "◀ Chapter",
            "Chapter ▶",
            "◀ Page",
            "Page ▶",
            "Widen",
            "Copy link",
            "Ask",
            "Close",
        ] {
            assert!(bar.contains(label), "{label} in {bar:?}");
        }
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
            .find(|(b, _)| *b == Button::Bar(7))
            .unwrap();
        let click = MouseEvent {
            column: close.x + 1,
            row: close.y,
            ..click
        };
        assert_eq!(app.mouse(click), Act::Quit);
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
            shown[(x, 1)].modifier.contains(Modifier::REVERSED),
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
        let buf = draw(&mut app);
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
}
