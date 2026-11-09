//! Markdown laid out for a terminal, as glow drew it for the PR, ticket and
//! link views with tmux/glow-style.json: glow's dark style without the `#`s
//! before headings, a two-column margin, and a link shown as its text alone,
//! clickable (an OSC 8 hyperlink), where glow printed the URL after it,
//! which in a PR or ticket is mostly long and says nothing the text doesn't
//! (tmux/pr-view.js put the text between markers and spliced the link back
//! into glow's output; here it is a span's own link).
//!
//! It is read once (`Markdown::parse`, pulldown-cmark with GitHub's tables,
//! task lists and strikethrough) into blocks, and laid out for a width
//! (`Markdown::layout`) as often as the pane is resized. Each line laid out
//! carries an `Anchor`, where in the text it starts (its block, and how far
//! into the block, in characters or rows, whatever the width), so a resize
//! opens the new layout at the line holding what was at the top, as
//! pr-view.js's `top` looked for the first line starting with the old top
//! line's words.
//!
//! Each Mermaid block is drawn as text (`mermaid`), as mermaid-ascii drew it
//! for pr-view.js, kept out of the wrapping, which would cut a drawing's
//! boxes apart; a left-to-right flowchart wider than the pane is drawn top
//! to bottom instead, and one still wider is left whole, for the pager to
//! cut at its edge. A diagram that can't be drawn (only flowcharts and
//! sequence diagrams can) keeps its source, with a line saying where it is
//! shown properly.
//!
//! Where the view can draw images (`super::pictures`), a picture on a line
//! of its own and a Mermaid diagram are drawn as pictures once they have
//! been fetched or drawn: `Markdown::wanted` says what the text holds, and
//! `Markdown::layout_with` lays each one that is ready out as rows of the
//! kitty graphics protocol's placeholder cells (`Line::picture`), sized to
//! fit the width and the pane's height. Until then, and anywhere else, each
//! is drawn as text, as above.
//!
//! HTML in the Markdown, as PR templates leave it, is read for what it
//! says: comments dropped, `<br>` a line break, `<img>` an image's line,
//! `<a>` a link, `<b>`, `<i>`, `<code>` and `<summary>` styled, any other
//! tag left out and its text kept.

use std::collections::HashMap;
use std::sync::LazyLock;

use pulldown_cmark::{
    Alignment, CodeBlockKind, Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd,
};
use regex::Regex;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::mermaid;

// glow-style.json's colours, as 256-colour indices.
pub const TEXT: u8 = 252;
const HEADING: u8 = 39;
const H1_FG: u8 = 228;
const H1_BG: u8 = 63;
const H6: u8 = 35;
/// A link's text: pr-view.js's LINK_STYLE, glow's link_text.
const LINK_TEXT: u8 = 35;
/// A bare URL, as glow's link.
const URL: u8 = 30;
const IMAGE_TEXT: u8 = 243;
const CODE_FG: u8 = 203;
const CODE_BG: u8 = 236;
const CODE_BLOCK: u8 = 244;
const RULE: u8 = 240;

/// The narrowest the text is laid out, however narrow the pane.
const MIN_WIDTH: usize = 20;

/// How a piece of text is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub fg: Option<u8>,
    pub bg: Option<u8>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub crossed: bool,
    pub reverse: bool,
}

impl Style {
    pub const fn fg(color: u8) -> Style {
        Style {
            fg: Some(color),
            bg: None,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            crossed: false,
            reverse: false,
        }
    }

    pub const fn bold(mut self) -> Style {
        self.bold = true;
        self
    }

    /// As a cell's style, for what draws it with ratatui (the diff's answer,
    /// the pane beside mission control's boards).
    pub fn ratatui(&self) -> ratatui::style::Style {
        use ratatui::style::{Color, Modifier};
        let mut style = ratatui::style::Style::new();
        if let Some(fg) = self.fg {
            style = style.fg(Color::Indexed(fg));
        }
        if let Some(bg) = self.bg {
            style = style.bg(Color::Indexed(bg));
        }
        for (on, m) in [
            (self.bold, Modifier::BOLD),
            (self.dim, Modifier::DIM),
            (self.italic, Modifier::ITALIC),
            (self.underline, Modifier::UNDERLINED),
            (self.crossed, Modifier::CROSSED_OUT),
            (self.reverse, Modifier::REVERSED),
        ] {
            if on {
                style = style.add_modifier(m);
            }
        }
        style
    }

    /// The escape that sets it, from nothing.
    pub fn sgr(&self) -> String {
        let mut codes = vec!["0".to_string()];
        for (on, code) in [
            (self.bold, "1"),
            (self.dim, "2"),
            (self.italic, "3"),
            (self.underline, "4"),
            (self.reverse, "7"),
            (self.crossed, "9"),
        ] {
            if on {
                codes.push(code.into());
            }
        }
        if let Some(fg) = self.fg {
            codes.push(format!("38;5;{fg}"));
        }
        if let Some(bg) = self.bg {
            codes.push(format!("48;5;{bg}"));
        }
        format!("\x1b[{}m", codes.join(";"))
    }
}

/// Some text in one style, and the link it opens, by its place in the
/// document's links.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
    pub link: Option<usize>,
    /// A row of a picture drawn in the span's place, its text not drawn: a
    /// person's photo on a line of the PR's or the ticket's board
    /// (`super::board`).
    pub picture: Option<PictureRow>,
}

impl Span {
    pub fn new(text: impl Into<String>, style: Style) -> Span {
        Span {
            text: text.into(),
            style,
            link: None,
            picture: None,
        }
    }

    /// The columns it takes: a picture's, else its text's.
    pub fn width(&self) -> usize {
        match self.picture {
            Some(p) => p.cols as usize,
            None => width(&self.text),
        }
    }
}

/// Where a line starts in the text, whatever the width it was laid out
/// for: its block, how far into the block (characters of a paragraph, rows
/// of a code block or table), and 0 for the blank line before a block, 1 on
/// for the block's own lines.
pub type Anchor = (usize, usize, usize);

/// One row of a picture, after a line's spans: the image's id, which row
/// of it, how many columns it takes, and the link a click on it opens (a
/// picture's own, as its text line had).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PictureRow {
    pub id: u32,
    pub row: u16,
    pub cols: u16,
    pub link: Option<usize>,
}

/// One line on the screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Line {
    pub spans: Vec<Span>,
    pub anchor: Anchor,
    pub picture: Option<PictureRow>,
    /// What a picture's first row is found by in a search, its text not
    /// drawn: the picture's own line (`Image: alt →`), or a diagram's source.
    pub searched: String,
}

/// A line with each tab made spaces to the next stop, every eight columns,
/// as a terminal moves to it: the lines a view writes leave control
/// characters out (`Line::ansi`), a tab among them.
pub fn expand_tabs(line: &str) -> String {
    if !line.contains('\t') {
        return line.to_string();
    }
    let mut out = String::new();
    let mut col = 0;
    for g in line.graphemes(true) {
        if g == "\t" {
            let n = 8 - col % 8;
            out.push_str(&" ".repeat(n));
            col += n;
        } else {
            out.push_str(g);
            col += width(g);
        }
    }
    out
}

/// The columns a terminal draws `text` in.
pub fn width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

impl Line {
    fn new(spans: Vec<Span>, anchor: Anchor) -> Line {
        let mut line = Line {
            spans: Vec::new(),
            anchor,
            picture: None,
            searched: String::new(),
        };
        for s in spans {
            line.push(s);
        }
        line
    }

    fn push(&mut self, span: Span) {
        if span.text.is_empty() && span.picture.is_none() {
            return;
        }
        match self.spans.last_mut() {
            Some(last)
                if last.style == span.style
                    && last.link == span.link
                    && last.picture.is_none()
                    && span.picture.is_none() =>
            {
                last.text.push_str(&span.text)
            }
            _ => self.spans.push(span),
        }
    }

    /// A line of `spans`, as a board builds its lines (`super::board`).
    pub fn of(spans: Vec<Span>, anchor: Anchor) -> Line {
        Line::new(spans, anchor)
    }

    /// Its text without styles, a picture in a span's place as spaces.
    pub fn plain(&self) -> String {
        self.spans
            .iter()
            .map(|s| match s.picture {
                Some(p) => " ".repeat(p.cols as usize),
                None => s.text.clone(),
            })
            .collect()
    }

    /// What a search looks in: its text, and what a picture's row is found by.
    pub fn searchable(&self) -> String {
        let mut text = self.plain();
        if !self.searched.is_empty() {
            text.push(' ');
            text.push_str(&self.searched);
        }
        text
    }

    pub fn width(&self) -> usize {
        let picture = self.picture.map_or(0, |p| p.cols as usize);
        self.spans.iter().map(Span::width).sum::<usize>() + picture
    }

    /// The link under a column, if any.
    pub fn link_at(&self, col: usize) -> Option<usize> {
        let mut at = 0;
        for s in &self.spans {
            let w = s.width();
            if col < at + w {
                return s.link;
            }
            at += w;
        }
        self.picture
            .filter(|p| col < at + p.cols as usize)
            .and_then(|p| p.link)
    }

