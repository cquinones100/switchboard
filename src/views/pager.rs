//! The pager every view is shown in, as less showed glow's output for
//! pr-view.sh (`less -RS~ --mouse`): less's keys, a line too wide for the
//! pane cut at its edge for Left and Right to scroll to (`-S`), where
//! wrapping would break a drawing's boxes apart, nothing past the end (`-~`),
//! and "(END)" on the prompt line at the end, nothing else.
//!
//! A resize lays the Markdown out again for the new width and opens it at
//! the line holding what was at the top (the WINCH trap's work in
//! pr-view.sh, which closed less, drew again and opened it at that line);
//! new content handed over while it is open (the copy read again in the
//! background) is laid out the same way, where it was.
//!
//! It draws with escapes of its own rather than through ratatui's cells,
//! since a link has to reach the terminal as an OSC 8 hyperlink around its
//! text, which a cell can't carry; Switchboard's multiplexer passes it on
//! (src/mux/compose.rs), so the terminal knows it as a link. A plain click
//! on one opens it from here too (the mouse is taken, as `less --mouse`
//! took it), through `links::open_url`, saying where it is going on its
//! prompt line first, as `links::opening_words` words it.
//!
//! Where the terminal can show images, the pictures on lines of their own
//! and the Mermaid diagrams are drawn as pictures once they are had
//! (`super::pictures`), each laid out again where it was as one arrives.
//!
//! A board (`super::board`, the PR and the ticket) scrolls its
//! column as any text, its panel fixed beside it in a pane wide enough, and
//! names its keys on the prompt line. One with a menu (the PR's) opens it
//! with m, a right click or a click on the prompt line's "m view actions"
//! (`super::menu`).
//!
//! Keys: j, k, Down, Up, Enter, e and y a line; Space, f, b, PageDown and
//! PageUp a screen; d and u half of one; g and G the top and the end; Left
//! and Right half a screen across; `/` searches (smart case), n and N go to
//! the next match and the one before; o opens the page being read in the
//! browser, where it has one; q or Esc closes, and Ctrl-C does nothing, as
//! in less. The wheel scrolls.
//!
//! A view that follows what it shows (`super::follow`, the PR and the
//! ticket) reads it again on r, and when it takes the focus after five
//! minutes or more without it, and says at the prompt line's right end how
//! old its copy is once that is five minutes ("read 12m ago").
//!
//! Markdown and a board's column have a cursor, with Settings' "A cursor in the views"
//! (`view_cursor`) on: the terminal's cursor on a character, and a yellow
//! `›` in the margin of its line, its `›` on every row of a picture or
//! diagram, the screen scrolling only to keep it in sight. Its keys are the
//! shared Vim's (crates/vim), read-only, as the code viewer's are, over the
//! text as it shows (no markup): every motion and text object, counts, `/`
//! and `?` with `n` and `N`, marks, `gg`, `G`, Ctrl-D and Ctrl-U, and
//! Visual mode (`v`, `V`, Ctrl-V) with `y` copying the selection to the
//! clipboard (`platform::copy`); a key that would change the text says
//! Vim's E21. What the view adds on top keeps its key where Vim's would
//! change the text or the view has it: `q` and Esc in Normal mode close it,
//! `m` opens a board's menu (setting a mark in a view with none), `o` opens
//! the page, `r` reads it again, Space pages down, and Enter or a click on a
//! picture or diagram shows it full size over the view (`Act::Full`,
//! `switchboard picture`), as Enter on a note's picture does; Enter
//! anywhere else is Vim's. A line too wide is still cut at the pane's edge,
//! the view scrolling across to keep the cursor in sight. Off, or for a log
//! or a file, the keys scroll as less's do.

use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant, SystemTime};

use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use crossterm::terminal;

use super::board::{self, Board};
use super::follow::{self, Follow, Update};
use super::markdown::{Doc, Markdown, Span, Style, expand_tabs};
use super::pictures::{Got, Pictures};
use crate::ui::draw::wrap;
use switchboard_ui::input::{Key, KeyCode as VimCode, Mods};
use switchboard_vim::{Effect, Folding, Mode, Note, Text};

/// What the view keeps beside the text Vim reads: its pictures' rows, a
/// picture or diagram being one line to Vim's motions, as a closed fold is.
#[derive(Clone, Debug, Default)]
struct ViewNote;

impl Note for ViewNote {
    type Folds = PictureRows;
}

/// Each picture's rows past its first, hidden under it.
#[derive(Clone, Debug, Default)]
struct PictureRows {
    closed: Vec<(usize, usize)>,
}

impl Folding for PictureRows {
    fn shown(&self, lines: &[String]) -> Vec<(usize, Option<usize>)> {
        let mut shown = Vec::with_capacity(lines.len());
        let mut row = 0;
        while row < lines.len() {
            match self.closed.iter().find(|(s, _)| *s == row) {
                Some(&(s, e)) => {
                    shown.push((s, Some(e - s)));
                    row = e + 1;
                }
                None => {
                    shown.push((row, None));
                    row += 1;
                }
            }
        }
        shown
    }

    fn closed_over(&self, _lines: &[String], row: usize) -> Option<(usize, usize)> {
        self.closed
            .iter()
            .copied()
            .find(|(s, e)| (*s..=*e).contains(&row))
    }
}

type Editor = switchboard_vim::Editor<ViewNote>;

/// Lines the wheel scrolls at a time.
const WHEEL: usize = 3;
const DIM: u8 = 244;
const RED: u8 = 203;
/// The cursor's `›`, the yellow every chosen row has.
const CURSOR: u8 = 220;

/// What a view shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    /// Still being read: what it says meanwhile ("Reading the PR...").
    Loading(String),
    /// It can't be read, and why; o opens the page in the browser when
    /// there is one, and any other key closes it.
    Failed(String),
    /// Markdown; `place` says where a diagram that can't be drawn here is
    /// shown properly ("M-r on GitHub").
    Markdown { text: String, place: String },
    /// The PR or the ticket as UX 2.0's boards draw them: a column with a
    /// panel beside it, or under it in a narrow pane (`super::board`).
    Board(Board),
    /// Something that can't be read here, with a link o opens (a Slack
    /// message); any other key closes it.
    Offer { text: String, url: String },
    /// Plain text, a line each, opened at its end, as `less +G` opened a
    /// CI job's log.
    Log(String),
    /// Plain text, a line each, opened at a line (from 1), as `less +N`
    /// opens a file (`switchboard view +<line> <file>`).
    Text { text: String, line: usize },
}

/// What came of a key or the mouse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    Same,
    Redraw,
    Quit,
    /// A link to open in the browser, the pager staying open.
    Open(String),
    /// A link to open, and then close.
    OpenAndQuit(String),
    /// The PR or ticket read again (`super::follow`).
    Again,
    /// A picture or diagram to show full size over the view: its key
    /// (`markdown::Wanted::key`) and what it is called.
    Full {
        key: String,
        title: String,
    },
    /// One of the workspace's actions run in the view's tab, from its menu
    /// (`switchboard press <name>`).
    Press(String),
}

pub struct Pager {
    content: Content,
    markdown: Option<Markdown>,
    doc: Doc,
    cols: usize,
    rows: usize,
    top: usize,
    left: usize,
    /// The last search, its matches lit.
    query: Option<String>,
    /// A search being typed after `/`.
    typing: Option<String>,
    /// What started the line being typed with the cursor: `/`, `?` or `:`.
    typing_kind: char,
    /// The shared Vim over the text shown, which moves the cursor while
    /// there is one (`has_cursor`).
    vim: Editor,
    /// How a yank reaches the clipboard.
    copy: fn(&str),
    /// What the prompt line says for a moment ("Pattern not found").
    note: Option<String>,
    /// The page being read, which o opens.
    url: Option<String>,
    /// The pictures and diagrams drawn as images, where they can be.
    pictures: Option<Pictures>,
    /// A board's panel, laid out, when it is beside the column, and the
    /// column's width then.
    panel: Option<(Doc, usize)>,
    look: board::Look,
    /// Whether Markdown and a board's column have a cursor (`view_cursor`),
    /// and the line it is on.
    cursor_on: bool,
    cursor: usize,
    /// Whether the text is laid out a column narrower for a scroll bar at
    /// the pane's right edge (Settings' `scroll_bars`); a board's panel
    /// beside the column draws the bar in its rule instead.
    bar: bool,
    /// The scroll bar's thumb being dragged, from where (`kit::scroll_grab`).
    dragging: Option<crate::ui::kit::Grab>,
    /// Whether it follows what it shows (`super::follow`), so r reads it
    /// again.
    follows: bool,
    /// When the copy shown was read.
    read_at: Option<SystemTime>,
    /// Whether r asked for the read under way, which then says how it went.
    asked: bool,
    /// Since when the view has been without the focus.
    unfocused: Option<Instant>,
    /// A board's menu, while it is up.
    menu: Option<super::menu::Open>,
    /// Whether the terminal reports every movement of the mouse, as it
    /// does while the menu is up.
    any_motion: bool,
}

/// The prompt line's words for the menu, which a click opens.
const VIEW_ACTIONS: &str = "m view actions";

/// What the prompt line says while r's read is under way.
const READING: &str = "Reading it again...";

fn said(text: &str, style: Style) -> Vec<Span> {
    vec![Span::new(format!("  {text}"), style)]
}

impl Pager {
    pub fn new(content: Content, cols: usize, rows: usize) -> Pager {
        let mut pager = Pager {
            content: Content::Loading(String::new()),
            markdown: None,
            doc: Doc::default(),
            cols: cols.max(1),
            rows: rows.max(2),
            top: 0,
            left: 0,
            query: None,
            typing: None,
            typing_kind: '/',
            vim: {
                let mut vim = Editor::new(Text::default());
                vim.read_only = true;
                vim.clipboard = true;
                vim
            },
            copy: |text| {
                let _ = crate::platform::copy(text);
            },
            note: None,
            url: None,
            pictures: None,
            panel: None,
            look: board::Look::default(),
            cursor_on: false,
            cursor: 0,
            bar: false,
            dragging: None,
            follows: false,
            read_at: None,
            asked: false,
            unfocused: None,
            menu: None,
            any_motion: false,
        };
        pager.set_content(content);
        pager
    }

    /// The page being read, which o opens in the browser.
    pub fn with_url(mut self, url: Option<String>) -> Pager {
        self.url = url;
        // A failure's keys depend on whether there is a page to open.
        if matches!(self.content, Content::Failed(_)) {
            self.lay_out();
        }
        self
    }

    /// Follows what it shows, a copy read at `read_at`.
    pub fn following(mut self, read_at: Option<SystemTime>) -> Pager {
        self.follows = true;
        self.read_at = read_at;
        // A failure's keys depend on whether it can be read again.
        if matches!(self.content, Content::Failed(_)) {
            self.lay_out();
        }
        self
    }

    /// What a read came to: a new copy laid out where the old was, and
    /// when it was read; one r asked for says when it couldn't be read.
    pub fn update(&mut self, update: Update, now: SystemTime) {
        if self.note.as_deref() == Some(READING) {
            self.note = None;
        }
        match update {
            Update::Show(content) => {
                if !matches!(content, Content::Failed(_)) {
                    self.read_at = Some(now);
                }
                self.set_content(content);
            }
            Update::Same => self.read_at = Some(now),
            Update::Kept(why) if self.asked => {
                self.note = Some(format!("Couldn't read it again: {why}"));
            }
            Update::Kept(_) => {}
        }
        self.asked = false;
    }

