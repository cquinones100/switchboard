//! The story as a map (the diff_story_map setting), new with the port: the
//! diff's Map mode, beside the file view it had (Files mode), as boards 121
//! to 125 draw them, across the whole width as boards 134 and 135 do.
//!
//! A story told as a map (story.rs's `Chapter`s, `Need`s and `Thing`s) opens
//! on its first chapter's map: the chapters in a strip across the top, each
//! with how many of its pages are read; under it a box a page, the goal at
//! the top, an arrow from each to the pages it builds on, as Haiku gave
//! them, the pages read marked. The boxes are laid out by the Mermaid layout
//! the Markdown views draw flowcharts with (`switchboard_ui::mermaid`),
//! which gives the drawing as text: each box is found in it by its first
//! line, so the cursor can move box to box and a click can choose one. The
//! layout is worked out as the screen is drawn, from the story alone,
//! reading and starting nothing. A page is numbered in its chapter
//! wherever the reader sees it (`page_number`), the story's own count
//! staying inside.
//!
//! Enter opens a page alone, as ]p shows one, with its own map over the
//! diff (the things it adds or changes, an arrow from each to what it
//! uses), the pages it builds on and those that need it under its files,
//! and a hunk it shows from another page under an "also in page" line. ]c
//! and [c show the next chapter, Tab gives the strip the keys. `g m` comes
//! back to the map, `g f` shows the chosen page's file in Files mode,
//! and `m` asks which. The mode used last is kept in the state folder, and
//! the pages read beside the story in the cache, by its key.

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
use crate::story::{self, Kind, Story};
use crate::{order, paths, tell, util};
use switchboard_ui::draw::{cut, put_line, width, wrap};
use switchboard_ui::kit::{self, Surface, Tone};
use switchboard_ui::{Theme, mermaid};

/// The two ways the diff shows a story told as a map.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// The file list in reading order with the file beside it, as the
    /// diff always was.
    #[default]
    Files,
    /// The story's map, and a page opened from it.
    Map,
}

impl Mode {
    fn word(self) -> &'static str {
        match self {
            Mode::Files => "files",
            Mode::Map => "map",
        }
    }
}

/// The options of `m`'s menu, in its order.
pub(super) const MENU: [(&str, Mode); 2] = [("Map", Mode::Map), ("Files", Mode::Files)];

/// Something of the map a click lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Hit {
    /// A page, by its number: its box, or its row in the list.
    Page(usize),
    /// A chapter, by its place among the chapters: its entry in the strip.
    Chapter(usize),
    /// The strip's chip of the files no page tells.
    FilesOnly,
    /// A thing of the open page's own map.
    Thing(usize),
    /// Back from a page to its chapter's map.
    Back,
}

/// Where Files mode was when Map mode took its place: the page, the file
/// under the list's cursor and the view's file by their keys, the diff's
/// cursor and view, and which pane had the keys.
#[derive(Clone, Debug)]
pub(super) struct Place_ {
    alone: Option<usize>,
    file: Option<String>,
    view: Option<String>,
    cur: usize,
    col: usize,
    top: usize,
    focus: Focus,
    /// The path filter, which is the file list's and never the map's.
    filter: String,
    /// Whether the view was scrolled off the cursor's file.
    adrift: bool,
    /// The file the whole diff showed before the page, which [p from the
    /// first page goes back to.
    whole_at: Option<String>,
    /// The thing chosen on a page's own map while it had the keys.
    thing: Option<usize>,
    /// The list's first row on the screen, and the row it last followed
    /// the cursor to.
    list_top: usize,
    list_followed: Option<usize>,
}

/// How a box of a map is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct BoxLook {
    chosen: bool,
    lit: bool,
    read: bool,
}

/// A map laid out: the drawing's lines, a character a cell, and where each
/// box is in it, by the number it was given.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Laid {
    lines: Vec<Vec<char>>,
    boxes: Vec<(usize, Rect)>,
    width: usize,
}

impl Laid {
    /// Lays out a Mermaid flowchart, and finds each box by its label: the
    /// number it goes by and its lines, the first of which no other box
    /// starts with.
    pub(super) fn lay(source: &str, labels: &[(usize, Vec<String>)]) -> Option<Laid> {
        let drawn = mermaid::draw(source, usize::MAX)?;
        let lines: Vec<Vec<char>> = drawn.lines().map(|l| l.chars().collect()).collect();
        let mut boxes: Vec<(usize, Rect)> = Vec::new();
        for (y, line) in lines.iter().enumerate() {
            let edges: Vec<usize> = (0..line.len()).filter(|&x| line[x] == '│').collect();
            for pair in edges.windows(2) {
                let inside: String = line[pair[0] + 1..pair[1]].iter().collect();
                let inside = inside.trim();
                let found = labels.iter().find(|(id, label)| {
                    label.first().is_some_and(|first| first.trim() == inside)
                        && !boxes.iter().any(|(seen, _)| seen == id)
                });
                if let (Some((id, label)), true) = (found, y > 0 && !inside.is_empty()) {
                    boxes.push((
                        *id,
                        Rect::new(
                            pair[0] as u16,
                            y as u16 - 1,
                            (pair[1] - pair[0] + 1) as u16,
                            label.len() as u16 + 2,
                        ),
                    ));
                }
            }
        }
        let width = lines.iter().map(Vec::len).max().unwrap_or(0);
        Some(Laid {
            lines,
            boxes,
            width,
        })
    }

    fn rect(&self, id: usize) -> Option<Rect> {
        self.boxes.iter().find(|(i, _)| *i == id).map(|(_, r)| *r)
    }

    /// The part of the drawing shown in `w` columns and `h` rows: its first
    /// column and row, kept round box `around` when the drawing is larger,
    /// and the columns left of it when it is narrower, so it sits in the
    /// middle.
    fn window(&self, w: usize, h: usize, around: Option<usize>) -> (usize, usize, usize) {
        let b = around.and_then(|id| self.rect(id));
        let start = |len: usize, room: usize, at: Option<(usize, usize)>| {
            if len <= room {
                return 0;
            }
            at.map_or(0, |(from, size)| {
                (from + size / 2).saturating_sub(room / 2).min(len - room)
            })
        };
        let left = start(self.width, w, b.map(|r| (r.x as usize, r.width as usize)));
        let top = start(
            self.lines.len(),
            h,
            b.map(|r| (r.y as usize, r.height as usize)),
        );
        (left, top, w.saturating_sub(self.width) / 2)
    }

    /// The rows the drawing takes in `h`.
    fn height(&self, h: usize) -> usize {
        self.lines.len().min(h)
    }