    /// The line as a terminal draws it: `cols` columns from column `left`,
    /// each style set from nothing, each link an OSC 8 hyperlink opened and
    /// closed within the line, and the columns in `marks` (a search's
    /// matches) in reverse video, then the picture's row of placeholder
    /// cells, in the image's id as their color. Control characters in the
    /// text are left out, so nothing a PR or ticket says can reach the
    /// terminal as an escape (a graphics command among them, which the
    /// views' pane may pass on). Ends with the style reset.
    pub fn ansi(
        &self,
        links: &[String],
        left: usize,
        cols: usize,
        marks: &[(usize, usize)],
    ) -> String {
        let mut out = String::new();
        let mut col = 0;
        let mut style: Option<Style> = None;
        let mut link: Option<usize> = None;
        'spans: for span in &self.spans {
            if let Some(p) = span.picture {
                // A picture in the span's place: the cells of it that show,
                // with no style or link of the text around it.
                if link.take().is_some() {
                    out.push_str("\x1b]8;;\x07");
                }
                if style.take().is_some() {
                    out.push_str("\x1b[0m");
                }
                out.push_str(&placeholder_cells(p, col, left, left + cols));
                col += p.cols as usize;
                if col >= left + cols {
                    break 'spans;
                }
                continue;
            }
            for g in span.text.graphemes(true) {
                if g.chars().any(char::is_control) {
                    continue;
                }
                let w = width(g);
                if col < left {
                    col += w;
                    continue;
                }
                if col + w > left + cols {
                    break 'spans;
                }
                let mut want = span.style;
                if marks.iter().any(|(a, b)| col >= *a && col < *b) {
                    want.reverse = !want.reverse;
                }
                let target = span.link.filter(|i| *i < links.len());
                if target != link {
                    if link.is_some() {
                        out.push_str("\x1b]8;;\x07");
                    }
                    if let Some(i) = target {
                        let url: String = links[i].chars().filter(|c| !c.is_control()).collect();
                        out.push_str(&format!("\x1b]8;;{url}\x07"));
                    }
                    link = target;
                }
                if style != Some(want) {
                    out.push_str(&want.sgr());
                    style = Some(want);
                }
                out.push_str(g);
                col += w;
            }
        }
        if link.is_some() {
            out.push_str("\x1b]8;;\x07");
        }
        if style.is_some() {
            out.push_str("\x1b[0m");
        }
        if let Some(p) = self.picture {
            let at = self.spans.iter().map(Span::width).sum::<usize>();
            let cells = placeholder_cells(p, at, left, left + cols);
            if !cells.is_empty() {
                let link = p
                    .link
                    .and_then(|i| links.get(i))
                    .map(|url| url.chars().filter(|c| !c.is_control()).collect::<String>());
                if let Some(url) = &link {
                    out.push_str(&format!("\x1b]8;;{url}\x07"));
                }
                out.push_str(&cells);
                if link.is_some() {
                    out.push_str("\x1b]8;;\x07");
                }
            }
        }
        out
    }
}

/// A picture's row of placeholder cells, in the image's id as their color:
/// the picture starting at column `at`, the cells between columns `from`
/// and `to` drawn; nothing when none of it shows.
fn placeholder_cells(p: PictureRow, at: usize, from: usize, to: usize) -> String {
    let first = from.saturating_sub(at);
    let last = (p.cols as usize).min(to.saturating_sub(at));
    if first >= last {
        return String::new();
    }
    let mut out = format!(
        "\x1b[38;2;{};{};{}m",
        (p.id >> 16) & 0xff,
        (p.id >> 8) & 0xff,
        p.id & 0xff
    );
    for c in first..last {
        out.push_str(&crate::ui::images::placeholder(p.row, c as u16));
    }
    out.push_str("\x1b[0m");
    out
}

/// What a text has that a view can draw as a picture.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Wanted {
    /// A picture on a line of its own, by its link.
    Picture(String),
    /// A Mermaid diagram, by its source.
    Diagram(String),
    /// Someone's photo, for an avatar on a board (`super::board`).
    Photo(crate::photos::Who),
}

impl Wanted {
    /// What the layout knows it by.
    pub fn key(&self) -> String {
        match self {
            Wanted::Picture(url) => url.clone(),
            Wanted::Diagram(source) => format!("mermaid:{source}"),
            Wanted::Photo(who) => photo_key(who),
        }
    }
}

/// What an avatar's photo is known by among a view's pictures.
pub fn photo_key(who: &crate::photos::Who) -> String {
    format!("photo:{:?}:{}", who.service, who.id)
}

/// The pictures ready to be drawn, for `Markdown::layout_with`: each by its
/// key (`Wanted::key`), with its image's id and its size in pixels; the
/// terminal's cell in pixels; and the most rows one may take, the pane's
/// height, so a picture shows whole.
#[derive(Clone, Debug, Default)]
pub struct Images {
    pub cell: (u32, u32),
    pub rows: usize,
    pub ready: HashMap<String, (u32, (u32, u32))>,
}

/// An image placed by a layout: its key, its id and its size in cells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    pub key: String,
    pub id: u32,
    pub cols: u16,
    pub rows: u16,
}

/// The cells a picture `px` pixels in size takes, in cells `cell` pixels
/// in size: its own size, made smaller to fit `width` columns and
/// `max_rows` rows (and the placeholders' marks), its shape kept. Counts
/// the frame `ui::images::raster_picture` draws around it.
pub fn picture_cells(
    px: (u32, u32),
    cell: (u32, u32),
    width: usize,
    max_rows: usize,
) -> (u16, u16) {
    let most = crate::ui::images::MARKS.len();
    let (cw, ch) = (cell.0.max(1) as f64, cell.1.max(1) as f64);
    let (w, h) = (px.0.max(1) as f64, px.1.max(1) as f64);
    let (max_cols, max_rows) = (width.clamp(1, most) as f64, max_rows.clamp(1, most) as f64);
    // The border and the padding inside it, either side.
    let frame = 10.0;
    let mut cols = ((w + frame) / cw).ceil().min(max_cols);
    let mut rows = (((cols * cw - frame).max(1.0) * h / w + frame) / ch).ceil();
    if rows > max_rows {
        rows = max_rows;
        cols = (((rows * ch - frame).max(1.0) * w / h + frame) / cw)
            .ceil()
            .min(max_cols);
    }
    (cols.max(1.0) as u16, rows.max(1.0) as u16)
}

/// A document laid out for a width.
#[derive(Clone, Debug, Default)]
pub struct Doc {
    pub lines: Vec<Line>,
    /// Every link, by the number its spans give.
    pub links: Vec<String>,
    /// The pictures its lines place.
    pub placed: Vec<Placement>,
}

impl Doc {
    /// A document of plain lines, as the pager says something with.
    pub fn plain(lines: Vec<Vec<Span>>) -> Doc {
        Doc {
            lines: lines
                .into_iter()
                .enumerate()
                .map(|(i, spans)| Line::new(spans, (i, 0, 1)))
                .collect(),
            links: Vec::new(),
            placed: Vec::new(),
        }
    }

    /// The line that shows what was at `anchor` in another layout: the last
    /// one starting at or before it.
    pub fn line_at(&self, anchor: Anchor) -> usize {
        self.lines
            .iter()
            .rposition(|l| l.anchor <= anchor)
            .unwrap_or(0)
    }
}

// ── Reading it ──────────────────────────────────────────────────────────────

/// A piece of inline text.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Run {
    Text(Span),
    /// A picture as its text (`Image: alt →`), and its link.
    Picture(Span, String),
    /// A line break where it is written (`<br>`, two spaces at a line's end).
    Break,
}

#[derive(Clone, Debug)]
struct Item {
    task: Option<bool>,
    blocks: Vec<Block>,
}

#[derive(Clone, Debug)]
enum Block {
    Para(Vec<Run>),
    Heading(u8, Vec<Run>),
    Quote(Vec<Block>),
    List {
        start: Option<u64>,
        items: Vec<Item>,
        loose: bool,
    },
    Code(String),
    Mermaid(String),
    Table {
        aligns: Vec<Alignment>,
        rows: Vec<Vec<Vec<Run>>>,
    },
    Rule,
}

/// A Markdown text, read, to be laid out for any width.
#[derive(Clone, Debug)]
pub struct Markdown {
    blocks: Vec<Block>,
    links: Vec<String>,
    /// Where a diagram that can't be drawn is shown properly ("M-r on GitHub").
    place: String,
}

enum Container {
    Root(Vec<Block>),
    Quote(Vec<Block>),
    List {
        start: Option<u64>,
        items: Vec<Item>,
        loose: bool,
    },
    Item(Item),
}