    /// The focus taken or lost. Whether to read again: taken after
    /// `follow::STALE` or more without it.
    pub fn focus(&mut self, gained: bool, now: Instant) -> bool {
        if !gained {
            self.unfocused.get_or_insert(now);
            return false;
        }
        let since = self.unfocused.take();
        self.follows && since.is_some_and(|at| now.duration_since(at) >= follow::STALE)
    }

    /// How old the copy shown is, once that is worth saying.
    pub fn age(&self, now: SystemTime) -> Option<String> {
        follow::age(self.read_at.filter(|_| self.follows)?, now)
    }

    /// Gives Markdown a cursor, on the first line with something on it.
    pub fn with_cursor(mut self, on: bool) -> Pager {
        self.cursor_on = on;
        self.cursor = self.snap(self.top);
        self.vim_follow_layout();
        self
    }

    /// The text Vim reads made the lines as laid out now, its cursor on the
    /// cursor's line, its column kept as far as the line allows, and its
    /// search kept.
    fn vim_follow_layout(&mut self) {
        if !self.has_cursor() {
            return;
        }
        let lines: Vec<String> = self.doc.lines.iter().map(|l| l.plain()).collect();
        let text = Text::from_lines(&lines);
        let first = self.vim.doc.text.lines() == [""];
        if text != self.vim.doc.text {
            // The text laid out again (a resize, a picture arriving, a new
            // copy read) taken in as a change from outside, so the marks
            // follow their lines and a selection, the search and the
            // register stay.
            let mode = self.vim.mode;
            self.vim.change_outside(|doc| doc.text = text);
            self.vim.mode = mode;
            let last = lines.len().saturating_sub(1);
            let row = self.vim.anchor.0.min(last);
            let col = self.vim.anchor.1.min(lines.get(row).map_or(0, String::len));
            self.vim.anchor = (row, col);
        }
        if first || self.vim.row() != self.cursor {
            // A new view opens on its line's text, past its margin.
            let col = match self.vim.col() {
                0 => lines
                    .get(self.cursor)
                    .map_or(0, |l| l.len() - l.trim_start().len()),
                col => col,
            };
            let mode = self.vim.mode;
            self.vim.go_to(self.cursor, col);
            self.vim.mode = mode;
        }
        self.vim.folds = PictureRows {
            closed: (0..self.doc.lines.len())
                .filter(|i| self.doc.lines[*i].picture.is_some_and(|p| p.row == 0))
                .map(|i| (i, self.picture_end(i)))
                .filter(|(s, e)| e > s)
                .collect(),
        };
        self.vim.height = self.body();
    }

    /// Draws its pictures and diagrams as images with `pictures`, those
    /// shown now asked for at once.
    pub fn with_pictures(mut self, pictures: Pictures) -> Pager {
        self.pictures = Some(pictures);
        self.ask_pictures();
        self.lay_out();
        self
    }

    /// How a board is drawn: the theme and the terminal's half circles.
    pub fn with_look(mut self, look: board::Look) -> Pager {
        self.look = look;
        self.lay_out();
        self
    }

    fn ask_pictures(&mut self) {
        let Some(pictures) = &mut self.pictures else {
            return;
        };
        match (&self.content, &self.markdown) {
            (Content::Board(b), _) => pictures.ask(b.wanted(&b.place)),
            (_, Some(md)) => pictures.ask(md.wanted()),
            _ => {}
        }
    }

    /// Whether it shows a text laid out for the width, which a resize or
    /// new content opens at the line holding what was at the top.
    fn laid_out(&self) -> bool {
        self.markdown.is_some() || matches!(self.content, Content::Board(_))
    }

    /// A picture or diagram had: laid out again with it, the top line kept.
    /// Whether anything changed.
    pub fn picture(&mut self, got: Got) -> bool {
        let changed = self.pictures.as_mut().is_some_and(|p| p.got(got));
        if changed {
            let anchor = self.doc.lines.get(self.top).map(|l| l.anchor);
            let at = self.cursor_anchor();
            self.lay_out();
            if let Some(anchor) = anchor {
                self.top = self.doc.line_at(anchor);
            }
            self.clamp();
            self.put_cursor_back(at);
        }
        changed
    }

    /// The graphics commands that bring the terminal's pictures to what the
    /// layout places, sent before the frame.
    pub fn graphics(&mut self) -> Vec<u8> {
        let mut placed = self.doc.placed.clone();
        if let Some((panel, _)) = &self.panel {
            placed.extend(panel.placed.iter().cloned());
        }
        match &mut self.pictures {
            Some(p) => p.commands(&placed),
            None => Vec::new(),
        }
    }

    /// Shows new content: Markdown laid out where the old was, at the line
    /// holding what was at the top.
    pub fn set_content(&mut self, content: Content) {
        let anchor = self
            .laid_out()
            .then(|| self.doc.lines.get(self.top))
            .flatten()
            .map(|l| l.anchor);
        let at = self.cursor_anchor();
        self.markdown = match &content {
            Content::Markdown { text, place } => Some(Markdown::parse(text, place)),
            _ => None,
        };
        self.content = content;
        self.ask_pictures();
        self.lay_out();
        self.top = match anchor {
            Some(anchor) if self.laid_out() => self.doc.line_at(anchor),
            _ if matches!(self.content, Content::Log(_)) => self.last_top(),
            // A couple of lines above it, to read it in.
            _ if let Content::Text { line, .. } = self.content => line.saturating_sub(3),
            _ => 0,
        };
        self.clamp();
        self.put_cursor_back(at);
    }