    /// The drawing's rows as shown in `w` by `h`, each box in its look: its
    /// edge the accent when chosen and bright under the mouse, its first
    /// line bold when chosen and dim when read, its other lines dim; an
    /// arrow dim, a dotted one in the needs tone.
    fn rows(
        &self,
        w: usize,
        h: usize,
        around: Option<usize>,
        look: &dyn Fn(usize) -> BoxLook,
        theme: &Theme,
    ) -> Vec<Vec<Span<'static>>> {
        let (left, top, pad) = self.window(w, h, around);
        let looks: Vec<(Rect, BoxLook)> =
            self.boxes.iter().map(|(id, r)| (*r, look(*id))).collect();
        let mut out = Vec::new();
        for (y, line) in self.lines.iter().enumerate().skip(top).take(h) {
            let mut spans: Vec<Span<'static>> = vec![Span::raw(" ".repeat(pad))];
            let mut run = String::new();
            let mut run_style = Style::new();
            for (x, &c) in line.iter().enumerate().skip(left).take(w) {
                let inside = looks.iter().find(|(r, _)| {
                    x >= r.x as usize
                        && x < (r.x + r.width) as usize
                        && y >= r.y as usize
                        && y < (r.y + r.height) as usize
                });
                let style = match inside {
                    Some((r, look)) => {
                        let edge = x == r.x as usize
                            || x + 1 == (r.x + r.width) as usize
                            || y == r.y as usize
                            || y + 1 == (r.y + r.height) as usize;
                        if edge && look.chosen {
                            Tone::Accent.style(theme)
                        } else if edge && look.lit {
                            Style::new().fg(theme.bright)
                        } else if edge {
                            Style::new().fg(theme.faint)
                        } else if c == '✓' {
                            Tone::Done.style(theme)
                        } else if y != r.y as usize + 1 {
                            Tone::Dim.style(theme)
                        } else if look.chosen {
                            Style::new().fg(theme.bright).add_modifier(Modifier::BOLD)
                        } else if look.read {
                            Tone::Dim.style(theme)
                        } else {
                            Tone::Text.style(theme)
                        }
                    }
                    None if matches!(c, '┄' | '┆') => Tone::Needs.style(theme),
                    None => Tone::Dim.style(theme),
                };
                if style != run_style && !run.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut run), run_style));
                }
                run_style = style;
                run.push(c);
            }
            if !run.is_empty() {
                spans.push(Span::styled(run, run_style));
            }
            out.push(spans);
        }
        out
    }

    /// Where each box is on the screen, the drawing shown in `w` by `h`
    /// from `(x0, y0)`: the part of it in sight.
    fn hits(
        &self,
        (x0, y0): (u16, u16),
        w: usize,
        h: usize,
        around: Option<usize>,
    ) -> Vec<(usize, Rect)> {
        let (left, top, pad) = self.window(w, h, around);
        let h = self.height(h);
        self.boxes
            .iter()
            .filter_map(|(id, r)| {
                let (x, y) = (r.x as usize, r.y as usize);
                let (right, bottom) = (x + r.width as usize, y + r.height as usize);
                let (x, right) = (x.max(left), right.min(left + w));
                let (y, bottom) = (y.max(top), bottom.min(top + h));
                (x < right && y < bottom).then(|| {
                    (
                        *id,
                        Rect::new(
                            x0 + (pad + x - left) as u16,
                            y0 + (y - top) as u16,
                            (right - x) as u16,
                            (bottom - y) as u16,
                        ),
                    )
                })
            })
            .collect()
    }
}

/// The other page whose thing of that name page `page`'s things
/// use: the one page listing it, else the one of those it builds on;
/// none where that still leaves two, or the page lists it itself.
fn outside_page(story: &Story, page: usize, name: &str) -> Option<usize> {
    if story
        .pages
        .get(page - 1)
        .is_some_and(|c| c.things.iter().any(|t| t.name == name))
    {
        return None;
    }
    let holders: Vec<usize> = (1..=story.pages.len())
        .filter(|&n| n != page && story.pages[n - 1].things.iter().any(|t| t.name == name))
        .collect();
    let on: Vec<usize> = holders
        .iter()
        .copied()
        .filter(|n| {
            story.pages[page - 1]
                .needs
                .iter()
                .any(|need| need.page == *n)
        })
        .collect();
    let found = if holders.len() == 1 {
        holders.first().copied()
    } else {
        None
    };
    let found = found.or_else(|| (on.len() == 1).then(|| on[0]));
    // A page listing two things of the name has no one thing meant.
    found.filter(|n| {
        story.pages[n - 1]
            .things
            .iter()
            .filter(|t| t.name == name)
            .count()
            == 1
    })
}

/// Where a page's map numbers the things of other pages it uses,
/// apart from its own.
const OUTSIDE: usize = 1000;