impl Container {
    fn blocks(&mut self) -> Option<&mut Vec<Block>> {
        match self {
            Container::Root(b) | Container::Quote(b) => Some(b),
            Container::Item(item) => Some(&mut item.blocks),
            Container::List { .. } => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InlineKind {
    Para,
    Heading(u8),
    Cell,
}

#[derive(Default)]
struct TableBuild {
    aligns: Vec<Alignment>,
    rows: Vec<Vec<Vec<Run>>>,
    row: Vec<Vec<Run>>,
}

struct Builder {
    stack: Vec<Container>,
    links: Vec<String>,
    inline: Option<(InlineKind, Vec<Run>)>,
    /// Inline styles open (emphasis, strong, a link's text), innermost last.
    styles: Vec<Style>,
    link: Option<usize>,
    code: Option<(String, String)>,
    html: Option<String>,
    image: Option<(String, String)>,
    table: Option<TableBuild>,
}

static COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?(-->|$)").unwrap());
static TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<(/?)([A-Za-z][A-Za-z0-9-]*)((?:[^>"']|"[^"]*"|'[^']*')*)>"#).unwrap()
});
static ATTR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"([A-Za-z-]+)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#).unwrap()
});
static BARE_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"https?://[A-Za-z0-9._~:/?#\[\]@!$&'()*+,;=%-]+").unwrap());