    /// The pane's new size: laid out again, the top line kept.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let anchor = self.doc.lines.get(self.top).map(|l| l.anchor);
        let at = self.cursor_anchor();
        self.cols = cols.max(1);
        self.rows = rows.max(2);
        self.lay_out();
        if let Some(anchor) = anchor.filter(|_| self.laid_out()) {
            self.top = self.doc.line_at(anchor);
        }
        self.clamp();
        self.put_cursor_back(at);
    }

    /// Laid out at the pane's width, and again a column narrower when it
    /// runs past the screen with no panel beside it, for the scroll bar.
    fn lay_out(&mut self) {
        self.bar = false;
        self.lay_out_at_width();
        let overflows = crate::ui::kit::has_scrollbar(self.doc.lines.len(), self.body());
        if overflows && self.panel.is_none() && self.cols > 1 {
            self.cols -= 1;
            self.lay_out_at_width();
            self.cols += 1;
            self.bar = true;
        }
        // Vim reads the text as it is now drawn, whatever laid it out again.
        self.vim_follow_layout();
    }

    fn lay_out_at_width(&mut self) {
        let body = self.body();
        let images = self.pictures.as_mut().map(|p| p.layout(body));
        self.panel = None;
        if let Content::Board(b) = &self.content {
            let beside = self.cols >= board::PANEL_MIN;
            let side = board::panel_width(self.cols);
            // The panel's sections beside the column while they fit its
            // height and width (a code line kept whole may not), the rest
            // under the column, where it scrolls, so none is out of reach.
            let mut kept: Vec<board::Part> = Vec::new();
            let mut under: Vec<board::Part> = Vec::new();
            for section in board::sections(&b.panel) {
                let mut with = kept.clone();
                with.extend(section.iter().cloned());
                let laid = beside && under.is_empty();
                let fits = laid && {
                    let doc = board::lay_out(&with, side, &b.place, images.as_ref(), &self.look);
                    doc.lines.len() <= body && doc.lines.iter().all(|l| l.width() <= side)
                };
                if fits {
                    kept = with;
                } else {
                    under.extend(section.iter().cloned());
                }
            }
            let main_cols = if kept.is_empty() {
                self.cols
            } else {
                self.cols - side - 1
            };
            let mut parts = b.main.clone();
            if !under.is_empty() {
                parts.extend([board::Part::Blank, board::Part::Rule, board::Part::Blank]);
                parts.extend(under);
            }
            parts.push(board::Part::Blank);
            self.doc = board::lay_out(&parts, main_cols, &b.place, images.as_ref(), &self.look);
            if !kept.is_empty() {
                let laid = board::lay_out(&kept, side, &b.place, images.as_ref(), &self.look);
                self.panel = Some((laid, main_cols));
            }
            return;
        }
        self.doc = match (&self.content, &self.markdown) {
            (_, Some(md)) => md.layout_with(self.cols, images.as_ref()),
            (Content::Loading(text), _) => Doc::plain(vec![vec![], said(text, Style::fg(DIM))]),
            (Content::Failed(text), _) => {
                let mut lines = vec![vec![]];
                for (i, line) in wrap(text, self.cols.saturating_sub(4))
                    .into_iter()
                    .enumerate()
                {
                    let style = if i == 0 {
                        Style::fg(RED)
                    } else {
                        Style::fg(252)
                    };
                    lines.push(said(&line, style));
                }
                lines.push(vec![]);
                let keys = if self.url.is_some() {
                    "o opens it in the browser · any other key closes"
                } else if self.follows {
                    "r reads it again · any other key closes"
                } else {
                    "Press any key to close."
                };
                lines.push(said(keys, Style::fg(DIM)));
                Doc::plain(lines)
            }
            (Content::Offer { text, .. }, _) => {
                let mut lines = vec![vec![]];
                for line in wrap(text, self.cols.saturating_sub(4)) {
                    lines.push(said(&line, Style::fg(252)));
                }
                lines.push(vec![]);
                lines.push(said(
                    "o opens it in the browser · any other key closes",
                    Style::fg(DIM),
                ));
                Doc::plain(lines)
            }
            (Content::Log(text) | Content::Text { text, .. }, _) => Doc::plain(
                text.lines()
                    .map(|l| vec![Span::new(expand_tabs(l), Style::fg(252))])
                    .collect(),
            ),
            (Content::Markdown { .. } | Content::Board(_), None) => Doc::default(),
        };
    }

    fn body(&self) -> usize {
        self.rows.saturating_sub(1).max(1)
    }

    fn last_top(&self) -> usize {
        self.doc.lines.len().saturating_sub(self.body())
    }

    /// The top kept within the document, and the scroll across within its
    /// widest line, so a layout that got narrower (a picture in place of a
    /// wide text drawing) never leaves the screen past it.
    fn clamp(&mut self) {
        self.top = self.top.min(self.last_top());
        let widest = self.doc.lines.iter().map(|l| l.width()).max().unwrap_or(0);
        self.left = self.left.min(widest.saturating_sub(self.across_cols()));
    }

    fn at_end(&self) -> bool {
        self.top >= self.last_top()
    }

    /// The line at the top, for the tests and the resize.
    pub fn top(&self) -> usize {
        self.top
    }

    pub fn doc(&self) -> &Doc {
        &self.doc
    }

    fn scroll_to(&mut self, top: usize) -> Act {
        let top = top.min(self.last_top());
        if top == self.top {
            return Act::Same;
        }
        self.top = top;
        Act::Redraw
    }

    fn scroll_by(&mut self, lines: isize) -> Act {
        let to = (self.top as isize + lines).max(0) as usize;
        self.scroll_to(to)
    }

    /// The line a search goes on from: the cursor's, or the top.
    fn place(&self) -> usize {
        if self.has_cursor() {
            self.cursor
        } else {
            self.top
        }
    }

    fn has_cursor(&self) -> bool {
        self.cursor_on && self.laid_out()
    }

    /// Where the cursor is in the text, to find it again in a new layout.
    fn cursor_anchor(&self) -> Option<super::markdown::Anchor> {
        self.has_cursor()
            .then(|| self.doc.lines.get(self.cursor).map(|l| l.anchor))
            .flatten()
    }

    /// The cursor on the line holding what it was on before a new layout,
    /// or on the first line on the screen when it had none.
    fn put_cursor_back(&mut self, at: Option<super::markdown::Anchor>) {
        if !self.has_cursor() {
            return;
        }
        self.cursor = self.snap(at.map_or(self.top, |a| self.doc.line_at(a)));
        self.show_cursor();
        self.vim_follow_layout();
    }

    /// The first row of the picture line `i` is a row of, or `i`.
    fn picture_start(&self, i: usize) -> usize {
        match self.doc.lines.get(i).and_then(|l| l.picture) {
            Some(p) => i.saturating_sub(p.row as usize),
            None => i,
        }
    }

    /// The last row of the picture line `i` starts, or `i`.
    fn picture_end(&self, i: usize) -> usize {
        let Some(p) = self.doc.lines.get(i).and_then(|l| l.picture) else {
            return i;
        };
        let mut end = i;
        while self
            .doc
            .lines
            .get(end + 1)
            .and_then(|l| l.picture)
            .is_some_and(|q| q.id == p.id && q.row as usize == p.row as usize + (end + 1 - i))
        {
            end += 1;
        }
        end
    }

    /// Whether the cursor stops on line `i`: a picture's first row, or a
    /// line with something on it but a rule and a quote's bar.
    fn is_stop(&self, i: usize) -> bool {
        self.doc.lines.get(i).is_some_and(|l| match l.picture {
            Some(p) => p.row == 0,
            None => l
                .plain()
                .chars()
                .any(|c| !c.is_whitespace() && c != '─' && c != '│'),
        })
    }

    /// The stop at or after line `i`, else the last one before it.
    fn snap(&self, i: usize) -> usize {
        let i = self.picture_start(i.min(self.doc.lines.len().saturating_sub(1)));
        (i..self.doc.lines.len())
            .find(|j| self.is_stop(*j))
            .or_else(|| (0..i).rev().find(|j| self.is_stop(*j)))
            .unwrap_or(0)
    }

    /// The screen scrolled as little as keeps the cursor's line, or the
    /// whole of its picture, in sight.
    fn show_cursor(&mut self) {
        let end = self.picture_end(self.cursor);
        let body = self.body();
        if end >= self.top + body {
            self.top = end + 1 - body;
        }
        if self.cursor < self.top {
            self.top = self.cursor;
        }
        self.top = self.top.min(self.last_top());
    }

    fn move_cursor(&mut self, to: usize) -> Act {
        let before = (self.cursor, self.top);
        self.cursor = self.snap(to);
        self.show_cursor();
        if (self.cursor, self.top) == before {
            Act::Same
        } else {
            Act::Redraw
        }
    }

    /// The screen and the cursor moved by `lines` together, as far as the
    /// text goes.
    fn page_by(&mut self, lines: isize) -> Act {
        let before = (self.cursor, self.top);
        self.top = ((self.top as isize + lines).max(0) as usize).min(self.last_top());
        let mut to = (self.cursor as isize + lines).max(0) as usize;
        // Forward into a picture goes past it, where its first row would
        // take the cursor back and leave a tall one stuck.
        if lines > 0 && self.picture_start(to) < to {
            to = self.picture_end(to) + 1;
        }
        self.cursor = self.snap(to);
        self.show_cursor();
        if (self.cursor, self.top) == before {
            Act::Same
        } else {
            Act::Redraw
        }
    }

    /// The cursor brought back onto the screen after the wheel scrolled it.
    /// The first stop wholly in sight from the top, or the last from the
    /// bottom, else, with the screen inside a tall picture, that picture.
    fn keep_cursor_in_sight(&mut self) {
        if !self.has_cursor() {
            return;
        }
        let bottom = self.top + self.body() - 1;
        let shown = self.top..=bottom.min(self.doc.lines.len().saturating_sub(1));
        let picture_shown = || {
            shown
                .clone()
                .find(|j| self.doc.lines[*j].picture.is_some())
                .map(|j| self.picture_start(j))
        };
        if self.cursor < self.top {
            let first = shown
                .clone()
                .find(|j| self.is_stop(*j) && self.picture_start(*j) >= self.top);
            if let Some(j) = first.or_else(picture_shown) {
                self.cursor = j;
            }
        } else if self.picture_end(self.cursor) > bottom {
            let last = shown
                .clone()
                .rev()
                .find(|j| self.is_stop(*j) && self.picture_end(*j) <= bottom);
            if let Some(j) = last.or_else(picture_shown) {
                self.cursor = j;
            }
        }
    }

    /// The picture or diagram on line `i`, to show full size: its key and
    /// what it is called (a picture's words, else its file's name).
    fn full_at(&self, i: usize) -> Option<Act> {
        let start = self.picture_start(i);
        let id = self.doc.lines.get(start)?.picture?.id;
        let key = self.doc.placed.iter().find(|p| p.id == id)?.key.clone();
        let title = match key.strip_prefix("mermaid:") {
            Some(_) => "Diagram".to_string(),
            None => {
                let words = self.doc.lines[start].searched.as_str();
                let alt = words
                    .strip_prefix("Image:")
                    .unwrap_or("")
                    .trim_end_matches('→')
                    .trim();
                if alt.is_empty() {
                    key.split(['?', '#'])
                        .next()
                        .and_then(|k| k.rsplit('/').next())
                        .filter(|n| !n.is_empty())
                        .unwrap_or("Picture")
                        .to_string()
                } else {
                    alt.to_string()
                }
            }
        };
        Some(Act::Full { key, title })
    }

    /// The columns of each match of the search in a line's text: the case
    /// ignored unless the search has a capital, as less's `-i` did.
    fn matches(&self, text: &str) -> Vec<(usize, usize)> {
        let Some(query) = self.query.as_deref().filter(|q| !q.is_empty()) else {
            return Vec::new();
        };
        let fold = !query.chars().any(char::is_uppercase);
        let (hay, needle) = if fold {
            (text.to_lowercase(), query.to_lowercase())
        } else {
            (text.to_string(), query.to_string())
        };
        // Lower-casing can change a string's length, so columns are counted
        // on the text searched, which is the line's own outside of that.
        let mut found = Vec::new();
        let mut from = 0;
        while let Some(at) = hay[from..].find(&needle) {
            let start = from + at;
            let end = start + needle.len();
            let col = super::markdown::width(&hay[..start]);
            found.push((col, col + super::markdown::width(&hay[start..end])));
            from = end.max(start + 1);
            while !hay.is_char_boundary(from) {
                from += 1;
            }
            if from >= hay.len() {
                break;
            }
        }
        found
    }

    /// The next line holding a match from line `from` on (or, backwards,
    /// the last one before it), brought to the top; at the end, as far as
    /// the end allows, which keeps it in sight.
    fn find(&mut self, from: usize, forward: bool) -> Act {
        if self.query.is_none() {
            return Act::Same;
        }
        let hit = |i: &usize| !self.matches(&self.doc.lines[*i].searchable()).is_empty();
        let found = if forward {
            (from..self.doc.lines.len()).find(hit)
        } else {
            (0..from.min(self.doc.lines.len())).rev().find(hit)
        };
        // A board's panel is in sight whatever the scroll, its matches lit.
        let in_panel = self.panel.as_ref().is_some_and(|(panel, _)| {
            panel
                .lines
                .iter()
                .any(|l| !self.matches(&l.searchable()).is_empty())
        });
        match found {
            Some(i) => {
                self.top = i.min(self.last_top());
                if self.has_cursor() {
                    self.cursor = self.picture_start(i);
                    self.show_cursor();
                }
            }
            None if in_panel => {}
            None => self.note = Some("Pattern not found".into()),
        }
        Act::Redraw
    }

    /// A key while a search is typed after `/`.
    fn typing_key(&mut self, key: KeyEvent) -> Act {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let vim = self.has_cursor();
        let Some(typed) = &mut self.typing else {
            return Act::Same;
        };
        match key.code {
            KeyCode::Enter if vim => {
                let typed = self.typing.take().unwrap_or_default();
                match self.typing_kind {
                    ':' => self.vim.ex(&typed),
                    kind => {
                        if !typed.is_empty() {
                            self.query = Some(typed.clone());
                        }
                        let from = self.cursor;
                        self.vim.search_for(&typed, kind == '/');
                        self.vim.prompt_done();
                        return self.vim_found_or_find(from, kind == '/');
                    }
                }
                self.vim.prompt_done();
                return self.vim_after();
            }
            KeyCode::Enter => {
                let query = self.typing.take().unwrap_or_default();
                if !query.is_empty() {
                    self.query = Some(query);
                }
                // From the top line on: what is at the top counts.
                return self.find(self.top, true);
            }
            KeyCode::Esc => self.typing = None,
            KeyCode::Backspace => {
                if typed.pop().is_none() {
                    self.typing = None;
                }
            }
            KeyCode::Char('u') if ctrl => typed.clear(),
            KeyCode::Char(c) if !ctrl => typed.push(c),
            _ => return Act::Same,
        }
        Act::Redraw
    }

    pub fn key(&mut self, key: KeyEvent) -> Act {
        if key.kind == KeyEventKind::Release {
            return Act::Same;
        }
        if let Some(menu) = &mut self.menu {
            let came = menu.key(key);
            return self.menu_came(came);
        }
        let had_note = self.note.take().is_some();
        if self.typing.is_some() {
            return self.typing_key(key);
        }
        match &self.content {
            Content::Failed(_) => {
                return match (key.code, &self.url) {
                    (KeyCode::Char('o'), Some(url)) => Act::OpenAndQuit(url.clone()),
                    // A read that failed with nothing to show is tried again.
                    (KeyCode::Char('r'), _) if self.follows => {
                        self.note = Some(READING.into());
                        self.asked = true;
                        Act::Again
                    }
                    _ => Act::Quit,
                };
            }
            Content::Offer { url, .. } => {
                return match key.code {
                    KeyCode::Char('o') => Act::OpenAndQuit(url.clone()),
                    _ => Act::Quit,
                };
            }
            _ => {}
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let page = self.body() as isize;
        let half = (page / 2).max(1);
        if self.has_cursor()
            && let Some(act) = self.vim_key(key)
        {
            return if had_note { act_or_redraw(act) } else { act };
        }
        let act = match key.code {
            KeyCode::Char('q' | 'Q') | KeyCode::Esc => Act::Quit,
            // Ctrl-C leaves the view up, as less did under pr-view.sh's
            // `trap '' INT`, so a Ctrl-C meant for the pane beside it, a
            // moment after the focus moved, loses nothing.
            KeyCode::Char('c') if ctrl => Act::Same,
            KeyCode::Char('n' | 'e' | 'j') if ctrl => self.scroll_by(1),
            KeyCode::Char('p' | 'y' | 'k') if ctrl => self.scroll_by(-1),
            KeyCode::Char('f' | 'v') if ctrl => self.scroll_by(page),
            KeyCode::Char('b') if ctrl => self.scroll_by(-page),
            KeyCode::Char('d') if ctrl => self.scroll_by(half),
            KeyCode::Char('u') if ctrl => self.scroll_by(-half),
            KeyCode::Char('j' | 'e') | KeyCode::Down | KeyCode::Enter => self.scroll_by(1),
            KeyCode::Char('k' | 'y') | KeyCode::Up => self.scroll_by(-1),
            KeyCode::Char(' ' | 'f') | KeyCode::PageDown => self.scroll_by(page),
            KeyCode::Char('b') | KeyCode::PageUp => self.scroll_by(-page),
            KeyCode::Char('d') => self.scroll_by(half),
            KeyCode::Char('u') => self.scroll_by(-half),
            KeyCode::Char('g' | '<') | KeyCode::Home => self.scroll_to(0),
            KeyCode::Char('G' | '>') | KeyCode::End => self.scroll_to(usize::MAX),
            KeyCode::Left => {
                let before = self.left;
                self.left = self.left.saturating_sub(self.text_cols() / 2);
                if self.left == before {
                    Act::Same
                } else {
                    Act::Redraw
                }
            }
            KeyCode::Right => {
                let widest = self.doc.lines.iter().map(|l| l.width()).max().unwrap_or(0);
                if self.left + self.across_cols() < widest {
                    self.left += (self.text_cols() / 2).max(1);
                    Act::Redraw
                } else {
                    Act::Same
                }
            }
            KeyCode::Char('/') => {
                self.typing = Some(String::new());
                Act::Redraw
            }
            // From the cursor's line where there is one, else the top's.
            KeyCode::Char('n') => self.find(self.place() + 1, true),
            KeyCode::Char('N') => self.find(self.place(), false),
            KeyCode::Char('o') => match &self.url {
                Some(url) => Act::Open(url.clone()),
                None => Act::Same,
            },
            KeyCode::Char('r') if self.follows => self.read_again(),
            KeyCode::Char('m') => self.open_menu(None),
            _ => Act::Same,
        };
        if had_note { act_or_redraw(act) } else { act }
    }

    /// r: what it shows read again, the prompt line saying so meanwhile.
    fn read_again(&mut self) -> Act {
        self.note = Some(READING.into());
        self.asked = true;
        Act::Again
    }

    /// A board's menu opened, beside `near` (a click's cell) or in the
    /// middle; nothing for a view with no menu.
    fn open_menu(&mut self, near: Option<(u16, u16)>) -> Act {
        let Some((title, actions)) = self.menu_of() else {
            return Act::Same;
        };
        self.menu = Some(super::menu::Open::new(title, self.follows, actions, near));
        Act::Redraw
    }

    /// The menu what it shows has, its title and actions; none for one
    /// with no actions that doesn't read again.
    fn menu_of(&self) -> Option<(&str, &[(String, board::Do)])> {
        let (title, actions) = match &self.content {
            Content::Board(b) => (b.title.as_str(), b.actions.as_slice()),
            _ => return None,
        };
        // One with a menu (a title) that reads again has it with that alone.
        let has = !actions.is_empty() || (self.follows && !title.is_empty());
        has.then_some((title, actions))
    }

    /// What the menu's key or click came to.
    fn menu_came(&mut self, came: super::menu::Came) -> Act {
        use super::board::Do;
        use super::menu::{Came, Choice};
        match came {
            Came::Up(true) => Act::Redraw,
            Came::Up(false) => Act::Same,
            Came::Closed(chosen) => {
                self.menu = None;
                match chosen {
                    Some(Choice::Again) => self.read_again(),
                    Some(Choice::Board(Do::Open(url))) => Act::Open(url),
                    Some(Choice::Board(Do::Press(name))) => Act::Press(name),
                    None => Act::Redraw,
                }
            }
        }
    }

    /// Whether `col` of the prompt line is on its "m view actions".
    fn on_view_actions(&self, col: usize) -> bool {
        if self.menu_of().is_none() || self.typing.is_some() || self.note.is_some() {
            return false;
        }
        let Content::Board(b) = &self.content else {
            return false;
        };
        let mut keys = b.keys.clone();
        if self.has_cursor() && self.full_at(self.cursor).is_some() {
            keys.insert(0, ("Enter".to_string(), "full size".to_string()));
        }
        let text = board::hints(&keys, &self.look).plain();
        let Some(at) = text.find(VIEW_ACTIONS) else {
            return false;
        };
        let from = crate::ui::draw::width(&text[..at]);
        (from..from + VIEW_ACTIONS.len()).contains(&col)
    }

    /// A key while there is a cursor: the view's own (closing it, its
    /// menu, the page, reading it again, paging with Space, a picture full
    /// size), else read-only Vim's; None for one neither takes, which the
    /// rest of `key` does as less's.
    fn vim_key(&mut self, key: KeyEvent) -> Option<Act> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let idle = self.vim.mode == Mode::Normal && self.vim.no_keys();
        if idle {
            match key.code {
                KeyCode::Char('q' | 'Q') if !ctrl => return Some(Act::Quit),
                KeyCode::Esc => return Some(Act::Quit),
                KeyCode::Char('c') if ctrl => return Some(Act::Same),
                KeyCode::Char('m') if !ctrl && self.menu_of().is_some() => {
                    return Some(self.open_menu(None));
                }
                KeyCode::Char('o') if !ctrl => {
                    return Some(match &self.url {
                        Some(url) => Act::Open(url.clone()),
                        None => Act::Same,
                    });
                }
                KeyCode::Char('r') if !ctrl && self.follows => return Some(self.read_again()),
                KeyCode::Char(' ') => {
                    let page = self.body() as isize;
                    let act = self.page_by(page);
                    self.vim_follow_layout();
                    return Some(act);
                }
                KeyCode::Enter => {
                    if let Some(full) = self.full_at(self.cursor) {
                        return Some(full);
                    }
                }
                _ => {}
            }
        }
        let vim_key = vim_key_of(key)?;
        let searching = idle && !ctrl && matches!(key.code, KeyCode::Char('n' | 'N'));
        if searching {
            let from = self.cursor;
            self.vim.key(vim_key);
            let back = key.code == KeyCode::Char('N');
            let forward = (self.vim.search.as_ref().is_none_or(|(_, f)| *f)) != back;
            return Some(self.vim_found_or_find(from, forward));
        }
        let before = self.vim_state();
        self.vim.top = self.top;
        self.vim.height = self.body();
        self.vim.begin();
        self.vim.key(vim_key);
        self.vim.end();
        let act = self.vim_after_from(before.0);
        Some(if act == Act::Redraw && self.vim_state() == before {
            Act::Same
        } else {
            act
        })
    }

    /// What a Vim key can change of what is drawn.
    fn vim_state(&self) -> ((usize, usize), usize, usize, Mode, bool, bool) {
        (
            (self.vim.row(), self.vim.col()),
            self.top,
            self.left,
            self.vim.mode,
            self.note.is_some(),
            self.typing.is_some(),
        )
    }

    /// What a Vim key came to: its effects done, the cursor's line and the
    /// screen following it.
    fn vim_after(&mut self) -> Act {
        let from = (self.cursor, self.vim.col());
        self.vim_after_from(from)
    }

    /// A search Vim made from line `from`: where Vim found it, or, when
    /// its text (the lines as they show) has none, where the view finds
    /// it, by a picture's or diagram's words too and quiet about a match
    /// only in a board's panel, as its own search was.
    fn vim_found_or_find(&mut self, from: usize, forward: bool) -> Act {
        let effects = self.vim.effects();
        let missed = effects
            .iter()
            .any(|e| matches!(e, Effect::Message(m) if m.starts_with("E486") || m.starts_with("E384") || m.starts_with("E385")));
        if !missed {
            return self.vim_apply(effects, (from, 0));
        }
        let at = if forward { from + 1 } else { from };
        let act = self.find(at, forward);
        self.vim.go_to(self.cursor, 0);
        act
    }

    /// `vim_after` for a key that started with the cursor at `from`.
    fn vim_after_from(&mut self, from: (usize, usize)) -> Act {
        let effects = self.vim.effects();
        self.vim_apply(effects, from)
    }

    /// `vim_after_from` with the effects already taken.
    fn vim_apply(&mut self, effects: Vec<Effect>, from: (usize, usize)) -> Act {
        for effect in effects {
            match effect {
                Effect::Copy(text) => {
                    (self.copy)(&text);
                    let lines = text.trim_end_matches('\n').lines().count().max(1);
                    self.note = Some(if lines == 1 {
                        "Copied".into()
                    } else {
                        format!("Copied {lines} lines")
                    });
                }
                Effect::Message(text) => self.note = Some(text),
                Effect::Prompt(kind) => {
                    self.typing = Some(String::new());
                    self.typing_kind = kind;
                }
                Effect::Quit | Effect::SaveQuit => return Act::Quit,
                _ => {}
            }
        }
        let last = self.doc.lines.len().saturating_sub(1);
        // On a picture's first row, whatever row of it a motion reached,
        // or past it for one that paged down into it, where its first row
        // would take the cursor back and leave a tall one stuck.
        let landed = self.vim.row().min(last);
        let start = self.picture_start(landed);
        self.cursor = if start < landed && start <= from.0 && landed > from.0 {
            (self.picture_end(landed) + 1).min(last)
        } else {
            start
        };
        if self.cursor != self.vim.row() {
            self.vim.go_to(self.cursor, 0);
        }
        if std::mem::take(&mut self.vim.center) {
            self.top = self.cursor.saturating_sub(self.body() / 2);
        } else {
            self.top = self.vim.top;
        }
        self.show_cursor();
        self.vim.top = self.top;
        self.show_cursor_across();
        Act::Redraw
    }

    /// The text scrolled across as little as keeps the cursor's character
    /// in sight, for a line cut at the pane's edge.
    fn show_cursor_across(&mut self) {
        let Some(line) = self.doc.lines.get(self.cursor) else {
            return;
        };
        let plain = line.plain();
        let at = width_to(&plain, self.vim.col());
        let cols = self.across_cols().max(1);
        if at < self.left {
            self.left = at;
        } else if at >= self.left + cols {
            self.left = at + 1 - cols;
        }
    }

    /// The selection's columns on line `i`, lit as a search's matches are.
    fn selected_cols(&self, i: usize, plain: &str) -> Option<(usize, usize)> {
        if !self.has_cursor() {
            return None;
        }
        let (s, e) = self.vim.selection()?.span(i, plain)?;
        let width = |b: usize| width_to(plain, b);
        // A line selected whole with nothing on it is lit a column wide.
        let end = width(e).max(width(s) + 1);
        Some((width(s), end))
    }

    /// The screen cell the cursor's character is in, while it is in sight
    /// and no menu is up.
    fn vim_cursor_cell(&self) -> Option<(usize, usize)> {
        if !self.has_cursor() || self.menu.is_some() {
            return None;
        }
        let row = self.vim.row().checked_sub(self.top)?;
        if row >= self.body() {
            return None;
        }
        let plain = self.doc.lines.get(self.vim.row())?.plain();
        let at = width_to(&plain, self.vim.col());
        let x = (at.checked_sub(self.left)? + self.gutter()).min(self.cols.saturating_sub(1));
        Some((x, row))
    }

    pub fn mouse(&mut self, mouse: MouseEvent) -> Act {
        if let Some(menu) = &mut self.menu {
            let came = menu.mouse(mouse);
            return self.menu_came(came);
        }
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Right) => {
                return self.open_menu(Some((mouse.column, mouse.row)));
            }
            MouseEventKind::Down(MouseButton::Left)
                if mouse.row as usize + 1 == self.rows
                    && self.on_view_actions(mouse.column as usize) =>
            {
                return self.open_menu(None);
            }
            _ => {}
        }
        match mouse.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let by = WHEEL as isize;
                let down = matches!(mouse.kind, MouseEventKind::ScrollDown);
                let act = self.scroll_by(if down { by } else { -by });
                self.keep_cursor_in_sight();
                self.vim_follow_layout();
                act
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging.is_some() => {
                let total = self.doc.lines.len();
                let body = self.body();
                let grab = self.dragging.unwrap_or_default();
                let to = crate::ui::kit::scroll_drag(total, body, body, mouse.row as usize, grab);
                let act = self.scroll_to(to);
                self.keep_cursor_in_sight();
                act
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.dragging = None;
                Act::Same
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if matches!(self.content, Content::Failed(_)) {
                    return Act::Quit;
                }
                let row = mouse.row as usize;
                if row >= self.body() {
                    return Act::Same;
                }
                let col = mouse.column as usize;
                if let (Some(_), Some(x)) = self.scroll_bar()
                    && col == x
                {
                    let total = self.doc.lines.len();
                    let body = self.body();
                    self.dragging = Some(crate::ui::kit::scroll_grab(
                        total, self.top, body, body, row,
                    ));
                    let to = crate::ui::kit::scroll_press(total, self.top, body, body, row);
                    let act = self.scroll_to(to.unwrap_or(self.top));
                    self.keep_cursor_in_sight();
                    return act;
                }
                let line = self.top + row;
                let in_column = self.panel.as_ref().is_none_or(|(_, at)| col <= *at);
                if self.has_cursor() && in_column && line < self.doc.lines.len() {
                    let moved = self.move_cursor(line);
                    if self.cursor == line {
                        let at = col.checked_sub(self.gutter()).map_or(0, |c| self.left + c);
                        let byte = byte_at(&self.doc.lines[line].plain(), at);
                        if matches!(self.vim.mode, Mode::Visual { .. }) {
                            self.vim.leave_visual();
                        }
                        self.vim.go_to(line, byte);
                    } else {
                        self.vim_follow_layout();
                    }
                    if self.on_picture(line, col)
                        && let Some(full) = self.full_at(line)
                    {
                        return full;
                    }
                    let link = col
                        .checked_sub(self.gutter())
                        .and_then(|c| self.doc.lines[line].link_at(self.left + c))
                        .and_then(|i| self.doc.links.get(i));
                    return match link {
                        Some(url) => Act::Open(url.clone()),
                        None => moved,
                    };
                }
                let link = match &self.panel {
                    Some((panel, at)) if col > *at => panel
                        .lines
                        .get(row)
                        .and_then(|l| l.link_at(col - at - 1))
                        .and_then(|i| panel.links.get(i)),
                    _ => self
                        .doc
                        .lines
                        .get(self.top + row)
                        .and_then(|l| l.link_at(self.left + col))
                        .and_then(|i| self.doc.links.get(i)),
                };
                match link {
                    Some(url) => Act::Open(url.clone()),
                    None => Act::Same,
                }
            }
            _ => Act::Same,
        }
    }

    /// The screen, as the bytes that draw it whole, held back until done
    /// (synchronized output) so it never shows half drawn.
    pub fn frame(&self) -> String {
        self.frame_at(SystemTime::now())
    }

    /// `frame` with the menu over it while it is up, the terminal asked
    /// for every movement of the mouse meanwhile, so its options light
    /// under it, and only for movement with a button held again after.
    pub fn frame_with_menu(&mut self) -> String {
        let mut out = self.frame();
        let end = out.len() - "\x1b[?2026l".len();
        let mut over = String::new();
        if self.menu.is_some() != self.any_motion {
            self.any_motion = self.menu.is_some();
            over.push_str(if self.any_motion {
                "\x1b[?1003h"
            } else {
                "\x1b[?1003l\x1b[?1002h"
            });
        }
        if let Some(menu) = &mut self.menu {
            over.push_str("\x1b[?25l");
            over.push_str(&menu.draw(self.cols as u16, self.rows as u16));
        }
        out.insert_str(end, &over);
        out
    }

    /// Whether its menu is up.
    pub fn menu_open(&self) -> bool {
        self.menu.is_some()
    }

    /// `frame` at a time, which says how old the copy is.
    pub fn frame_at(&self, now: SystemTime) -> String {
        let mut out = String::from("\x1b[?2026h\x1b[?25l");
        let body = self.body();
        let gutter = self.gutter();
        let (bar, bar_x) = self.scroll_bar();
        for r in 0..body {
            out.push_str(&format!("\x1b[{};{}H", r + 1, gutter + 1));
            if let Some(line) = self.doc.lines.get(self.top + r) {
                let mut marks = self.matches(&line.plain());
                marks.extend(self.selected_cols(self.top + r, &line.plain()));
                let cols = self.text_cols().saturating_sub(gutter);
                out.push_str(&line.ansi(&self.doc.links, self.left, cols, &marks));
            }
            out.push_str("\x1b[0m\x1b[K");
            if let Some((panel, at)) = &self.panel {
                let rule = super::markdown::Style::fg(board::rule_color(&self.look));
                out.push_str(&format!("\x1b[{};{}H{}│\x1b[0m", r + 1, at + 1, rule.sgr()));
                if let Some(line) = panel.lines.get(r) {
                    let marks = self.matches(&line.plain());
                    out.push_str(&line.ansi(&panel.links, 0, self.cols - at - 1, &marks));
                }
            }
            if let Some(x) = bar_x {
                let thumb = bar.is_some_and(|(at, len)| (at..at + len).contains(&r));
                let (style, mark) = if thumb {
                    (
                        super::markdown::Style::fg(board::text_color(&self.look)),
                        '┃',
                    )
                } else {
                    (
                        super::markdown::Style::fg(board::rule_color(&self.look)),
                        '│',
                    )
                };
                out.push_str(&format!(
                    "\x1b[{};{}H{}{mark}\x1b[0m",
                    r + 1,
                    x + 1,
                    style.sgr()
                ));
            }
            if self.on_cursor(self.top + r) {
                out.push_str(&format!("\x1b[{};1H\x1b[0;1;38;5;{CURSOR}m›\x1b[0m", r + 1));
            } else if gutter > 0 {
                out.push_str(&format!("\x1b[{};1H\x1b[0m ", r + 1));
            }
        }
        out.push_str(&format!("\x1b[{};1H\x1b[0m\x1b[K", self.rows));
        // How old the copy is, at the line's right end, the keys cut short
        // of it; a search being typed or a message has the line.
        let age = self
            .age(now)
            .filter(|_| self.typing.is_none() && self.note.is_none())
            .map(|age| (crate::ui::draw::width(&age), age))
            .filter(|(wide, _)| *wide < self.cols);
        let room = match &age {
            Some((wide, _)) => self.cols.saturating_sub(wide + 2),
            None => self.cols,
        };
        out.push_str(&self.prompt(room));
        if let Some((wide, age)) = age {
            out.push_str(&format!(
                "\x1b[0m\x1b[{};{}H\x1b[38;5;{DIM}m{age}\x1b[0m",
                self.rows,
                self.cols - wide + 1
            ));
        }
        if self.typing.is_some() {
            out.push_str("\x1b[?25h");
        } else if let Some((x, y)) = self.vim_cursor_cell() {
            out.push_str(&format!("\x1b[{};{}H\x1b[?25h", y + 1, x + 1));
        }
        out.push_str("\x1b[?2026l");
        out
    }

    /// Whether screen column `col` of line `i` is on its picture's cells,
    /// not the margin or the space beside it.
    fn on_picture(&self, i: usize, col: usize) -> bool {
        let Some(line) = self.doc.lines.get(i) else {
            return false;
        };
        let Some(p) = line.picture else {
            return false;
        };
        let at: usize = line.spans.iter().map(Span::width).sum();
        col.checked_sub(self.gutter())
            .map(|c| self.left + c)
            .is_some_and(|c| c >= at && c < at + p.cols as usize)
    }

    /// The column kept for the cursor's `›` while the text is scrolled
    /// across, past the margin it otherwise takes, so it covers no text.
    fn gutter(&self) -> usize {
        usize::from(self.has_cursor() && self.left > 0)
    }

    /// The columns of text shown once scrolled across: the text's, less
    /// the cursor's gutter.
    fn across_cols(&self) -> usize {
        self.text_cols()
            .saturating_sub(usize::from(self.has_cursor()))
    }

    /// Whether line `i` is the cursor's, or a row of its picture.
    fn on_cursor(&self, i: usize) -> bool {
        self.has_cursor() && i >= self.cursor && i <= self.picture_end(self.cursor)
    }

    /// The prompt line, as less's `-Ps'?e(END) .'` and its search, then
    /// what Enter does on a picture under the cursor; a board's keys in
    /// `room` columns.
    fn prompt(&self, room: usize) -> String {
        if let Some(typed) = &self.typing {
            let kind = if self.has_cursor() {
                self.typing_kind
            } else {
                '/'
            };
            return format!("{kind}{typed}");
        }
        if self.has_cursor()
            && let Mode::Visual { line, block, .. } = self.vim.mode
        {
            let which = match (line, block) {
                (true, _) => "-- VISUAL LINE --",
                (_, true) => "-- VISUAL BLOCK --",
                _ => "-- VISUAL --",
            };
            return format!("\x1b[1m{which}\x1b[0m");
        }
        if let Some(note) = &self.note {
            // Printed as it is, so without control characters, which a
            // crafted link's error could carry to the terminal, and cut to
            // the pane, short of its last column, so it never wraps and
            // scrolls the page.
            let note: String = note.chars().filter(|c| !c.is_control()).collect();
            let note = crate::ui::draw::cut(&note, self.cols.saturating_sub(1));
            return format!("\x1b[7m{note}\x1b[0m");
        }
        let full = self.has_cursor() && self.full_at(self.cursor).is_some();
        if let Content::Board(b) = &self.content {
            let mut keys = b.keys.clone();
            if full {
                keys.insert(0, ("Enter".to_string(), "full size".to_string()));
            }
            let line = board::hints(&keys, &self.look);
            return line.ansi(&[], 0, room, &[]);
        }
        let mut out = String::new();
        if self.markdown.is_some() && self.at_end() {
            out.push_str("\x1b[7m(END)\x1b[0m ");
        }
        if full {
            out.push_str(&format!("\x1b[38;5;{DIM}mEnter shows it full size\x1b[0m"));
        }
        out
    }

    /// The columns the scrolling text is drawn in: the pane's, less a
    /// board's panel beside it or the scroll bar.
    fn text_cols(&self) -> usize {
        match &self.panel {
            Some((_, at)) => *at,
            None => self.cols - usize::from(self.bar),
        }
    }

    /// The scroll bar's thumb (its first row and length) and the column
    /// it is drawn in, the pane's last or, with a board's panel beside, the
    /// rule between them; `None`s while everything fits or it is off.
    fn scroll_bar(&self) -> (Option<(usize, usize)>, Option<usize>) {
        let body = self.body();
        let total = self.doc.lines.len();
        if !crate::ui::kit::has_scrollbar(total, body) {
            return (None, None);
        }
        let x = match &self.panel {
            Some((_, at)) => *at,
            None if self.bar => self.cols - 1,
            None => return (None, None),
        };
        (crate::ui::kit::thumb(total, self.top, body, body), Some(x))
    }

    /// The screen as text, for the tests: a board's panel after a `│`.
    pub fn screen(&self) -> Vec<String> {
        (0..self.body())
            .map(|r| {
                let text = self
                    .doc
                    .lines
                    .get(self.top + r)
                    .map(|l| l.plain())
                    .unwrap_or_default();
                match &self.panel {
                    Some((panel, at)) => {
                        let pad = at.saturating_sub(super::markdown::width(&text));
                        let side = panel.lines.get(r).map(|l| l.plain()).unwrap_or_default();
                        format!("{text}{}│{side}", " ".repeat(pad))
                    }
                    None => text,
                }
            })
            .collect()
    }
}

