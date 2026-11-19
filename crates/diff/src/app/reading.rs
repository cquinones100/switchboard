//! The story read down one column (the diff_story_map setting), as boards
//! 140 to 142 draw it, in place of the map of boards 121 to 125: the
//! diff's Story view, beside the file view it had (Files mode).
//!
//! A story told in chapters (story.rs's `Chapter`s) opens on what changes:
//! Before and After in plain words, then how it gets there, a line of why
//! for each of the first chapter's pages in reading order. Down the left,
//! the outline: the chapters, What changes and each page indented under
//! them, ✓ on each read, the files no chapter holds, and a Read meter.
//! Enter on the outline reads from there, down one column: what changes
//! first, then each page a section with its claim, a block of what it
//! Decided, Instead of what and what to Watch, then its changes inline,
//! a file of tests folded last, which Enter unfolds. `]p` and `[p` move
//! page to page, Tab between the outline and the column, the outline
//! following where the column is; a page is read once the column has been
//! on it. What runs (the overview's line, or the menu) lays the steps
//! that run in the order they happen over the column, a box each naming
//! its page, the page being read lit; `h` and `l` move along it.
//!
//! The column is laid out from the story and the diff's own text alone,
//! reading and starting nothing, again only when its width or what is
//! unfolded changes. Enter on a change opens its menu: shown in the
//! files, or its place copied. `g m` and `g f` switch between the two
//! views, and `m` asks which; the view used last is kept in the state
//! folder, and the pages read beside the story in the cache, by its key.

use std::collections::HashSet;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as Spans, Span};

use super::select::Pane;
use super::{Act, App, Button, Cell, Focus, Line};
use crate::git::Entry;
use crate::story::{self, Story};
use crate::{order, paths, tell, util};
use switchboard_ui::Theme;
use switchboard_ui::draw::{cut, put_line, width, wrap};
use switchboard_ui::kit::{self, Surface, Tone};

/// The two ways the diff shows a story told in chapters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// The file list in reading order with the file beside it, as the
    /// diff always was.
    #[default]
    Files,
    /// What changes, the outline and the column read down.
    Story,
    /// The story's map of what builds on what (map.rs).
    Map,
}

impl Mode {
    fn word(self) -> &'static str {
        match self {
            Mode::Files => "files",
            Mode::Story => "story",
            Mode::Map => "map",
        }
    }
}

/// The options of `m`'s menu that switch views, in its order.
pub(super) const MENU: [(&str, Mode); 3] = [
    ("Story", Mode::Story),
    ("Map", Mode::Map),
    ("Files", Mode::Files),
];

/// Something of the Story view a click lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Hit {
    /// An entry of the outline, by its place among them.
    Item(usize),
    /// A line of the column that does something, by its place.
    Line(usize),
    /// A step of What runs, by its place.
    Run(usize),
    /// A page of the map, by its number: its box.
    Page(usize),
    /// A chapter of the map's strip, by its place.
    Chapter(usize),
    /// The strip's chip of the files no page tells.
    FilesOnly,
}

/// Where Files mode was when the Story view took its place: the page, the
/// file under the list's cursor and the view's file by their keys, the
/// diff's cursor and view, and which pane had the keys.
#[derive(Clone, Debug)]
pub(super) struct Place_ {
    alone: Option<usize>,
    file: Option<String>,
    view: Option<String>,
    cur: usize,
    col: usize,
    top: usize,
    focus: Focus,
    /// The path filter, which is the file list's and never the story's.
    filter: String,
    /// Whether the view was scrolled off the cursor's file.
    adrift: bool,
    /// The file the whole diff showed before the page, which [p from the
    /// first page goes back to.
    whole_at: Option<String>,
    /// The list's first row on the screen, and the row it last followed
    /// the cursor to.
    list_top: usize,
    list_followed: Option<usize>,
    /// The cursor and the view's first line by what they are on, so the
    /// place comes back to the same lines after the list's width changed
    /// in the other view (width.rs).
    anchors: Option<super::width::Anchors>,
    /// The list's place by its rows, for the same reason.
    list_place: super::width::ListPlace,
}

/// An entry of the outline: what changes, a page by its number, or the
/// files no chapter holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Item {
    Overview,
    Page(usize),
    FilesOnly,
}

/// What a line of the column does: nothing, or what Enter and a click
/// on it do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Does {
    Nothing,
    /// A file's header: shown in the files.
    File {
        path: String,
        run: usize,
    },
    /// A line of a change, at its line on the new side or the old.
    Change {
        path: String,
        run: usize,
        new: Option<usize>,
        old: Option<usize>,
    },
    /// A file of tests, folded: unfolded.
    Folded {
        page: usize,
        path: String,
    },
    /// The overview's line that lays What runs over the column.
    Runs,
    /// The overview's last line, which reads on from page 1.
    FirstPage,
}

/// A line of the column: what it shows, the section it is in (0 for what
/// changes, else a page's number), and what it does.
#[derive(Clone, Debug)]
pub(super) struct Row {
    spans: Vec<Span<'static>>,
    section: usize,
    pub(super) does: Does,
    /// A page's heading, which ]p and [p stop on.
    heading: bool,
    /// Drawn on a surface of its own (the Decided block, a change's lines).
    bg: Option<ratatui::style::Color>,
}

/// The Story view's own state: the outline's chosen entry, the column's
/// cursor line and first line, What runs (whether it is laid over the
/// column, and its chosen step), the files of tests unfolded, the line a
/// change's menu was opened on, and the column as last laid out with the
/// width and the unfolded it was laid out for.
#[derive(Clone, Debug, Default)]
pub(super) struct Reader {
    pub(super) item: usize,
    pub(super) cur: usize,
    pub(super) top: usize,
    pub(super) runs: bool,
    pub(super) run: Option<usize>,
    /// The outline's first row on the screen, past what a long one hides.
    outline_top: usize,
    unfolded: HashSet<(usize, String)>,
    pub(super) target: Option<Does>,
    /// Where a selection of the column's lines started (`v`).
    pub(super) visual: Option<usize>,
    /// The last key was ] or [ alone, so a p or c after it adds nothing.
    pub(super) after_bracket: bool,
    laid: Option<(u16, Vec<Row>)>,
    /// The column to lay out again on its next draw, the rows before kept
    /// to find the cursor's section in.
    stale: bool,
}