/// `n` of something, by its name: "1 file", "3 files".
fn counted(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// Where the mode used last is kept.
fn mode_file() -> PathBuf {
    paths::state_dir().join("diff-mode")
}

impl App {
    // ── What is shown ───────────────────────────────────────────────────────

    /// Whether the story shown has a map: told as one, with the
    /// diff_story_map setting on, in the branch's own diff.
    pub(super) fn mapped(&self) -> bool {
        self.opts.map
            && !self.in_history
            && self.story.as_ref().is_some_and(|s| !s.chapters.is_empty())
    }

    /// Whether the map itself is on the screen.
    pub(super) fn on_map(&self) -> bool {
        self.mapped() && self.mode == Mode::Map && self.alone.is_none()
    }

    /// Whether a page opened from the map is.
    pub(super) fn in_page(&self) -> bool {
        self.mapped() && self.mode == Mode::Map && self.alone.is_some()
    }

    /// Page `n`'s number as the reader counts it: in its chapter in a
    /// story told as a map, else among the pages told.
    pub(super) fn page_shown(&self, n: usize) -> usize {
        match self.told_map() {
            Some(story) => story::page_number(story, n).1,
            None => n,
        }
    }

    /// Page `n` headed as the reader counts it, as boards 135 and 136
    /// head it: "Chapter 2 · page 1 of 3" in a story of several
    /// chapters, else "Page 4 of 5".
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

    /// Page `n` named from where the reader is, the chapter shown: its
    /// number alone in it, its chapter and number in another.
    pub(super) fn page_named(&self, n: usize) -> String {
        match self.told_map() {
            Some(story) => story::page_name(story, n, Some(self.chapter())),
            None => n.to_string(),
        }
    }

    pub(super) fn told_map(&self) -> Option<&Story> {
        self.story.as_ref().filter(|_| self.mapped())
    }

    /// The chapter whose map is shown, kept among those the story has.
    fn chapter(&self) -> usize {
        let n = self.story.as_ref().map_or(0, |s| s.chapters.len());
        self.chapter.min(n.saturating_sub(1))
    }

    /// The name of the chapter shown.
    pub(super) fn shown_story_title(&self) -> String {
        self.story
            .as_ref()
            .and_then(|s| s.chapters.get(self.chapter()))
            .map(|t| t.title.clone())
            .unwrap_or_default()
    }

    pub(super) fn read_of_shown(&self) -> (usize, usize) {
        self.read_of(self.chapter())
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

    /// The files page `n` has a hunk in, a hunk repeated there among
    /// them, and its hunks.
    fn page_counts(&self, story: &Story, n: usize) -> (usize, usize) {
        let files = story.pages.get(n - 1).map_or(0, |c| c.touches.len());
        let own = story
            .hunks
            .values()
            .flatten()
            .filter(|h| h.page == n)
            .count();
        let again = story
            .repeats
            .values()
            .filter(|shown| shown.contains(&n))
            .count();
        (files, own + again)
    }

    // ── The modes ───────────────────────────────────────────────────────────

    /// The story told has just come: with a map, the diff opens in the
    /// mode used last, Map the first time, on chapter 1's goal, with
    /// the pages read before marked; where someone has already moved
    /// through the files, it stays on them.
    pub(super) fn enter_story(&mut self) {
        self.thing = None;
        self.things = None;
        // Where Files mode was left is kept while the map stays shown
        // across a story told again: its file is found again by its key.
        if self.mode != Mode::Map {
            self.files_place = None;
        }
        self.read.clear();
        self.chapter = 0;
        if !self.mapped() {
            // A story told as a list: the files where they were left.
            if self.mode == Mode::Map {
                self.files_again();
            }
            self.mode = Mode::Files;
            return;
        }
        self.map_at = self.goal_of(0);
        self.read = self.kept_read();
        // A story told again while the map was shown (the change read
        // again) stays on the map; the first to come stays on the files
        // where someone is already reading them.
        self.mode = if self.mode == Mode::Map {
            Mode::Map
        // Someone reading the files already (moved through them, or in a
        // side of the diff) stays on them.
        } else if self.moved || self.alone.is_some() || self.focus != Focus::List {
            Mode::Files
        } else {
            self.kept_mode()
        };
        // The map has the keys: typing the file list's filter ends.
        if self.mode == Mode::Map {
            self.filtering = false;
        }
        // A page left open across the story told again is that
        // page of the new story, its own map laid out anew.
        if self.mode == Mode::Map && self.alone.is_some() {
            self.alone_opened();
        }
    }

    fn goal_of(&self, chapter: usize) -> usize {
        self.story
            .as_ref()
            .and_then(|s| s.chapters.get(chapter))
            .and_then(|t| t.goal.or(t.pages.first().copied()))
            .unwrap_or(1)
    }

    fn kept_mode(&self) -> Mode {
        let kept = self
            .opts
            .keep
            .then(|| util::read_text(mode_file()))
            .flatten();
        match kept.as_deref().map(str::trim) {
            Some("files") => Mode::Files,
            _ => Mode::Map,
        }
    }

    fn keep_mode(&self) {
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

    fn mark_read(&mut self, n: usize) {
        // With what another diff open on the same story has read since,
        // taken in whether or not this page is new here.
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

    fn place(&self) -> Place_ {
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
            thing: self.thing,
            list_top: self.list_top,
            list_followed: self.list_followed,
        }
    }

    /// The map gone from under the reader: Files mode where it was left,
    /// its filter back and its file found again by its key.
    pub(super) fn files_again(&mut self) {
        self.mode = Mode::Files;
        self.thing = None;
        self.things = None;
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
        self.map_alone = None;
        self.map_place = None;
    }

    /// Whether a row of the screen is in the open page's own map.
    pub(super) fn over_things(&self, y: u16) -> bool {
        if !self.things_shown() {
            return false;
        }
        let area = self.diff_area();
        let cols = area.width as usize;
        let from = area.y + self.page_title(cols).len() as u16;
        let to = from + self.things_rows(cols).len() as u16;
        (from..to).contains(&y)
    }

    /// Why this diff has no map, said when one is asked for.
    fn no_map(&mut self) {
        let text = if self.in_history {
            "A commit of the history has no map.\nc goes back to the whole branch's."
        } else if !self.opts.map {
            "No map for this diff: Settings' \"Tell the diff's story as a map\" (diff_story_map) is off."
        } else if self.story.is_none() {
            return self.no_story();
        } else {
            "This story was told as a list, with no map.\nClearing this tab's diff cache tells it again."
        };
        self.say(text);
    }

    /// `g m`, and Map in the switch and the menu: the story's map, from
    /// anywhere. Files mode keeps its place for the way back.
    pub(super) fn show_map(&mut self) {
        if !self.mapped() {
            return self.no_map();
        }
        if self.mode == Mode::Files {
            self.files_place = Some(self.place());
        }
        self.mode = Mode::Map;
        self.keep_mode();
        // The filter is the file list's: kept with its place, and its
        // typing ended, so the map has the keys.
        self.filtering = false;
        self.path_filter.clear();
        // Nor is a selection of the file list's rows the map's to copy.
        self.list_visual = None;
        self.close_page();
    }

    /// An open page closed, the map in its place.
    fn close_page(&mut self) {
        self.thing = None;
        self.things = None;
        self.focus = Focus::List;
        self.visual = None;
        if self.alone.take().is_some() {
            self.whole_at = None;
            self.lines.clear();
            self.sort();
            self.show_at(0);
        }
    }

    /// Files in the switch and the menu (`restore`): Files mode where it
    /// was left. `g f`: Files mode on the chosen page's first file.
    pub(super) fn show_files(&mut self, restore: bool) {
        // While a map is still being told, the files where they were left.
        if !self.mapped() && self.mode == Mode::Map {
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
        let chosen = self.alone.unwrap_or(self.map_at);
        // The page left, for the menu's Map, whichever way Files came.
        self.map_alone = self.alone.filter(|_| self.mode == Mode::Map);
        self.map_place = self.map_alone.map(|_| self.place());
        self.mode = Mode::Files;
        self.keep_mode();
        self.thing = None;
        self.things = None;
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
                    self.mode = Mode::Map;
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
        // Where the map was left, for the menu's Map: the page open,
        // or none when the map itself was shown.
        if self.mode == Mode::Map {
            self.map_alone = self.alone;
            self.map_place = self.alone.map(|_| self.place());
        }
        self.mode = Mode::Files;
        self.keep_mode();
        self.thing = None;
        self.things = None;
        self.visual = None;
        self.alone = None;
        self.whole_at = None;
        // A filter typed in a page's list would hide the line's file.
        self.filtering = false;
        self.path_filter.clear();
        self.lines.clear();
        self.sort();
    }

    /// A page opened from the map, as a page of its own with its own
    /// map over it; it is read from then on.
    pub(super) fn open_page(&mut self, n: usize) {
        if self
            .story
            .as_ref()
            .is_none_or(|s| n < 1 || n > s.pages.len())
        {
            return;
        }
        self.mode = Mode::Map;
        self.focus = Focus::List;
        // The file list's filter would hide a page's files: it stays
        // with Files mode's place, and a page opens on all of its own.
        self.filtering = false;
        self.path_filter.clear();
        self.open_alone(n);
    }

    /// A page has opened: in Map mode its page is the one chosen on the
    /// map, in its own chapter, and read, and its own map is laid out.
    pub(super) fn alone_opened(&mut self) {
        self.thing = None;
        self.things = None;
        let Some(n) = self.alone.filter(|_| self.in_page()) else {
            return;
        };
        if let Some(t) = self.story.as_ref().and_then(|s| story::chapter_of(s, n)) {
            self.chapter = t;
            self.map_at = n;
        }
        // A page whose files the diff no longer has (changed back
        // since their commits) has nothing to read, and stays unread.
        if self.order.is_empty() && self.path_filter.trim().is_empty() {
            return self.say(
                "This page's files aren't in the diff as it is now: they changed back since its commits.",
            );
        }
        self.mark_read(n);
        self.lay_things();
    }

    /// The page a hunk is also in, gone to: its page in Map mode and
    /// where ]p turns pages, else its first file in the list.
    pub(super) fn go_to_page(&mut self, n: usize) {
        if self.in_page() || self.on_map() {
            return self.open_page(n);
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
                "Page {n} heads no file in the list: its page shows it, with Settings' \"Diff story a page at a time\" (diff_story_pages) on."
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
            self.at_hunk(new_line, old_line);
        }
    }

    /// The cursor on a hunk's first changed line, on the side that has it:
    /// the old for a line only taken out, whatever context is around it.
    fn at_hunk(&mut self, new_line: (usize, usize), old_line: (usize, usize)) {
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
            return;
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
        let Some(i) = found.or_else(|| lines.iter().position(inside)) else {
            return;
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
    }

    /// The wheel over the map: the chapter read from the goal down, the
    /// next page or the one before, chosen on the map.
    pub(super) fn map_step(&mut self, step: i32) {
        let Some(story) = self.told_map() else {
            return;
        };
        let order = story::goal_down(story, self.chapter());
        let here = self.alone.unwrap_or(self.map_at);
        let to = match order.iter().position(|&n| n == here) {
            Some(i) => i as i32 + step,
            None => 0,
        };
        let Some(&n) = usize::try_from(to).ok().and_then(|i| order.get(i)) else {
            return;
        };
        if self.alone.is_some() {
            self.open_page(n);
        } else {
            self.map_at = n;
        }
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
        // The list's groups in its order, each chapter with a file in it
        // and the files no chapter holds, past those a filter leaves
        // empty: the next group's first file, or the one before's.
        let mut groups: Vec<(Option<usize>, usize)> = Vec::new();
        for k in 0..self.order.len() {
            let group = chapter_at(self, k);
            if !groups.iter().any(|(g, _)| *g == group) {
                groups.push((group, k));
            }
        }
        let here = (self.at < self.order.len()).then(|| chapter_at(self, self.at));
        let Some(i) = here.and_then(|h| groups.iter().position(|(g, _)| *g == h)) else {
            return;
        };
        let Some(&(_, first)) = usize::try_from(i as i32 + step)
            .ok()
            .and_then(|j| groups.get(j))
        else {
            return;
        };
        self.moved = true;
        self.show_at(first);
    }

    /// ]p and [p in Map mode: from the map, ]p opens the chosen page;
    /// in an open page they turn to the next read from the goal down
    /// and the one before, past a chapter's last page into the next
    /// chapter's first and back, [p from chapter 1's first going back to
    /// the map.
    pub(super) fn map_alone(&mut self, step: i32) {
        let Some(story) = self.told_map() else {
            return;
        };
        let Some(here) = self.alone else {
            if step > 0 {
                self.open_page(self.map_at);
            }
            return;
        };
        let chapter = self.chapter();
        let order = story::goal_down(story, chapter);
        let to = order
            .iter()
            .position(|&n| n == here)
            .map_or(0, |i| i as i32 + step);
        if let Some(&n) = usize::try_from(to).ok().and_then(|i| order.get(i)) {
            return self.open_page(n);
        }
        // Past either end of the chapter: the next chapter's first page,
        // the one before's last, or from chapter 1's first the map.
        let next = if to < 0 {
            chapter.checked_sub(1)
        } else {
            Some(chapter + 1).filter(|&c| c < story.chapters.len())
        };
        let Some(next) = next else {
            if to < 0 {
                self.close_page();
            }
            return;
        };
        let pages = story::goal_down(story, next);
        let n = if to < 0 { pages.last() } else { pages.first() };
        if let Some(&n) = n {
            self.open_page(n);
        }
    }

    /// ] and [ on the map, and ]c and [c from an open page's list: the
    /// next chapter's map, or the one before.
    pub(super) fn chapter_step(&mut self, step: i32) {
        let n = self.story.as_ref().map_or(0, |s| s.chapters.len()) as i32;
        if !self.mapped() || n < 2 {
            return;
        }
        self.show_chapter((self.chapter() as i32 + step).rem_euclid(n) as usize);
    }

    fn show_chapter(&mut self, chapter: usize) {
        if !self.mapped() {
            return;
        }
        if self.mode != Mode::Map {
            self.show_map();
        }
        self.close_page();
        self.chapter = chapter;
        self.map_at = self.goal_of(chapter);
        // The strip, with the keys on a chapter, follows the one shown.
        let chapters = self.story.as_ref().map_or(0, |s| s.chapters.len());
        if self.strip.is_some_and(|s| s < chapters) {
            self.strip = Some(chapter);
        }
    }

    // ── The map's keys ──────────────────────────────────────────────────────

    /// Whether a pair of keys does anything on the map, where there is no
    /// file under the cursor.
    pub(super) fn pair_on_map(first: char, second: char) -> bool {
        matches!(
            (first, second),
            ('g', 'm' | 'f' | 'a') | ('y', 'S') | ('w', _)
        )
    }

    /// A key while the chapters' strip has the keys, as mission control's
    /// sections are one stop of Tab: h and l (and the arrows) move between
    /// the chapters, showing each, and on to the chip of the files no page
    /// tells; Enter on the chip opens Files mode on them; Tab, Enter on a
    /// chapter, j, the down arrow and Esc give the keys to the map.
    fn strip_key(&mut self, key: KeyEvent) -> Option<Act> {
        let at = self.strip?;
        let story = self.told_map()?;
        let chapters = story.chapters.len();
        let last = if Self::files_only(story).1 > 0 {
            chapters
        } else {
            chapters.saturating_sub(1)
        };
        let to = |app: &mut App, to: usize| {
            app.strip = Some(to);
            if to < chapters && to != app.chapter() {
                app.show_chapter(to);
            }
        };
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => to(self, at.saturating_sub(1)),
            KeyCode::Char('l') | KeyCode::Right => to(self, (at + 1).min(last)),
            KeyCode::Enter | KeyCode::Char('o' | ' ') if at >= chapters => {
                self.open_files_only();
            }
            KeyCode::Enter
            | KeyCode::Char('o' | ' ' | 'j')
            | KeyCode::Down
            | KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Esc => self.strip = None,
            // What the diff takes anywhere goes on to the map's keys.
            KeyCode::Char('q' | 'y' | 'g' | ']' | '[' | 'm' | 'R' | 'c' | 'E') | KeyCode::F(10) => {
                return None;
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => return None,
            _ => return Some(Act::Same),
        }
        Some(Act::Redraw)
    }

    /// A key on the map: the cursor moves box to box along the arrows (j
    /// and k down and up a level, h and l across one), Enter opens the
    /// chosen page, ] and [ show the next chapter and the one before, Tab
    /// gives the keys to the chapters' strip. `None` for
    /// a key the diff takes anywhere.
    pub(super) fn map_key(&mut self, key: KeyEvent) -> Option<Act> {
        if let Some(act) = self.strip_key(key) {
            return Some(act);
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let story = self.told_map()?;
        let chapter = self.chapter();
        let levels = story::levels(story, chapter);
        let at = self.map_at;
        let row = levels.iter().position(|l| l.contains(&at));
        // Across a level by where the boxes are drawn, which the layout
        // orders by what each connects to.
        let laid = self.lay_map(story, chapter);
        let across = |step: i32| -> Option<usize> {
            let mut level: Vec<usize> = levels[row?].clone();
            if let Some(laid) = &laid {
                level.sort_by_key(|&n| laid.rect(n).map_or(u16::MAX, |r| r.x));
            }
            let i = level.iter().position(|&n| n == at)? as i32 + step;
            usize::try_from(i).ok().and_then(|i| level.get(i)).copied()
        };
        let to = match key.code {
            // To the nearest level linked that way, the first there.
            KeyCode::Char('j') | KeyCode::Down if !ctrl => {
                let level_of = |n: usize| levels.iter().position(|l| l.contains(&n));
                story::builds_on(story, at)
                    .into_iter()
                    .map(|(n, _)| n)
                    .filter(|&n| level_of(n).is_some())
                    .min_by_key(|&n| level_of(n))
            }
            KeyCode::Char('k') | KeyCode::Up if !ctrl => {
                let level_of = |n: usize| levels.iter().position(|l| l.contains(&n));
                story::needed_by(story, at)
                    .into_iter()
                    .map(|(n, _)| n)
                    .filter(|&n| level_of(n).is_some())
                    .min_by_key(|&n| std::cmp::Reverse(level_of(n)))
            }
            KeyCode::Char('h') | KeyCode::Left if !ctrl => across(-1),
            KeyCode::Char('l') | KeyCode::Right if !ctrl => across(1),
            KeyCode::Enter | KeyCode::Char('o' | ' ') if !ctrl => {
                self.open_page(at);
                return Some(Act::Redraw);
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.strip = Some(self.chapter());
                return Some(Act::Redraw);
            }
            // The map has no file or change to jump to: ] and [ alone
            // show the next chapter and the one before.
            KeyCode::Char(']') if !ctrl => {
                self.chapter_step(1);
                return Some(Act::Redraw);
            }
            KeyCode::Char('[') if !ctrl => {
                self.chapter_step(-1);
                return Some(Act::Redraw);
            }
            KeyCode::Esc => return Some(Act::Quit),
            // What the diff takes anywhere: closing, the first key of a
            // pair, the menu, reading the change again, the commits.
            KeyCode::Char('q' | 'y' | 'g' | 'm' | 'R' | 'c' | 'E') if !ctrl => {
                return None;
            }
            KeyCode::Char('w') if ctrl => return None,
            KeyCode::F(10) => return None,
            _ => return Some(Act::Same),
        };
        if let Some(to) = to {
            self.map_at = to;
        }
        Some(Act::Redraw)
    }

    /// A key while the open page's own map has the keys: the cursor
    /// moves thing to thing, Enter goes to the chosen thing's hunk, Esc
    /// gives the keys back to the list and Tab on to the diff.
    pub(super) fn thing_key(&mut self, key: KeyEvent) -> Option<Act> {
        let at = self.thing?;
        let count = self
            .story
            .as_ref()
            .zip(self.alone)
            .and_then(|(s, p)| s.pages.get(p - 1))
            .map_or(0, |c| c.things.len());
        match key.code {
            KeyCode::Char('j' | 'l') | KeyCode::Down | KeyCode::Right => {
                self.thing = Some((at + 1).min(count.saturating_sub(1)));
            }
            KeyCode::Char('k' | 'h') | KeyCode::Up | KeyCode::Left => {
                self.thing = Some(at.saturating_sub(1));
            }
            KeyCode::Enter | KeyCode::Char('o' | ' ') => self.go_to_thing(at),
            KeyCode::Esc | KeyCode::BackTab => {
                self.thing = None;
                self.focus = Focus::List;
            }
            KeyCode::Tab => {
                self.thing = None;
                self.focus = self.main_side();
            }
            _ => return None,
        }
        Some(Act::Redraw)
    }

    /// The hunk that holds a thing of the open page, shown: its file,
    /// the cursor on the hunk's first line.
    pub(super) fn go_to_thing(&mut self, i: usize) {
        self.thing = None;
        let found = self
            .story
            .as_ref()
            .zip(self.alone)
            .and_then(|(s, p)| Some((s, s.pages.get(p - 1)?.things.get(i)?)))
            .and_then(|(s, thing)| {
                let hunk = s
                    .hunks
                    .get(&thing.path)?
                    .iter()
                    .find(|h| h.id == thing.hunk)?;
                Some((
                    thing.path.clone(),
                    (hunk.new_start, hunk.new_count),
                    (hunk.old_start, hunk.old_count),
                    hunk.run,
                ))
            });
        let Some((path, new_line, old_line, run)) = found else {
            return self.say("Haiku named no hunk for it");
        };
        // A file both staged and changed since is listed twice: the one
        // whose diff the hunk is from.
        let find = |app: &App| {
            app.order.iter().position(|&i| {
                let entry = &app.entries()[i];
                entry.path == path && app.run_of(entry).is_none_or(|r| r == run)
            })
        };
        // A filter of the page's list that hides it gives way.
        let at = find(self).or_else(|| {
            if self.path_filter.trim().is_empty() {
                return None;
            }
            self.filtering = false;
            self.path_filter.clear();
            self.sort();
            find(self)
        });
        let Some(at) = at else {
            return self.say(format!("{path} isn't among the files listed"));
        };
        self.moved = true;
        self.show_at(at);
        self.focus = self.main_side();
        // A file changed since its commits has its hunks at the committed
        // lines, which the working tree may have moved: its first change.
        if self.current().is_some_and(|e| self.changed_since(e)) {
            return self.say(
                "This file has changed since its commits, so its hunk may have moved:\nshown at its first change.",
            );
        }
        self.at_hunk(new_line, old_line);
    }

    /// One of the two modes chosen, in the menu or the switch.
    pub(super) fn choose_mode(&mut self, mode: Mode) {
        self.menu = None;
        match mode {
            // Back to the page that was open, where there was one.
            // The mode already shown stays as it is, a page open.
            Mode::Map if self.mode == Mode::Map => self.keep_mode(),
            Mode::Map => {
                let open = self.map_alone.take();
                let place = self.map_place.take();
                self.show_map();
                if let Some(n) = open.filter(|_| self.mapped()) {
                    self.open_page(n);
                    // Where it was in the page: its file, line, focus.
                    if let Some(place) = place {
                        let thing = place.thing;
                        self.restore_place(place);
                        // Its own map's thing, with the keys, where it still is.
                        self.thing = thing.filter(|_| self.things_shown());
                    }
                }
            }
            Mode::Files => self.show_files(true),
        }
    }

    // ── Clicks ──────────────────────────────────────────────────────────────

    /// What of the map is at a cell of the screen.
    pub(super) fn hit_at(&self, x: u16, y: u16) -> Option<Hit> {
        self.hits
            .iter()
            .find(|(_, r)| x >= r.x && x < r.right() && y >= r.y && y < r.bottom())
            .map(|(hit, _)| *hit)
    }

    /// A click on something of the map: a page is chosen, and opened
    /// when it already was; a chapter's map is shown; a thing's hunk is gone
    /// to; and the way back goes back.
    pub(super) fn map_click(&mut self, hit: Hit) -> Act {
        match hit {
            // The chosen page opens as the mouse is let go, so a drag
            // from it selects its text instead.
            Hit::Page(n) if n == self.map_at => self.open_on_up = Some(n),
            Hit::Page(n) => self.map_at = n,
            Hit::Chapter(t) => self.show_chapter(t),
            Hit::FilesOnly => self.open_files_only(),
            Hit::Thing(i) => self.go_to_thing(i),
            // Back to the map, the file list's filter ended as g m ends it.
            Hit::Back => self.show_map(),
        }
        Act::Redraw
    }

    // ── A chapter's map ────────────────────────────────────────────────────

    /// A chapter's map laid out: each page a box with its number and
    /// title, a tick before them once read, over how many files and hunks
    /// it has, the next one to read from the goal down saying so.
    fn lay_map(&self, story: &Story, chapter: usize) -> Option<Laid> {
        let order = story::goal_down(story, chapter);
        let next = order.iter().copied().find(|n| !self.read.contains(n));
        let label = |n: usize| -> Vec<String> {
            let title = story.pages.get(n - 1).map_or("", |c| c.title.as_str());
            let tick = if self.read.contains(&n) { "✓ " } else { "" };
            let (_, k, _) = story::page_number(story, n);
            let (files, hunks) = self.page_counts(story, n);
            vec![
                story::label_text(&cut(&format!("{tick}{k} {title}"), 44)),
                format!(
                    "{} · {}{}",
                    counted(files, "file"),
                    counted(hunks, "hunk"),
                    if next == Some(n) { " · next" } else { "" }
                ),
            ]
        };
        let labels: Vec<(usize, Vec<String>)> = order.iter().map(|&n| (n, label(n))).collect();
        Laid::lay(&story::mermaid(story, chapter, &label), &labels)
    }

    /// The map's screen, as boards 134 and 135 draw it, across the diff's
    /// whole width: the chapters' strip, with what the change is compared
    /// with and the files no page tells at its right; the chosen chapter's
    /// pages as the map, the goal on chapter 1's top page; and under it
    /// the legend and the chosen page, what needs it and its why.
    pub(super) fn draw_map(&mut self, buf: &mut Buffer) {
        let theme = self.theme();
        let area = self.diff_area();
        let Some(story) = self.told_map().cloned() else {
            return;
        };
        let chapter = self.chapter();
        let Some(told) = story.chapters.get(chapter).cloned() else {
            return;
        };
        let order = story::goal_down(&story, chapter);
        if !order.contains(&self.map_at) {
            self.map_at = self.goal_of(chapter);
        }
        let at = self.map_at;
        let (x0, pane) = (area.x + 1, area.width.saturating_sub(2) as usize);
        // Each row of the map is text a drag selects.
        for y in 0..area.height {
            self.spot(
                Pane::Head,
                Rect::new(x0, y, pane as u16, 1),
                x0,
                i64::from(y),
                false,
            );
        }
        self.draw_strip(buf, &story, chapter, (x0, pane), &theme);
        put_line(
            buf,
            area.x,
            1,
            area.width,
            &Spans::from(kit::rule(area.width as usize, &theme)),
        );
        // Under the map: the chosen page, and the map's legend over it.
        let detail = self.map_detail(&story, chapter, at, pane, &theme);
        // The blank row, the legend and the blank row after it, then the lines.
        let below = (detail.len() + 3).min((area.height as usize).saturating_sub(9));
        let top = 2u16;
        let room = (area.height as usize).saturating_sub(top as usize + 1 + below);
        let Some(laid) = self.lay_map(&story, chapter) else {
            return;
        };
        let look = |n: usize| BoxLook {
            chosen: n == at,
            lit: self.lit == Some(Hit::Page(n)),
            read: self.read.contains(&n),
        };
        let drawn = laid.rows(pane, room, Some(at), &look, &theme);
        let hits = laid.hits((x0, top + 1), pane, room, Some(at));
        // Over the map: the goal named over chapter 1's top page, or that
        // none of a chapter's pages builds on another.
        let linked = order
            .iter()
            .any(|&n| !story::builds_on(&story, n).is_empty());
        let goal = told
            .goal
            .filter(|_| told.kind == Kind::Main)
            .and_then(|g| hits.iter().find(|(n, _)| *n == g));
        match goal {
            Some((_, r)) if linked && r.y == top + 1 => {
                let text = "THE GOAL";
                let gx = r.x + r.width.saturating_sub(width(text) as u16) / 2;
                put_line(
                    buf,
                    gx,
                    top,
                    area.right().saturating_sub(gx),
                    &Spans::from(Span::styled(text, Tone::Accent.bold(&theme))),
                );
            }
            _ if !linked && order.len() > 1 => {
                let text = format!(" {} PAGES, NONE BUILDS ON ANOTHER", order.len());
                put_line(
                    buf,
                    x0,
                    top,
                    pane as u16,
                    &Spans::from(Span::styled(text, Tone::Dim.bold(&theme))),
                );
            }
            _ => {}
        }
        for (i, spans) in drawn.iter().enumerate() {
            put_line(
                buf,
                x0,
                top + 1 + i as u16,
                pane as u16,
                &Spans::from(spans.clone()),
            );
        }
        for (n, r) in hits {
            self.hits.push((Hit::Page(n), r));
        }
        let mut y = top + 1 + laid.height(room) as u16 + 1;
        let legend = vec![
            Span::styled(" ──▼", Tone::Dim.style(&theme)),
            Span::styled(" builds on    ", Tone::Dim.style(&theme)),
            Span::styled("┄┄▼", Tone::Needs.style(&theme)),
            Span::styled(
                " a link the code can't confirm    ",
                Tone::Dim.style(&theme),
            ),
            Span::styled("✓", Tone::Done.style(&theme)),
            Span::styled(" read", Tone::Dim.style(&theme)),
        ];
        put_line(buf, x0, y, pane as u16, &Spans::from(legend));
        y += 2;
        for spans in detail {
            if y >= area.bottom() {
                break;
            }
            put_line(buf, x0, y, pane as u16, &Spans::from(spans));
            y += 1;
        }
    }

    /// The pages no chapter holds (what isn't committed yet, generated
    /// files, changes Haiku left out), which Files mode alone shows, and
    /// how many files they have.
    pub(super) fn files_only(story: &Story) -> (Vec<usize>, usize) {
        let pages: Vec<usize> = (1..=story.pages.len())
            .filter(|&n| story::chapter_of(story, n).is_none())
            .collect();
        let files = story.page.values().filter(|n| pages.contains(n)).count();
        (pages, files)
    }

    /// The chapters' strip, as board 135 draws it: each chapter's number,
    /// name and how many of its pages are read, the one shown lit (in the
    /// accent while the strip has the keys); at its right what the change
    /// is compared with and its files, and an accent chip for the files no
    /// page tells, which opens Files mode on them.
    fn draw_strip(
        &mut self,
        buf: &mut Buffer,
        story: &Story,
        chapter: usize,
        (x0, pane): (u16, usize),
        theme: &Theme,
    ) {
        let against = self
            .change
            .as_ref()
            .map(|c| format!("{} · {}", c.against(), counted(self.totals.files, "file")))
            .unwrap_or_default();
        let (_, only) = Self::files_only(story);
        let strip_at = self.strip;
        let chip = (only > 0).then(|| {
            let chosen = strip_at == Some(story.chapters.len());
            let lit = chosen || self.lit == Some(Hit::FilesOnly);
            let mut chip = kit::chip(
                &format!("{} · {only} ›", super::FILES_ONLY),
                Tone::Accent,
                false,
                theme,
            );
            if lit {
                for span in &mut chip {
                    span.style = span.style.add_modifier(Modifier::BOLD);
                }
            }
            chip
        });
        let chip_w = chip.as_ref().map_or(0, |c| kit::spans_width(c));
        let right = x0 + pane as u16;
        let chip_x = right.saturating_sub(chip_w as u16);
        let against_x = chip_x.saturating_sub(width(&against) as u16 + 2);
        // The chapters share what is left, each name cut to its share.
        let room = (against_x.saturating_sub(x0) as usize).max(pane / 2);
        let count = story.chapters.len().max(1);
        let entries: Vec<(usize, Vec<Span<'static>>)> = story
            .chapters
            .iter()
            .enumerate()
            .map(|(t, told)| {
                let (read, of) = self.read_of(t);
                let shown = t == chapter;
                let keys = shown && strip_at.is_some_and(|s| s < story.chapters.len());
                let lit = self.lit == Some(Hit::Chapter(t));
                let tally = format!("  {read} of {of} read");
                let share = (room / count).saturating_sub(width(&tally) + 6).max(6);
                let fill = if keys {
                    Tone::Accent.tint(theme)
                } else {
                    Surface::Chosen.color(theme)
                };
                let spans = vec![
                    Span::styled(format!(" {} ", t + 1), Tone::Accent.bold(theme)),
                    Span::styled(
                        cut(&told.title, share),
                        if shown || lit {
                            Tone::Text.bold(theme)
                        } else {
                            Tone::Dim.style(theme)
                        },
                    ),
                    Span::styled(
                        tally,
                        if shown && read > 0 {
                            Tone::Done.style(theme)
                        } else {
                            Tone::Faint.style(theme)
                        },
                    ),
                    Span::raw(" "),
                ];
                let spans = if shown {
                    spans
                        .into_iter()
                        .map(|s| {
                            let style = s.style.bg(fill);
                            Span::styled(s.content, style)
                        })
                        .collect()
                } else {
                    spans
                };
                (t, spans)
            })
            .collect();
        let widths: Vec<usize> = entries
            .iter()
            .map(|(_, s)| kit::spans_width(s) + 1)
            .collect();
        let total: usize = widths.iter().sum();
        // What the change is compared with gives way first; then, when the
        // chapters still don't fit, those in sight are the shown one and its
        // neighbours, a ‹ and a › at the ends reaching the rest by a click.
        let keep_against = total <= against_x.saturating_sub(x0) as usize;
        let avail = if keep_against {
            against_x.saturating_sub(x0) as usize
        } else {
            chip_x.saturating_sub(x0 + 1) as usize
        };
        let (mut from, mut to) = (0, entries.len());
        if total > avail {
            let room = avail.saturating_sub(4);
            let (mut lo, mut hi) = (chapter, chapter + 1);
            let mut used = widths.get(chapter).copied().unwrap_or(0);
            loop {
                let right = widths.get(hi).copied().filter(|w| used + w <= room);
                if let Some(w) = right {
                    used += w;
                    hi += 1;
                }
                let left = lo
                    .checked_sub(1)
                    .and_then(|l| widths.get(l))
                    .copied()
                    .filter(|w| used + w <= room);
                if let Some(w) = left {
                    used += w;
                    lo -= 1;
                }
                if right.is_none() && left.is_none() {
                    break;
                }
            }
            (from, to) = (lo, hi);
        }
        let mut x = x0;
        let arrow = |buf: &mut Buffer, x: u16, text: &str| {
            put_line(
                buf,
                x,
                0,
                2,
                &Spans::from(Span::styled(text.to_string(), Tone::Accent.bold(theme))),
            );
        };
        if from > 0 {
            arrow(buf, x, "‹ ");
            self.hits
                .push((Hit::Chapter(from - 1), Rect::new(x, 0, 1, 1)));
            x += 2;
        }
        for (t, spans) in entries.into_iter().take(to).skip(from) {
            let w = kit::spans_width(&spans) as u16;
            put_line(buf, x, 0, w, &Spans::from(spans));
            self.hits.push((Hit::Chapter(t), Rect::new(x, 0, w, 1)));
            x += w + 1;
        }
        if to < story.chapters.len() {
            arrow(buf, x, " ›");
            self.hits
                .push((Hit::Chapter(to), Rect::new(x + 1, 0, 1, 1)));
            x += 2;
        }
        if keep_against && against_x > x && !against.is_empty() {
            put_line(
                buf,
                against_x,
                0,
                chip_x.saturating_sub(against_x),
                &Spans::from(Span::styled(against, Tone::Dim.style(theme))),
            );
        }
        if let Some(chip) = chip
            && chip_x >= x
        {
            put_line(buf, chip_x, 0, chip_w as u16, &Spans::from(chip));
            self.hits.push((
                Hit::FilesOnly,
                Rect::new(chip_x, 0, (chip_w as u16).saturating_sub(1), 1),
            ));
        }
    }

    /// The lines under the map about the chosen page, as board 135 draws
    /// them: "Page n of m" and its title, with what needs it, its files
    /// and its hunks at the right; what it builds on in another chapter;
    /// its why; and why a chapter beside the goal's is apart.
    fn map_detail(
        &self,
        story: &Story,
        chapter: usize,
        at: usize,
        cols: usize,
        theme: &Theme,
    ) -> Vec<Vec<Span<'static>>> {
        let Some(page) = story.pages.get(at - 1) else {
            return Vec::new();
        };
        let told = &story.chapters[chapter];
        let main = story.chapters.first().map_or("", |t| t.title.as_str());
        let here = Some(chapter);
        let needers = story::needed_by(story, at);
        let needs = if !needers.is_empty() {
            format!("needed by {}", story::pages_named(story, &needers, here))
        } else if told.goal == Some(at) && told.kind == Kind::Main {
            "the goal".to_string()
        } else if told.kind != Kind::Main {
            format!("nothing in {main} needs it")
        } else {
            "nothing needs it".to_string()
        };
        let (files, hunks) = self.page_counts(story, at);
        let right = format!(
            "{needs} · {} · {}",
            counted(files, "file"),
            counted(hunks, "hunk")
        );
        let (_, k, m) = story::page_number(story, at);
        let room = cols.saturating_sub(width(&right) + 2);
        let mut first = kit::fit(
            vec![
                Span::raw(" "),
                Span::styled(format!("Page {k} of {m}"), Tone::Accent.bold(theme)),
                Span::styled(format!("  {}", page.title), Tone::Text.bold(theme)),
            ],
            room,
            Style::new(),
        );
        first.push(Span::raw(" "));
        first.push(Span::styled(right, Tone::Dim.style(theme)));
        let mut out = vec![first];
        let text = |out: &mut Vec<Vec<Span<'static>>>, words: &str, tone: Tone, most: usize| {
            let mut lines = wrap(words, cols.saturating_sub(2).max(10));
            lines.retain(|l| !l.is_empty());
            for line in lines.into_iter().take(most) {
                out.push(vec![Span::raw(" "), Span::styled(line, tone.style(theme))]);
            }
        };
        // What it builds on in another chapter, which no arrow here shows:
        // first, so a short pane cuts the why's end before it.
        for need in &page.needs {
            let other = story::chapter_of(story, need.page).filter(|t| *t != chapter);
            if let (Some(t), Some(on)) = (other, story.pages.get(need.page - 1)) {
                text(
                    &mut out,
                    &format!(
                        "Builds on {} {}, in {}{}.",
                        story::page_name(story, need.page, here),
                        on.title,
                        story.chapters[t].title,
                        if need.guess { " (a guess)" } else { "" }
                    ),
                    Tone::Dim,
                    2,
                );
            }
        }
        if !page.circles.is_empty() {
            let left: Vec<(usize, bool)> = page.circles.iter().map(|n| (*n, false)).collect();
            text(
                &mut out,
                &format!(
                    "Haiku also said it builds on {}, left off the map: it would close a circle.",
                    story::pages_named(story, &left, here)
                ),
                Tone::Faint,
                2,
            );
        }
        text(&mut out, &page.why, Tone::Dim, 3);
        // A dashed link: why the code can't confirm it.
        let guesses: Vec<(usize, bool)> = story::builds_on(story, at)
            .into_iter()
            .filter(|(_, guess)| *guess)
            .collect();
        if !guesses.is_empty() {
            text(
                &mut out,
                &format!(
                    "Dashed to {}: Haiku linked them, but the code can't confirm it.",
                    story::pages_named(story, &guesses, here)
                ),
                Tone::Dim,
                1,
            );
        }
        if told.kind != Kind::Main {
            out.push(Vec::new());
            out.push(kit::label(
                "Why this chapter is apart",
                Tone::Dim,
                None,
                cols,
                theme,
            ));
            let outside: Vec<(usize, &str)> = told
                .pages
                .iter()
                .flat_map(|&n| story.pages[n - 1].needs.iter())
                .filter(|need| !told.pages.contains(&need.page))
                .filter_map(|need| {
                    Some((need.page, story.pages.get(need.page - 1)?.title.as_str()))
                })
                .collect();
            // A page of another chapter that builds on one of these,
            // which the checked links may show whatever Haiku said.
            let needed: Option<(usize, &str)> = story
                .pages
                .iter()
                .enumerate()
                .filter(|(i, _)| !told.pages.contains(&(i + 1)))
                .find(|(_, c)| c.needs.iter().any(|need| told.pages.contains(&need.page)))
                .map(|(i, c)| (i + 1, c.title.as_str()));
            let name = |n: usize| story::page_name(story, n, here);
            let why = match (told.kind, outside.first()) {
                _ if needed.is_some() => {
                    let (n, title) = needed.unwrap_or_default();
                    format!(
                        "{} was told apart from {main}, but {} {title} builds on it.",
                        told.title,
                        name(n)
                    )
                }
                (Kind::Side, _) => format!(
                    "{} is what {main} can be read without: a rename, formatting, a drive-by fix.",
                    told.title
                ),
                (_, Some((n, title))) => format!(
                    "{} is a chapter of its own: it builds on {} {title}, but nothing in {main} needs it.",
                    told.title,
                    name(*n)
                ),
                (_, None) => format!(
                    "{} is a chapter of its own: nothing in {main} needs it.",
                    told.title
                ),
            };
            text(&mut out, &why, Tone::Dim, 3);
        }
        out
    }

    // ── A page's own map ─────────────────────────────────────────────────

    /// The open page's own map laid out, once as it opens: each thing a
    /// box with its name over its kind, its file and whether the change
    /// adds it, an arrow to each thing of the page it uses.
    pub(super) fn lay_things(&mut self) {
        self.things = None;
        let Some((story, at)) = self
            .story
            .as_ref()
            .zip(self.alone)
            .filter(|_| self.in_page())
        else {
            return;
        };
        let Some(page) = story.pages.get(at - 1).filter(|c| !c.things.is_empty()) else {
            return;
        };
        let label = |thing: &story::Thing| -> Vec<String> {
            let file = thing.path.rsplit('/').next().unwrap_or("");
            let users: Vec<(usize, bool)> = story
                .pages
                .iter()
                .enumerate()
                // As their own arrows name it: a page whose use of the
                // name means this page's thing.
                .filter(|(i, c)| {
                    *i + 1 != at
                        && c.things.iter().any(|t| t.uses.contains(&thing.name))
                        && outside_page(story, *i + 1, &thing.name) == Some(at)
                        // Not when two of this page's things share the name.
                        && page.things.iter().filter(|t| t.name == thing.name).count() == 1
                })
                .map(|(i, _)| (i + 1, false))
                .collect();
            let mut facts = vec![thing.kind.clone(), file.to_string()];
            facts.push(if thing.new { "new" } else { "changed" }.to_string());
            facts.retain(|f| !f.is_empty());
            let mut second = facts.join(" · ");
            if !users.is_empty() {
                second.push_str(&format!(
                    " · used by {}",
                    story::pages_named(story, &users, story::chapter_of(story, at))
                ));
            }
            vec![
                story::label_text(&cut(&thing.name, 40)),
                story::label_text(&second),
            ]
        };
        let labels: Vec<(usize, Vec<String>)> = page
            .things
            .iter()
            .enumerate()
            .map(|(i, thing)| (i, label(thing)))
            .collect();
        let mut labels = labels;
        let mut source = vec!["flowchart TD".to_string()];
        let mut edges: Vec<String> = Vec::new();
        // A thing of another page's that one here uses: a box of its
        // own, named with its page, which no click opens from here.
        let mut outside: Vec<(String, usize)> = Vec::new();
        for (i, thing) in page.things.iter().enumerate() {
            for used in &thing.uses {
                // A name two of its things share is no one thing: no arrow.
                let same: Vec<usize> = (0..page.things.len())
                    .filter(|&j| page.things[j].name == *used)
                    .collect();
                if let [j] = same.as_slice() {
                    edges.push(format!("  t{i} --> t{j}"));
                    continue;
                }
                if !same.is_empty() {
                    continue;
                }
                let Some(n) = outside_page(story, at, used) else {
                    continue;
                };
                let k = match outside.iter().position(|(name, _)| name == used) {
                    Some(k) => k,
                    None => {
                        outside.push((used.clone(), n));
                        outside.len() - 1
                    }
                };
                edges.push(format!("  t{i} --> t{}", OUTSIDE + k));
            }
        }
        for (k, (name, n)) in outside.iter().enumerate() {
            labels.push((
                OUTSIDE + k,
                vec![
                    story::label_text(&cut(name, 40)),
                    story::in_page(story, *n, story::chapter_of(story, at)),
                ],
            ));
        }
        // A box is found in the drawing by its first line: two names cut
        // to the same words are told apart by their place among the boxes, the other pages' among them.
        let labels: Vec<(usize, Vec<String>)> = labels
            .iter()
            .map(|(i, lines)| {
                let twin = labels
                    .iter()
                    .any(|(j, other)| j != i && other.first() == lines.first());
                let mut lines = lines.clone();
                if let (true, Some(first)) = (twin, lines.first_mut()) {
                    first.push_str(&match i.checked_sub(OUTSIDE) {
                        Some(k) => format!(" (other {})", k + 1),
                        None => format!(" ({})", i + 1),
                    });
                }
                (*i, lines)
            })
            .collect();
        for (i, lines) in &labels {
            source.push(format!("  t{i}[\"{}\"]", lines.join("<br>")));
        }
        source.extend(edges);
        self.things = Laid::lay(&source.join("\n"), &labels);
    }

    /// The rows the open page's own map may take over the diff: at most
    /// about a third of the pane, and none on a pane too short for both.
    fn things_room(&self) -> usize {
        let h = self.diff_area().height as usize;
        if h < 16 { 0 } else { (h / 3).max(5) }
    }

    /// The open page's own map over the diff, as board 122 draws it:
    /// its label with how many things it has, the map, and a rule.
    pub(super) fn things_rows(&self, cols: usize) -> Vec<Vec<Span<'static>>> {
        let (Some(laid), true) = (&self.things, self.in_page()) else {
            return Vec::new();
        };
        let room = self.things_room();
        if room == 0 {
            return Vec::new();
        }
        let theme = self.theme();
        let mut label = kit::label(
            "What it adds or changes",
            Tone::Dim,
            None,
            cols.saturating_sub(34),
            &theme,
        );
        let hint = if self.thing.is_some() {
            "↵ goes to its hunk · esc the list"
        } else {
            "Tab reaches it"
        };
        let count = counted(
            laid.boxes.iter().filter(|(i, _)| *i < OUTSIDE).count(),
            "thing",
        );
        label.push(Span::styled(
            format!("{count} · {hint} "),
            Tone::Faint.style(&theme),
        ));
        let look = |i: usize| BoxLook {
            chosen: self.thing == Some(i),
            lit: self.lit == Some(Hit::Thing(i)),
            read: false,
        };
        let mut out = vec![label];
        out.extend(laid.rows(cols, room, self.thing, &look, &theme));
        out.push(vec![kit::rule(cols, &theme)]);
        out
    }

    /// Where the things of the open page's own map are on the screen,
    /// found again once the diff is drawn under the rows before them.
    pub(super) fn things_hits(&mut self) {
        let (Some(laid), true) = (&self.things, self.in_page()) else {
            return;
        };
        let area = self.diff_area();
        let cols = area.width as usize;
        let room = self.things_room();
        if room == 0 || self.current().is_none() {
            return;
        }
        let before = self.page_title(cols).len() as u16 + 1;
        let hits = laid.hits((area.x, area.y + before), cols, room, self.thing);
        self.hits.extend(
            hits.into_iter()
                .filter(|(i, _)| *i < OUTSIDE)
                .map(|(i, r)| (Hit::Thing(i), r)),
        );
    }

    /// Under an open page's files in the list: the pages it builds
    /// on, and those that need it, each by its number and title, a guess
    /// saying so.
    pub(super) fn page_links(&self) -> Vec<(String, Vec<String>)> {
        let Some((story, page)) = self
            .story
            .as_ref()
            .zip(self.alone)
            .filter(|_| self.in_page())
        else {
            return Vec::new();
        };
        let lines = |pages: Vec<(usize, bool)>| -> Vec<String> {
            if pages.is_empty() {
                return vec!["nothing".into()];
            }
            pages
                .into_iter()
                .filter_map(|(n, guess)| {
                    let title = &story.pages.get(n - 1)?.title;
                    let guess = if guess { "  a guess" } else { "" };
                    Some(format!(
                        "{} {title}{guess}",
                        story::page_name(story, n, story::chapter_of(story, page))
                    ))
                })
                .collect()
        };
        let on: Vec<(usize, bool)> = story
            .pages
            .get(page - 1)
            .map(|c| c.needs.iter().map(|n| (n.page, n.guess)).collect())
            .unwrap_or_default();
        let mut out = vec![
            ("Builds on".into(), lines(on)),
            ("Needed by".into(), lines(story::needed_by(story, page))),
        ];
        // The things of other pages its things use, which its own map
        // may have no room to show.
        let mut used: Vec<String> = Vec::new();
        for thing in story.pages.get(page - 1).map_or(&[][..], |c| &c.things) {
            for name in &thing.uses {
                if let Some(n) = outside_page(story, page, name) {
                    let line = format!(
                        "{name}  {}",
                        story::in_page(story, n, story::chapter_of(story, page))
                    );
                    if !used.contains(&line) {
                        used.push(line);
                    }
                }
            }
        }
        if !used.is_empty() {
            out.push(("Uses from others".into(), used));
        }
        out
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

    /// Whether the open page's own map is on the screen, for the keys
    /// to reach: laid out, with a file shown under it and room over it.
    pub(super) fn things_shown(&self) -> bool {
        self.in_page()
            && self.things.is_some()
            && self.things_room() > 0
            && self.current().is_some()
    }

    /// The line over a hunk a page shows from another, as board 123
    /// draws it: where its home is, and that Enter goes there.
    pub(super) fn also_line(&self, story: &Story, home: usize, hunk: Option<String>) -> Line {
        let theme = self.theme();
        let title = story.pages.get(home - 1).map_or("", |c| c.title.as_str());
        Line::Also(
            home,
            hunk,
            vec![
                Span::styled(" ↗ ", Tone::Accent.style(&theme)),
                Span::styled(
                    format!(
                        "also {}: {title}",
                        story::in_page(story, home, Some(self.chapter()))
                    ),
                    Tone::Dim.style(&theme),
                ),
                Span::styled("   its home · ↵ goes there", Tone::Faint.style(&theme)),
            ],
        )
    }

    /// The switch between the two modes at the head of easy mode's row of
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

    /// The first file someone wrote among those listed, else the first.
    /// The strip's chip: Files mode on the first file of a page no
    /// chapter holds, which only Files mode shows.
    pub(super) fn open_files_only(&mut self) {
        let Some(story) = self.told_map() else {
            return;
        };
        let (pages, _) = Self::files_only(story);
        self.strip = None;
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

    pub(super) fn first_written(&self) -> usize {
        self.order
            .iter()
            .position(|&i| !order::generated(&self.entries()[i].path))
            .unwrap_or(0)
    }
}