/// A terminal's key as the shared Vim takes it; None for one it has no
/// use for.
fn vim_key_of(key: KeyEvent) -> Option<Key> {
    let code = match key.code {
        KeyCode::Char(c) => VimCode::Char(c),
        KeyCode::Enter => VimCode::Enter,
        KeyCode::Esc => VimCode::Esc,
        KeyCode::Backspace => VimCode::Backspace,
        KeyCode::Tab => VimCode::Tab,
        KeyCode::Left => VimCode::Left,
        KeyCode::Right => VimCode::Right,
        KeyCode::Up => VimCode::Up,
        KeyCode::Down => VimCode::Down,
        KeyCode::Home => VimCode::Home,
        KeyCode::End => VimCode::End,
        KeyCode::PageUp => VimCode::PageUp,
        KeyCode::PageDown => VimCode::PageDown,
        _ => return None,
    };
    let mut mods = Mods::NONE;
    mods.ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    mods.alt = key.modifiers.contains(KeyModifiers::ALT);
    mods.shift = key.modifiers.contains(KeyModifiers::SHIFT);
    Some(Key::new(code, mods))
}

/// The columns a line's text takes up to byte `byte`, a byte inside a
/// character counted from that character's start.
fn width_to(plain: &str, byte: usize) -> usize {
    let mut end = byte.min(plain.len());
    while !plain.is_char_boundary(end) {
        end -= 1;
    }
    super::markdown::width(&plain[..end])
}