/// A row of the outline, and the entry it is, where it is one.
type OutlineRow = (Vec<Span<'static>>, Option<usize>);

/// The column's width from which a change is shown side by side: each
/// side at least 60 columns, as boards 144 and 145 say.
const SIDE_BY_SIDE: usize = 121;
/// The widest a page's prose is laid out, its changes under it taking the
/// column's whole width.
const PROSE: usize = 88;

/// How many rows What runs takes over the column: its label, a box's
/// four rows, and a rule.
const RUNS_ROWS: u16 = 6;

/// `n` of something, by its name: "1 file", "3 files".
fn counted(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// A why's first sentence, its full stop kept: the overview's line.
fn first_sentence(text: &str) -> String {
    let text = text.trim();
    match text.find(". ") {
        Some(at) => text[..=at].to_string(),
        None => text.to_string(),
    }
}

/// Where the view used last is kept.
fn mode_file() -> PathBuf {
    paths::state_dir().join("diff-mode")
}

/// A hunk's lines side by side, as Files mode shows a change: the old
/// side at the left and the new at the right, each line beside the one it
/// became (`sides::align`), the words that changed in the stronger shade,
/// a line only one side has facing a gap, a long line going on in rows
/// under it. Each row with the new and the old line it is at; a missing
/// newline at the end said under its line, across both.
#[allow(clippy::type_complexity)]
fn side_by_side(
    lines: &[(char, String, Option<usize>, Option<usize>)],
    cols: usize,
    (old_bg, old_word, new_bg, new_word): (
        ratatui::style::Color,
        ratatui::style::Color,
        ratatui::style::Color,
        ratatui::style::Color,
    ),
    theme: &Theme,
) -> Vec<(Vec<Span<'static>>, Option<usize>, Option<usize>)> {
    use crate::sides::{Kind, align};
    let mut old: Vec<(usize, String)> = Vec::new();
    let mut new: Vec<(usize, String)> = Vec::new();
    let mut no_newline = false;
    for (mark, text, o, n) in lines {
        let text = text.replace('\t', "    ");
        match mark {
            '-' => old.push((o.unwrap_or(0), text)),
            '+' => new.push((n.unwrap_or(0), text)),
            ' ' => {
                old.push((o.unwrap_or(0), text.clone()));
                new.push((n.unwrap_or(0), text));
            }
            _ => no_newline = true,
        }
    }
    let texts = |side: &[(usize, String)]| side.iter().map(|(_, t)| t.clone()).collect::<Vec<_>>();
    let half = cols.saturating_sub(1) / 2;
    let room = half.saturating_sub(6).max(1);
    let sunken = Surface::Sunken.color(theme);
    let faint = Tone::Faint.style(theme);
    let text_style = Tone::Text.style(theme);
    // One side of a row: its rows of spans, each `half` wide.
    let side = |line: Option<(usize, &str, &[std::ops::Range<usize>])>, bg, strong| {
        let Some((number, text, words)) = line else {
            return vec![vec![Span::styled(
                " ".repeat(half),
                Style::new().bg(sunken),
            )]];
        };
        let mut out: Vec<Vec<Span<'static>>> = Vec::new();
        let mut piece: Vec<(char, bool)> = Vec::new();
        let mut used = 0;
        let mut pieces: Vec<Vec<(char, bool)>> = Vec::new();
        for (at, c) in text.char_indices() {
            let w = crate::sides::char_width(c);
            if used + w > room && used > 0 {
                pieces.push(std::mem::take(&mut piece));
                used = 0;
            }
            piece.push((c, words.iter().any(|r| r.contains(&at))));
            used += w;
        }
        pieces.push(piece);
        for (k, chars) in pieces.into_iter().enumerate() {
            let num = if k == 0 {
                number.to_string()
            } else {
                String::new()
            };
            let mut spans = vec![Span::styled(format!("{num:>5} "), faint.bg(bg))];
            let mut run = String::new();
            let mut lit = false;
            let mut width = 0;
            for (c, w) in chars {
                if w != lit && !run.is_empty() {
                    let b = if lit { strong } else { bg };
                    spans.push(Span::styled(std::mem::take(&mut run), text_style.bg(b)));
                }
                lit = w;
                width += crate::sides::char_width(c);
                run.push(c);
            }
            if !run.is_empty() {
                let b = if lit { strong } else { bg };
                spans.push(Span::styled(run, text_style.bg(b)));
            }
            spans.push(Span::styled(
                " ".repeat(room.saturating_sub(width)),
                Style::new().bg(bg),
            ));
            out.push(spans);
        }
        out
    };
    let mut rows = Vec::new();
    for row in align(&texts(&old), &texts(&new)) {
        let (left_bg, right_bg) = match row.kind {
            Kind::Same => (sunken, sunken),
            Kind::Changed => (old_bg, new_bg),
            Kind::Removed => (old_bg, sunken),
            Kind::Added => (sunken, new_bg),
        };
        let left_line = row
            .old
            .map(|i| (old[i].0, old[i].1.as_str(), row.old_words.as_slice()));
        let right_line = row
            .new
            .map(|i| (new[i].0, new[i].1.as_str(), row.new_words.as_slice()));
        let left = side(left_line, left_bg, old_word);
        let right = side(right_line, right_bg, new_word);
        let blank = |bg| vec![Span::styled(" ".repeat(half), Style::new().bg(bg))];
        for k in 0..left.len().max(right.len()) {
            let mut spans = left.get(k).cloned().unwrap_or_else(|| blank(left_bg));
            spans.push(Span::styled("│", faint));
            spans.extend(right.get(k).cloned().unwrap_or_else(|| blank(right_bg)));
            rows.push((spans, row.new.map(|i| new[i].0), row.old.map(|i| old[i].0)));
        }
    }
    if no_newline {
        rows.push((
            vec![Span::styled("      ⏎ no newline at end of file", faint)],
            None,
            None,
        ));
    }
    rows
}

/// A line of code in pieces of at most `cols` columns, each piece whole
/// characters, spaces kept: at least one piece, empty for an empty line.
fn pieces(text: &str, cols: usize) -> Vec<String> {
    let cols = cols.max(1);
    let mut out = vec![String::new()];
    let mut used = 0;
    for c in text.chars() {
        let w = width(c.encode_utf8(&mut [0; 4]));
        if used + w > cols && used > 0 {
            out.push(String::new());
            used = 0;
        }
        if let Some(last) = out.last_mut() {
            last.push(c);
        }
        used += w;
    }
    out
}

/// Each line of a hunk, from its header's next line to the next header:
/// its text without its mark, its kind (' ', '+' or '-'), and its line on
/// the old side and the new.
fn hunk_lines(
    lines: &[String],
    hunk: &story::Hunk,
) -> Vec<(char, String, Option<usize>, Option<usize>)> {
    let (mut old, mut new) = (hunk.old_start, hunk.new_start);
    // Only the lines the header counts: what follows the last hunk of a
    // patch (its closing empty line) is no line of it.
    let (mut olds, mut news) = (hunk.old_count, hunk.new_count);
    let mut out = Vec::new();
    for line in lines.iter().skip(hunk.line) {
        // git's "\ No newline at end of file" goes with the line before
        // it, even the hunk's last.
        let marker = line.starts_with('\\');
        if line.starts_with("@@") || line.starts_with("diff --git") || (olds + news == 0 && !marker)
        {
            break;
        }
        let mut chars = line.chars();
        let mark = chars.next().unwrap_or(' ');
        let text: String = chars.collect();
        match mark {
            '+' => {
                out.push(('+', text, None, Some(new)));
                new += 1;
                news = news.saturating_sub(1);
            }
            '-' => {
                out.push(('-', text, Some(old), None));
                old += 1;
                olds = olds.saturating_sub(1);
            }
            ' ' => {
                out.push((' ', text, Some(old), Some(new)));
                old += 1;
                new += 1;
                olds = olds.saturating_sub(1);
                news = news.saturating_sub(1);
            }
            // The line before has no newline at its end: said under it.
            '\\' => out.push(('\\', "no newline at end of file".into(), None, None)),
            _ => {}
        }
    }
    out
}

impl App {
    // ── What is shown ───────────────────────────────────────────────────────

    /// Whether the story shown is told in chapters, with the
    /// diff_story_map setting on, in the branch's own diff.
    pub(super) fn mapped(&self) -> bool {
        self.opts.map
            && !self.in_history
            && self.story.as_ref().is_some_and(|s| !s.chapters.is_empty())
    }

    /// Whether the Story view is on the screen.
    pub(super) fn on_story(&self) -> bool {
        self.mapped() && self.mode == Mode::Story
    }

    /// Page `n`'s number as the reader counts it: in its chapter in a
    /// story told in chapters, else among the pages told.
    pub(super) fn page_shown(&self, n: usize) -> usize {
        match self.told_map() {
            Some(story) => story::page_number(story, n).1,
            None => n,
        }
    }

    /// Page `n` headed as the reader counts it: "Chapter 2 · page 1 of
    /// 3" in a story of several chapters, else "Page 4 of 5".
    pub(super) fn page_heading(&self, n: usize) -> String {
        let Some(story) = self.told_map() else {
            return format!("Page {n} of {}", self.told_pages());
        };
        match story::page_number(story, n) {
            (Some(c), k, m) if story.chapters.len() > 1 => {
                format!("Chapter {} · page {k} of {m}", c + 1)
            }
            (_, k, m) => format!("Page {k} of {m}"),
        }
    }

    /// Page `n` named from where the reader is: its number alone in the
    /// chapter of the page showing `path`, its chapter and number in
    /// another.
    pub(super) fn page_named(&self, n: usize, path: &str) -> String {
        match self.told_map() {
            Some(story) => {
                let from = self
                    .alone
                    .or_else(|| self.home(path))
                    .and_then(|p| story::chapter_of(story, p));
                story::page_name(story, n, from)
            }
            None => n.to_string(),
        }
    }

    pub(super) fn told_map(&self) -> Option<&Story> {
        self.story.as_ref().filter(|_| self.mapped())
    }

    /// How many of a chapter's pages are read, and how many it has.
    pub(super) fn read_of(&self, chapter: usize) -> (usize, usize) {
        let Some(t) = self.story.as_ref().and_then(|s| s.chapters.get(chapter)) else {
            return (0, 0);
        };
        (
            t.pages.iter().filter(|n| self.read.contains(n)).count(),
            t.pages.len(),
        )
    }

    /// How many of the chapters' pages are read, and how many they have.
    pub(super) fn read_count(&self) -> (usize, usize) {
        let Some(story) = self.told_map() else {
            return (0, 0);
        };
        let pages: Vec<usize> = story
            .chapters
            .iter()
            .flat_map(|c| c.pages.iter().copied())
            .collect();
        (
            pages.iter().filter(|n| self.read.contains(n)).count(),
            pages.len(),
        )
    }

    // ── The views ───────────────────────────────────────────────────────────

    /// The story told has just come: told in chapters, the diff opens in
    /// the view used last, the Story view the first time, on what
    /// changes, with the pages read before marked; where someone has
    /// already moved through the files, it stays on them.
    pub(super) fn enter_story(&mut self) {
        // Where Files mode was left is kept while the Story view stays
        // shown across a story told again: its file is found again by its key.
        if self.mode == Mode::Files {
            self.files_place = None;
        }
        self.read.clear();
        self.reader = Reader::default();
        self.mapper = super::map::Mapper::default();
        if !self.mapped() {
            // A story told as a list: the files where they were left.
            if self.mode != Mode::Files {
                self.files_again();
            }
            self.mode = Mode::Files;
            return;
        }
        self.read = self.kept_read();
        // A story told again while the Story view was shown (the change
        // read again) stays there; the first to come stays on the files
        // where someone is already reading them.
        self.mode = if self.mode != Mode::Files {
            self.mode
        } else if self.moved || self.alone.is_some() || self.focus != Focus::List {
            Mode::Files
        } else {
            self.kept_mode()
        };
        if self.mode != Mode::Files {
            // The outline has the keys: typing the file list's filter ends.
            self.filtering = false;
            self.focus = Focus::List;
            self.mark_read(0);
        }
    }

    fn kept_mode(&self) -> Mode {
        let kept = self
            .opts
            .keep
            .then(|| util::read_text(mode_file()))
            .flatten();
        match kept.as_deref().map(str::trim) {
            Some("files") => Mode::Files,
            Some("map") => Mode::Map,
            _ => Mode::Story,
        }
    }

    pub(super) fn keep_mode(&self) {
        if self.opts.keep {
            let _ = util::write_atomic(mode_file(), format!("{}\n", self.mode.word()));
        }
    }

    /// Where the pages read of the story shown are kept: beside the
    /// story, by its key.
    fn read_file(&self) -> Option<PathBuf> {
        let key = self.story_key.as_ref()?;
        Some(tell::story_dir().join(format!("{key}.read")))
    }

    fn kept_read(&self) -> HashSet<usize> {
        if !self.opts.keep {
            return HashSet::new();
        }
        self.read_file()
            .and_then(util::read_text)
            .and_then(|text| serde_json::from_str::<Vec<usize>>(&text).ok())
            .map(|read| read.into_iter().collect())
            .unwrap_or_default()
    }

    /// Section `n` read (0 what changes, else a page), kept beside the
    /// story with what another diff open on it has read since.
    fn mark_read(&mut self, n: usize) {
        let kept = self.kept_read();
        self.read.extend(kept);
        if !self.read.insert(n) || !self.opts.keep {
            return;
        }
        if let Some(file) = self.read_file() {
            let mut read: Vec<usize> = self.read.iter().copied().collect();
            read.sort();
            let _ = util::write_atomic(&file, util::json_compact(&read));
        }
    }

    /// Why this diff has no Story view, said when one is asked for.
    pub(super) fn no_map(&mut self) {
        let text = if self.in_history {
            "A commit of the history has no story view.\nc goes back to the whole branch's."
        } else if !self.opts.map {
            "No story view for this diff: Settings' \"Open the diff on what changes\" (diff_story_map) is off."
        } else if self.story.is_none() {
            return self.no_story();
        } else {
            "This story was told as a list, with no chapters.\nThe menu's Tell it again tells it anew."
        };
        self.say(text);
    }

    /// `g m`, and Story in the switch and the menu: the Story view, from
    /// anywhere, where it was left. Files mode keeps its place for the way
    /// back.
    pub(super) fn open_story_view(&mut self) {
        if !self.mapped() {
            return self.no_map();
        }
        if self.mode == Mode::Files {
            self.files_place = Some(self.place());
        }
        self.mode = Mode::Story;
        self.keep_mode();
        // The filter is the file list's: kept with its place, and its
        // typing ended, so the outline has the keys.
        self.filtering = false;
        self.path_filter.clear();
        // Nor is a selection of the file list's rows the story's to copy.
        self.list_visual = None;
        self.visual = None;
        // Nor one made in the column before it was left.
        self.reader.visual = None;
        self.focus = Focus::List;
        if self.alone.take().is_some() {
            self.whole_at = None;
            self.lines.clear();
            self.sort();
            self.show_at(0);
        }
    }

    // ── Files mode, and the way between ─────────────────────────────────────

    pub(super) fn place(&mut self) -> Place_ {
        let anchors = self.anchors();
        let list_place = self.list_row();
        let key = |k: usize| {
            self.order
                .get(k)
                .and_then(|&i| self.entry(i))
                .map(Entry::key)
        };
        Place_ {
            alone: self.alone,
            file: key(self.at),
            view: key(self.view),
            cur: self.cur,
            col: self.col,
            top: self.top,
            focus: self.focus,
            filter: self.path_filter.clone(),
            adrift: self.adrift,
            whole_at: self.whole_at.clone(),
            list_top: self.list_top,
            list_followed: self.list_followed,
            anchors,
            list_place,
        }
    }

    /// The map gone from under the reader: Files mode where it was left,
    /// its filter back and its file found again by its key.
    pub(super) fn files_again(&mut self) {
        self.mode = Mode::Files;
        let Some(place) = self.files_place.take() else {
            // Nowhere saved: the whole list as it is now, a page's
            // filter let go, its cursor on a file.
            self.alone = None;
            self.whole_at = None;
            self.filtering = false;
            self.path_filter.clear();
            self.lines.clear();
            self.sort();
            return self.show_at(self.at);
        };
        // As the menu's Files restores it, on the change read again.
        self.alone = None;
        self.whole_at = None;
        self.restore_place(place);
    }

    /// A place put back on the list as it is now, its page already open:
    /// its filter, file, cursor, scroll and focus.
    pub(super) fn restore_place(&mut self, place: Place_) {
        self.filtering = false;
        self.path_filter = place.filter.clone();
        self.lines.clear();
        self.sort();
        let at_key = |app: &App, key: &Option<String>| {
            key.as_ref().and_then(|key| {
                app.order
                    .iter()
                    .position(|&i| app.entries()[i].key() == *key)
            })
        };
        let Some(at) = at_key(self, &place.file) else {
            return self.show_at(0);
        };
        self.show_at(at);
        // Restored: the story arriving doesn't move it to its first file.
        self.moved = true;
        self.view = at_key(self, &place.view).unwrap_or(self.at);
        self.cur = place.cur;
        self.col = place.col;
        self.top = place.top;
        self.focus = place.focus;
        self.adrift = place.adrift;
        self.list_top = place.list_top;
        self.list_followed = place.list_followed;
        self.list_rewrapped(place.list_place.clone());
        if let Some(anchors) = place.anchors {
            self.anchor_back(anchors);
        }
        self.keep_side();
        self.fit_view();
        // The cursor kept within its file as it is now; the view's line
        // is its own file's, which fit_view keeps.
        let n = self.screen_lines().len();
        self.cur = self.cur.min(n.saturating_sub(1));
    }

    /// The story is another one: where Files mode was left keeps its file
    /// and lets go of its page, a number of the story before.
    pub(super) fn forget_alone(&mut self) {
        if let Some(place) = &mut self.files_place {
            place.alone = None;
            place.whole_at = None;
        }
    }

    /// Files in the switch and the menu (`restore`): Files mode where it
    /// was left. `g f`: Files mode on the chosen page's first file.
    pub(super) fn show_files(&mut self, restore: bool) {
        // While a map is still being told, the files where they were left.
        if !self.mapped() && self.mode != Mode::Files {
            self.files_again();
            return self.keep_mode();
        }
        if !self.mapped() {
            return self.no_map();
        }
        // Files already shown: the choice is kept for the next diff.
        if self.mode == Mode::Files {
            return self.keep_mode();
        }
        let chosen = if self.mode == Mode::Map {
            self.mapper.at.max(1)
        } else {
            self.section().max(1)
        };
        self.mode = Mode::Files;
        self.keep_mode();
        self.visual = None;
        // Where Files mode was left is taken only to go back to it: g f
        // leaves it for the menu, should it find no file to show.
        let place = if restore {
            self.files_place.take()
        } else {
            None
        };
        // A page the story no longer has (told again since) is let go.
        let pages = self.story.as_ref().map_or(0, |s| s.pages.len());
        self.alone = place.as_ref().and_then(|p| p.alone).filter(|p| *p <= pages);
        self.whole_at = place.as_ref().and_then(|p| p.whole_at.clone());
        self.filtering = false;
        self.path_filter = place.as_ref().map(|p| p.filter.clone()).unwrap_or_default();
        self.lines.clear();
        self.sort();
        let at_key = |app: &App, key: &Option<String>| {
            key.as_ref().and_then(|key| {
                app.order
                    .iter()
                    .position(|&i| app.entries()[i].key() == *key)
            })
        };
        match place {
            Some(place) => {
                let at = at_key(self, &place.file).unwrap_or(0);
                self.show_at(at);
                if at_key(self, &place.file).is_some() {
                    self.view = at_key(self, &place.view).unwrap_or(self.at);
                    self.cur = place.cur;
                    self.col = place.col;
                    self.top = place.top;
                    self.focus = place.focus;
                    self.adrift = place.adrift;
                    self.list_top = place.list_top;
                    self.list_followed = place.list_followed;
                    self.list_rewrapped(place.list_place.clone());
                    if let Some(anchors) = place.anchors {
                        self.anchor_back(anchors);
                    }
                    self.keep_side();
                    self.fit_view();
                }
            }
            None => {
                // An entry with a hunk of the page's from its own diff
                // first, so a file both staged and changed since opens as
                // the one the page is about.
                let holds = |i: usize| {
                    let entry = &self.entries()[i];
                    let run = self.run_of(entry);
                    self.story
                        .as_ref()
                        .and_then(|s| s.hunks.get(&entry.path))
                        .is_some_and(|hunks| {
                            hunks
                                .iter()
                                .filter(|h| run.is_none_or(|r| h.run == r))
                                .any(|h| {
                                    // Its own, or one it shows again.
                                    h.page == chosen
                                        || self.story.as_ref().is_some_and(|s| {
                                            s.repeats
                                                .get(&h.id)
                                                .is_some_and(|c| c.contains(&chosen))
                                        })
                                })
                        })
                };
                let first = self
                    .order
                    .iter()
                    .position(|&i| holds(i))
                    .or_else(|| {
                        self.order.iter().position(|&i| {
                            self.story
                                .as_ref()
                                .and_then(|s| s.page.get(&self.entries()[i].path))
                                == Some(&chosen)
                        })
                    })
                    // A page that heads no file: the first it touches.
                    .or_else(|| {
                        let touches = &self.story.as_ref()?.pages.get(chosen - 1)?.touches;
                        self.order
                            .iter()
                            .position(|&i| touches.contains(&self.entries()[i].path))
                    });
                // None of its files in the diff as it is now: the map
                // stays, saying so, rather than some other page's file.
                let Some(first) = first else {
                    // The menu and the switch open the list whatever the
                    // chosen page has left in it; only g f asks for
                    // that page's file.
                    if restore {
                        let first = self.first_written();
                        self.focus = Focus::List;
                        return self.show_at(first);
                    }
                    self.mode = Mode::Story;
                    self.keep_mode();
                    return self.say(
                        "This page's files aren't in the diff as it is now: they changed back since its commits.",
                    );
                };
                self.focus = Focus::List;
                self.moved = true;
                self.show_at(first);
            }
        }
    }

    /// The whole diff in Files mode, for a line gone to wherever it is.
    pub(super) fn whole_files(&mut self) {
        self.mode = Mode::Files;
        self.keep_mode();
        self.visual = None;
        self.alone = None;
        self.whole_at = None;
        // A filter typed in a page's list would hide the line's file.
        self.filtering = false;
        self.path_filter.clear();
        self.lines.clear();
        self.sort();
    }

    /// The page a hunk is also in, gone to: its page in Map mode and
    /// where ]p turns pages, else its first file in the list.
    pub(super) fn go_to_page(&mut self, n: usize) {
        if self.on_story() {
            return self.go_to_section(n);
        }
        // Its page, where its hunks are its own and carry their notes.
        if self.opts.pages_alone {
            return self.open_alone(n);
        }
        let first = self.order.iter().position(|&i| {
            self.story
                .as_ref()
                .and_then(|s| s.page.get(&self.entries()[i].path))
                == Some(&n)
        });
        // A filter that hides the page's files gives way, as a page's
        // opening does.
        let first = first.or_else(|| {
            if self.path_filter.trim().is_empty() {
                return None;
            }
            self.filtering = false;
            self.path_filter.clear();
            self.sort();
            self.order.iter().position(|&i| {
                self.story
                    .as_ref()
                    .and_then(|s| s.page.get(&self.entries()[i].path))
                    == Some(&n)
            })
        });
        match first {
            Some(first) => {
                self.moved = true;
                self.show_at(first);
            }
            // A page that heads no file of the list has only its page.
            None => self.say(format!(
                "{} heads no file in the list: its page shows it, with Settings' \"Diff story a page at a time\" (diff_story_pages) on.",
                self.page_heading(n)
            )),
        }
    }

    /// Enter or a click on a hunk's "also in page" line: its home
    /// page, on the file the hunk is in.
    pub(super) fn go_to_also(&mut self, page: usize, hunk: Option<String>) {
        // By its key, so a file both staged and changed since opens the
        // diff the hunk is from; and the hunk itself, the one of the home
        // page's this page shows again.
        let key = self.current().map(Entry::key);
        let shown_in = self.current().and_then(|e| self.home(&e.path));
        let target = self.current().zip(self.story.as_ref()).and_then(|(e, s)| {
            let run = self.run_of(e);
            s.hunks
                .get(&e.path)?
                .iter()
                .find(|h| {
                    // The hunk the line was over, where it says which.
                    hunk.as_ref().is_none_or(|id| *id == h.id)
                        && h.page == page
                        && run.is_none_or(|r| h.run == r)
                        && s.repeats
                            .get(&h.id)
                            .is_some_and(|c| shown_in.is_some_and(|p| c.contains(&p)))
                })
                .map(|h| ((h.new_start, h.new_count), (h.old_start, h.old_count)))
        });
        self.go_to_page(page);
        // On the page alone the hunk's own file; with no page, the
        // whole list shows the page's first file, where it starts.
        if self.alone != Some(page) {
            return;
        }
        let at = key.and_then(|key| {
            self.order
                .iter()
                .position(|&i| self.entries()[i].key() == key)
        });
        if let Some(at) = at.filter(|at| *at != self.at) {
            self.show_at(at);
        }
        // At the hunk, where its lines still are (a file not edited since).
        if let Some((new_line, old_line)) = target
            && self.current().is_some_and(|e| !self.changed_since(e))
        {
            let _ = self.at_hunk(new_line, old_line);
        }
    }

    /// The cursor on a hunk's first changed line, on the side that has it:
    /// the old for a line only taken out, whatever context is around it.
    /// Whether a row of the hunk was found to stand on.
    fn at_hunk(&mut self, new_line: (usize, usize), old_line: (usize, usize)) -> bool {
        self.at_hunk_changed(new_line, old_line, false)
    }

    /// As `at_hunk`, a row with no change refused with `changed`: a line
    /// added or taken out is found only where it is still a change.
    fn at_hunk_changed(
        &mut self,
        new_line: (usize, usize),
        old_line: (usize, usize),
        changed_only: bool,
    ) -> bool {
        let Some((changed, rows)) =
            self.current()
                .and_then(|e| self.files.get(&e.key()))
                .map(|s| {
                    (
                        s.rows.iter().map(|r| r.changes()).collect::<Vec<bool>>(),
                        s.rows.iter().map(|r| (r.old, r.new)).collect::<Vec<_>>(),
                    )
                })
        else {
            return false;
        };
        // A hunk inside a fold (its lines show no difference): the fold
        // opened first, so its lines are there to stand on.
        let held = |(old, new): (Option<usize>, Option<usize>)| {
            let has = |line: Option<usize>, (start, count): (usize, usize)| {
                line.is_some_and(|l| l + 1 >= start && l + 1 < start + count.max(1))
            };
            has(old, old_line) || has(new, new_line)
        };
        let fold = self.screen_lines().iter().position(|l| {
            matches!(l, Line::Fold { first, count }
                if rows.get(*first..*first + *count).is_some_and(|r| r.iter().any(|&x| held(x))))
        });
        if let Some(i) = fold {
            self.cur = i;
            self.open_fold();
        }
        let lines = self.screen_lines().to_vec();
        // Within the hunk's own lines on either side.
        let within = |cell: &Cell, (start, count): (usize, usize)| {
            matches!(cell, Cell::Text { line, range }
                if *line + 1 >= start && *line + 1 < start + count.max(1) && range.start == 0)
        };
        let inside = |l: &Line| matches!(l, Line::Pair { old, new, .. } if within(old, old_line) || within(new, new_line));
        let changed_row =
            |l: &Line| matches!(l, Line::Pair { row, .. } if changed.get(*row) == Some(&true));
        let found = lines.iter().position(|l| inside(l) && changed_row(l));
        if changed_only && found.is_none() {
            return false;
        }
        let Some(i) = found.or_else(|| lines.iter().position(inside)) else {
            return false;
        };
        if found.is_none() {
            self.say("Its change shows no difference here (only spacing): shown at its lines.");
        }
        if let Line::Pair { old, new, .. } = &lines[i] {
            let only_old = matches!(new, Cell::Gap) && matches!(old, Cell::Text { .. });
            self.focus = if only_old && self.side_widths().0 > 0 {
                Focus::Old
            } else {
                self.main_side()
            };
        }
        self.cur = i;
        self.col = 0;
        self.top = i.saturating_sub(self.body_height() / 3);
        true
    }

    /// ]c and [c in Files mode's list, in a story told as a map: the
    /// first file of the next chapter, or of the one before, as the list
    /// shows them.
    pub(super) fn chapter_jump(&mut self, step: i32) {
        let Some(story) = self.told_map() else {
            return;
        };
        let chapter_at = |app: &App, k: usize| {
            let path = &app.entries()[app.order[k]].path;
            app.story
                .as_ref()
                .and_then(|s| s.page.get(path))
                .and_then(|&n| story::chapter_of(app.story.as_ref()?, n))
        };
        if story.chapters.is_empty() {
            return;
        }
        if self.order.is_empty() {
            return self.say("The filter leaves no files.\n/ then esc clears it.");
        }
        // The list's groups in its order, each chapter with a file in it
        // and the files no chapter holds, past those a filter leaves
        // empty: the next group's first file, or the one before's.
        // A run of rows of one group each, as the list shows them (a
        // working tree's changes and its staged ones may each hold the
        // same chapters).
        let mut groups: Vec<(_, usize)> = Vec::new();
        for k in 0..self.order.len() {
            let section = self.entries()[self.order[k]].section;
            let group = (section, chapter_at(self, k));
            if groups.last().is_none_or(|(g, _)| *g != group) {
                groups.push((group, k));
            }
        }
        if self.at >= self.order.len() {
            return;
        }
        let Some(i) = groups.iter().rposition(|(_, first)| *first <= self.at) else {
            return;
        };
        let Some(&(_, first)) = usize::try_from(i as i32 + step)
            .ok()
            .and_then(|j| groups.get(j))
        else {
            if !self.path_filter.trim().is_empty() {
                let way = if step > 0 { "after" } else { "before" };
                self.say(format!(
                    "No chapter {way} this one has a file the filter leaves.\n/ then esc clears it."
                ));
            }
            return;
        };
        self.moved = true;
        self.show_at(first);
    }

    /// One of the two modes chosen, in the menu or the switch.
    pub(super) fn choose_mode(&mut self, mode: Mode) {
        self.menu = None;
        match mode {
            // The view already shown stays as it is.
            Mode::Story if self.mode == Mode::Story => self.keep_mode(),
            Mode::Story => self.open_story_view(),
            Mode::Map if self.mode == Mode::Map => self.keep_mode(),
            Mode::Map => self.open_map_view(),
            Mode::Files => self.show_files(true),
        }
    }

    /// The pages no chapter holds (what isn't committed yet, generated
    /// files, changes Haiku left out), which Files mode alone shows, and
    /// how many of the diff's files, as it is now, they have.
    pub(super) fn files_only(&self, story: &Story) -> (Vec<usize>, usize) {
        let pages: Vec<usize> = (1..=story.pages.len())
            .filter(|&n| story::chapter_of(story, n).is_none())
            .collect();
        let mut paths: Vec<&str> = self
            .entries()
            .iter()
            .filter(|e| story.page.get(&e.path).is_some_and(|n| pages.contains(n)))
            .map(|e| e.path.as_str())
            .collect();
        paths.sort();
        paths.dedup();
        (pages, paths.len())
    }

    /// The page a file of the open page is shown from: one whose hunks
    /// here are all another page's, shown again.
    pub(super) fn also_of(&self, path: &str, run: Option<usize>) -> Option<usize> {
        let (story, page) = self.story.as_ref().zip(self.alone)?;
        // Of the diff the entry is from, where a file is listed twice.
        let hunks: Vec<&story::HunkNote> = story
            .hunks
            .get(path)?
            .iter()
            .filter(|h| run.is_none_or(|r| h.run == r))
            .collect();
        hunks
            .iter()
            .find(|h| story.repeats.get(&h.id).is_some_and(|c| c.contains(&page)))
            .map(|h| h.page)
    }

    /// The pages a file shows hunks from under the page it is shown
    /// in, each once, whether or not that page has hunks of its own in
    /// the file.
    pub(super) fn also_any(&self, path: &str, run: Option<usize>) -> Vec<usize> {
        // The page the file is shown under: the page's, else its own.
        let Some((story, page)) = self.story.as_ref().zip(self.home(path)) else {
            return Vec::new();
        };
        let mut homes: Vec<usize> = story
            .hunks
            .get(path)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .filter(|h| run.is_none_or(|r| h.run == r))
            .filter(|h| {
                h.page != page && story.repeats.get(&h.id).is_some_and(|c| c.contains(&page))
            })
            .map(|h| h.page)
            .collect();
        homes.sort();
        homes.dedup();
        homes
    }

    /// The line over a hunk a page shows from another, as board 123
    /// draws it: where its home is, and that Enter goes there.
    pub(super) fn also_line(
        &self,
        story: &Story,
        path: &str,
        home: usize,
        hunk: Option<String>,
    ) -> Line {
        let theme = self.theme();
        let title = story.pages.get(home - 1).map_or("", |c| c.title.as_str());
        // Named from the page showing the hunk: the open one, else its
        // file's own.
        let from = self
            .alone
            .or_else(|| self.home(path))
            .and_then(|p| story::chapter_of(story, p));
        Line::Also(
            home,
            hunk,
            vec![
                Span::styled(" ↗ ", Tone::Accent.style(&theme)),
                Span::styled(
                    format!("also {}: {title}", story::in_page(story, home, from)),
                    Tone::Dim.style(&theme),
                ),
                Span::styled("   its home · ↵ goes there", Tone::Faint.style(&theme)),
            ],
        )
    }

    /// The switch between the two views at the head of easy mode's row of
    /// buttons, as board 125 draws it; gives the column after it.
    pub(super) fn draw_switch(&mut self, buf: &mut Buffer, x: u16, y: u16) -> u16 {
        if !self.mapped() {
            return x;
        }
        let theme = self.theme();
        let chosen = MENU.iter().position(|(_, mode)| *mode == self.mode);
        let lit = MENU
            .iter()
            .position(|(_, mode)| self.hover == Some(Button::Mode(*mode)));
        let parts: Vec<&str> = MENU.iter().map(|(label, _)| *label).collect();
        let (spans, places) = kit::choice_switch(&parts, chosen, lit, &[], false, &theme);
        let w = kit::spans_width(&spans) as u16;
        if x + w >= self.cols {
            return x;
        }
        put_line(buf, x, y, w, &Spans::from(spans));
        for ((_, mode), (at, pw)) in MENU.iter().zip(places) {
            self.buttons.push((
                Button::Mode(*mode),
                Rect::new(x + at as u16, y, pw as u16, 1),
            ));
        }
        x + w + 1
    }

    /// The outline's In the files only: Files mode on the first file of a
    /// page no chapter holds, which only Files mode shows.
    pub(super) fn open_files_only(&mut self) {
        let Some(story) = self.told_map() else {
            return;
        };
        let (pages, _) = self.files_only(story);
        self.show_files(true);
        // The whole list, a filter or a page kept with Files mode's place
        // let go, so none of the chip's files is hidden.
        let first = {
            self.alone = None;
            self.whole_at = None;
            self.filtering = false;
            self.path_filter.clear();
            self.lines.clear();
            self.sort();
            self.order.iter().position(|&i| {
                self.story
                    .as_ref()
                    .and_then(|s| s.page.get(&self.entries()[i].path))
                    .is_some_and(|n| pages.contains(n))
            })
        };
        if let Some(first) = first {
            self.focus = Focus::List;
            self.moved = true;
            self.show_at(first);
        }
    }

    /// The first file someone wrote among those listed, else the first.
    pub(super) fn first_written(&self) -> usize {
        self.order
            .iter()
            .position(|&i| !order::generated(&self.entries()[i].path))
            .unwrap_or(0)
    }

    // ── The outline ─────────────────────────────────────────────────────────

    /// The outline's entries in its order: what changes, each chapter's
    /// pages in reading order, then the files no chapter holds.
    pub(super) fn items(&self) -> Vec<Item> {
        let Some(story) = self.told_map() else {
            return Vec::new();
        };
        let mut items = vec![Item::Overview];
        for chapter in &story.chapters {
            items.extend(chapter.pages.iter().map(|&n| Item::Page(n)));
        }
        if self.files_only(story).1 > 0 {
            items.push(Item::FilesOnly);
        }
        items
    }

    /// The section the column's cursor is in: 0 for what changes, else
    /// a page's number.
    pub(super) fn section(&mut self) -> usize {
        let (cols, cur) = (self.column_cols(), self.reader.cur);
        self.column(cols).get(cur).map_or(0, |r| r.section)
    }

    /// The column's row under the cursor.
    pub(super) fn row_at_cur(&mut self) -> Option<Row> {
        let (cols, cur) = (self.column_cols(), self.reader.cur);
        self.column(cols).get(cur).cloned()
    }

    /// The outline's entry for where the column is, the section read.
    fn follow(&mut self) {
        self.follow_marking(true);
    }

    /// The outline's entry for where the column is; the section counted
    /// as read only with `read`, never while the outline is only moved
    /// through.
    fn follow_marking(&mut self, read: bool) {
        let section = self.section();
        let item = if section == 0 {
            Item::Overview
        } else {
            Item::Page(section)
        };
        if let Some(i) = self.items().iter().position(|it| *it == item) {
            self.reader.item = i;
        }
        // What runs follows the page being read: its step lit, or none.
        self.reader.run = self
            .told_map()
            .and_then(|s| s.runs.iter().position(|r| r.page == section));
        if read {
            self.mark_read(section);
        }
    }

    /// The section under the column's cursor read, while the column has
    /// the keys: giving it them (Tab, l, a click) reads what it shows.
    pub(super) fn read_if_reading(&mut self) {
        // A selection of the column's lines is the column's alone.
        if self.focus == Focus::List {
            self.reader.visual = None;
        }
        if self.on_story() && self.focus != Focus::List {
            let section = self.section();
            self.mark_read(section);
        }
    }

    /// The column at section `n`: its first line under the cursor, at the
    /// top of the column.
    pub(super) fn go_to_section(&mut self, n: usize) {
        self.show_section(n, true);
    }

    fn show_section(&mut self, n: usize, read: bool) {
        let cols = self.column_cols();
        let rows = self.column(cols);
        let Some(at) = rows.iter().position(|r| r.section == n) else {
            return;
        };
        self.reader.cur = at;
        self.reader.top = at;
        self.follow_marking(read);
    }

    /// The outline's entry `i` chosen: the column shows where it is.
    fn choose_item(&mut self, i: usize) {
        let items = self.items();
        let Some(item) = items.get(i).copied() else {
            return;
        };
        self.reader.item = i;
        // Shown, not read: moving through the outline ticks nothing.
        match item {
            Item::Overview => self.show_section(0, false),
            Item::Page(n) => self.show_section(n, false),
            Item::FilesOnly => {}
        }
        self.reader.item = i;
    }

    /// Enter on the outline: read from the chosen entry, the column taking
    /// the keys; on the files no chapter holds, Files mode on them.
    fn read_from_here(&mut self) {
        match self.items().get(self.reader.item) {
            Some(Item::FilesOnly) => self.open_files_only(),
            // What changes reads on from page 1, as it says.
            Some(Item::Overview) => {
                let first = self
                    .told_map()
                    .and_then(|s| s.chapters.first())
                    .and_then(|c| c.pages.first().copied());
                if let Some(n) = first {
                    self.go_to_section(n);
                }
                self.focus = Focus::New;
            }
            Some(_) => {
                self.choose_item(self.reader.item);
                let section = self.section();
                self.mark_read(section);
                self.focus = Focus::New;
            }
            None => {}
        }
    }

    /// ]p and [p in the Story view: the next page's heading at the top of
    /// the column, or the one before's, from what changes on to page 1.
    pub(super) fn page_step(&mut self, step: i32) {
        let here = self.reader.cur;
        let cols = self.column_cols();
        let rows = self.column(cols);
        let found = if step > 0 {
            rows.iter()
                .enumerate()
                .skip(here + 1)
                .find(|(_, r)| r.heading)
                .map(|(i, _)| i)
        } else {
            // The heading of the section before the cursor's, wherever in
            // its section the cursor is; from the first page, what changes.
            let own = rows[..(here + 1).min(rows.len())]
                .iter()
                .rposition(|r| r.heading)
                .unwrap_or(0);
            rows[..own].iter().rposition(|r| r.heading).or(Some(0))
        };
        if let Some(at) = found {
            self.reader.cur = at;
            self.reader.top = at;
            self.follow();
        }
    }

    /// ]c and [c in the Story view: the first page of the next chapter,
    /// or of the one before.
    pub(super) fn chapter_step(&mut self, step: i32) {
        let section = self.section();
        let Some(story) = self.told_map() else {
            return;
        };
        let here = story::chapter_of(story, section).unwrap_or(0) as i32;
        let to = here + step;
        let first = usize::try_from(to)
            .ok()
            .and_then(|c| story.chapters.get(c))
            .and_then(|c| c.pages.first().copied());
        match first {
            Some(n) => self.go_to_section(n),
            None if to < 0 => self.go_to_section(0),
            None => {}
        }
    }

    /// What runs laid over the column, or taken away.
    pub(super) fn toggle_runs(&mut self) {
        let has = self.told_map().is_some_and(|s| !s.runs.is_empty());
        if !has && !self.reader.runs {
            return self.say(
                "This story names nothing that runs: told before What runs was asked for, or a change where nothing runs.",
            );
        }
        self.reader.runs = !self.reader.runs;
        // Its step lit for where the column is, nothing more read.
        if self.reader.runs {
            self.follow_marking(false);
        }
        self.keep_reading();
    }

    /// What runs' step `i` chosen, its page shown.
    pub(super) fn go_to_run(&mut self, i: usize) {
        let Some(page) = self.told_map().and_then(|s| s.runs.get(i)).map(|r| r.page) else {
            return;
        };
        self.go_to_section(page);
        self.reader.run = Some(i);
    }

    // ── The column ──────────────────────────────────────────────────────────

    /// The column's width, inside its margins.
    pub(super) fn column_cols(&self) -> usize {
        (self.diff_area().width as usize).saturating_sub(3).max(10)
    }

    /// The column's rows on the screen, under What runs when it is shown.
    pub(super) fn column_rect(&self) -> Rect {
        let area = self.diff_area();
        let over = if self.reader.runs { RUNS_ROWS } else { 0 };
        Rect::new(
            area.x,
            area.y + over,
            area.width,
            area.height.saturating_sub(over),
        )
    }

    /// The column laid out for `cols`, again only when its width or what
    /// is unfolded has changed.
    pub(super) fn column(&mut self, cols: usize) -> &[Row] {
        let fresh = self
            .reader
            .laid
            .as_ref()
            .is_some_and(|(w, _)| *w as usize == cols && !self.reader.stale);
        if !fresh {
            self.lay_out(cols);
        }
        self.reader
            .laid
            .as_ref()
            .map_or(&[], |(_, rows)| rows.as_slice())
    }

    fn lay_out(&mut self, cols: usize) {
        // Laid out again at another width (the outline widened): the
        // cursor and the view kept on their section, as far into it.
        let before = self.reader.laid.take().map(|(_, rows)| {
            let place = |at: usize| {
                let section = rows.get(at).map_or(0, |r| r.section);
                let start = rows.iter().position(|r| r.section == section).unwrap_or(0);
                (section, at.saturating_sub(start))
            };
            (place(self.reader.cur), place(self.reader.top))
        });
        let rows = self.lay_column(cols);
        if let Some((cur, top)) = before {
            let back = |(section, into): (usize, usize)| {
                let start = rows.iter().position(|r| r.section == section).unwrap_or(0);
                let len = rows.iter().filter(|r| r.section == section).count();
                start + into.min(len.saturating_sub(1))
            };
            self.reader.cur = back(cur);
            self.reader.top = back(top);
        }
        self.reader.laid = Some((cols as u16, rows));
        self.reader.stale = false;
    }

    /// The column laid out again on its next draw.
    pub(super) fn relay(&mut self) {
        self.reader.stale = true;
    }

    /// The cursor kept on a line and in sight.
    fn keep_reading(&mut self) {
        let cols = self.column_cols();
        let n = self.column(cols).len();
        let h = self.column_rect().height as usize;
        let r = &mut self.reader;
        r.cur = r.cur.min(n.saturating_sub(1));
        if r.cur < r.top {
            r.top = r.cur;
        } else if h > 0 && r.cur >= r.top + h {
            r.top = r.cur + 1 - h;
        }
        r.top = r.top.min(n.saturating_sub(1));
    }

    /// A press or a drag on the column's scroll bar at `row`: its view
    /// there, the cursor kept in it.
    pub(super) fn scroll_story_to(&mut self, row: usize, drag: Option<kit::Grab>) {
        let total = self.column(self.column_cols()).len();
        let h = self.column_rect().height as usize;
        let top = self.reader.top;
        self.reader.top = match drag {
            Some(grab) => kit::scroll_drag(total, h, h, row, grab),
            None => kit::scroll_press(total, top, h, h, row).unwrap_or(top),
        };
        let r = &mut self.reader;
        r.cur = r
            .cur
            .clamp(r.top, (r.top + h.max(1) - 1).min(total.saturating_sub(1)));
        self.follow();
    }

    fn move_reading(&mut self, step: isize) {
        let cols = self.column_cols();
        let n = self.column(cols).len();
        let to = (self.reader.cur as isize + step).clamp(0, n.saturating_sub(1) as isize);
        self.reader.cur = to as usize;
        self.keep_reading();
        self.follow();
    }

    /// The column: what changes, then each chapter's pages in reading
    /// order, as boards 140 and 141 draw them.
    fn lay_column(&self, cols: usize) -> Vec<Row> {
        let Some(story) = self.told_map() else {
            return Vec::new();
        };
        let theme = self.theme();
        let mut rows = self.overview_rows(story, cols, &theme);
        let told = self.change.as_ref().map(|c| c.told_part());
        let (files, hunks) = told.map(|c| story::split(&c.runs())).unwrap_or_default();
        for chapter in &story.chapters {
            for &n in &chapter.pages {
                rows.extend(self.page_rows(story, n, cols, (&files, &hunks), &theme));
            }
        }
        rows
    }

    /// What changes, as board 140 draws it: the story's name and size over
    /// a rule, Before and After side by side (one over the other where
    /// the column is narrow), how it gets there, the chapters besides the
    /// first, What runs, and what it was told from.
    fn overview_rows(&self, story: &Story, cols: usize, theme: &Theme) -> Vec<Row> {
        let mut rows: Vec<Row> = Vec::new();
        let plain = |spans: Vec<Span<'static>>| Row {
            spans,
            section: 0,
            does: Does::Nothing,
            heading: false,
            bg: None,
        };
        let pages = story.chapters.iter().map(|c| c.pages.len()).sum::<usize>();
        let name = story
            .chapters
            .first()
            .map(|c| c.title.clone())
            .unwrap_or_default();
        let (added, deleted) = (self.totals.added, self.totals.deleted);
        let right = self
            .change
            .as_ref()
            .map(|c| c.against())
            .unwrap_or_default();
        let left = vec![
            Span::styled(name, Tone::Text.bold(theme)),
            Span::styled(
                format!(
                    "  a story in {}, {} · +{added} -{deleted}",
                    counted(story.chapters.len(), "chapter"),
                    counted(pages, "page"),
                ),
                Tone::Dim.style(theme),
            ),
        ];
        let room = cols.saturating_sub(width(&right) + 1);
        let mut first = kit::fit(left, room, Style::new());
        first.push(Span::raw(" "));
        first.push(Span::styled(right, Tone::Dim.style(theme)));
        rows.push(Row {
            heading: true,
            ..plain(first)
        });
        rows.push(plain(vec![kit::rule(cols, theme)]));
        rows.push(plain(Vec::new()));
        rows.push(plain(vec![Span::styled(
            "WHAT CHANGES",
            Tone::Accent.bold(theme),
        )]));
        rows.push(plain(Vec::new()));
        if story.before.is_empty() && story.after.is_empty() {
            for line in wrap(
                "This story was told before the diff said what changes: the menu's Tell it again tells it anew.",
                cols,
            ) {
                rows.push(plain(vec![Span::styled(line, Tone::Dim.style(theme))]));
            }
        } else {
            for spans in sides_boxes(&story.before, &story.after, cols, theme) {
                rows.push(plain(spans));
            }
        }
        rows.push(plain(Vec::new()));
        rows.push(plain(vec![Span::styled(
            "HOW IT GETS THERE",
            Tone::Dim.bold(theme),
        )]));
        if let Some(chapter) = story.chapters.first() {
            for &n in &chapter.pages {
                let Some(page) = story.pages.get(n - 1) else {
                    continue;
                };
                let (_, k, _) = story::page_number(story, n);
                let number = format!("{k}  ");
                rows.push(plain(kit::fit(
                    vec![
                        Span::styled(number.clone(), Tone::Accent.bold(theme)),
                        Span::styled(page.title.clone(), Tone::Text.bold(theme)),
                    ],
                    cols,
                    Style::new(),
                )));
                let why = first_sentence(if page.why.is_empty() {
                    &page.claim
                } else {
                    &page.why
                });
                let indent = " ".repeat(width(&number) + 2);
                for line in wrap(&why, cols.saturating_sub(indent.len()).max(10)) {
                    rows.push(plain(vec![
                        Span::raw(indent.clone()),
                        Span::styled(line, Tone::Dim.style(theme)),
                    ]));
                }
                rows.push(plain(Vec::new()));
            }
        }
        for chapter in story.chapters.iter().skip(1) {
            let titles: Vec<String> = chapter
                .pages
                .iter()
                .filter_map(|&n| story.pages.get(n - 1))
                .map(|p| p.title.to_lowercase())
                .collect();
            rows.push(plain(kit::fit(
                vec![
                    Span::styled("Also in this change: ", Tone::Dim.style(theme)),
                    Span::styled(chapter.title.clone(), Tone::Accent.style(theme)),
                    Span::styled(format!("  {}", titles.join("; ")), Tone::Dim.style(theme)),
                ],
                cols,
                Style::new(),
            )));
        }
        if !story.runs.is_empty() {
            rows.push(plain(Vec::new()));
            rows.push(Row {
                does: Does::Runs,
                ..plain(vec![
                    Span::styled("What runs ▸ ", Tone::Accent.bold(theme)),
                    Span::styled(
                        format!(
                            "{} in the order they happen",
                            counted(story.runs.len(), "step")
                        ),
                        Tone::Dim.style(theme),
                    ),
                ])
            });
        }
        rows.push(plain(Vec::new()));
        // What it was told from, as kept with it; unsaid for one kept before.
        let from = match story.from_plan {
            Some(true) => "From the plan note and the commits. ",
            Some(false) => "From the commits and the diff. ",
            None => "",
        };
        let model = if story.model.is_empty() {
            String::new()
        } else {
            format!("Told by {}. ", story::model_name(&story.model))
        };
        // Wrapped, so a narrow column cuts neither what it was told from
        // nor which model told it.
        for line in wrap(format!("{from}{model}").trim_end(), cols) {
            if !line.is_empty() {
                rows.push(plain(vec![Span::styled(line, Tone::Faint.style(theme))]));
            }
        }
        // A row of its own, so a narrow column never cuts it.
        rows.push(Row {
            does: Does::FirstPage,
            ..plain(vec![
                Span::styled("Enter", Tone::Accent.style(theme)),
                Span::styled(" reads from 1.", Tone::Faint.style(theme)),
            ])
        });
        rows.push(plain(Vec::new()));
        rows
    }

    /// Page `n` as a section of the column, as board 141 draws it: its
    /// number and title with where it is at the right, its claim, the
    /// block of what it Decided, Instead of what and what to Watch, then
    /// each file it changes with its changes inline, files of tests
    /// folded last, and a rule.
    fn page_rows(
        &self,
        story: &Story,
        n: usize,
        cols: usize,
        (files, hunks): (&story::Files, &story::Hunks),
        theme: &Theme,
    ) -> Vec<Row> {
        let Some(page) = story.pages.get(n - 1) else {
            return Vec::new();
        };
        let mut rows: Vec<Row> = Vec::new();
        let plain = |spans: Vec<Span<'static>>| Row {
            spans,
            section: n,
            does: Does::Nothing,
            heading: false,
            bg: None,
        };
        let (chapter, k, m) = story::page_number(story, n);
        let right = match chapter {
            Some(c) if story.chapters.len() > 1 => format!("page {k} of {m} · chapter {}", c + 1),
            _ => format!("page {k} of {m}"),
        };
        // The prose keeps to `PROSE` columns, as board 144 lays it out;
        // the changes under it take the column's whole width.
        let prose = cols.min(PROSE);
        let room = prose.saturating_sub(width(&right) + 1);
        let mut head = kit::fit(
            vec![
                Span::styled(format!("{k}  "), Tone::Accent.bold(theme)),
                Span::styled(page.title.clone(), Tone::Text.bold(theme)),
            ],
            room,
            Style::new(),
        );
        head.push(Span::raw(" "));
        head.push(Span::styled(right, Tone::Dim.style(theme)));
        rows.push(Row {
            heading: true,
            ..plain(head)
        });
        // The claim; a story told before claims were asked for has its why.
        let claim = if page.claim.is_empty() {
            page.why.clone()
        } else {
            page.claim.clone()
        };
        for line in wrap(&claim, prose) {
            rows.push(plain(vec![Span::styled(line, Tone::Text.style(theme))]));
        }
        let block: Vec<(&str, &String, Tone)> = [
            ("Decided", &page.decided, Tone::Done),
            ("Instead", &page.instead, Tone::Dim),
            ("Watch", &page.watch, Tone::Needs),
        ]
        .into_iter()
        .filter(|(_, text, _)| !text.is_empty())
        .collect();
        if !block.is_empty() {
            rows.push(plain(Vec::new()));
            let sunken = Surface::Sunken.color(theme);
            for (label, text, tone) in block {
                let label_style = if tone == Tone::Dim {
                    tone.style(theme)
                } else {
                    tone.bold(theme)
                };
                let text_tone = if label == "Instead" {
                    Tone::Dim
                } else {
                    Tone::Text
                };
                for (i, line) in wrap(text, prose.saturating_sub(10).max(10))
                    .into_iter()
                    .enumerate()
                {
                    let lead = if i == 0 {
                        Span::styled(format!(" {label:<8} "), label_style)
                    } else {
                        Span::raw(" ".repeat(10))
                    };
                    rows.push(Row {
                        bg: Some(sunken),
                        ..plain(vec![lead, Span::styled(line, text_tone.style(theme))])
                    });
                }
            }
        }
        rows.push(plain(Vec::new()));
        // Each file in the order the page touches them, tests last.
        let mut paths: Vec<&String> = page.touches.iter().collect();
        if paths.is_empty() {
            paths = page.files.iter().collect();
        }
        paths.sort_by_key(|p| order::kind(p) == order::Kind::Tests);
        let (removed, _, added, _) = self.palette();
        // A path listed twice (staged, and changed since) is a file of
        // each diff, its header from that diff's own entry.
        let mut files_shown: Vec<(&String, usize)> = Vec::new();
        for path in paths {
            let mut runs: Vec<usize> = Vec::new();
            for h in story.hunks.get(path).map(Vec::as_slice).unwrap_or(&[]) {
                let here = h.page == n || story.repeats.get(&h.id).is_some_and(|c| c.contains(&n));
                if here && !runs.contains(&h.run) {
                    runs.push(h.run);
                }
            }
            // A file with no hunk (a binary one) is each diff's that lists it.
            if runs.is_empty()
                && let Some(c) = &self.change
            {
                for e in c.told_part().entries.iter().filter(|e| e.path == *path) {
                    let r = self.run_of(e).unwrap_or(1);
                    if !runs.contains(&r) {
                        runs.push(r);
                    }
                }
            }
            if runs.is_empty() {
                runs.push(1);
            }
            files_shown.extend(runs.into_iter().map(|r| (path, r)));
        }
        for (path, run) in files_shown {
            let notes: Vec<&story::HunkNote> = story
                .hunks
                .get(path)
                .map(Vec::as_slice)
                .unwrap_or(&[])
                .iter()
                .filter(|h| h.run == run)
                .filter(|h| h.page == n || story.repeats.get(&h.id).is_some_and(|c| c.contains(&n)))
                .collect();
            // The told part's entry, whose diff the column shows, of the
            // hunks' own diff where a path is listed twice.
            let entry = self.change.as_ref().and_then(|c| {
                c.told_part()
                    .entries
                    .iter()
                    .find(|e| e.path == *path && self.run_of(e).is_none_or(|r| r == run))
            });
            let status = match entry.map(|e| e.status) {
                Some('A') => "new file",
                Some('D') => "deleted",
                Some('R') => "renamed",
                _ => "",
            };
            let (a, d) = entry.map_or((0, 0), |e| (e.additions, e.deletions));
            let tests = order::kind(path) == order::Kind::Tests;
            let folded = tests && !self.reader.unfolded.contains(&(n, path.clone()));
            let mut left = vec![
                Span::styled(path.clone(), Tone::Text.bold(theme)),
                Span::styled(format!("   +{a}"), Tone::Done.style(theme)),
                Span::styled(format!(" -{d}"), Tone::Failed.style(theme)),
            ];
            if folded {
                left.push(Span::styled(
                    "   ▸ folded: tests last",
                    Tone::Faint.style(theme),
                ));
            }
            let room = cols.saturating_sub(width(status) + 1);
            let mut header = kit::fit(left, room, Style::new());
            header.push(Span::raw(" "));
            header.push(Span::styled(status, Tone::Dim.style(theme)));
            rows.push(Row {
                does: if folded {
                    Does::Folded {
                        page: n,
                        path: path.clone(),
                    }
                } else {
                    Does::File {
                        path: path.clone(),
                        run,
                    }
                },
                ..plain(header)
            });
            if folded {
                rows.push(plain(Vec::new()));
                continue;
            }
            let text = files.get(path).map(Vec::as_slice).unwrap_or(&[]);
            for note in notes {
                let Some(hunk) = hunks
                    .get(path)
                    .and_then(|list| list.iter().find(|h| h.id == note.id))
                else {
                    continue;
                };
                if note.page != n
                    && let Some(home) = story.pages.get(note.page.wrapping_sub(1))
                {
                    let from = story::chapter_of(story, n);
                    rows.push(plain(kit::fit(
                        vec![
                            Span::styled(" ↗ ", Tone::Accent.style(theme)),
                            Span::styled(
                                format!(
                                    "also {}: {}",
                                    story::in_page(story, note.page, from),
                                    home.title
                                ),
                                Tone::Dim.style(theme),
                            ),
                        ],
                        cols,
                        Style::new(),
                    )));
                } else if self.opts.notes && !note.note.is_empty() {
                    rows.push(plain(kit::fit(
                        vec![Span::styled(
                            format!(" ‣ {}", note.note),
                            Tone::Dim.style(theme),
                        )],
                        cols,
                        Style::new(),
                    )));
                }
                let lines = hunk_lines(text, hunk);
                // Side by side where the column has room, as Files mode
                // shows a change; one column, removed over added, where not.
                if cols >= SIDE_BY_SIDE {
                    let target = |new: Option<usize>, old: Option<usize>| Does::Change {
                        path: path.clone(),
                        run: hunk.run,
                        new,
                        old,
                    };
                    for (spans, new, old) in side_by_side(&lines, cols, self.palette(), theme) {
                        rows.push(Row {
                            does: target(new, old),
                            ..plain(spans)
                        });
                    }
                }
                for (mark, line, old, new) in lines.into_iter().filter(|_| cols < SIDE_BY_SIDE) {
                    if mark == '\\' {
                        rows.push(plain(vec![Span::styled(
                            format!("      ⏎ {line}"),
                            Tone::Faint.style(theme),
                        )]));
                        continue;
                    }
                    let number = match mark {
                        '-' => old,
                        _ => new,
                    }
                    .map_or(String::new(), |l| l.to_string());
                    let bg = match mark {
                        '+' => Some(added),
                        '-' => Some(removed),
                        _ => Some(Surface::Sunken.color(theme)),
                    };
                    let text = line.replace('\t', "    ");
                    // A long line goes on in rows of its own under it, so
                    // a change near its end is read here too.
                    for (k, piece) in pieces(&text, cols.saturating_sub(8))
                        .into_iter()
                        .enumerate()
                    {
                        let number = if k == 0 { number.as_str() } else { "" };
                        rows.push(Row {
                            bg,
                            does: Does::Change {
                                path: path.clone(),
                                run: hunk.run,
                                new,
                                old,
                            },
                            ..plain(vec![
                                Span::styled(format!("{number:>5} "), Tone::Faint.style(theme)),
                                // One column marks each line, as board 145.
                                Span::styled(
                                    format!("{} ", if k == 0 { mark } else { ' ' }),
                                    match mark {
                                        '+' => Tone::Done.style(theme),
                                        '-' => Tone::Failed.style(theme),
                                        _ => Tone::Faint.style(theme),
                                    },
                                ),
                                Span::styled(piece, Tone::Text.style(theme)),
                            ])
                        });
                    }
                }
                // A new file the diff gives only the start of (one not
                // yet tracked, cut for what is sent): the rest is the
                // files' to show, said rather than left out.
                let more = a.saturating_sub(hunk.new_count);
                if entry.is_some_and(|e| e.status == 'A') && hunk.old_count == 0 && more > 0 {
                    rows.push(Row {
                        does: Does::File {
                            path: path.clone(),
                            run: hunk.run,
                        },
                        ..plain(vec![Span::styled(
                            format!("      … {} more, shown in the files", counted(more, "line")),
                            Tone::Faint.style(theme),
                        )])
                    });
                }
            }
            rows.push(plain(Vec::new()));
        }
        rows.push(plain(vec![kit::rule(cols, theme)]));
        rows.push(plain(Vec::new()));
        rows
    }

    // ── Doing what a line does ──────────────────────────────────────────────

    /// Enter on the column's cursor line, or a click on it: a change's
    /// menu, a file of tests unfolded, What runs shown.
    fn act_on_line(&mut self, near: Option<(u16, u16)>) {
        let Some(row) = self.row_at_cur() else {
            return;
        };
        match row.does {
            Does::Nothing => {}
            Does::Runs => self.toggle_runs(),
            Does::FirstPage => {
                let first = self
                    .told_map()
                    .and_then(|s| s.chapters.first())
                    .and_then(|c| c.pages.first().copied());
                if let Some(n) = first {
                    self.go_to_section(n);
                }
            }
            Does::Folded { page, path } => {
                self.reader.unfolded.insert((page, path));
                self.relay();
            }
            target @ (Does::File { .. } | Does::Change { .. }) => {
                self.reader.target = Some(target);
                self.open_line_menu(near);
            }
        }
    }

    /// The change a menu was opened on, shown in the files: its file, the
    /// cursor on its line.
    pub(super) fn show_target(&mut self) {
        let Some(target) = self.reader.target.take() else {
            return;
        };
        let (path, run, new, old) = match target {
            Does::File { path, run } => (path, run, None, None),
            Does::Change {
                path,
                run,
                new,
                old,
            } => (path, run, new, old),
            _ => return,
        };
        if self.mode != Mode::Files {
            self.files_place = Some(self.place());
        }
        self.whole_files();
        // A file both staged and changed since is listed twice: the one
        // whose diff the change is from.
        // The entry of the change's own diff, else, for a file with no
        // change to place it (a binary one), the one listing the path.
        let at = self
            .order
            .iter()
            .position(|&i| {
                let entry = &self.entries()[i];
                entry.path == path && self.run_of(entry).is_none_or(|r| r == run)
            })
            .or_else(|| {
                let whole = new.is_none() && old.is_none();
                self.order
                    .iter()
                    .position(|&i| whole && self.entries()[i].path == path)
            });
        let Some(at) = at else {
            return self.say(format!("{path} isn't among the files listed"));
        };
        self.moved = true;
        self.focus = Focus::List;
        self.show_at(at);
        if new.is_none() && old.is_none() {
            return;
        }
        self.focus = self.main_side();
        // A line only the old side has is at the commits' own lines, which
        // edits since never move; a new side's line may have moved.
        // The side the row shows its number from: the new one, but for a
        // line only the old side has.
        let line = |l: Option<usize>| l.map_or((0, 0), |l| (l, 1));
        // A line added or taken out is found only where it is still one.
        let changed = !(new.is_some() && old.is_some());
        let found = if new.is_some() {
            self.at_hunk_changed(line(new), (0, 0), changed)
        } else {
            self.at_hunk_changed((0, 0), line(old), changed)
        };
        if !found {
            return self.say(
                "That line isn't among the file's changes as it is now: shown at its first change.",
            );
        }
        // Edits since the commits may have moved a new side's line: shown
        // at its number, which edits below it keep, and said.
        if new.is_some() && self.current().is_some_and(|e| self.changed_since(e)) {
            self.say("This file has changed since its commits: the line is shown at its number, which edits above it may have moved.");
        }
        // A line taken out is the old side's, whatever faces it.
        if new.is_none() && old.is_some() && self.side_widths().0 > 0 {
            self.focus = Focus::Old;
        }
    }

    /// The column's lines selected with `v`, their text copied.
    fn copy_reading(&mut self) {
        let Some(from) = self.reader.visual.take() else {
            return;
        };
        let cur = self.reader.cur;
        let cols = self.column_cols();
        let text: Vec<String> = self
            .column(cols)
            .iter()
            .skip(from.min(cur))
            .take(from.abs_diff(cur) + 1)
            .map(|r| {
                r.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .map(|l| l.trim_end().to_string())
            .collect();
        let n = text.len();
        self.clip(&text.join("\n"));
        self.say(format!("Copied {}", counted(n, "line")));
    }

    /// The change a menu was opened on, its place copied as path:line.
    pub(super) fn copy_target(&mut self) {
        let Some(target) = self.reader.target.take() else {
            return;
        };
        let place = match target {
            Does::File { path, .. } => path,
            // A line only the old side has is at its old path (a rename's).
            Does::Change {
                path,
                new,
                old,
                run,
            } => match (new, old) {
                (Some(line), _) => format!("{path}:{line}"),
                (None, Some(line)) => {
                    // The entry of the line's own diff, where a file is
                    // listed twice (staged, and changed since).
                    // From the told part, whose diff the column shows.
                    let old_path =
                        self.change
                            .as_ref()
                            .and_then(|c| {
                                c.told_part().entries.iter().find(|e| {
                                    e.path == path && self.run_of(e).is_none_or(|r| r == run)
                                })
                            })
                            .map_or(path.clone(), |e| e.old().to_string());
                    format!("{old_path}:{line}")
                }
                (None, None) => path,
            },
            _ => return,
        };
        self.clip(&place);
        self.say(format!("Copied {place}"));
    }

    /// Where the column's cursor is, for a question: the change under it,
    /// while the column has the keys.
    pub(super) fn reading_cursor(&mut self) -> crate::ask::Cursor {
        let row = self.row_at_cur();
        match row.map(|r| r.does) {
            Some(Does::Change { path, new, old, .. }) if self.focus != Focus::List => {
                crate::ask::Cursor {
                    path: Some(path),
                    line: new.or(old),
                    old_side: new.is_none(),
                    hunk: None,
                }
            }
            Some(Does::File { path, .. }) if self.focus != Focus::List => crate::ask::Cursor {
                path: Some(path),
                ..crate::ask::Cursor::default()
            },
            _ => crate::ask::Cursor::default(),
        }
    }

    // ── The keys ────────────────────────────────────────────────────────────

    /// A key in the Story view; `None` for one the diff takes anywhere
    /// (closing, the first key of a pair, reading the change again, the
    /// commits, C-w).
    pub(super) fn story_key(&mut self, key: KeyEvent, count: usize) -> Option<Act> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let outline = self.focus == Focus::List;
        let page = self.column_rect().height.max(1) as isize;
        let count = count as isize;
        // ] and [ alone turn the pages: a p or c typed after one from the
        // habit of ]p and ]c adds nothing (c would open the commits).
        let after_bracket = std::mem::take(&mut self.reader.after_bracket);
        if after_bracket && matches!(key.code, KeyCode::Char('p' | 'c')) && !ctrl {
            return Some(Act::Same);
        }
        match key.code {
            KeyCode::Char(']') if !ctrl => {
                self.reader.after_bracket = true;
                if self.opts.pages_alone {
                    self.page_step(1);
                }
            }
            KeyCode::Char('[') if !ctrl => {
                self.reader.after_bracket = true;
                if self.opts.pages_alone {
                    self.page_step(-1);
                }
            }
            // v and V select the column's lines, y copies their text, as
            // a drag of the mouse does.
            KeyCode::Char('v' | 'V') if !outline && !ctrl => {
                self.reader.visual = match self.reader.visual {
                    Some(_) => None,
                    None => Some(self.reader.cur),
                };
            }
            KeyCode::Char('y') if !ctrl && !outline && self.reader.visual.is_some() => {
                self.copy_reading();
            }
            KeyCode::Esc if !outline && self.reader.visual.is_some() => self.reader.visual = None,
            KeyCode::Char('q' | 'y' | 'g' | 'R' | 'c' | 'E') if !ctrl => return None,
            KeyCode::Char('w') if ctrl => return None,
            KeyCode::F(10) if key.modifiers == KeyModifiers::SHIFT => return None,
            KeyCode::Char('m') if key.modifiers.is_empty() => self.open_menu(None),
            KeyCode::Esc if self.reader.runs => self.toggle_runs(),
            KeyCode::Esc if outline => return Some(Act::Quit),
            KeyCode::Esc | KeyCode::Tab | KeyCode::BackTab if !outline => self.focus = Focus::List,
            KeyCode::Tab | KeyCode::BackTab => self.focus = Focus::New,
            KeyCode::Char('h') | KeyCode::Left if self.reader.runs && !ctrl => {
                self.go_to_run(self.reader.run.map_or(0, |r| r.saturating_sub(1)));
            }
            KeyCode::Char('l') | KeyCode::Right if self.reader.runs && !ctrl => {
                let last = self
                    .told_map()
                    .map_or(0, |s| s.runs.len().saturating_sub(1));
                self.go_to_run(self.reader.run.map_or(0, |r| (r + 1).min(last)));
            }
            KeyCode::Char('h') | KeyCode::Left if !outline && !ctrl => self.focus = Focus::List,
            KeyCode::Char('l') | KeyCode::Right if outline && !ctrl => self.focus = Focus::New,
            // J and K from the outline scroll the column beside it, as they
            // scroll the file from the file list.
            KeyCode::Char('J') if outline => {
                self.scroll_column(super::SCROLL * count as usize, true, false)
            }
            KeyCode::Char('K') if outline => {
                self.scroll_column(super::SCROLL * count as usize, false, false)
            }
            KeyCode::Char('j') | KeyCode::Down if outline && !ctrl => {
                let last = self.items().len().saturating_sub(1);
                self.choose_item((self.reader.item + count as usize).min(last));
            }
            KeyCode::Char('k') | KeyCode::Up if outline && !ctrl => {
                self.choose_item(self.reader.item.saturating_sub(count as usize));
            }
            KeyCode::Char('G') | KeyCode::End if outline => {
                self.choose_item(self.items().len().saturating_sub(1));
            }
            KeyCode::Home if outline => self.choose_item(0),
            KeyCode::Enter | KeyCode::Char('o' | ' ') if outline && !ctrl => {
                self.read_from_here();
            }
            KeyCode::Char('j') | KeyCode::Down if !ctrl => self.move_reading(count),
            KeyCode::Char('k') | KeyCode::Up if !ctrl => self.move_reading(-count),
            KeyCode::Char('d') if ctrl => self.move_reading(page / 2),
            KeyCode::Char('u') if ctrl => self.move_reading(-(page / 2)),
            KeyCode::Char('f') if ctrl => self.move_reading(page),
            KeyCode::Char('b') if ctrl => self.move_reading(-page),
            KeyCode::PageDown => self.move_reading(page),
            KeyCode::PageUp => self.move_reading(-page),
            KeyCode::Char('G') | KeyCode::End => self.move_reading(isize::MAX / 2),
            KeyCode::Home => self.move_reading(isize::MIN / 2),
            KeyCode::Enter | KeyCode::Char('o' | ' ') if !ctrl => self.act_on_line(None),
            _ => return Some(Act::Same),
        }
        Some(Act::Redraw)
    }

    /// The second key of a pair in the Story view.
    pub(super) fn story_pair(&mut self, first: char, c: char) {
        match (first, c) {
            ('g', 'm') => self.open_map_view(),
            ('g', 'f') => self.show_files(false),
            ('g', 'g') if self.focus == Focus::List => self.choose_item(0),
            ('g', 'g') => self.move_reading(isize::MIN / 2),
            ('g', 'a') if self.opts.ask => self.open_ask(),
            ('y', 'S') => self.copy_story(),
            ('y', 'l') => self.say(
                "A link to a line is the files' to copy: Enter on the change, Show it in the files, then yl.",
            ),
            ('y', 'p') => {
                let row = self.row_at_cur();
                match row.map(|r| r.does) {
                    Some(target @ (Does::File { .. } | Does::Change { .. }))
                        if self.focus != Focus::List =>
                    {
                        self.reader.target = Some(target);
                        self.copy_target();
                    }
                    _ => self.say(
                        "No change under the cursor: Enter on one in the column copies its place.",
                    ),
                }
            }
            ('w', 'w') => {
                self.focus = if self.focus == Focus::List {
                    Focus::New
                } else {
                    Focus::List
                };
            }
            ('w', dir @ ('h' | 'j' | 'k' | 'l')) => self.window_move(dir),
            _ => {}
        }
    }

    /// The wheel over the Story view: the outline moves its entry, the
    /// column and What runs scroll.
    pub(super) fn story_wheel(&mut self, x: u16, down: bool) {
        if x < self.list_cols() {
            let last = self.items().len().saturating_sub(1);
            let to = if down {
                (self.reader.item + 1).min(last)
            } else {
                self.reader.item.saturating_sub(1)
            };
            return self.choose_item(to);
        }
        self.scroll_column(3, down, true);
    }

    /// The column scrolled `by` lines, its cursor kept in sight; the page
    /// it lands on read with `read`, never while the outline has the keys.
    fn scroll_column(&mut self, by: usize, down: bool, read: bool) {
        let cols = self.column_cols();
        let n = self.column(cols).len();
        let h = self.column_rect().height as usize;
        let r = &mut self.reader;
        r.top = if down {
            (r.top + by).min(n.saturating_sub(1))
        } else {
            r.top.saturating_sub(by)
        };
        r.cur = r
            .cur
            .clamp(r.top, (r.top + h.max(1) - 1).min(n.saturating_sub(1)));
        self.follow_marking(read);
    }

    // ── Clicks ──────────────────────────────────────────────────────────────

    /// What of the Story view is at a cell of the screen.
    pub(super) fn hit_at(&self, x: u16, y: u16) -> Option<Hit> {
        self.hits
            .iter()
            .find(|(_, r)| x >= r.x && x < r.right() && y >= r.y && y < r.bottom())
            .map(|(hit, _)| *hit)
    }

    /// A press on something of the Story view: an entry of the outline
    /// is chosen, and read from when it already was; a line of the column
    /// takes the cursor, and does what it does when it already had it; a
    /// step of What runs shows its page. What a second press does waits
    /// for the mouse to be let go, so a drag selects instead.
    pub(super) fn story_press(&mut self, hit: Hit) -> Act {
        match hit {
            Hit::Item(i) => {
                self.focus = Focus::List;
                if i == self.reader.item {
                    self.open_on_up = Some(hit);
                } else {
                    self.choose_item(i);
                }
            }
            Hit::Line(l) => {
                self.focus = Focus::New;
                if l == self.reader.cur {
                    self.open_on_up = Some(hit);
                } else {
                    self.reader.cur = l;
                    self.follow();
                }
            }
            Hit::Run(i) => self.go_to_run(i),
            Hit::Page(_) | Hit::Chapter(_) | Hit::FilesOnly => return self.map_click(hit),
        }
        self.read_if_reading();
        Act::Redraw
    }

    /// A click on the column's row `row` from its top: the cursor there,
    /// and the page it is in read.
    pub(super) fn click_reading_row(&mut self, row: usize) {
        let n = self.column(self.column_cols()).len();
        let at = (self.reader.top + row).min(n.saturating_sub(1));
        self.reader.cur = at;
        self.follow();
    }

    /// The mouse let go where it was pressed, with nothing selected.
    pub(super) fn story_release(&mut self, hit: Hit, at: (u16, u16)) {
        match hit {
            Hit::Item(_) => self.read_from_here(),
            Hit::Line(_) => self.act_on_line(Some(at)),
            Hit::Run(_) | Hit::Chapter(_) | Hit::FilesOnly => {}
            Hit::Page(n) => self.read_page(n),
        }
    }

    // ── Drawing ─────────────────────────────────────────────────────────────

    /// The Story view's screen: the outline at the left where the file
    /// list is, the column beside it, What runs over the column when it
    /// is shown, and the pane without the keys greyed.
    pub(super) fn draw_story(&mut self, buf: &mut Buffer) {
        let theme = self.theme();
        let area = self.diff_area();
        let list = Rect::new(0, 0, self.list_cols(), area.height);
        self.draw_outline(buf, list, &theme);
        for y in area.y..area.bottom() {
            put_line(buf, list.right(), y, 1, &Spans::from(kit::rule_v(&theme)));
        }
        if self.reader.runs {
            self.draw_runs(buf, &theme);
        }
        self.draw_column(buf, &theme);
        // The column shows what the outline has chosen, a list's preview,
        // so only the outline is greyed, while the column has the keys.
        if self.focus != Focus::List {
            kit::unfocused(buf, list, &theme);
        }
    }

    /// The outline, as boards 140 to 142 draw it: the first chapter's
    /// name and the files over it, each chapter with What changes and its
    /// pages indented under it, ✓ before each read, the chosen one on the
    /// highlight with the accent's bar, then the files no chapter holds
    /// and the Read meter.
    fn draw_outline(&mut self, buf: &mut Buffer, list: Rect, theme: &Theme) {
        let Some(story) = self.told_map().cloned() else {
            return;
        };
        let cols = list.width as usize;
        let name = story
            .chapters
            .first()
            .map(|c| c.title.clone())
            .unwrap_or_default();
        let mut lines: Vec<OutlineRow> = vec![
            (
                kit::label(&name, Tone::Dim, Some(self.totals.files), cols, theme)
                    .into_iter()
                    .map(|s| Span::styled(s.content.into_owned(), s.style))
                    .collect(),
                None,
            ),
            (Vec::new(), None),
        ];
        let items = self.items();
        let item_of = |item: Item| items.iter().position(|i| *i == item);
        let tick = |read: bool| if read { "✓ " } else { "  " };
        for (c, chapter) in story.chapters.iter().enumerate() {
            if c > 0 {
                lines.push((Vec::new(), None));
            }
            lines.push((
                vec![
                    Span::raw(" "),
                    Span::styled(format!("{} ", c + 1), Tone::Dim.style(theme)),
                    Span::styled(chapter.title.clone(), Tone::Text.style(theme)),
                ],
                None,
            ));
            if c == 0 {
                let read = self.read.contains(&0);
                lines.push((
                    vec![
                        Span::styled(format!("   {}", tick(read)), Tone::Done.style(theme)),
                        Span::styled("What changes", Tone::Text.style(theme)),
                    ],
                    item_of(Item::Overview),
                ));
            }
            for &n in &chapter.pages {
                let title = story.pages.get(n - 1).map_or("", |p| p.title.as_str());
                let (_, k, _) = story::page_number(&story, n);
                let read = self.read.contains(&n);
                lines.push((
                    vec![
                        Span::styled(format!("   {}", tick(read)), Tone::Done.style(theme)),
                        Span::styled(format!("{k} "), Tone::Dim.style(theme)),
                        Span::styled(title.to_string(), Tone::Text.style(theme)),
                    ],
                    item_of(Item::Page(n)),
                ));
            }
        }
        let (_, only) = self.files_only(&story);
        if only > 0 {
            lines.push((Vec::new(), None));
            lines.push((
                vec![Span::styled(
                    format!(" {} · {only}", super::FILES_ONLY),
                    Tone::Dim.style(theme),
                )],
                item_of(Item::FilesOnly),
            ));
        }
        let (read, of) = self.read_count();
        let mut foot: Vec<OutlineRow> = Vec::new();
        let count = format!("{read} of {of}");
        foot.push((
            vec![
                Span::raw(" "),
                Span::styled("READ", Tone::Done.bold(theme)),
                Span::raw(" ".repeat(cols.saturating_sub(width(&count) + 7))),
                Span::styled(count, Tone::Done.style(theme)),
            ],
            None,
        ));
        let share = (read * 100).checked_div(of).unwrap_or(0);
        let mut meter = vec![Span::raw(" ")];
        meter.extend(kit::meter(
            share as u8,
            cols.saturating_sub(3),
            Tone::Done,
            theme,
        ));
        foot.push((meter, None));
        // A long outline scrolls to keep the chosen entry in sight, the Read
        // meter kept at its foot; a short one has the meter two rows under it.
        let height = list.height as usize;
        let rows: Vec<(usize, OutlineRow)> = if lines.len() + 4 <= height {
            self.reader.outline_top = 0;
            lines.push((Vec::new(), None));
            lines.push((Vec::new(), None));
            lines.extend(foot);
            lines.into_iter().enumerate().collect()
        } else {
            let room = height.saturating_sub(3).max(1);
            let at = lines
                .iter()
                .position(|(_, item)| *item == Some(self.reader.item))
                .unwrap_or(0);
            let top = &mut self.reader.outline_top;
            if at < *top {
                *top = at;
            } else if at >= *top + room {
                *top = at + 1 - room;
            }
            *top = (*top).min(lines.len().saturating_sub(room));
            let top = *top;
            let mut rows: Vec<_> = lines.into_iter().skip(top).take(room).enumerate().collect();
            let from = height.saturating_sub(foot.len());
            rows.extend(foot.into_iter().enumerate().map(|(i, l)| (from + i, l)));
            rows
        };
        for (y, (spans, item)) in rows {
            let y = y as u16;
            if y >= list.height {
                break;
            }
            let row = Rect::new(list.x, list.y + y, list.width, 1);
            let chosen = item.is_some_and(|i| i == self.reader.item);
            let lit = item.is_some_and(|i| self.lit == Some(Hit::Item(i)));
            if chosen || lit {
                let bg = if chosen {
                    Surface::Chosen.color(theme)
                } else {
                    Surface::Raised.color(theme)
                };
                buf.set_style(row, Style::new().bg(bg));
            }
            let spans: Vec<Span<'static>> = if chosen {
                spans
                    .into_iter()
                    .map(|s| {
                        let style = s.style.add_modifier(Modifier::BOLD);
                        Span::styled(s.content, style)
                    })
                    .collect()
            } else {
                spans
            };
            put_line(buf, row.x, row.y, row.width, &Spans::from(spans));
            if chosen {
                put_line(
                    buf,
                    row.x,
                    row.y,
                    1,
                    &Spans::from(Span::styled("▌", Tone::Accent.style(theme))),
                );
            }
            if let Some(i) = item {
                self.hits.push((Hit::Item(i), row));
            }
            self.spot(Pane::List, row, row.x, i64::from(y), false);
        }
    }

    /// The column, from its first line, the cursor's line on the
    /// highlight, a scroll bar at its right edge.
    fn draw_column(&mut self, buf: &mut Buffer, theme: &Theme) {
        let rect = self.column_rect();
        let cols = self.column_cols();
        self.keep_reading();
        let (top, cur) = (self.reader.top, self.reader.cur);
        let x0 = rect.x + 1;
        let h = rect.height as usize;
        // Only the rows in sight are taken from the layout kept.
        let all = self.column(cols);
        let total = all.len();
        let shown: Vec<Row> = all.iter().skip(top).take(h).cloned().collect();
        for (k, row) in shown.iter().enumerate() {
            let i = top + k;
            let y = rect.y + (i - top) as u16;
            let line = Rect::new(x0, y, cols as u16, 1);
            if let Some(bg) = row.bg {
                buf.set_style(line, Style::new().bg(bg));
            }
            let selected = self
                .reader
                .visual
                .is_some_and(|v| (v.min(cur)..=v.max(cur)).contains(&i));
            if i == cur || selected {
                buf.set_style(line, Style::new().bg(Surface::Chosen.color(theme)));
            } else if self.lit == Some(Hit::Line(i)) {
                buf.set_style(line, Style::new().bg(Surface::Raised.color(theme)));
            }
            put_line(buf, x0, y, cols as u16, &Spans::from(row.spans.clone()));
            if row.does != Does::Nothing {
                self.hits.push((Hit::Line(i), line));
            }
            self.spot(Pane::Wide, line, x0, i as i64, false);
        }
        if kit::scroll_bars() && kit::has_scrollbar(total, h) {
            kit::draw_scrollbar(
                buf,
                (rect.right().saturating_sub(1), rect.y, rect.height),
                (total, top, h),
                theme,
            );
        }
    }

    /// What runs over the column, as board 142 draws it: its label, then
    /// a box a step in the order it happens, an arrow between, each
    /// naming its page, the chosen one lit; scrolled so it is in sight.
    fn draw_runs(&mut self, buf: &mut Buffer, theme: &Theme) {
        let Some(story) = self.told_map().cloned() else {
            return;
        };
        let area = self.diff_area();
        let (x0, cols) = (area.x + 1, area.width.saturating_sub(3) as usize);
        let note = "in the order it happens · a click goes to its page";
        let mut head = kit::fit(
            vec![Span::styled("WHAT RUNS", Tone::Dim.bold(theme))],
            cols.saturating_sub(width(note) + 1),
            Style::new(),
        );
        head.push(Span::raw(" "));
        head.push(Span::styled(note, Tone::Faint.style(theme)));
        put_line(buf, x0, area.y, cols as u16, &Spans::from(head));
        let chosen = self.reader.run.filter(|&r| r < story.runs.len());
        // Two columns kept at each end for the ‹ and › to the steps out
        // of sight, which a click goes to as h and l do.
        let room = cols.saturating_sub(4);
        // Each box's width, no wider than the room, and the first shown so
        // the chosen one fits.
        let boxes: Vec<(String, String, usize)> = story
            .runs
            .iter()
            .map(|r| {
                // A page of a later chapter is named with its chapter.
                let page = match story::page_number(&story, r.page) {
                    (Some(c), k, _) if c > 0 => format!("chapter {} · page {k}", c + 1),
                    (_, k, _) => format!("page {k}"),
                };
                let most = room.saturating_sub(4).clamp(1, 28);
                let (step, page) = (cut(&r.step, most), cut(&page, most));
                let w = width(&step).max(width(&page)) + 4;
                (step, page, w)
            })
            .collect();
        const ARROW: usize = 5;
        let mut first = 0;
        let span_of = |from: usize, to: usize| -> usize {
            boxes[from..=to].iter().map(|b| b.2).sum::<usize>() + ARROW * (to - from)
        };
        let keep = chosen.unwrap_or(0);
        while first < keep && span_of(first, keep) > room {
            first += 1;
        }
        let mut x = x0 as usize + 2;
        let mut shown = first;
        for (i, (step, page, w)) in boxes.iter().enumerate().skip(first) {
            if x + w > x0 as usize + 2 + room {
                break;
            }
            shown = i + 1;
            let lit = chosen == Some(i);
            let edge = if lit || self.lit == Some(Hit::Run(i)) {
                Tone::Accent.style(theme)
            } else {
                Style::new().fg(kit::border(theme))
            };
            let inner = w - 2;
            let (bx, by) = (x as u16, area.y + 1);
            let bg = if lit {
                Style::new().bg(Surface::Chosen.color(theme))
            } else {
                Style::new()
            };
            put_line(
                buf,
                bx,
                by,
                *w as u16,
                &Spans::from(Span::styled(format!("╭{}╮", "─".repeat(inner)), edge)),
            );
            for (dy, (text, style)) in [
                (step.clone(), Tone::Text.bold(theme)),
                (page.clone(), Tone::Accent.style(theme)),
            ]
            .into_iter()
            .enumerate()
            {
                let pad = inner.saturating_sub(width(&text) + 1);
                put_line(
                    buf,
                    bx,
                    by + 1 + dy as u16,
                    *w as u16,
                    &Spans::from(vec![
                        Span::styled("│", edge),
                        Span::styled(format!(" {text}{}", " ".repeat(pad)), style.patch(bg)),
                        Span::styled("│", edge),
                    ]),
                );
            }
            put_line(
                buf,
                bx,
                by + 3,
                *w as u16,
                &Spans::from(Span::styled(format!("╰{}╯", "─".repeat(inner)), edge)),
            );
            self.hits
                .push((Hit::Run(i), Rect::new(bx, by, *w as u16, 4)));
            x += w;
            if i + 1 < boxes.len() && x + ARROW < x0 as usize + cols {
                put_line(
                    buf,
                    x as u16,
                    by + 1,
                    ARROW as u16,
                    &Spans::from(Span::styled(" ──▶ ", Tone::Faint.style(theme))),
                );
                x += ARROW;
            }
        }
        let by = area.y + 2;
        let ends = [
            (first > 0, x0, "‹", first.wrapping_sub(1)),
            (shown < boxes.len(), x0 + cols as u16 - 1, "›", shown),
        ];
        for (more, at, mark, to) in ends {
            if more {
                let style = if self.lit == Some(Hit::Run(to)) {
                    Tone::Accent.bold(theme)
                } else {
                    Tone::Accent.style(theme)
                };
                put_line(buf, at, by, 1, &Spans::from(Span::styled(mark, style)));
                self.hits.push((Hit::Run(to), Rect::new(at, by - 1, 1, 3)));
            }
        }
        put_line(
            buf,
            area.x,
            area.y + RUNS_ROWS - 1,
            area.width,
            &Spans::from(kit::rule(area.width as usize, theme)),
        );
    }
}