/// HTML's few entities that turn up in Markdown, as text.
pub fn decode_entities(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

fn attr(attrs: &str, name: &str) -> Option<String> {
    ATTR.captures_iter(attrs)
        .find(|c| c[1].eq_ignore_ascii_case(name))
        .and_then(|c| c.get(2).or(c.get(3)).or(c.get(4)))
        .map(|m| decode_entities(m.as_str()))
}

impl Builder {
    fn new() -> Builder {
        Builder {
            stack: vec![Container::Root(Vec::new())],
            links: Vec::new(),
            inline: None,
            styles: Vec::new(),
            link: None,
            code: None,
            html: None,
            image: None,
            table: None,
        }
    }

    fn base(&self) -> Style {
        match self.inline {
            Some((InlineKind::Heading(1), _)) => Style {
                bg: Some(H1_BG),
                ..Style::fg(H1_FG).bold()
            },
            Some((InlineKind::Heading(2), _)) => Style {
                underline: true,
                ..Style::fg(HEADING).bold()
            },
            Some((InlineKind::Heading(6), _)) => Style::fg(H6),
            Some((InlineKind::Heading(_), _)) => Style::fg(HEADING).bold(),
            _ => Style::fg(TEXT),
        }
    }

    /// The style text is in now: the block's, with every inline style open
    /// laid over it.
    fn style(&self) -> Style {
        let mut style = self.base();
        for s in &self.styles {
            if s.fg.is_some() {
                style.fg = s.fg;
            }
            style.bold |= s.bold;
            style.italic |= s.italic;
            style.underline |= s.underline;
            style.crossed |= s.crossed;
        }
        style
    }

    fn blocks(&mut self) -> &mut Vec<Block> {
        let top = self.stack.len() - 1;
        if self.stack[top].blocks().is_none() {
            // Something block-level straight inside a list, which pulldown
            // doesn't give; it goes into an item of its own.
            self.stack.push(Container::Item(Item {
                task: None,
                blocks: Vec::new(),
            }));
        }
        let top = self.stack.len() - 1;
        self.stack[top].blocks().expect("a container with blocks")
    }

    fn push_block(&mut self, block: Block) {
        self.blocks().push(block);
    }

    /// Inline text with nowhere to go (a tight list's item has no
    /// paragraph): a paragraph of its own.
    fn ensure_inline(&mut self) {
        if self.inline.is_none() {
            self.inline = Some((InlineKind::Para, Vec::new()));
        }
    }

    /// Ends a paragraph opened for loose inline text.
    fn end_loose(&mut self) {
        if !matches!(self.inline, Some((InlineKind::Para, _))) {
            return;
        }
        let Some((_, runs)) = self.inline.take() else {
            return;
        };
        if runs
            .iter()
            .any(|r| matches!(r, Run::Text(s) | Run::Picture(s, _) if !s.text.trim().is_empty()))
        {
            self.push_block(Block::Para(runs));
        }
    }

    fn run(&mut self, text: &str, style: Style, link: Option<usize>) {
        self.ensure_inline();
        if let Some((_, runs)) = &mut self.inline {
            runs.push(Run::Text(Span {
                text: text.to_string(),
                style,
                link,
                picture: None,
            }));
        }
    }

    fn line_break(&mut self) {
        self.ensure_inline();
        if let Some((_, runs)) = &mut self.inline {
            runs.push(Run::Break);
        }
    }

    fn add_link(&mut self, url: &str) -> usize {
        self.links.push(url.to_string());
        self.links.len() - 1
    }

    /// Text, its bare URLs made links of their own (outside a link).
    fn text(&mut self, text: &str) {
        let style = self.style();
        if self.link.is_some() {
            self.run(text, style, self.link);
            return;
        }
        let mut at = 0;
        for m in BARE_URL.find_iter(text) {
            let mut url = m.as_str();
            // Punctuation after a link in a sentence isn't the link's.
            while let Some(last) = url.chars().last() {
                let unbalanced = last == ')' && url.matches('(').count() < url.matches(')').count();
                if ".,;:!?'".contains(last) || unbalanced {
                    url = &url[..url.len() - last.len_utf8()];
                } else {
                    break;
                }
            }
            if url.len() <= "https://".len() {
                continue;
            }
            self.run(&text[at..m.start()], style, None);
            let link = self.add_link(url);
            let url_style = Style {
                underline: true,
                ..Style::fg(URL)
            };
            self.run(url, url_style, Some(link));
            at = m.start() + url.len();
        }
        self.run(&text[at..], style, None);
    }

    fn image(&mut self, url: &str, alt: &str) {
        let link = self.add_link(url);
        let alt = alt.trim();
        let text = if alt.is_empty() {
            "Image →".to_string()
        } else {
            format!("Image: {alt} →")
        };
        self.ensure_inline();
        if let Some((_, runs)) = &mut self.inline {
            runs.push(Run::Picture(
                Span {
                    text,
                    style: Style::fg(IMAGE_TEXT),
                    link: Some(link),
                    picture: None,
                },
                url.to_string(),
            ));
        }
    }

    /// HTML, read for what it says (the module's comment says how).
    fn html(&mut self, html: &str) {
        let html = COMMENT.replace_all(html, "");
        let mut at = 0;
        for tag in TAG.captures_iter(&html) {
            let whole = tag.get(0).expect("a match");
            self.html_text(&html[at..whole.start()]);
            at = whole.end();
            let closing = !tag[1].is_empty();
            let name = tag[2].to_ascii_lowercase();
            let attrs = &tag[3];
            match (name.as_str(), closing) {
                ("br", _) => self.line_break(),
                ("img", false) => {
                    if let Some(src) = attr(attrs, "src") {
                        self.image(&src, &attr(attrs, "alt").unwrap_or_default());
                    }
                }
                ("a", false) => {
                    if let Some(href) = attr(attrs, "href") {
                        self.styles.push(Style::fg(LINK_TEXT).bold());
                        self.link = Some(self.add_link(&href));
                    }
                }
                ("a", true) if self.link.is_some() => {
                    self.link = None;
                    self.styles.pop();
                }
                ("b" | "strong" | "summary", false) => {
                    if name == "summary" {
                        self.run("▸ ", self.style(), None);
                    }
                    self.styles.push(Style::default().bold());
                }
                ("i" | "em", false) => self.styles.push(Style {
                    italic: true,
                    ..Style::default()
                }),
                ("code", false) => self.styles.push(Style::fg(CODE_FG)),
                ("b" | "strong" | "summary" | "i" | "em" | "code", true) => {
                    self.styles.pop();
                    if name == "summary" {
                        self.line_break();
                    }
                }
                ("p" | "div" | "details" | "li" | "tr", true) => self.line_break(),
                _ => {}
            }
        }
        self.html_text(&html[at..]);
    }

    fn html_text(&mut self, text: &str) {
        let text = decode_entities(text);
        let squeezed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if squeezed.is_empty() {
            if !text.is_empty() && self.inline.as_ref().is_some_and(|(_, r)| !r.is_empty()) {
                self.run(" ", self.style(), self.link);
            }
            return;
        }
        let lead = if text.starts_with(char::is_whitespace) {
            " "
        } else {
            ""
        };
        let trail = if text.ends_with(char::is_whitespace) {
            " "
        } else {
            ""
        };
        self.text(&format!("{lead}{squeezed}{trail}"));
    }

    fn event(&mut self, event: Event<'_>) {
        if let Some((_, code)) = &mut self.code {
            match event {
                Event::Text(t) => code.push_str(&t),
                Event::End(TagEnd::CodeBlock) => {
                    let (lang, text) = self.code.take().expect("in a code block");
                    let text = text.trim_end_matches('\n').to_string();
                    if lang.eq_ignore_ascii_case("mermaid") {
                        self.push_block(Block::Mermaid(text));
                    } else {
                        self.push_block(Block::Code(text));
                    }
                }
                _ => {}
            }
            return;
        }
        if let Some(html) = &mut self.html {
            match event {
                Event::Html(t) | Event::Text(t) => html.push_str(&t),
                Event::End(TagEnd::HtmlBlock) => {
                    let html = self.html.take().expect("in an HTML block");
                    self.end_loose();
                    self.html(&html);
                    self.end_loose();
                }
                _ => {}
            }
            return;
        }
        if let Some((_, alt)) = &mut self.image {
            match event {
                Event::Text(t) | Event::Code(t) => alt.push_str(&t),
                Event::End(TagEnd::Image) => {
                    let (url, alt) = self.image.take().expect("in an image");
                    self.image(&url, &alt);
                }
                _ => {}
            }
            return;
        }
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => self.text(&t),
            Event::Code(t) => {
                let style = Style {
                    bg: Some(CODE_BG),
                    ..Style::fg(CODE_FG)
                };
                // glow-style.json's prefix and suffix, as no-break spaces,
                // so they stay with the code and in its colours.
                self.run(&format!("\u{a0}{t}\u{a0}"), style, self.link);
            }
            Event::InlineMath(t) | Event::DisplayMath(t) => self.text(&t),
            Event::Html(t) | Event::InlineHtml(t) => self.html(&t),
            Event::FootnoteReference(name) => {
                let style = self.style();
                self.run(&format!("[^{name}]"), style, None);
            }
            Event::SoftBreak => {
                let style = self.style();
                self.run(" ", style, self.link);
            }
            Event::HardBreak => self.line_break(),
            Event::Rule => {
                self.end_loose();
                self.push_block(Block::Rule);
            }
            Event::TaskListMarker(done) => {
                if let Some(Container::Item(item)) = self.stack.last_mut() {
                    item.task = Some(done);
                }
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                self.end_loose();
                // A paragraph of its own in a list's item makes the list
                // loose, its items a blank line apart.
                let n = self.stack.len();
                if n >= 2
                    && matches!(self.stack[n - 1], Container::Item(_))
                    && let Container::List { loose, .. } = &mut self.stack[n - 2]
                {
                    *loose = true;
                }
                self.inline = Some((InlineKind::Para, Vec::new()));
            }
            Tag::Heading { level, .. } => {
                self.end_loose();
                let level = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                self.inline = Some((InlineKind::Heading(level), Vec::new()));
            }
            Tag::BlockQuote(kind) => {
                self.end_loose();
                self.stack.push(Container::Quote(Vec::new()));
                // GitHub's alerts ("> [!NOTE]") say what they are first.
                if let Some(kind) = kind {
                    let word = format!("{kind:?}");
                    self.inline = Some((InlineKind::Para, Vec::new()));
                    self.run(&word, Style::fg(TEXT).bold(), None);
                    self.end_loose();
                }
            }
            Tag::CodeBlock(kind) => {
                self.end_loose();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::HtmlBlock => self.html = Some(String::new()),
            Tag::List(start) => {
                self.end_loose();
                self.stack.push(Container::List {
                    start,
                    items: Vec::new(),
                    loose: false,
                });
            }
            Tag::Item => {
                self.end_loose();
                self.stack.push(Container::Item(Item {
                    task: None,
                    blocks: Vec::new(),
                }));
            }
            Tag::Table(aligns) => {
                self.end_loose();
                self.table = Some(TableBuild {
                    aligns,
                    ..TableBuild::default()
                });
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(t) = &mut self.table {
                    t.row.clear();
                }
            }
            Tag::TableCell => self.inline = Some((InlineKind::Cell, Vec::new())),
            Tag::Emphasis => self.styles.push(Style {
                italic: true,
                ..Style::default()
            }),
            Tag::Strong => self.styles.push(Style::default().bold()),
            Tag::Strikethrough => self.styles.push(Style {
                crossed: true,
                ..Style::default()
            }),
            Tag::Link {
                link_type,
                dest_url,
                ..
            } => {
                let url = match link_type {
                    LinkType::Email => format!("mailto:{dest_url}"),
                    _ => dest_url.to_string(),
                };
                let style = if matches!(link_type, LinkType::Autolink | LinkType::Email) {
                    Style {
                        underline: true,
                        ..Style::fg(URL)
                    }
                } else {
                    Style::fg(LINK_TEXT).bold()
                };
                self.styles.push(style);
                self.link = Some(self.add_link(&url));
            }
            Tag::Image { dest_url, .. } => self.image = Some((dest_url.to_string(), String::new())),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                if let Some((_, runs)) = self.inline.take() {
                    self.push_block(Block::Para(runs));
                }
            }
            TagEnd::Heading(_) => {
                if let Some((InlineKind::Heading(level), runs)) = self.inline.take() {
                    self.push_block(Block::Heading(level, runs));
                }
            }
            TagEnd::BlockQuote(_) => {
                self.end_loose();
                if let Some(Container::Quote(blocks)) = self.stack.pop() {
                    self.push_block(Block::Quote(blocks));
                }
            }
            TagEnd::List(_) => {
                self.end_loose();
                if matches!(self.stack.last(), Some(Container::Item(_))) {
                    self.end(TagEnd::Item);
                }
                if let Some(Container::List {
                    start,
                    items,
                    loose,
                }) = self.stack.pop()
                {
                    self.push_block(Block::List {
                        start,
                        items,
                        loose,
                    });
                }
            }
            TagEnd::Item => {
                self.end_loose();
                if let Some(Container::Item(item)) = self.stack.pop()
                    && let Some(Container::List { items, .. }) = self.stack.last_mut()
                {
                    items.push(item);
                }
            }
            TagEnd::TableCell => {
                let runs = self.inline.take().map(|(_, r)| r).unwrap_or_default();
                if let Some(t) = &mut self.table {
                    t.row.push(runs);
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(t) = &mut self.table {
                    let row = std::mem::take(&mut t.row);
                    t.rows.push(row);
                }
            }
            TagEnd::Table => {
                if let Some(t) = self.table.take() {
                    self.push_block(Block::Table {
                        aligns: t.aligns,
                        rows: t.rows,
                    });
                }
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.styles.pop();
            }
            TagEnd::Link => {
                self.link = None;
                self.styles.pop();
            }
            _ => {}
        }
    }

    fn finish(mut self) -> (Vec<Block>, Vec<String>) {
        self.end_loose();
        while self.stack.len() > 1 {
            match self.stack.pop() {
                Some(Container::Quote(blocks)) => self.push_block(Block::Quote(blocks)),
                Some(Container::Item(item)) => {
                    if let Some(Container::List { items, .. }) = self.stack.last_mut() {
                        items.push(item);
                    }
                }
                Some(Container::List {
                    start,
                    items,
                    loose,
                }) => self.push_block(Block::List {
                    start,
                    items,
                    loose,
                }),
                _ => {}
            }
        }
        match self.stack.pop() {
            Some(Container::Root(blocks)) => (blocks, self.links),
            _ => (Vec::new(), self.links),
        }
    }
}

impl Markdown {
    /// Reads `text`; `place` says where a diagram that can't be drawn here
    /// is shown properly ("M-r on GitHub").
    pub fn parse(text: &str, place: &str) -> Markdown {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let options = Options::ENABLE_TABLES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_GFM;
        let mut builder = Builder::new();
        for event in Parser::new_ext(&text, options) {
            builder.event(event);
        }
        let (blocks, links) = builder.finish();
        Markdown {
            blocks,
            links,
            place: place.to_string(),
        }
    }

    /// What it has that a view can draw as a picture: each picture on a
    /// line of its own, by an https link, and each Mermaid diagram, once.
    pub fn wanted(&self) -> Vec<Wanted> {
        fn walk(blocks: &[Block], out: &mut Vec<Wanted>) {
            for b in blocks {
                match b {
                    Block::Para(runs) if pictures_alone(runs) => {
                        for r in runs {
                            if let Run::Picture(_, url) = r
                                && url.starts_with("https://")
                            {
                                out.push(Wanted::Picture(url.clone()));
                            }
                        }
                    }
                    Block::Mermaid(source) => out.push(Wanted::Diagram(source.clone())),
                    Block::Quote(inner) => walk(inner, out),
                    Block::List { items, .. } => {
                        for item in items {
                            walk(&item.blocks, out);
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut found = Vec::new();
        walk(&self.blocks, &mut found);
        let mut seen = std::collections::HashSet::new();
        found.retain(|w| seen.insert(w.clone()));
        found
    }

    /// Laid out for a pane `cols` wide: two columns of margin each side.
    pub fn layout(&self, cols: usize) -> Doc {
        self.layout_with(cols, None)
    }

    /// Laid out for a pane `cols` wide, each picture or diagram `images`
    /// has ready drawn as one.
    pub fn layout_with(&self, cols: usize, images: Option<&Images>) -> Doc {
        let mut out = Out {
            lines: Vec::new(),
            links: self.links.clone(),
            block: 0,
            place: &self.place,
            images,
            placed: Vec::new(),
        };
        let width = cols.saturating_sub(4).max(MIN_WIDTH);
        let margin = vec![Span::new("  ", Style::fg(TEXT))];
        out.blank(&[]);
        for (i, b) in self.blocks.iter().enumerate() {
            if i > 0 {
                out.blank(&[]);
            }
            out.block(b, &margin, &margin, width);
        }
        out.blank(&[]);
        Doc {
            lines: out.lines,
            links: out.links,
            placed: out.placed,
        }
    }
}

/// An image's id for one size of it, so the same picture shown at two
/// sizes (at the top level and in a quote) is two images, each drawn at its
/// own size: the id mixed with the size, in the id's half of the range,
/// never 0.
pub fn sized_id(id: u32, cols: u16, rows: u16) -> u32 {
    let mut h = id;
    for b in cols.to_le_bytes().iter().chain(&rows.to_le_bytes()) {
        h = (h ^ *b as u32).wrapping_mul(16_777_619);
    }
    ((id & 0x80_0000) | (h & 0x7f_ffff)).max(1)
}

/// How far apart the anchors of a paragraph's pictures are: more rows than
/// a picture can take (`ui::images::MARKS`).
const PICTURE_STRIDE: usize = 1 << 16;

/// Whether a paragraph is pictures alone, a line each or side by side.
fn pictures_alone(runs: &[Run]) -> bool {
    runs.iter().any(|r| matches!(r, Run::Picture(..)))
        && runs.iter().all(|r| match r {
            Run::Picture(..) | Run::Break => true,
            Run::Text(s) => s.text.trim().is_empty(),
        })
}

// ── Laying it out ───────────────────────────────────────────────────────────

struct Out<'a> {
    lines: Vec<Line>,
    links: Vec<String>,
    /// The blocks laid out so far, for the anchors.
    block: usize,
    place: &'a str,
    images: Option<&'a Images>,
    placed: Vec<Placement>,
}

fn spaces(n: usize) -> Span {
    Span::new(" ".repeat(n), Style::fg(TEXT))
}

/// A word, as pieces of runs with no space between them, which wrap as one.
struct Word {
    pieces: Vec<Span>,
    width: usize,
    /// Characters of the text before it, for the anchors.
    offset: usize,
    chars: usize,
}

enum Token {
    Word(Word),
    Space(Span),
    Break,
}

fn tokens(runs: &[Run]) -> Vec<Token> {
    let mut out = Vec::new();
    let mut word: Option<Word> = None;
    let mut offset = 0;
    let flush = |word: &mut Option<Word>, out: &mut Vec<Token>| {
        if let Some(w) = word.take() {
            out.push(Token::Word(w));
        }
    };
    for run in runs {
        let span = match run {
            Run::Break => {
                flush(&mut word, &mut out);
                out.push(Token::Break);
                offset += 1;
                continue;
            }
            Run::Text(span) | Run::Picture(span, _) => span,
        };
        for piece in span.text.split_word_bounds() {
            let chars = piece.chars().count();
            if piece.chars().all(|c| c.is_whitespace() && c != '\u{a0}') {
                flush(&mut word, &mut out);
                out.push(Token::Space(Span {
                    text: " ".into(),
                    ..span.clone()
                }));
                offset += chars;
                continue;
            }
            let w = word.get_or_insert_with(|| Word {
                pieces: Vec::new(),
                width: 0,
                offset,
                chars: 0,
            });
            w.width += width(piece);
            w.chars += chars;
            w.pieces.push(Span {
                text: piece.to_string(),
                ..span.clone()
            });
            offset += chars;
        }
    }
    flush(&mut word, &mut out);
    out
}

/// Inline runs wrapped to `width`: each line's spans and the characters of
/// the text before it. Spaces at a wrap go; a word longer than a line is
/// broken where the line ends.
fn wrap(runs: &[Run], cols: usize) -> Vec<(Vec<Span>, usize)> {
    let cols = cols.max(1);
    let mut lines: Vec<(Vec<Span>, usize)> = Vec::new();
    let mut line: Vec<Span> = Vec::new();
    let mut used = 0;
    let mut start = 0;
    let mut space: Option<Span> = None;
    let mut started = false;
    for token in tokens(runs) {
        match token {
            Token::Break => {
                lines.push((std::mem::take(&mut line), start));
                used = 0;
                space = None;
                started = false;
            }
            Token::Space(s) => {
                if started {
                    space = Some(s);
                }
            }
            Token::Word(word) => {
                let gap = usize::from(space.is_some());
                if started && used + gap + word.width > cols {
                    lines.push((std::mem::take(&mut line), start));
                    used = 0;
                    space = None;
                    started = false;
                }
                if !started {
                    start = word.offset;
                    started = true;
                }
                if let Some(s) = space.take() {
                    line.push(s);
                    used += 1;
                }
                for piece in word.pieces {
                    if used + width_of(&piece) <= cols {
                        used += width_of(&piece);
                        line.push(piece);
                        continue;
                    }
                    // Longer than what is left: broken by characters.
                    let mut head = String::new();
                    for g in piece.text.graphemes(true) {
                        let w = width(g);
                        if used + w > cols && used > 0 {
                            if !head.is_empty() {
                                line.push(Span {
                                    text: std::mem::take(&mut head),
                                    ..piece.clone()
                                });
                            }
                            lines.push((std::mem::take(&mut line), start));
                            used = 0;
                        }
                        head.push_str(g);
                        used += w;
                    }
                    if !head.is_empty() {
                        line.push(Span {
                            text: head,
                            ..piece
                        });
                    }
                }
            }
        }
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push((line, start));
    }
    lines
}

fn width_of(span: &Span) -> usize {
    span.width()
}

impl Out<'_> {
    fn blank(&mut self, prefix: &[Span]) {
        let anchor = (self.block + 1, 0, 0);
        let spans: Vec<Span> = prefix
            .iter()
            .map(|s| Span {
                text: s.text.trim_end().to_string(),
                ..s.clone()
            })
            .collect();
        self.lines.push(Line::new(spans, anchor));
    }

    fn push(&mut self, prefix: &[Span], spans: Vec<Span>, anchor: Anchor) {
        let mut all = prefix.to_vec();
        all.extend(spans);
        self.lines.push(Line::new(all, anchor));
    }

    fn text(
        &mut self,
        runs: &[Run],
        first: &[Span],
        rest: &[Span],
        width: usize,
        pad: Option<Style>,
    ) {
        self.block += 1;
        for (i, (mut spans, offset)) in wrap(runs, width).into_iter().enumerate() {
            if let Some(style) = pad {
                // h1's bar: a space either side, in its colours.
                spans.insert(0, Span::new(" ", style));
                spans.push(Span::new(" ", style));
            }
            let prefix = if i == 0 { first } else { rest };
            self.push(prefix, spans, (self.block, offset, 1));
        }
    }

    /// The image ready for `key`, if any, and the cells it takes in
    /// `width` columns.
    fn ready(&self, key: &str, width: usize) -> Option<(u32, u16, u16)> {
        let images = self.images?;
        let (id, px) = *images.ready.get(key)?;
        let (cols, rows) = picture_cells(px, images.cell, width, images.rows);
        Some((sized_id(id, cols, rows), cols, rows))
    }

    /// A picture's rows of placeholder cells, after the prefix.
    fn picture(
        &mut self,
        key: &str,
        (id, cols, rows): (u32, u16, u16),
        link: Option<usize>,
        first: &[Span],
        rest: &[Span],
        at: &mut usize,
    ) {
        for row in 0..rows {
            let prefix = if *at == 0 { first } else { rest };
            let mut line = Line::new(prefix.to_vec(), (self.block, *at, 1));
            line.picture = Some(PictureRow {
                id,
                row,
                cols,
                link,
            });
            self.lines.push(line);
            *at += 1;
        }
        if !self.placed.iter().any(|p| p.id == id) {
            self.placed.push(Placement {
                key: key.to_string(),
                id,
                cols,
                rows,
            });
        }
    }

    /// A paragraph of pictures alone, each ready one drawn as a picture and
    /// the rest as their text, a line each. Each picture's rows are anchored
    /// by its place among the paragraph's pictures (`PICTURE_STRIDE` apart)
    /// and its own row, never by the rows of the pictures before it, so a
    /// place in one stays put as another arrives. Only where the view draws
    /// pictures (`Images` given), where it is laid out this way from the
    /// start, a picture still to come on a line of its own; elsewhere the
    /// paragraph is text, as any other.
    fn pictures(&mut self, runs: &[Run], first: &[Span], rest: &[Span], width: usize) -> bool {
        let ready: Vec<Option<(u32, u16, u16)>> = runs
            .iter()
            .map(|r| match r {
                Run::Picture(_, url) => self.ready(url, width),
                _ => None,
            })
            .collect();
        if self.images.is_none() {
            return false;
        }
        self.block += 1;
        let mut at = 0;
        let pictures = runs.iter().zip(ready).filter_map(|(run, ready)| match run {
            Run::Picture(span, url) => Some((span, url, ready)),
            _ => None,
        });
        for (k, (span, url, ready)) in pictures.enumerate() {
            let row = self.lines.len();
            match ready {
                Some(image) => {
                    self.picture(url, image, span.link, first, rest, &mut at);
                    self.lines[row].searched = span.text.clone();
                }
                None => {
                    for (spans, _) in wrap(&[Run::Text(span.clone())], width) {
                        let prefix = if at == 0 { first } else { rest };
                        self.push(prefix, spans, (self.block, 0, 1));
                        at += 1;
                    }
                }
            }
            for (i, line) in self.lines[row..].iter_mut().enumerate() {
                line.anchor = (self.block, k * PICTURE_STRIDE + i, 1);
            }
        }
        true
    }

    fn block(&mut self, block: &Block, first: &[Span], rest: &[Span], width: usize) {
        match block {
            Block::Para(runs)
                if pictures_alone(runs) && self.pictures(runs, first, rest, width) => {}
            Block::Para(runs) => self.text(runs, first, rest, width, None),
            Block::Heading(level, runs) => {
                let h1 = (*level == 1).then(|| Style {
                    bg: Some(H1_BG),
                    ..Style::fg(H1_FG).bold()
                });
                self.text(runs, first, rest, width.saturating_sub(2).max(1), h1);
            }
            Block::Quote(blocks) => {
                let bar = Span::new("│ ", Style::fg(TEXT));
                let mut qfirst = first.to_vec();
                qfirst.push(bar.clone());
                let mut qrest = rest.to_vec();
                qrest.push(bar);
                for (i, b) in blocks.iter().enumerate() {
                    if i > 0 {
                        self.blank(&qrest);
                    }
                    let lead = if i == 0 { &qfirst } else { &qrest };
                    self.block(b, lead, &qrest, width.saturating_sub(2).max(1));
                }
            }
            Block::List {
                start,
                items,
                loose,
            } => {
                let numbers = start.map(|s| {
                    let last = s + items.len().saturating_sub(1) as u64;
                    (s, last.to_string().len())
                });
                for (i, item) in items.iter().enumerate() {
                    if i > 0 && *loose {
                        self.blank(rest);
                    }
                    let marker = match (item.task, numbers) {
                        (Some(true), _) => "[✓] ".to_string(),
                        (Some(false), _) => "[ ] ".to_string(),
                        (None, Some((s, digits))) => {
                            format!("{:>digits$}. ", s + i as u64)
                        }
                        (None, None) => "• ".to_string(),
                    };
                    let mark_w = UnicodeWidthStr::width(marker.as_str());
                    let mut ifirst = if i == 0 {
                        first.to_vec()
                    } else {
                        rest.to_vec()
                    };
                    ifirst.push(Span::new(marker, Style::fg(TEXT)));
                    let mut irest = rest.to_vec();
                    irest.push(spaces(mark_w));
                    let inner = width.saturating_sub(mark_w).max(1);
                    if item.blocks.is_empty() {
                        self.block += 1;
                        self.push(&ifirst, Vec::new(), (self.block, 0, 1));
                    }
                    for (j, b) in item.blocks.iter().enumerate() {
                        if j > 0 && *loose {
                            self.blank(&irest);
                        }
                        let lead = if j == 0 { &ifirst } else { &irest };
                        self.block(b, lead, &irest, inner);
                    }
                }
            }
            Block::Code(text) => {
                self.block += 1;
                let style = Style::fg(CODE_BLOCK);
                for (i, line) in text.split('\n').enumerate() {
                    let prefix = if i == 0 { first } else { rest };
                    let mut spans = vec![spaces(2)];
                    spans.push(Span::new(line.replace('\t', "    "), style));
                    self.push(prefix, spans, (self.block, i, 1));
                }
            }
            Block::Mermaid(source)
                if let Some(image) = self.ready(&Wanted::Diagram(source.clone()).key(), width) =>
            {
                self.block += 1;
                let key = Wanted::Diagram(source.clone()).key();
                let row = self.lines.len();
                self.picture(&key, image, None, first, rest, &mut 0);
                self.lines[row].searched = source.replace('\n', " ");
                // As many blocks as its text takes (a line saying where it
                // is drawn, then its source, when it can't be drawn as
                // text), so the blocks after it keep their anchors and the
                // line at the top stays as the picture arrives.
                if mermaid::draw(source, width).is_none() {
                    self.block += 1;
                }
                // A place in its text (the drawing, or the line and the
                // source) comes back as the picture's first row: the rows
                // after it sit past every place in the text.
                let rows = self.lines.len() - row;
                for (i, line) in self.lines[row + 1..].iter_mut().enumerate() {
                    line.anchor = (self.block, usize::MAX - rows + 1 + i, 1);
                }
            }
            Block::Mermaid(source) => match mermaid::draw(source, width) {
                Some(drawing) => {
                    self.block += 1;
                    for (i, line) in drawing.split('\n').enumerate() {
                        let prefix = if i == 0 { first } else { rest };
                        self.push(
                            prefix,
                            vec![Span::new(line, Style::fg(TEXT))],
                            (self.block, i, 1),
                        );
                    }
                }
                None => {
                    let note = format!(
                        "A Mermaid diagram that can't be drawn as text; {} shows it:",
                        self.place
                    );
                    let style = Style {
                        italic: true,
                        ..Style::fg(TEXT)
                    };
                    self.text(
                        &[Run::Text(Span::new(note, style))],
                        first,
                        rest,
                        width,
                        None,
                    );
                    self.blank(rest);
                    self.block(&Block::Code(source.clone()), rest, rest, width);
                }
            },
            Block::Table { aligns, rows } => self.table(aligns, rows, first, rest, width),
            Block::Rule => {
                self.block += 1;
                self.push(
                    first,
                    vec![Span::new("--------", Style::fg(RULE))],
                    (self.block, 0, 1),
                );
            }
        }
    }

    fn table(
        &mut self,
        aligns: &[Alignment],
        rows: &[Vec<Vec<Run>>],
        first: &[Span],
        rest: &[Span],
        width: usize,
    ) {
        self.block += 1;
        let n = rows.iter().map(Vec::len).max().unwrap_or(0);
        if n == 0 {
            return;
        }
        let natural = |runs: &[Run]| -> usize {
            wrap(runs, usize::MAX / 2)
                .iter()
                .map(|(spans, _)| spans.iter().map(width_of).sum::<usize>())
                .max()
                .unwrap_or(0)
        };
        let mut cols: Vec<usize> = (0..n)
            .map(|c| {
                rows.iter()
                    .filter_map(|r| r.get(c))
                    .map(|cell| natural(cell))
                    .max()
                    .unwrap_or(0)
                    .max(1)
            })
            .collect();
        // A space before the first column and " │ " between the rest; the
        // widest column gives way until the table fits.
        let total = |cols: &[usize]| 1 + cols.iter().sum::<usize>() + 3 * (cols.len() - 1);
        while total(&cols) > width {
            let (at, widest) = cols
                .iter()
                .enumerate()
                .max_by_key(|(_, w)| **w)
                .map(|(i, w)| (i, *w))
                .expect("a column");
            if widest <= 3 {
                break;
            }
            cols[at] -= 1;
        }
        let border = Style::fg(RULE);
        let mut line_no = 0;
        for (r, row) in rows.iter().enumerate() {
            let cells: Vec<Vec<(Vec<Span>, usize)>> = (0..n)
                .map(|c| {
                    let empty = Vec::new();
                    let runs = row.get(c).unwrap_or(&empty);
                    let mut lines = wrap(runs, cols[c]);
                    if r == 0 {
                        for (spans, _) in &mut lines {
                            for s in spans {
                                s.style.bold = true;
                            }
                        }
                    }
                    lines
                })
                .collect();
            let height = cells.iter().map(Vec::len).max().unwrap_or(1).max(1);
            for h in 0..height {
                let mut spans = vec![Span::new(" ", Style::fg(TEXT))];
                for c in 0..n {
                    if c > 0 {
                        spans.push(Span::new(" │ ", border));
                    }
                    let (cell, used) = match cells[c].get(h) {
                        Some((s, _)) => (s.clone(), s.iter().map(width_of).sum::<usize>()),
                        None => (Vec::new(), 0),
                    };
                    let free = cols[c].saturating_sub(used);
                    let (before, after) = match aligns.get(c) {
                        Some(Alignment::Right) => (free, 0),
                        Some(Alignment::Center) => (free / 2, free - free / 2),
                        _ => (0, free),
                    };
                    if before > 0 {
                        spans.push(spaces(before));
                    }
                    spans.extend(cell);
                    // The last column's padding is left off, as trailing space.
                    if after > 0 && c + 1 < n {
                        spans.push(spaces(after));
                    }
                }
                let prefix = if line_no == 0 { first } else { rest };
                self.push(prefix, spans, (self.block, r, 1 + h));
                line_no += 1;
            }
            if r == 0 && rows.len() > 1 {
                let rule: Vec<String> = cols
                    .iter()
                    .enumerate()
                    .map(|(c, w)| "─".repeat(w + if c + 1 < n { 2 } else { 1 }))
                    .collect();
                self.push(
                    rest,
                    vec![Span::new(rule.join("┼"), border)],
                    (self.block, r, 1 + height),
                );
                line_no += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(doc: &Doc) -> Vec<String> {
        doc.lines.iter().map(Line::plain).collect()
    }

    #[test]
    fn headings_lose_their_hashes_and_keep_their_colours() {
        let doc = Markdown::parse("# Fix the login #12\n\n## Why\n\nBecause.", "").layout(80);
        let shown = lines(&doc);
        assert_eq!(shown[0], "", "a blank line first, as glow's block_prefix");
        assert_eq!(shown[1], "   Fix the login #12 ", "h1 padded on its bar");
        assert_eq!(shown[3], "  Why");
        assert_eq!(shown[5], "  Because.");
        let h1 = &doc.lines[1].spans[1];
        assert_eq!(
            (h1.style.fg, h1.style.bg, h1.style.bold),
            (Some(228), Some(63), true)
        );
        let h2 = &doc.lines[3].spans[1];
        assert_eq!(
            (h2.style.fg, h2.style.bold, h2.style.underline),
            (Some(39), true, true)
        );
        assert!(!shown.iter().any(|l| l.contains("##")));
    }

    #[test]
    fn a_link_is_its_text_alone_and_an_osc_8_hyperlink() {
        let doc = Markdown::parse("See [the ticket](https://linear.app/x/issue/AB-1) now.", "")
            .layout(80);
        let line = &doc.lines[1];
        assert_eq!(line.plain(), "  See the ticket now.");
        assert_eq!(doc.links, vec!["https://linear.app/x/issue/AB-1"]);
        let ansi = line.ansi(&doc.links, 0, 80, &[]);
        assert!(
            ansi.contains(
                "\x1b]8;;https://linear.app/x/issue/AB-1\x07\x1b[0;1;38;5;35mthe ticket\x1b]8;;\x07"
            ),
            "{ansi:?}"
        );
        assert_eq!(line.link_at(6), Some(0), "the text opens the link");
        assert_eq!(line.link_at(2), None);
    }

    #[test]
    fn a_bare_url_is_a_link_too_without_its_full_stop() {
        let doc = Markdown::parse("Read https://example.com/a.", "").layout(80);
        assert_eq!(doc.links, vec!["https://example.com/a"]);
        assert_eq!(doc.lines[1].plain(), "  Read https://example.com/a.");
    }

    #[test]
    fn paragraphs_wrap_to_the_width_and_lists_hang() {
        let text = "- one two three four five six seven eight nine ten eleven twelve\n- [x] done\n- [ ] not yet\n\n1. first\n2. second";
        let doc = Markdown::parse(text, "").layout(30);
        let shown = lines(&doc);
        assert_eq!(shown[1], "  • one two three four five");
        assert_eq!(shown[2], "    six seven eight nine ten");
        assert_eq!(shown[3], "    eleven twelve");
        assert_eq!(shown[4], "  [✓] done");
        assert_eq!(shown[5], "  [ ] not yet");
        assert_eq!(shown[7], "  1. first");
        assert_eq!(shown[8], "  2. second");
        assert!(shown.iter().all(|l| width(l) <= 30));
    }

    #[test]
    fn a_table_lines_its_columns_up() {
        let text = "| Name | Value |\n|------|------:|\n| a | 1 |\n| longer | 22 |";
        let doc = Markdown::parse(text, "").layout(80);
        let shown = lines(&doc);
        assert_eq!(shown[1], "   Name   │ Value");
        assert_eq!(shown[2], "  ────────┼──────");
        assert_eq!(shown[3], "   a      │     1");
        assert_eq!(shown[4], "   longer │    22");
        assert!(
            doc.lines[1]
                .spans
                .iter()
                .any(|s| s.text.contains("Name") && s.style.bold)
        );
    }

    #[test]
    fn a_code_block_keeps_its_lines_and_quotes_get_a_bar() {
        let text = "```rust\nfn main() {\n    let long = \"a line longer than the pane is wide, kept whole\";\n}\n```\n\n> quoted\n> words";
        let doc = Markdown::parse(text, "").layout(40);
        let shown = lines(&doc);
        assert_eq!(shown[1], "    fn main() {");
        assert!(
            shown[2].starts_with("        let long = \"a line longer"),
            "not wrapped: {:?}",
            shown[2]
        );
        assert!(width(&shown[2]) > 40);
        assert_eq!(shown[3], "    }");
        assert_eq!(doc.lines[1].spans.last().unwrap().style.fg, Some(244));
        assert_eq!(shown[5], "  │ quoted words");
    }

    #[test]
    fn mermaid_blocks_are_drawn_and_others_kept_as_source() {
        let text = "```mermaid\nflowchart TD\n  A[Start] --> B[End]\n```\n\n```mermaid\npie\n  \"a\": 1\n```";
        let doc = Markdown::parse(text, "M-r on GitHub").layout(80);
        let shown = lines(&doc).join("\n");
        assert!(shown.contains("│ Start │"), "{shown}");
        assert!(shown.contains("▼"), "{shown}");
        assert!(
            shown.contains("can't be drawn as text; M-r on GitHub shows it:"),
            "{shown}"
        );
        assert!(shown.contains("    pie"), "{shown}");
    }

    #[test]
    fn html_is_read_for_what_it_says() {
        let text = "<!-- describe your change -->\nIntro<br>next line with <b>bold</b>\n\n<img src=\"https://x/y.png\" alt=\"shot\">";
        let doc = Markdown::parse(text, "").layout(80);
        let shown = lines(&doc);
        assert!(!shown.iter().any(|l| l.contains("describe")), "{shown:?}");
        assert!(shown.iter().any(|l| l.trim() == "Intro"), "{shown:?}");
        assert!(
            shown.iter().any(|l| l.trim() == "next line with bold"),
            "{shown:?}"
        );
        assert!(
            shown.iter().any(|l| l.trim() == "Image: shot →"),
            "{shown:?}"
        );
        assert!(doc.links.contains(&"https://x/y.png".to_string()));
    }

    #[test]
    fn a_resize_finds_the_line_holding_what_was_at_the_top() {
        let words: Vec<String> = (0..120).map(|i| format!("word{i}")).collect();
        let text = format!("# Title\n\n{}\n\nThe end.", words.join(" "));
        let md = Markdown::parse(&text, "");
        let wide = md.layout(80);
        let top = 5;
        let first_word = wide.lines[top]
            .plain()
            .split_whitespace()
            .next()
            .unwrap()
            .to_string();
        let narrow = md.layout(40);
        let at = narrow.line_at(wide.lines[top].anchor);
        assert!(
            narrow.lines[at].plain().contains(&first_word),
            "{:?} holds {first_word}",
            narrow.lines[at].plain()
        );
        assert_eq!(narrow.line_at(narrow.lines[0].anchor), 0);
    }

    fn ready(keys: &[(&str, u32, (u32, u32))]) -> Images {
        Images {
            cell: (10, 20),
            rows: 30,
            ready: keys
                .iter()
                .map(|(k, id, px)| (k.to_string(), (*id, *px)))
                .collect(),
        }
    }

    #[test]
    fn pictures_alone_on_a_line_and_diagrams_are_wanted_once() {
        let text = "![a](https://x/a.png)\n\nSee ![b](https://x/b.png) here.\n\n- ![c](https://x/c.png)\n\n![a again](https://x/a.png)\n\n![local](a.png)\n\n```mermaid\npie\n  \"a\": 1\n```";
        let md = Markdown::parse(text, "");
        assert_eq!(
            md.wanted(),
            vec![
                Wanted::Picture("https://x/a.png".into()),
                Wanted::Picture("https://x/c.png".into()),
                Wanted::Diagram("pie\n  \"a\": 1".into()),
            ]
        );
    }

    #[test]
    fn a_picture_ready_is_rows_of_placeholders_and_one_not_ready_its_text() {
        let text = "Before\n\n![shot](https://x/a.png)\n![other](https://x/b.png)\n\nAfter";
        let md = Markdown::parse(text, "");
        // Where the view draws no pictures: the lines it always had.
        let plain = md.layout_with(80, None);
        assert!(
            lines(&plain)
                .iter()
                .any(|l| l.trim() == "Image: shot → Image: other →")
        );
        assert!(plain.placed.is_empty());
        // Where it does, nothing ready yet: a line each, anchored as their
        // pictures will be.
        let waiting = md.layout_with(80, Some(&ready(&[])));
        let other = waiting
            .lines
            .iter()
            .find(|l| l.plain().trim() == "Image: other →")
            .unwrap();
        assert_eq!(other.anchor.1, PICTURE_STRIDE);
        // 200 by 100 pixels in cells of 10 by 20, with its frame: 21 by 6.
        let doc = md.layout_with(80, Some(&ready(&[("https://x/a.png", 7, (200, 100))])));
        let rows: Vec<&Line> = doc.lines.iter().filter(|l| l.picture.is_some()).collect();
        assert_eq!(rows.len(), 6);
        let id = sized_id(7, 21, 6);
        assert_eq!(
            rows[0].picture,
            Some(PictureRow {
                id,
                row: 0,
                cols: 21,
                link: Some(0)
            })
        );
        // A click on the picture opens it, as one on its text line did.
        assert_eq!(rows[0].link_at(5), Some(0));
        assert_eq!(rows[0].link_at(30), None);
        assert_eq!(doc.links[0], "https://x/a.png");
        assert_eq!(rows[5].picture.unwrap().row, 5);
        assert_eq!(
            doc.placed,
            vec![Placement {
                key: "https://x/a.png".into(),
                id,
                cols: 21,
                rows: 6
            }]
        );
        assert!(lines(&doc).iter().any(|l| l.trim() == "Image: other →"));
        assert!(lines(&doc).iter().any(|l| l.trim() == "After"));
        // Drawn as cells in the id's color, each with its row and column.
        let ansi = rows[1].ansi(&doc.links, 0, 80, &[]);
        let color = format!("\x1b[38;2;{};{};{}m", id >> 16, (id >> 8) & 0xff, id & 0xff);
        assert!(
            ansi.contains(&format!("  \x1b[0m\x1b]8;;https://x/a.png\x07{color}")),
            "{ansi:?}"
        );
        assert!(ansi.contains(&crate::ui::images::placeholder(1, 20)));
        assert!(!ansi.contains(&crate::ui::images::placeholder(1, 21)));
        // Scrolled across, the cells start where the screen does.
        let across = rows[1].ansi(&doc.links, 12, 80, &[]);
        assert!(across.contains(&crate::ui::images::placeholder(1, 10)));
        assert!(!across.contains(&crate::ui::images::placeholder(1, 9)));
    }

    #[test]
    fn a_diagram_ready_is_a_picture_and_otherwise_its_text_drawing() {
        let source = "flowchart TD\n  A[Start] --> B[End]";
        let md = Markdown::parse(&format!("```mermaid\n{source}\n```"), "");
        let text = md.layout_with(80, Some(&ready(&[])));
        assert!(lines(&text).iter().any(|l| l.contains("Start")));
        let key = Wanted::Diagram(source.into()).key();
        let doc = md.layout_with(80, Some(&ready(&[(&key, 9, (400, 200))])));
        assert!(!lines(&doc).iter().any(|l| l.contains("Start")));
        assert_eq!(doc.placed.len(), 1);
        assert!(
            doc.lines
                .iter()
                .any(|l| l.picture.is_some_and(|p| p.id == doc.placed[0].id))
        );
    }

    #[test]
    fn the_same_picture_at_two_sizes_is_two_images() {
        let md = Markdown::parse("![a](https://x/a.png)\n\n> ![a](https://x/a.png)", "");
        let doc = md.layout_with(30, Some(&ready(&[("https://x/a.png", 7, (2000, 200))])));
        assert_eq!(doc.placed.len(), 2, "{:?}", doc.placed);
        assert_ne!(doc.placed[0].id, doc.placed[1].id);
        assert_ne!(doc.placed[0].cols, doc.placed[1].cols);
    }

    #[test]
    fn a_picture_fits_the_width_and_the_height_its_shape_kept() {
        // Its own size where it fits.
        assert_eq!(picture_cells((90, 30), (10, 20), 80, 30), (10, 2));
        // Made narrower to fit the width.
        assert_eq!(picture_cells((2000, 500), (10, 20), 50, 30), (50, 7));
        // And lower to fit the height, narrower with it.
        assert_eq!(picture_cells((500, 2000), (10, 20), 80, 10), (6, 10));
    }

    #[test]
    fn control_characters_never_reach_the_terminal() {
        let md = Markdown::parse("Hi \u{1b}_Ga=T,t=f;L2V0Yw==\u{1b}\\ there", "");
        let doc = md.layout(80);
        let all: String = doc
            .lines
            .iter()
            .map(|l| l.ansi(&doc.links, 0, 80, &[]))
            .collect();
        assert!(!all.contains("\x1b_G"), "{all:?}");
        let link = Markdown::parse("[x](https://e.com/\u{7}\u{1b}]8;;evil)", "").layout(80);
        let all: String = link
            .lines
            .iter()
            .map(|l| l.ansi(&link.links, 0, 80, &[]))
            .collect();
        assert!(!all.contains("\x07\x1b]8;;evil"), "{all:?}");
    }

    #[test]
    fn a_diagram_arriving_keeps_the_anchors_after_it() {
        let source = "pie\n  \"a\": 1";
        let md = Markdown::parse(&format!("```mermaid\n{source}\n```\n\nAfter it"), "");
        let anchor_of = |doc: &Doc| {
            doc.lines
                .iter()
                .find(|l| l.plain().contains("After it"))
                .map(|l| l.anchor)
        };
        let text = md.layout_with(80, Some(&ready(&[])));
        let key = Wanted::Diagram(source.into()).key();
        let drawn = md.layout_with(80, Some(&ready(&[(&key, 9, (400, 200))])));
        assert_eq!(anchor_of(&text), anchor_of(&drawn));
    }

    #[test]
    fn a_picture_and_a_diagram_are_found_by_their_words() {
        let source = "flowchart TD\n  A[Start] --> B[End]";
        let text = format!("![a shot](https://x/a.png)\n\n```mermaid\n{source}\n```");
        let md = Markdown::parse(&text, "");
        let key = Wanted::Diagram(source.into()).key();
        let doc = md.layout_with(
            80,
            Some(&ready(&[
                ("https://x/a.png", 7, (200, 100)),
                (&key, 9, (400, 200)),
            ])),
        );
        let found = |word: &str| {
            doc.lines
                .iter()
                .filter(|l| l.searchable().contains(word))
                .count()
        };
        assert_eq!(found("a shot"), 1);
        assert_eq!(found("Start"), 1);
    }

    #[test]
    fn a_place_in_a_diagrams_source_comes_back_as_its_pictures_first_row() {
        let source = "pie\n  \"a\": 1\n  \"b\": 2";
        let md = Markdown::parse(&format!("```mermaid\n{source}\n```\n\nAfter it"), "");
        let text = md.layout_with(80, Some(&ready(&[])));
        let in_source = text
            .lines
            .iter()
            .find(|l| l.plain().contains("\"b\": 2"))
            .unwrap()
            .anchor;
        let key = Wanted::Diagram(source.into()).key();
        let drawn = md.layout_with(80, Some(&ready(&[(&key, 9, (400, 200))])));
        let at = drawn.line_at(in_source);
        assert!(drawn.lines[at].picture.is_some_and(|p| p.row == 0), "{at}");
        let after = text
            .lines
            .iter()
            .find(|l| l.plain().contains("After it"))
            .unwrap()
            .anchor;
        assert!(
            drawn.lines[drawn.line_at(after)]
                .plain()
                .contains("After it")
        );
    }

    #[test]
    fn a_place_in_a_picture_stays_as_one_before_it_arrives() {
        let md = Markdown::parse("![a](https://x/a.png)\n![b](https://x/b.png)\n\nAfter", "");
        let b_only = md.layout_with(80, Some(&ready(&[("https://x/b.png", 8, (200, 100))])));
        let b_top = b_only
            .lines
            .iter()
            .find(|l| l.picture.is_some())
            .unwrap()
            .anchor;
        let both = md.layout_with(
            80,
            Some(&ready(&[
                ("https://x/a.png", 7, (200, 100)),
                ("https://x/b.png", 8, (200, 100)),
            ])),
        );
        let at = both.line_at(b_top);
        let b_id = sized_id(8, 21, 6);
        assert_eq!(
            both.lines[at].picture.map(|p| (p.id, p.row)),
            Some((b_id, 0))
        );
    }

    #[test]
    fn a_place_deep_in_a_text_drawing_comes_back_as_the_pictures_first_row() {
        let source = "flowchart TD\n  A --> B\n  B --> C\n  C --> D\n  D --> E";
        let md = Markdown::parse(&format!("```mermaid\n{source}\n```\n\nAfter it"), "");
        let text = md.layout_with(80, Some(&ready(&[])));
        let deep = text
            .lines
            .iter()
            .rfind(|l| l.plain().contains('E'))
            .unwrap()
            .anchor;
        let key = Wanted::Diagram(source.into()).key();
        let mut images = ready(&[(&key, 9, (400, 200))]);
        images.rows = 3;
        let drawn = md.layout_with(80, Some(&images));
        let at = drawn.line_at(deep);
        assert!(drawn.lines[at].picture.is_some_and(|p| p.row == 0), "{at}");
    }

    #[test]
    fn a_picture_waiting_wraps_its_text_line() {
        let alt = "a very long description of the screenshot that goes on";
        let md = Markdown::parse(&format!("![{alt}](https://x/a.png)"), "");
        let doc = md.layout_with(30, Some(&ready(&[])));
        assert!(
            doc.lines.iter().all(|l| l.width() <= 30),
            "{:?}",
            lines(&doc)
        );
        assert!(lines(&doc).iter().filter(|l| !l.trim().is_empty()).count() > 1);
    }
}