/// The byte of a line's text at display column `col`, its end past it.
fn byte_at(plain: &str, col: usize) -> usize {
    let mut at = 0;
    for (byte, c) in plain.char_indices() {
        let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if col < at + w.max(1) {
            return byte;
        }
        at += w;
    }
    plain.len()
}

fn act_or_redraw(act: Act) -> Act {
    match act {
        Act::Same => Act::Redraw,
        other => other,
    }
}

/// The terminal as the pager wants it while it runs, and put back however
/// it ends: raw, on the alternate screen, the cursor hidden, clicks and the
/// wheel reported, and movement with the button held for the scroll bar
/// (1002 and SGR's 1006, not every movement).
struct Screen;

impl Screen {
    /// `focus` has the terminal say when the view takes and loses the
    /// focus (1004), for a view that follows what it shows.
    fn enter(focus: bool) -> io::Result<Screen> {
        terminal::enable_raw_mode()?;
        let mut out = io::stdout();
        out.write_all(b"\x1b[?1049h\x1b[?25l\x1b[?1002h\x1b[?1006h")?;
        if focus {
            out.write_all(b"\x1b[?1004h")?;
        }
        out.flush()?;
        Ok(Screen)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let mut out = io::stdout();
        let _ = out
            .write_all(b"\x1b[?1004l\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[0m\x1b[?25h\x1b[?1049l");
        let _ = out.flush();
        let _ = terminal::disable_raw_mode();
    }
}