/// Before and After as board 140 draws them: two boxes side by side with
/// an arrow between, After's edge in the accent, each its text wrapped;
/// one over the other where the column is too narrow for two.
fn sides_boxes(before: &str, after: &str, cols: usize, theme: &Theme) -> Vec<Vec<Span<'static>>> {
    let dim = Style::new().fg(kit::border(theme));
    let accent = Tone::Accent.style(theme);
    let boxed = |title: &str, text: &str, w: usize, edge: Style, tone: Tone| {
        let inner = w.saturating_sub(4).max(4);
        let mut out = vec![vec![
            Span::styled("╭─ ", edge),
            Span::styled(title.to_string(), Tone::Text.bold(theme)),
            Span::styled(
                format!(" {}╮", "─".repeat(w.saturating_sub(width(title) + 5))),
                edge,
            ),
        ]];
        for line in wrap(text, inner) {
            let pad = inner.saturating_sub(width(&line));
            out.push(vec![
                Span::styled("│ ", edge),
                Span::styled(format!("{line}{}", " ".repeat(pad)), tone.style(theme)),
                Span::styled(" │", edge),
            ]);
        }
        out.push(vec![Span::styled(
            format!("╰{}╯", "─".repeat(w.saturating_sub(2))),
            edge,
        )]);
        out
    };
    if cols < 60 {
        let mut out = boxed("Before", before, cols, dim, Tone::Dim);
        out.push(vec![Span::styled("   ▼", Tone::Faint.style(theme))]);
        out.extend(boxed("After", after, cols, accent, Tone::Text));
        return out;
    }
    let w = (cols - 5) / 2;
    let mut left = boxed("Before", before, w, dim, Tone::Dim);
    let mut right = boxed("After", after, w, accent, Tone::Text);
    // Both as tall as the taller, each bottom edge kept last.
    let tall = left.len().max(right.len());
    for (side, edge) in [(&mut left, dim), (&mut right, accent)] {
        let bottom = side.pop().unwrap_or_default();
        while side.len() + 1 < tall {
            side.push(vec![
                Span::styled("│ ", edge),
                Span::raw(" ".repeat(w.saturating_sub(4))),
                Span::styled(" │", edge),
            ]);
        }
        side.push(bottom);
    }
    left.into_iter()
        .zip(right)
        .enumerate()
        .map(|(i, (mut l, r))| {
            l.push(Span::styled(
                if i == 1 { " ──▶ " } else { "     " },
                Tone::Faint.style(theme),
            ));
            l.extend(r);
            l
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_line_goes_on_in_pieces_of_the_column() {
        assert_eq!(pieces("abcdefg", 3), ["abc", "def", "g"]);
        assert_eq!(pieces("", 3), [""]);
        assert_eq!(pieces("a  b", 10), ["a  b"]);
    }

    #[test]
    fn a_missing_newline_at_the_end_is_said_under_its_line() {
        let (files, hunks) = story::split(&[
            "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-one\n\\ No newline at end of file\n+one\n",
        ]);
        let marks: Vec<char> = hunk_lines(&files["a.rs"], &hunks["a.rs"][0])
            .iter()
            .map(|l| l.0)
            .collect();
        assert_eq!(marks, ['-', '\\', '+']);
    }

    #[test]
    fn a_hunk_has_only_the_lines_its_header_counts() {
        // A patch's last hunk, the empty line after its closing newline
        // included, as story::split keeps it.
        let (files, hunks) = story::split(&[
            "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1,2 @@\n one\n+two\n",
        ]);
        let lines = hunk_lines(&files["a.rs"], &hunks["a.rs"][0]);
        let numbers: Vec<(char, Option<usize>)> = lines.iter().map(|l| (l.0, l.3)).collect();
        assert_eq!(numbers, [(' ', Some(1)), ('+', Some(2))]);
    }
}
