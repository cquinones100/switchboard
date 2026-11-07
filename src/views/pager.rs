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
//! took it), through `links::open_url`.
//!
//! Keys: j, k, Down, Up, Enter, e and y a line; Space, f, b, PageDown and
//! PageUp a screen; d and u half of one; g and G the top and the end; Left
//! and Right half a screen across; `/` searches (smart case), n and N go to
//! the next match and the one before; o opens the page being read in the
//! browser, where it has one; q or Esc closes, and Ctrl-C does nothing, as
//! in less. The wheel scrolls.

use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use crossterm::terminal;

use super::markdown::{Doc, Markdown, Span, Style};
use crate::ui::draw::wrap;

/// Lines the wheel scrolls at a time.
const WHEEL: usize = 3;
const DIM: u8 = 244;
const RED: u8 = 203;

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
    /// Something that can't be read here, with a link o opens (a Slack
    /// message); any other key closes it.
    Offer { text: String, url: String },
    /// Plain text, a line each, opened at its end, as `less +G` opened a
    /// CI job's log.
    Log(String),
    /// Plain text, a line each, opened at a line (from 1), as `less +N`
    /// opens a file: a file the diff's answer names, read beside it.
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
    /// What the prompt line says for a moment ("Pattern not found").
    note: Option<String>,
    /// The page being read, which o opens.
    url: Option<String>,
}

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
            note: None,
            url: None,
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

    /// Shows new content: Markdown laid out where the old was, at the line
    /// holding what was at the top.
    pub fn set_content(&mut self, content: Content) {
        let anchor = self
            .markdown
            .as_ref()
            .and_then(|_| self.doc.lines.get(self.top))
            .map(|l| l.anchor);
        self.markdown = match &content {
            Content::Markdown { text, place } => Some(Markdown::parse(text, place)),
            _ => None,
        };
        self.content = content;
        self.lay_out();
        self.top = match (anchor, &self.markdown) {
            (Some(anchor), Some(_)) => self.doc.line_at(anchor),
            _ if matches!(self.content, Content::Log(_)) => self.last_top(),
            // A couple of lines above it, to read it in.
            _ if let Content::Text { line, .. } = self.content => line.saturating_sub(3),
            _ => 0,
        };
        self.clamp();
    }

    /// The pane's new size: laid out again, the top line kept.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let anchor = self.doc.lines.get(self.top).map(|l| l.anchor);
        self.cols = cols.max(1);
        self.rows = rows.max(2);
        self.lay_out();
        if let (Some(anchor), Some(_)) = (anchor, &self.markdown) {
            self.top = self.doc.line_at(anchor);
        }
        self.clamp();
    }

    fn lay_out(&mut self) {
        self.doc = match (&self.content, &self.markdown) {
            (_, Some(md)) => md.layout(self.cols),
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
                    .map(|l| vec![Span::new(l.to_string(), Style::fg(252))])
                    .collect(),
            ),
            (Content::Markdown { .. }, None) => Doc::default(),
        };
    }

    fn body(&self) -> usize {
        self.rows.saturating_sub(1).max(1)
    }

    fn last_top(&self) -> usize {
        self.doc.lines.len().saturating_sub(self.body())
    }

    fn clamp(&mut self) {
        self.top = self.top.min(self.last_top());
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
        let hit = |i: &usize| !self.matches(&self.doc.lines[*i].plain()).is_empty();
        let found = if forward {
            (from..self.doc.lines.len()).find(hit)
        } else {
            (0..from.min(self.doc.lines.len())).rev().find(hit)
        };
        match found {
            Some(i) => self.top = i.min(self.last_top()),
            None => self.note = Some("Pattern not found".into()),
        }
        Act::Redraw
    }

    /// A key while a search is typed after `/`.
    fn typing_key(&mut self, key: KeyEvent) -> Act {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(typed) = &mut self.typing else {
            return Act::Same;
        };
        match key.code {
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
        let had_note = self.note.take().is_some();
        if self.typing.is_some() {
            return self.typing_key(key);
        }
        match &self.content {
            Content::Failed(_) => {
                return match (key.code, &self.url) {
                    (KeyCode::Char('o'), Some(url)) => Act::OpenAndQuit(url.clone()),
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
                self.left = self.left.saturating_sub(self.cols / 2);
                if self.left == before {
                    Act::Same
                } else {
                    Act::Redraw
                }
            }
            KeyCode::Right => {
                let widest = self.doc.lines.iter().map(|l| l.width()).max().unwrap_or(0);
                if self.left + self.cols < widest {
                    self.left += (self.cols / 2).max(1);
                    Act::Redraw
                } else {
                    Act::Same
                }
            }
            KeyCode::Char('/') => {
                self.typing = Some(String::new());
                Act::Redraw
            }
            KeyCode::Char('n') => self.find(self.top + 1, true),
            KeyCode::Char('N') => self.find(self.top, false),
            KeyCode::Char('o') => match &self.url {
                Some(url) => Act::Open(url.clone()),
                None => Act::Same,
            },
            _ => Act::Same,
        };
        if had_note { act_or_redraw(act) } else { act }
    }

    pub fn mouse(&mut self, mouse: MouseEvent) -> Act {
        match mouse.kind {
            MouseEventKind::ScrollDown => self.scroll_by(WHEEL as isize),
            MouseEventKind::ScrollUp => self.scroll_by(-(WHEEL as isize)),
            MouseEventKind::Down(MouseButton::Left) => {
                if matches!(self.content, Content::Failed(_)) {
                    return Act::Quit;
                }
                let row = mouse.row as usize;
                if row >= self.body() {
                    return Act::Same;
                }
                let link = self
                    .doc
                    .lines
                    .get(self.top + row)
                    .and_then(|l| l.link_at(self.left + mouse.column as usize))
                    .and_then(|i| self.doc.links.get(i));
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
        let mut out = String::from("\x1b[?2026h\x1b[?25l");
        let body = self.body();
        for r in 0..body {
            out.push_str(&format!("\x1b[{};1H", r + 1));
            if let Some(line) = self.doc.lines.get(self.top + r) {
                let marks = self.matches(&line.plain());
                out.push_str(&line.ansi(&self.doc.links, self.left, self.cols, &marks));
            }
            out.push_str("\x1b[0m\x1b[K");
        }
        out.push_str(&format!("\x1b[{};1H\x1b[0m\x1b[K", self.rows));
        out.push_str(&self.prompt());
        if self.typing.is_some() {
            out.push_str("\x1b[?25h");
        }
        out.push_str("\x1b[?2026l");
        out
    }

    /// The prompt line, as less's `-Ps'?e(END) .'` and its search.
    fn prompt(&self) -> String {
        if let Some(typed) = &self.typing {
            return format!("/{typed}");
        }
        if let Some(note) = &self.note {
            return format!("\x1b[7m{note}\x1b[0m");
        }
        if self.markdown.is_some() && self.at_end() {
            return "\x1b[7m(END)\x1b[0m".to_string();
        }
        String::new()
    }

    /// The screen as text, for the tests.
    pub fn screen(&self) -> Vec<String> {
        (0..self.body())
            .map(|r| {
                self.doc
                    .lines
                    .get(self.top + r)
                    .map(|l| l.plain())
                    .unwrap_or_default()
            })
            .collect()
    }
}

fn act_or_redraw(act: Act) -> Act {
    match act {
        Act::Same => Act::Redraw,
        other => other,
    }
}

/// The terminal as the pager wants it while it runs, and put back however
/// it ends: raw, on the alternate screen, the cursor hidden, clicks and the
/// wheel reported (1000 and SGR's 1006, not every movement).
struct Screen;

impl Screen {
    fn enter() -> io::Result<Screen> {
        terminal::enable_raw_mode()?;
        let mut out = io::stdout();
        out.write_all(b"\x1b[?1049h\x1b[?25l\x1b[?1000h\x1b[?1006h")?;
        out.flush()?;
        Ok(Screen)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let mut out = io::stdout();
        let _ = out.write_all(b"\x1b[?1006l\x1b[?1000l\x1b[0m\x1b[?25h\x1b[?1049l");
        let _ = out.flush();
        let _ = terminal::disable_raw_mode();
    }
}

/// Runs the pager on this terminal until it is closed, showing `content`
/// and then whatever `updates` hands over; `url` is the page o opens.
pub fn run(content: Content, updates: Option<Receiver<Content>>, url: Option<String>) -> ExitCode {
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let mut pager = Pager::new(content, cols as usize, rows as usize).with_url(url);
    let screen = match Screen::enter() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("switchboard: can't take the terminal: {e}");
            return ExitCode::FAILURE;
        }
    };
    let draw = |pager: &Pager| {
        let mut out = io::stdout().lock();
        let _ = out.write_all(pager.frame().as_bytes());
        let _ = out.flush();
    };
    draw(&pager);
    loop {
        let mut redraw = false;
        if let Some(rx) = &updates {
            while let Ok(content) = rx.try_recv() {
                pager.set_content(content);
                redraw = true;
            }
        }
        let act = match event::poll(Duration::from_millis(100)) {
            Ok(true) => match event::read() {
                Ok(Event::Key(key)) => pager.key(key),
                Ok(Event::Mouse(mouse)) => pager.mouse(mouse),
                Ok(Event::Resize(c, r)) => {
                    pager.resize(c as usize, r as usize);
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
            Act::OpenAndQuit(url) => {
                let _ = crate::links::open_url(&url);
                break;
            }
            Act::Open(url) => {
                let _ = crate::links::open_url(&url);
            }
            Act::Redraw => redraw = true,
            Act::Same => {}
        }
        if redraw {
            draw(&pager);
        }
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
}