/// Runs the pager on this terminal until it is closed, showing `content`
/// and then whatever `updates` hands over; `url` is the page o opens.
pub fn run(content: Content, updates: Option<Receiver<Content>>, url: Option<String>) -> ExitCode {
    run_with(content, updates, url, true, None)
}

/// `run` for a view that follows what it shows: `follow`'s reads shown as
/// they come, and asked for on r and on taking the focus again.
pub fn run_following(content: Content, follow: Follow) -> ExitCode {
    run_with(content, None, None, true, Some(follow))
}

/// `run` for a file of yours or a note, whose picture links are never
/// fetched, since a link is your words and nothing of a note leaves the
/// computer; its diagrams are drawn here as anywhere, and a person's photo
/// while Settings' `photos` is on.
pub fn run_local(content: Content) -> ExitCode {
    run_with(content, None, None, false, None)
}

/// `run_following` fetching no pictures: for what strangers write, whose
/// images would tell their server when it is read (a GitHub issue's
/// comments, a Linear project's). People's photos are still drawn while
/// Settings' `photos` is on, since they come from Photos' cache, fetched
/// only from the service's own avatars.
pub fn run_following_without_pictures(content: Content, follow: Follow) -> ExitCode {
    run_with(content, None, None, false, Some(follow))
}

fn run_with(
    content: Content,
    updates: Option<Receiver<Content>>,
    url: Option<String>,
    fetch_pictures: bool,
    follow: Option<Follow>,
) -> ExitCode {
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let mut pager = Pager::new(content, cols as usize, rows as usize)
        .with_url(url)
        .with_cursor(crate::settings::is_on("view_cursor"));
    if let Some(follow) = &follow {
        pager = pager.following(follow.read_at);
    }
    let (pictures_tx, pictures) = channel();
    // A link opened from here: what was said meanwhile, and how it went.
    let (opened_tx, opened) = channel::<(String, Result<bool, String>)>();
    let screen = match Screen::enter(follow.is_some()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("switchboard: can't take the terminal: {e}");
            return ExitCode::FAILURE;
        }
    };
    let drawing_pictures = super::pictures::on();
    if drawing_pictures {
        // Its pane may pass the pictures' graphics commands on while it
        // runs, marked once the terminal is taken so every way out unmarks it.
        crate::notes::app::mark_pane("@allow_passthrough", true);
        super::image::render::stop_on_hangup();
        // The terminal's background, for the `system` theme the pictures'
        // frames and the diagrams follow, as the diff asks it.
        let (colors, _) = crate::mux::terminal::ask(Duration::from_millis(200));
        if let Some(light) = colors.light() {
            crate::status::theme::set_system(light);
        }
        pager = pager.with_pictures(Pictures::new(pictures_tx, fetch_pictures));
    }
    pager = pager.with_look(super::board::Look::here());
    let draw = |pager: &mut Pager| {
        let graphics = pager.graphics();
        let mut out = io::stdout().lock();
        let _ = out.write_all(&graphics);
        let _ = out.write_all(pager.frame_with_menu().as_bytes());
        let _ = out.flush();
    };
    draw(&mut pager);
    let mut age = pager.age(SystemTime::now());
    loop {
        let mut redraw = false;
        if let Some(follow) = &follow {
            while let Ok(update) = follow.updates.try_recv() {
                pager.update(update, SystemTime::now());
                redraw = true;
            }
        }
        let age_now = pager.age(SystemTime::now());
        if age_now != age {
            age = age_now;
            redraw = true;
        }
        if let Some(rx) = &updates {
            while let Ok(content) = rx.try_recv() {
                pager.set_content(content);
                redraw = true;
            }
        }
        while let Ok((words, shown)) = opened.try_recv() {
            // Unless another note took its place; one closed by a key still
            // hears how the open went.
            if pager.note.is_none()
                || pager.note.as_deref() == Some(words.as_str())
                || shown.is_err()
            {
                pager.note = match shown {
                    Ok(true) => None,
                    Ok(false) => Some(format!("{words} The browser hasn't shown it yet.")),
                    Err(why) => Some(format!("Couldn't open it: {why}")),
                };
                redraw = true;
            }
        }
        while let Ok(got) = pictures.try_recv() {
            redraw |= pager.picture(got);
        }
        if let Some(p) = &mut pager.pictures
            && p.retry_failed(Instant::now())
        {
            pager.ask_pictures();
        }
        let act = match event::poll(Duration::from_millis(100)) {
            Ok(true) => match event::read() {
                Ok(Event::Key(key)) => pager.key(key),
                Ok(Event::Mouse(mouse)) => pager.mouse(mouse),
                Ok(Event::Resize(c, r)) => {
                    pager.resize(c as usize, r as usize);
                    Act::Redraw
                }
                Ok(Event::FocusGained) if pager.focus(true, Instant::now()) => Act::Again,
                Ok(Event::FocusLost) => {
                    pager.focus(false, Instant::now());
                    Act::Same
                }
                Ok(_) => Act::Same,
                Err(_) => Act::Quit,
            },
            Ok(false) => Act::Same,
            Err(_) => Act::Quit,
        };
        match act {
            Act::Quit => break,
            // The pager is gone as the link opens, so the server says where
            // it is going, when there is one.
            Act::OpenAndQuit(url) => {
                if crate::links::says_it() && crate::mux::socket::in_pane() {
                    let words = crate::links::opening_words(&url);
                    let _ = crate::mux::socket::ask(&["say", "--", &words]);
                }
                let _ = crate::links::open_url(&url);
                break;
            }
            // Drawn again too, since the click that asked moved the cursor.
            Act::Open(url) => {
                if crate::links::says_it() {
                    let words = crate::links::opening_words(&url);
                    pager.note = Some(words.clone());
                    draw(&mut pager);
                    // Handed over before the next key, so quitting at once
                    // still opens it; only the watch runs on.
                    let before = crate::platform::front_app();
                    match crate::links::open_url(&url) {
                        Ok(()) => {
                            let tx = opened_tx.clone();
                            std::thread::spawn(move || {
                                let shown = crate::links::front_changed(before);
                                let _ = tx.send((words, Ok(shown)));
                            });
                        }
                        Err(e) => {
                            let _ = opened_tx.send((words, Err(e.to_string())));
                        }
                    }
                } else {
                    let _ = crate::links::open_url(&url);
                }
                redraw = true;
            }
            Act::Full { key, title } => {
                if let Some(p) = &pager.pictures {
                    p.show_full(&key, &title);
                }
                redraw = true;
            }
            Act::Again => {
                if let Some(follow) = &follow {
                    follow.again();
                }
                redraw = true;
            }
            // Asked of the server, which says itself what can't be done.
            Act::Press(name) => {
                let asked = crate::mux::socket::ask(&["press", &name]);
                match asked {
                    Ok((0, _, _)) => {}
                    Ok((_, _, err)) => pager.note = Some(err.trim().to_string()),
                    Err(e) => pager.note = Some(format!("Couldn't ask the workspace: {e}")),
                }
                redraw = true;
            }
            Act::Redraw => redraw = true,
            Act::Same => {}
        }
        if redraw {
            draw(&mut pager);
        }
    }
    if let Some(p) = &mut pager.pictures {
        let mut out = io::stdout().lock();
        let _ = out.write_all(&p.forget());
        let _ = out.flush();
    }
    if drawing_pictures {
        crate::notes::app::mark_pane("@allow_passthrough", false);
    }
    drop(screen);
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn long_text() -> String {
        let para = |n: usize| {
            (0..40)
                .map(|i| format!("p{n}w{i}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let mut text = String::from("# A long description\n\n");
        for n in 0..12 {
            text.push_str(&para(n));
            text.push_str("\n\n");
        }
        text
    }

    fn markdown(text: &str) -> Content {
        Content::Markdown {
            text: text.to_string(),
            place: "M-r on GitHub".into(),
        }
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn a_page_past_the_screen_has_a_scroll_bar_that_pages_and_drags() {
        let mut pager = Pager::new(markdown(&long_text()), 60, 20);
        assert!(pager.bar, "laid out a column narrower");
        assert!(pager.doc.lines.iter().all(|l| l.width() <= 59));
        let frame = pager.frame();
        assert!(frame.contains(";60H"), "drawn in the last column");
        assert!(frame.contains('┃') && frame.contains('│'));
        // A press below the thumb pages down; a drag from there moves with
        // the mouse, stopping at the top.
        let act = pager.mouse(mouse(MouseEventKind::Down(MouseButton::Left), 59, 17));
        assert_eq!(act, Act::Redraw);
        assert_eq!(pager.top(), 19);
        pager.mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 59, 18));
        assert!(pager.top() > 19, "a row down moves it down");
        pager.mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 59, 0));
        assert_eq!(pager.top(), 0);
        pager.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 59, 0));
        assert!(pager.dragging.is_none());
    }

    #[test]
    fn a_note_pages_as_markdown() {
        // `switchboard view <note file>`, as M-u reads a note link.
        let note = "# Plan\n\n- [ ] the first thing\n\n<!-- note-id: n-0123456789ab -->\n";
        let pager = Pager::new(markdown(note), 60, 20);
        let screen = pager.screen().join("\n");
        assert!(screen.contains("the first thing"), "{screen}");
        assert!(!screen.contains("- [ ]"), "drawn, not its markup: {screen}");
    }

    #[test]
    fn a_page_that_fits_has_no_scroll_bar() {
        let pager = Pager::new(markdown("# Short\n\nOne line."), 60, 20);
        assert!(!pager.bar);
        assert!(!pager.frame().contains('┃'));
    }

    #[test]
    fn a_resize_keeps_the_top_line_in_place() {
        let mut pager = Pager::new(markdown(&long_text()), 100, 20);
        for _ in 0..17 {
            pager.key(key(KeyCode::Char('j')));
        }
        let top = pager.screen()[0].clone();
        let first = top.split_whitespace().next().unwrap().to_string();
        assert!(first.starts_with('p'), "in a paragraph: {top:?}");
        pager.resize(50, 20);
        let after = pager.screen()[0].clone();
        assert!(after.contains(&first), "{after:?} still shows {first}");
    }

    #[test]
    fn a_log_opens_at_its_end_a_line_each() {
        let log: String = (1..=50).map(|n| format!("line {n}\n")).collect();
        let mut pager = Pager::new(Content::Log(log), 40, 10);
        let screen = pager.screen();
        assert_eq!(screen[8].trim(), "line 50", "{screen:?}");
        assert_eq!(screen[0].trim(), "line 42");
        pager.key(key(KeyCode::Char('g')));
        assert_eq!(pager.screen()[0].trim(), "line 1");
        assert_eq!(pager.key(key(KeyCode::Char('q'))), Act::Quit);
    }

    #[test]
    fn a_files_tabs_are_spaces_to_the_next_stop() {
        let pager = Pager::new(
            Content::Text {
                text: "name\tvalue\n\tindented\n".into(),
                line: 1,
            },
            40,
            10,
        );
        assert_eq!(pager.screen()[0], "name    value");
        assert_eq!(pager.screen()[1], "        indented");
    }

    #[test]
    fn a_narrower_layout_brings_the_scroll_across_back() {
        let wide = format!("```\n{}\n```", "x".repeat(200));
        let mut pager = Pager::new(markdown(&wide), 40, 10);
        pager.key(key(KeyCode::Right));
        pager.key(key(KeyCode::Right));
        assert!(pager.left > 0);
        pager.set_content(markdown("short"));
        assert_eq!(pager.left, 0);
    }

    #[test]
    fn a_file_opens_at_its_line_with_two_above_it() {
        let text: String = (1..=50).map(|n| format!("line {n}\n")).collect();
        let pager = Pager::new(Content::Text { text, line: 20 }, 40, 10);
        assert_eq!(pager.screen()[0].trim(), "line 18");
    }

    #[test]
    fn new_content_opens_where_the_old_was() {
        let mut pager = Pager::new(markdown(&long_text()), 100, 20);
        pager.key(key(KeyCode::Char(' ')));
        let top = pager.screen()[0].clone();
        let changed = long_text().replace("p0w3 ", "p0w3 changed ");
        pager.set_content(markdown(&changed));
        assert_eq!(
            pager.screen()[0].split_whitespace().last(),
            top.split_whitespace().last()
        );
    }

    #[test]
    fn less_s_keys_move_it_and_search_finds() {
        let mut pager = Pager::new(markdown(&long_text()), 100, 20);
        assert_eq!(pager.top(), 0);
        pager.key(key(KeyCode::Char('G')));
        assert!(pager.top() > 0);
        assert!(pager.frame().contains("(END)"));
        pager.key(key(KeyCode::Char('g')));
        assert_eq!(pager.top(), 0);
        pager.key(key(KeyCode::Char('/')));
        for c in "p4w0".chars() {
            pager.key(key(KeyCode::Char(c)));
        }
        pager.key(key(KeyCode::Enter));
        assert!(
            pager.screen()[0].contains("p4w0"),
            "{:?}",
            pager.screen()[0]
        );
        assert!(
            pager.frame().contains("\x1b[0;7;38;5;252mp4w0"),
            "the match lit"
        );
        pager.key(key(KeyCode::Char('/')));
        for c in "nowhere".chars() {
            pager.key(key(KeyCode::Char(c)));
        }
        pager.key(key(KeyCode::Enter));
        assert!(pager.frame().contains("Pattern not found"));
        assert_eq!(pager.key(key(KeyCode::Char('q'))), Act::Quit);
    }

    #[test]
    fn a_click_on_a_link_opens_it_and_a_failure_closes_on_any_key() {
        let mut pager = Pager::new(
            markdown("See [the PR](https://github.com/o/r/pull/1)."),
            80,
            10,
        );
        let click = |column: u16, row: u16| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            pager.mouse(click(8, 1)),
            Act::Open("https://github.com/o/r/pull/1".into())
        );
        assert_eq!(pager.mouse(click(2, 1)), Act::Same);
        let mut failed = Pager::new(
            Content::Failed("No PR for this branch (main).".into()),
            80,
            10,
        );
        assert!(
            failed
                .screen()
                .iter()
                .any(|l| l.contains("No PR for this branch"))
        );
        assert_eq!(failed.key(key(KeyCode::Char('x'))), Act::Quit);
        assert_eq!(failed.key(key(KeyCode::Char('o'))), Act::Quit);
        assert!(
            failed
                .screen()
                .iter()
                .any(|l| l.contains("any key to close"))
        );
        // A page that couldn't be read, with a link: o opens it, as it says.
        let url = "https://example.com/a";
        let mut unread = Pager::new(Content::Failed("Couldn't read it: 404.".into()), 80, 10)
            .with_url(Some(url.into()));
        assert!(
            unread
                .screen()
                .iter()
                .any(|l| l.contains("o opens it in the browser · any other key closes"))
        );
        assert_eq!(
            unread.key(key(KeyCode::Char('o'))),
            Act::OpenAndQuit(url.into())
        );
        assert_eq!(unread.key(key(KeyCode::Char('q'))), Act::Quit);
        let mut offer = Pager::new(
            Content::Offer {
                text: "A Slack message can't be read here, only in Slack.".into(),
                url: "https://x.slack.com/archives/C1/p2".into(),
            },
            80,
            10,
        );
        assert_eq!(
            offer.key(key(KeyCode::Char('o'))),
            Act::OpenAndQuit("https://x.slack.com/archives/C1/p2".into())
        );
    }

    /// A pager with a cursor on `text`, its pictures laid out as ready, 40
    /// by 20 pixels each in 10 by 20 pixel cells.
    fn with_pictures(text: &str, cols: usize, rows: usize) -> Pager {
        with_pictures_of(text, cols, rows, (40, 40))
    }

    fn with_pictures_of(text: &str, cols: usize, rows: usize, px: (u32, u32)) -> Pager {
        use super::super::markdown::Images;
        let mut pager = Pager::new(markdown(text), cols, rows).with_cursor(true);
        let md = pager.markdown.clone().unwrap();
        let ready = md
            .wanted()
            .iter()
            .enumerate()
            .map(|(i, w)| (w.key(), (i as u32 + 1, px)))
            .collect();
        let images = Images {
            cell: (10, 20),
            rows: pager.body(),
            ready,
        };
        pager.doc = md.layout_with(cols, Some(&images));
        pager.cursor = pager.snap(0);
        pager
    }

    fn cursor_line(pager: &Pager) -> String {
        pager.doc.lines[pager.cursor].plain()
    }

    fn press(pager: &mut Pager, keys: &str) {
        for c in keys.chars() {
            pager.key(key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn the_cursor_moves_as_vim_s_and_scrolls_only_at_the_edge() {
        let mut pager = Pager::new(markdown(&long_text()), 100, 20).with_cursor(true);
        assert!(cursor_line(&pager).contains("A long description"));
        assert!(pager.frame().contains("›"), "the cursor drawn");
        let first = pager.cursor;
        press(&mut pager, "}j");
        assert!(
            cursor_line(&pager).contains("p0w0"),
            "{}",
            cursor_line(&pager)
        );
        assert_eq!(
            pager.cursor,
            first + 2,
            "past the blank line, then a line down"
        );
        press(&mut pager, "w");
        let word = cursor_line(&pager).find("p0w0");
        assert_eq!(
            Some(pager.vim.col()),
            word,
            "w past the margin to the first word"
        );
        press(&mut pager, "w");
        let next = cursor_line(&pager).find("p0w1");
        assert_eq!(Some(pager.vim.col()), next, "w to the next word");
        assert_eq!(pager.top(), 0, "the screen stays while the cursor is in it");
        for _ in 0..30 {
            pager.key(key(KeyCode::Down));
        }
        assert!(pager.top() > 0);
        assert!(pager.cursor < pager.top() + pager.body());
        press(&mut pager, "gg");
        assert_eq!((pager.top(), pager.cursor), (0, 0));
        pager.key(key(KeyCode::Char('/')));
        for c in "p6w0".chars() {
            pager.key(key(KeyCode::Char(c)));
        }
        pager.key(key(KeyCode::Enter));
        assert!(cursor_line(&pager).contains("p6w0"), "the search moves it");
        pager.resize(50, 20);
        assert!(cursor_line(&pager).contains("p6w0"), "kept on its line");
    }

    #[test]
    fn g_and_gg_go_to_the_ends_and_a_key_that_changes_the_text_says_e21() {
        let mut pager = Pager::new(markdown("Above\n\n---\n\nBelow"), 40, 10).with_cursor(true);
        assert_eq!(cursor_line(&pager).trim(), "Above");
        press(&mut pager, "Gk");
        assert_eq!(cursor_line(&pager).trim(), "Below", "G to the last line");
        press(&mut pager, "ggj");
        assert_eq!(cursor_line(&pager).trim(), "Above", "gg to the first line");
        let shown = pager.screen();
        for keys in ["x", "dd", "i"] {
            press(&mut pager, keys);
            assert_eq!(pager.screen(), shown, "{keys} changed nothing");
            assert_eq!(
                pager.note.as_deref(),
                Some(switchboard_vim::editor::READ_ONLY)
            );
        }
    }

    #[test]
    fn a_mark_and_a_selection_outlive_a_new_layout() {
        let mut pager = Pager::new(markdown(&long_text()), 100, 20).with_cursor(true);
        press(&mut pager, "}jma");
        let marked = cursor_line(&pager);
        press(&mut pager, "Gv");
        pager.resize(60, 20);
        assert!(
            matches!(pager.vim.mode, Mode::Visual { .. }),
            "still selecting"
        );
        assert_eq!(
            pager.key(key(KeyCode::Esc)),
            Act::Redraw,
            "Esc leaves the selection"
        );
        press(&mut pager, "'a");
        assert!(cursor_line(&pager).contains(marked.trim().split(' ').next().unwrap()));
    }

    #[test]
    fn a_column_inside_a_wide_character_is_measured_from_its_start() {
        assert_eq!(width_to("a\u{E0B6}b", 2), 1);
        assert_eq!(width_to("a\u{E0B6}b", 4), 2);
    }

    #[test]
    fn a_search_vim_s_text_misses_finds_a_picture_by_its_words() {
        let text = "Before\n\n![Login screen](https://x.example/a.png)\n\nAfter\n";
        let mut pager = with_pictures(text, 80, 30);
        press(&mut pager, "/Login");
        pager.key(key(KeyCode::Enter));
        assert!(
            pager.doc.lines[pager.cursor].picture.is_some(),
            "on the picture"
        );
        assert_eq!(pager.vim.row(), pager.cursor);
    }

    #[test]
    fn visual_mode_copies_the_selection_as_the_text_shows() {
        static COPIED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        let mut pager = Pager::new(
            markdown("Some **bold** words\n\nAnd a [link](https://x.example) too"),
            40,
            10,
        )
        .with_cursor(true);
        pager.copy = |t| COPIED.lock().unwrap().push(t.to_string());
        press(&mut pager, "wvey");
        assert_eq!(
            COPIED.lock().unwrap().pop().as_deref(),
            Some("bold"),
            "no markup"
        );
        assert_eq!(pager.note.as_deref(), Some("Copied"));
        press(&mut pager, "Vjjy");
        assert_eq!(
            COPIED.lock().unwrap().pop().as_deref(),
            Some("  Some bold words\n\n  And a link too\n"),
            "whole lines as they show"
        );
        assert_eq!(pager.vim.mode, Mode::Normal);
        press(&mut pager, "/link");
        pager.key(key(KeyCode::Enter));
        assert_eq!(
            (pager.cursor, pager.vim.col()),
            (3, "  And a ".len()),
            "/ searches"
        );
        assert_eq!(pager.key(key(KeyCode::Char('q'))), Act::Quit);
    }

    #[test]
    fn enter_or_a_click_on_a_picture_shows_it_full_size() {
        let text = "Before\n\n![Login screen](https://x.example/a.png)\n\nAfter\n\n![](https://x.example/b/shot.png?v=2)\n";
        let mut pager = with_pictures(text, 80, 30);
        assert_eq!(cursor_line(&pager).trim(), "Before");
        assert!(
            matches!(pager.key(key(KeyCode::Enter)), Act::Redraw),
            "a line down"
        );
        press(&mut pager, "j");
        assert!(
            pager.doc.lines[pager.cursor].picture.is_some(),
            "on the picture"
        );
        assert!(pager.frame().contains("Enter shows it full size"));
        assert_eq!(
            pager.key(key(KeyCode::Enter)),
            Act::Full {
                key: "https://x.example/a.png".into(),
                title: "Login screen".into()
            }
        );
        press(&mut pager, "jj");
        assert_eq!(
            cursor_line(&pager).trim(),
            "After",
            "past the picture in one step"
        );
        press(&mut pager, "kk");
        assert_eq!(
            pager.doc.lines[pager.cursor].picture.map(|p| p.row),
            Some(0),
            "back onto its first row"
        );
        let second = (0..pager.doc.lines.len())
            .rev()
            .find(|i| pager.doc.lines[*i].picture.is_some())
            .unwrap();
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 4,
            row: (second - pager.top()) as u16,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            pager.mouse(click),
            Act::Full {
                key: "https://x.example/b/shot.png?v=2".into(),
                title: "shot.png".into()
            }
        );
        let beside = MouseEvent {
            column: 60,
            ..click
        };
        assert!(
            !matches!(pager.mouse(beside), Act::Full { .. }),
            "a click beside it moves the cursor alone"
        );
    }

    #[test]
    fn paging_down_goes_past_a_tall_picture_rather_than_stopping_in_it() {
        let text = "Before\n\n![Tall](https://x.example/tall.png)\n\nAfter\n";
        let mut pager = with_pictures_of(text, 80, 24, (40, 400));
        let rows = (0..pager.doc.lines.len())
            .filter(|i| pager.doc.lines[*i].picture.is_some())
            .count();
        assert!(
            rows > pager.body() / 2,
            "taller than half the screen: {rows}"
        );
        press(&mut pager, "jj");
        assert!(pager.doc.lines[pager.cursor].picture.is_some());
        pager.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert!(
            pager.cursor > pager.picture_end(pager.picture_start(pager.cursor - 1)) - 1
                && pager.doc.lines[pager.cursor].picture.is_none(),
            "past the picture"
        );
    }

    #[test]
    fn the_wheel_into_a_picture_as_tall_as_the_screen_puts_the_cursor_on_it() {
        let text = "Before\n\n![Tall](https://x.example/tall.png)\n\nAfter\n";
        let mut pager = with_pictures_of(text, 80, 24, (40, 4000));
        let start = (0..pager.doc.lines.len())
            .find(|i| pager.doc.lines[*i].picture.is_some())
            .unwrap();
        pager.top = start + 1;
        pager.cursor = 1;
        pager.keep_cursor_in_sight();
        assert_eq!(pager.cursor, start, "on the picture in sight");
        assert!(pager.frame().contains("Enter shows it full size"));
    }

    #[test]
    fn scrolled_across_the_cursor_has_a_column_of_its_own() {
        let wide = format!("```\n{}\n```", "abcdefghij".repeat(20));
        let mut pager = Pager::new(markdown(&wide), 40, 10).with_cursor(true);
        press(&mut pager, "$");
        assert!(pager.left > 0);
        let frame = pager.frame();
        let row = frame
            .split("\x1b[")
            .position(|p| p.starts_with(&format!("{};2H", pager.cursor - pager.top() + 1)))
            .is_some();
        assert!(row, "the line drawn from its second column");
        let widest = pager.doc.lines.iter().map(|l| l.width()).max().unwrap();
        while pager.key(key(KeyCode::Right)) == Act::Redraw {}
        assert!(
            pager.left + pager.across_cols() >= widest,
            "its last column in reach"
        );
        assert!(frame.contains("›"));
    }

    #[test]
    fn without_the_cursor_enter_scrolls_as_less_does() {
        let mut pager = Pager::new(markdown(&long_text()), 100, 20);
        pager.key(key(KeyCode::Enter));
        assert_eq!(pager.top(), 1);
        assert!(!pager.frame().contains("›"));
    }

    #[test]
    fn r_reads_a_followed_view_again_and_says_when_it_could_not() {
        let at = SystemTime::UNIX_EPOCH;
        let mut plain = Pager::new(markdown("# A PR"), 80, 10);
        assert_eq!(plain.key(key(KeyCode::Char('r'))), Act::Same);

        let mut pager = Pager::new(markdown("# A PR\n\nfailing"), 80, 10).following(Some(at));
        assert_eq!(pager.key(key(KeyCode::Char('r'))), Act::Again);
        assert!(pager.frame().contains(READING));
        pager.update(Update::Show(markdown("# A PR\n\npassing")), at);
        assert!(pager.screen().join("\n").contains("passing"));
        assert!(!pager.frame().contains(READING), "said until it is read");

        // A read nobody asked for that fails says nothing; r's says why.
        pager.update(Update::Kept("GitHub is down".into()), at);
        assert!(!pager.frame().contains("GitHub is down"));
        pager.key(key(KeyCode::Char('r')));
        pager.update(Update::Kept("GitHub is down".into()), at);
        assert!(
            pager
                .frame()
                .contains("Couldn't read it again: GitHub is down")
        );
        assert!(
            pager.screen().join("\n").contains("passing"),
            "the copy stays"
        );

        // A first read that failed, with nothing to show, is tried again
        // by r, where any other key closes the view.
        let failed = || Content::Failed("GitHub is down".into());
        let mut pager =
            Pager::new(Content::Loading("Reading the PR...".into()), 80, 10).following(None);
        pager.update(Update::Show(failed()), at);
        assert!(
            pager
                .screen()
                .join("\n")
                .contains("r reads it again · any other key closes")
        );
        assert_eq!(pager.key(key(KeyCode::Char('r'))), Act::Again);
        pager.update(Update::Show(markdown("# A PR\n\npassing")), at);
        assert!(pager.screen().join("\n").contains("passing"));
        pager.update(Update::Show(failed()), at);
        assert_eq!(pager.key(key(KeyCode::Char('x'))), Act::Quit);
        let mut plain = Pager::new(failed(), 80, 10);
        assert!(
            plain
                .screen()
                .join("\n")
                .contains("Press any key to close.")
        );
        assert_eq!(plain.key(key(KeyCode::Char('r'))), Act::Quit);
    }

    #[test]
    fn a_followed_view_says_how_old_its_copy_is_once_it_is_stale() {
        let at = SystemTime::UNIX_EPOCH;
        let later = |minutes: u64| at + Duration::from_secs(minutes * 60);
        let mut pager = Pager::new(markdown("# A PR"), 80, 10).following(Some(at));
        assert!(!pager.frame_at(later(4)).contains("read "));
        let frame = pager.frame_at(later(12));
        assert!(
            frame.contains(&format!("\x1b[10;69H\x1b[38;5;{DIM}mread 12m ago")),
            "at the prompt line's right end: {frame:?}"
        );
        // Read again, the same or new, it is as old as that read.
        pager.update(Update::Same, later(12));
        assert_eq!(pager.age(later(13)), None);
        assert_eq!(pager.age(later(20)).as_deref(), Some("read 8m ago"));
        // A copy that couldn't be read again stays as old as it was.
        pager.update(Update::Kept("GitHub is down".into()), later(30));
        assert_eq!(pager.age(later(30)).as_deref(), Some("read 18m ago"));
        // A view that follows nothing says nothing.
        let plain = Pager::new(markdown("# A file"), 80, 10);
        assert_eq!(plain.age(later(60)), None);
    }

    #[test]
    fn taking_the_focus_after_five_minutes_without_it_reads_again() {
        let start = Instant::now();
        let after = |secs: u64| start + Duration::from_secs(secs);
        let mut pager = Pager::new(markdown("# A PR"), 80, 10).following(None);
        assert!(!pager.focus(true, start), "never without it");
        pager.focus(false, after(10));
        assert!(
            !pager.focus(true, after(60)),
            "back within the five minutes"
        );
        pager.focus(false, after(100));
        // Lost twice with no gain between (a terminal's and a pane's),
        // it counts from the first.
        pager.focus(false, after(350));
        assert!(pager.focus(true, after(400)));
        assert!(!pager.focus(true, after(401)), "once");
        let mut plain = Pager::new(markdown("# A file"), 80, 10);
        plain.focus(false, start);
        assert!(!plain.focus(true, after(900)));
    }
}
