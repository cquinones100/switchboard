//! Drawing a note the way Notes shows it, over its Markdown, ported from
//! notes/lua/notes/render.lua: the text stays plain Markdown, and only what
//! is shown changes.
//!
//! Paragraph styles, from each line's start:
//!
//! ```text
//!   # Title            ## Heading          ### Subheading
//!   * bulleted (•)     - dashed (–)        1. numbered
//!   - [ ] checklist (☐, ☑ once ticked, its text dimmed)
//!   > block quote (▎)  ```  monospaced, between fences
//!   | a | b |          a table (table.rs keeps it aligned)
//! ```
//!
//! and inside a line, outside `code`: **bold**, *italic*, <u>underline</u>,
//! ~~strikethrough~~, ==highlight==, [text](url) and bare https:// links,
//! [[Note title]] links to notes, ![name](path) images, and #tags.
//!
//! Markup is hidden, so it shows only on the line being typed in, the way
//! Notes shows formatting rather than symbols; a heading gets a ▾ to fold
//! its section by (fold.rs). Neovim drew these with extmarks: highlights by
//! priority, concealed ranges with a character in their place, and text
//! put in before a line. Each line's are worked out here as a `Look`, and
//! `cells` lays a line out as the cells it shows, each knowing the byte of
//! the line it came from, so the cursor and a click find their place.

use std::collections::HashMap;

use ratatui::style::{Color, Modifier, Style};
use unicode_segmentation::UnicodeSegmentation;

use super::table;
use crate::ui::draw::width;

/// What a line is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Title {
        marker: usize,
    },
    Heading {
        marker: usize,
    },
    Subheading {
        marker: usize,
    },
    Check {
        indent: usize,
        done: bool,
        marker: usize,
    },
    Bullet {
        indent: usize,
    },
    Dash {
        indent: usize,
    },
    Number {
        indent: usize,
        marker: usize,
    },
    Quote {
        indent: usize,
    },
    Table {
        indent: usize,
    },
    Fence,
    Code,
    Body,
    /// A note's ID (id.rs), which the editor keeps out of the text, here
    /// one pasted in before the save takes it out: hidden.
    Id,
}

impl Kind {
    pub fn name(&self) -> &'static str {
        match self {
            Kind::Title { .. } => "title",
            Kind::Heading { .. } => "heading",
            Kind::Subheading { .. } => "subheading",
            Kind::Check { .. } => "check",
            Kind::Bullet { .. } => "bullet",
            Kind::Dash { .. } => "dash",
            Kind::Number { .. } => "number",
            Kind::Quote { .. } => "quote",
            Kind::Table { .. } => "table",
            Kind::Fence => "fence",
            Kind::Code => "code",
            Kind::Body => "body",
            Kind::Id => "id",
        }
    }

    /// A heading's level for folding: a title 1, a heading 2, a
    /// subheading 3.
    pub fn level(&self) -> Option<usize> {
        match self {
            Kind::Title { .. } => Some(1),
            Kind::Heading { .. } => Some(2),
            Kind::Subheading { .. } => Some(3),
            _ => None,
        }
    }

    /// A heading's `#` marks with the space after them.
    pub fn marker(&self) -> usize {
        match self {
            Kind::Title { marker } | Kind::Heading { marker } | Kind::Subheading { marker } => {
                *marker
            }
            Kind::Check { marker, .. } | Kind::Number { marker, .. } => *marker,
            _ => 0,
        }
    }

    pub fn is_list(&self) -> bool {
        matches!(
            self,
            Kind::Check { .. } | Kind::Bullet { .. } | Kind::Dash { .. } | Kind::Number { .. }
        )
    }

    pub fn done(&self) -> bool {
        matches!(self, Kind::Check { done: true, .. })
    }
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

fn is_alnum(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

/// A line's indent in bytes.
pub fn indent_of(line: &str) -> usize {
    line.bytes().take_while(|b| is_space(*b)).count()
}

/// What a line is, read from how it starts; `first_text_line` says it is
/// the note's first line with text, which is its title, written as one or
/// not. Fences are told apart by the caller (`kinds`).
pub fn line_kind(line: &str, first_text_line: bool) -> Kind {
    if super::id::is_line(line) {
        return Kind::Id;
    }
    let indent = indent_of(line);
    let rest = &line[indent..];
    let b = rest.as_bytes();
    if rest.starts_with("```") {
        return Kind::Fence;
    }
    let hashes = b.iter().take_while(|c| **c == b'#').count();
    if hashes > 0 && b.len() > hashes && is_space(b[hashes]) && indent == 0 {
        let marker = hashes + 1;
        return if hashes == 1 || first_text_line {
            Kind::Title { marker }
        } else if hashes == 2 {
            Kind::Heading { marker }
        } else {
            Kind::Subheading { marker }
        };
    }
    // `- [ ] ` and its like: a list marker, spaces, a box, then a space.
    if matches!(b.first(), Some(b'-' | b'*' | b'+')) {
        let spaces = b[1..].iter().take_while(|c| is_space(**c)).count();
        let at = 1 + spaces;
        if spaces > 0
            && b.len() > at + 3
            && b[at] == b'['
            && matches!(b[at + 1], b' ' | b'x' | b'X')
            && b[at + 2] == b']'
            && is_space(b[at + 3])
        {
            return Kind::Check {
                indent,
                done: b[at + 1] != b' ',
                marker: at + 3,
            };
        }
    }
    if matches!(b.first(), Some(b'*' | b'+')) && b.get(1).is_some_and(|c| is_space(*c)) {
        return Kind::Bullet { indent };
    }
    if b.first() == Some(&b'-') && b.get(1).is_some_and(|c| is_space(*c)) {
        return Kind::Dash { indent };
    }
    let digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0
        && matches!(b.get(digits), Some(b'.' | b')'))
        && b.get(digits + 1).is_some_and(|c| is_space(*c))
    {
        return Kind::Number {
            indent,
            marker: digits + 1,
        };
    }
    if b.first() == Some(&b'>') {
        return Kind::Quote { indent };
    }
    if b.first() == Some(&b'|') {
        return Kind::Table { indent };
    }
    if first_text_line {
        return Kind::Title { marker: 0 };
    }
    Kind::Body
}

/// What every line is, tracking fences.
pub fn kinds<S: AsRef<str>>(lines: &[S]) -> Vec<Kind> {
    let mut out = Vec::with_capacity(lines.len());
    let mut in_code = false;
    let mut seen_text = false;
    for line in lines {
        let line = line.as_ref();
        let has_text = line.bytes().any(|b| !is_space(b)) && !super::id::is_line(line);
        let kind = if super::id::is_line(line) {
            // Hidden even inside a code block left open.
            Kind::Id
        } else if in_code {
            if line.trim_start().starts_with("```") {
                Kind::Fence
            } else {
                Kind::Code
            }
        } else {
            line_kind(line, !seen_text && has_text)
        };
        if kind == Kind::Fence {
            in_code = !in_code;
        }
        if has_text {
            seen_text = true;
        }
        out.push(kind);
    }
    out
}

// ── Inline styles ───────────────────────────────────────────────────────────

/// A highlight group of the plugin's, drawn in the theme's colours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hl {
    Title,
    Heading,
    Subheading,
    Bold,
    Italic,
    Underline,
    Strike,
    Highlight,
    Mono,
    MonoBlock,
    Fence,
    Quote,
    QuoteBar,
    Bullet,
    Box,
    BoxDone,
    Checked,
    CheckedStrike,
    Link,
    NoteLink,
    NoteLinkMissing,
    Tag,
    TableBorder,
    TableHeader,
    Fold,
    Search,
    Visual,
}

impl Hl {
    /// Its style: the foreground of a group every colorscheme has, and the
    /// plugin's own touches, as `render.highlights` set them. The title,
    /// subheadings and tags take the terminal's own blue, red and yellow,
    /// the ANSI colours ghostty.lua gave Title, Identifier and Type.
    pub fn style(self, light: bool) -> Style {
        let c = |dark: u8, lite: u8| Color::Indexed(if light { lite } else { dark });
        let s = Style::new();
        match self {
            Hl::Title => s
                .fg(Color::Indexed(4))
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            Hl::Heading => s.fg(c(117, 25)).add_modifier(Modifier::BOLD),
            Hl::Subheading => s.fg(Color::Indexed(1)).add_modifier(Modifier::BOLD),
            Hl::Bold => s.add_modifier(Modifier::BOLD),
            Hl::Italic => s.add_modifier(Modifier::ITALIC),
            Hl::Underline => s.add_modifier(Modifier::UNDERLINED),
            Hl::Strike => s.add_modifier(Modifier::CROSSED_OUT),
            Hl::Highlight => s
                .bg(if light {
                    Color::Rgb(0xf5, 0xe9, 0xb8)
                } else {
                    Color::Rgb(0x5c, 0x4f, 0x1d)
                })
                .fg(if light {
                    Color::Rgb(0x3a, 0x30, 0x0a)
                } else {
                    Color::Rgb(0xf5, 0xe9, 0xb8)
                }),
            Hl::Mono => s.fg(c(150, 28)),
            Hl::MonoBlock => s.bg(c(236, 254)),
            Hl::Fence => s.fg(c(243, 246)),
            Hl::Quote => s.fg(c(245, 242)).add_modifier(Modifier::ITALIC),
            Hl::QuoteBar | Hl::Box | Hl::Fold => s.fg(c(81, 31)),
            Hl::Bullet => s.fg(c(81, 31)).add_modifier(Modifier::BOLD),
            Hl::BoxDone => s.fg(c(114, 28)).add_modifier(Modifier::BOLD),
            Hl::Checked => s.fg(c(243, 246)),
            Hl::CheckedStrike => s.fg(c(243, 246)).add_modifier(Modifier::CROSSED_OUT),
            Hl::Link => s.fg(c(117, 25)).add_modifier(Modifier::UNDERLINED),
            Hl::NoteLink => s.fg(c(215, 130)).add_modifier(Modifier::UNDERLINED),
            Hl::NoteLinkMissing => s
                .fg(c(243, 246))
                .add_modifier(Modifier::UNDERLINED | Modifier::ITALIC),
            Hl::Tag => s.fg(Color::Indexed(3)).add_modifier(Modifier::BOLD),
            Hl::TableBorder => s.fg(c(243, 246)),
            Hl::TableHeader => s.add_modifier(Modifier::BOLD),
            Hl::Search => s.bg(c(58, 186)),
            Hl::Visual => s.bg(c(239, 252)),
        }
    }
}

/// An inline style's span, with 0-based byte columns, `e` exclusive, and
/// the markup to hide around it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub s: usize,
    pub e: usize,
    pub hl: Hl,
    pub conceal: Vec<(usize, usize)>,
    pub note: Option<String>,
    pub url: Option<String>,
    pub image: Option<String>,
    pub tag: Option<String>,
}

impl Span {
    fn new(s: usize, e: usize, hl: Hl, conceal: Vec<(usize, usize)>) -> Span {
        Span {
            s,
            e,
            hl,
            conceal,
            note: None,
            url: None,
            image: None,
            tag: None,
        }
    }

    /// Where it starts and ends with its markup.
    pub fn whole(&self) -> (usize, usize) {
        let from = self.conceal.first().map_or(self.s, |c| c.0).min(self.s);
        let to = self.conceal.last().map_or(self.e, |c| c.1).max(self.e);
        (from, to)
    }
}

/// The bytes a bare link's address is made of, as render.lua's class.
fn url_byte(b: u8) -> bool {
    is_alnum(b) || b"-._~:/?#[]@!$&'()*+,;=%".contains(&b)
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from > hay.len() || needle.is_empty() {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| i + from)
}

/// The spans of inline styles in a line. Each construct found blanks its
/// markers in a copy of the line, so a later one can't match them: `code`
/// first (its insides are never styled), then links, then the styles, which
/// may nest. The search starts at `from`, past a line's marker.
pub fn inline(line: &str, from: usize) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut m: Vec<u8> = line.as_bytes().to_vec();
    let n = m.len();
    fn blank(m: &mut [u8], s: usize, e: usize) {
        let end = e.min(m.len());
        for b in &mut m[s..end] {
            *b = 1;
        }
    }
    // Each matcher gives the end of a match starting at `s`, if one does.
    // A match turned down by the caller searches on from the next byte.
    let mut at = from;
    // `code`
    while at < n {
        let found = (at..n).find_map(|s| {
            (m[s] == b'`' && s + 1 < n && m[s + 1] != b'`')
                .then(|| find(&m, b"`", s + 1).map(|close| (s, close + 1)))
                .flatten()
        });
        let Some((s, x)) = found else { break };
        spans.push(Span::new(
            s + 1,
            x - 1,
            Hl::Mono,
            vec![(s, s + 1), (x - 1, x)],
        ));
        blank(&mut m, s, x);
        at = x;
    }
    // [[Note link]]
    at = from;
    while at < n {
        let found = (at..n).find_map(|s| {
            if !m[s..].starts_with(b"[[") || s + 2 >= n || m[s + 2] == b']' {
                return None;
            }
            let close = (s + 2..n).find(|&i| m[i] == b']')?;
            m[close..].starts_with(b"]]").then_some((s, close + 2))
        });
        let Some((s, x)) = found else { break };
        let mut span = Span::new(s + 2, x - 2, Hl::NoteLink, vec![(s, s + 2), (x - 2, x)]);
        span.note = Some(line[s + 2..x - 2].to_string());
        spans.push(span);
        blank(&mut m, s, x);
        at = x;
    }
    // ![name](path), an image (images.rs): its name styled as a link, the
    // rest hidden; the picture is drawn under it.
    at = from;
    while at < n {
        let found = (at..n).find_map(|s| {
            if !m[s..].starts_with(b"![") {
                return None;
            }
            let close = (s + 2..n).find(|&i| m[i] == b']')?;
            if m.get(close + 1) != Some(&b'(') {
                return None;
            }
            let path_end = (close + 2..n)
                .find(|&i| m[i] == b')' || is_space(m[i]))
                .filter(|&i| m[i] == b')' && i > close + 2)?;
            Some((s, close, path_end + 1))
        });
        let Some((s, close, x)) = found else { break };
        let mut span = Span::new(s + 2, close, Hl::Link, vec![(s, s + 2), (close, x)]);
        span.image = Some(line[close + 2..x - 1].to_string());
        spans.push(span);
        blank(&mut m, s, s + 2);
        blank(&mut m, close, x);
        at = x;
    }
    // [text](url)
    at = from;
    while at < n {
        let found = (at..n).find_map(|s| {
            if m[s] != b'[' || s + 1 >= n || m[s + 1] == b']' {
                return None;
            }
            let close = (s + 1..n).find(|&i| m[i] == b']')?;
            if m.get(close + 1) != Some(&b'(') {
                return None;
            }
            let url_end = (close + 2..n)
                .find(|&i| m[i] == b')' || is_space(m[i]))
                .filter(|&i| m[i] == b')' && i > close + 2)?;
            Some((s, close, url_end + 1))
        });
        let Some((s, close, x)) = found else { break };
        let mut span = Span::new(s + 1, close, Hl::Link, vec![(s, s + 1), (close, x)]);
        span.url = Some(line[close + 2..x - 1].to_string());
        spans.push(span);
        blank(&mut m, s, s + 1);
        blank(&mut m, close, x);
        at = x;
    }
    // Bare links.
    at = from;
    while at < n {
        let found = (at..n).find_map(|s| {
            let scheme = if m[s..].starts_with(b"https://") {
                8
            } else if m[s..].starts_with(b"http://") {
                7
            } else {
                return None;
            };
            let end = (s + scheme..n).find(|&i| !url_byte(m[i])).unwrap_or(n);
            (end > s + scheme).then_some((s, end))
        });
        let Some((s, x)) = found else { break };
        let mut end = x;
        while end > s && b".,;:!?)".contains(&m[end - 1]) {
            end -= 1;
        }
        let mut span = Span::new(s, end, Hl::Link, Vec::new());
        span.url = Some(line[s..end].to_string());
        spans.push(span);
        blank(&mut m, s, end);
        at = x.max(s + 1);
    }
    // Paired markers: the text between gets the style, the markers hide.
    let paired = |m: &mut Vec<u8>,
                  spans: &mut Vec<Span>,
                  open: &[u8],
                  close: &[u8],
                  hl: Hl,
                  first: &dyn Fn(u8) -> bool,
                  lazy_any: bool| {
        let mut at = from;
        while at < n {
            let found = (at..n).find_map(|s| {
                if !m[s..].starts_with(open) {
                    return None;
                }
                let inner = s + open.len();
                let c = *m.get(inner)?;
                if !first(c) {
                    return None;
                }
                if lazy_any {
                    find(m, close, inner + 1).map(|i| (s, i + close.len()))
                } else {
                    // `[^c]-c`: the first closing byte after the first.
                    (inner + 1..n)
                        .find(|&i| m[i] == close[0])
                        .map(|i| (s, i + 1))
                }
            });
            let Some((s, x)) = found else { break };
            spans.push(Span::new(
                s + open.len(),
                x - close.len(),
                hl,
                vec![(s, s + open.len()), (x - close.len(), x)],
            ));
            blank(m, s, s + open.len());
            blank(m, x - close.len(), x);
            at = x;
        }
    };
    // <u>.-</u>: anything, even nothing, between.
    {
        let mut at2 = from;
        while at2 < n {
            let found = (at2..n).find_map(|s| {
                m[s..]
                    .starts_with(b"<u>")
                    .then(|| find(&m, b"</u>", s + 3).map(|i| (s, i + 4)))
                    .flatten()
            });
            let Some((s, x)) = found else { break };
            spans.push(Span::new(
                s + 3,
                x - 4,
                Hl::Underline,
                vec![(s, s + 3), (x - 4, x)],
            ));
            blank(&mut m, s, s + 3);
            blank(&mut m, x - 4, x);
            at2 = x;
        }
    }
    paired(
        &mut m,
        &mut spans,
        b"~~",
        b"~~",
        Hl::Strike,
        &|c| c != b'~',
        true,
    );
    paired(
        &mut m,
        &mut spans,
        b"==",
        b"==",
        Hl::Highlight,
        &|c| c != b'=',
        true,
    );
    paired(
        &mut m,
        &mut spans,
        b"**",
        b"**",
        Hl::Bold,
        &|c| c != b'*' && !is_space(c),
        true,
    );
    paired(
        &mut m,
        &mut spans,
        b"__",
        b"__",
        Hl::Bold,
        &|c| c != b'_' && !is_space(c),
        true,
    );
    paired(
        &mut m,
        &mut spans,
        b"*",
        b"*",
        Hl::Italic,
        &|c| c != b'*' && !is_space(c),
        false,
    );
    // _italic_ only at word boundaries, so snake_case stays as it is.
    at = from;
    while at < n {
        let found = (at..n).find_map(|s| {
            if m[s] != b'_' || !m.get(s + 1).is_some_and(|c| *c != b'_' && !is_space(*c)) {
                return None;
            }
            let close = (s + 2..n).find(|&i| m[i] == b'_')?;
            let before = s.checked_sub(1).map(|i| m[i]);
            let after = m.get(close + 1).copied();
            if before.is_some_and(is_alnum) || after.is_some_and(is_alnum) {
                return None;
            }
            Some((s, close + 1))
        });
        let Some((s, x)) = found else { break };
        spans.push(Span::new(
            s + 1,
            x - 1,
            Hl::Italic,
            vec![(s, s + 1), (x - 1, x)],
        ));
        blank(&mut m, s, s + 1);
        blank(&mut m, x - 1, x);
        at = x;
    }
    // #tags, as the store finds them.
    at = from;
    while at < n {
        let found = (at..n).find_map(|s| {
            if m[s] != b'#' || !m.get(s + 1).is_some_and(|c| is_alnum(*c)) {
                return None;
            }
            if s > 0 && !is_space(m[s - 1]) {
                return None;
            }
            let end = (s + 2..n)
                .find(|&i| !(is_alnum(m[i]) || matches!(m[i], b'_' | b'-' | b'/')))
                .unwrap_or(n);
            Some((s, end))
        });
        let Some((s, x)) = found else { break };
        let mut span = Span::new(s, x, Hl::Tag, Vec::new());
        span.tag = Some(line[s + 1..x].to_string());
        spans.push(span);
        at = x;
    }
    spans
}

/// The inline span covering a byte of a line, markup included, if any: a
/// link, a note link, an image or a tag.
pub fn span_at(line: &str, col: usize) -> Option<Span> {
    inline(line, 0).into_iter().find(|span| {
        let (from, to) = span.whole();
        col >= from && col < to
    })
}

// ── A line's look ───────────────────────────────────────────────────────────

/// What a line shows besides its text: highlights by priority, ranges
/// hidden or shown as one character, text before it, and a background for
/// the whole row.
#[derive(Clone, Debug, Default)]
pub struct Look {
    /// (from, to, group, priority), byte columns.
    pub hls: Vec<(usize, usize, Hl, u16)>,
    /// (from, to, shown in its place), the character drawn in the first
    /// range's place in the group of the highlight over it.
    pub conceal: Vec<(usize, usize, Option<&'static str>)>,
    /// Text drawn before the line (a heading's ▾).
    pub before: Option<(&'static str, Hl)>,
    pub row_bg: Option<Hl>,
}

impl Look {
    fn hl(&mut self, s: usize, e: usize, hl: Hl, priority: u16) {
        if e > s {
            self.hls.push((s, e, hl, priority));
        }
    }

    fn conceal(&mut self, s: usize, e: usize, with: Option<&'static str>) {
        if e > s {
            self.conceal.push((s, e, with));
        }
    }

    /// Whether a group covers any of the line.
    pub fn has(&self, hl: Hl) -> bool {
        self.hls.iter().any(|h| h.2 == hl)
    }
}

/// The options drawing follows: a ticked item, and what is indented under
/// it, struck through as well as dimmed (strike_checked), and whether a
/// [[link]] names a note there is, for drawing one that doesn't as missing.
pub struct Drawing<'a> {
    pub strike_checked: bool,
    pub note_exists: &'a dyn Fn(&str) -> bool,
}

/// Every line's look.
pub fn looks<S: AsRef<str>>(lines: &[S], kinds: &[Kind], d: &Drawing<'_>) -> Vec<Look> {
    let mut out = Vec::with_capacity(lines.len());
    // A ticked item's indent while the lines under it are drawn struck
    // through with it.
    let mut ticked: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        let line = line.as_ref();
        let k = kinds[i];
        let mut look = Look::default();
        let len = line.len();
        let own = indent_of(line);
        let blank = line.bytes().all(is_space);
        if ticked.is_some_and(|t| blank || own <= t) {
            ticked = None;
        }
        if ticked.is_some() && d.strike_checked {
            look.hl(own, len, Hl::CheckedStrike, 120);
        }
        if let Kind::Check {
            indent, done: true, ..
        } = k
            && ticked.is_none()
        {
            ticked = Some(indent);
        }
        let mut text_from = 0;
        match k {
            Kind::Fence => look.hl(0, len, Hl::Fence, 150),
            Kind::Code => {
                look.row_bg = Some(Hl::MonoBlock);
                look.hl(0, len, Hl::Mono, 150);
            }
            Kind::Title { marker } | Kind::Heading { marker } | Kind::Subheading { marker } => {
                let group = match k {
                    Kind::Title { .. } => Hl::Title,
                    Kind::Heading { .. } => Hl::Heading,
                    _ => Hl::Subheading,
                };
                look.conceal(0, marker, None);
                look.before = Some(("▾ ", Hl::Fold));
                look.hl(marker, len, group, 150);
                text_from = marker;
            }
            Kind::Check {
                indent,
                done,
                marker,
            } => {
                look.conceal(indent, indent + marker, Some(if done { "☑" } else { "☐" }));
                look.hl(
                    indent,
                    indent + marker,
                    if done { Hl::BoxDone } else { Hl::Box },
                    210,
                );
                if done {
                    look.hl(
                        indent + marker,
                        len,
                        if d.strike_checked {
                            Hl::CheckedStrike
                        } else {
                            Hl::Checked
                        },
                        120,
                    );
                }
                text_from = indent + marker;
            }
            Kind::Bullet { indent } | Kind::Dash { indent } => {
                let mark = if matches!(k, Kind::Bullet { .. }) {
                    "•"
                } else {
                    "–"
                };
                look.conceal(indent, indent + 1, Some(mark));
                look.hl(indent, indent + 1, Hl::Bullet, 210);
                text_from = indent + 2;
            }
            Kind::Number { indent, marker } => {
                look.hl(indent, indent + marker, Hl::Bullet, 150);
                text_from = indent + marker;
            }
            Kind::Quote { .. } => {
                let gt = line.find('>').unwrap_or(0);
                look.conceal(gt, gt + 1, Some("▎"));
                look.hl(gt, gt + 1, Hl::QuoteBar, 210);
                look.hl(gt + 1, len, Hl::Quote, 120);
                text_from = gt + 1;
            }
            Kind::Table { .. } => {
                let header = i == 0 || !matches!(kinds[i - 1], Kind::Table { .. });
                table::draw(line, header, &mut look);
            }
            Kind::Body => {}
            Kind::Id => look.conceal(0, len, None),
        }
        if !matches!(k, Kind::Code | Kind::Fence | Kind::Id) {
            for span in inline(line, text_from.min(len)) {
                let group = match &span.note {
                    Some(title) if !(d.note_exists)(title) => Hl::NoteLinkMissing,
                    _ => span.hl,
                };
                look.hl(span.s, span.e, group, 160);
                for (s, e) in &span.conceal {
                    look.conceal(*s, *e, None);
                }
            }
        }
        out.push(look);
    }
    out
}

// ── UX 2.0's chips ──────────────────────────────────────────────────────────

/// What a chip at the right of a checklist item says, with UX 2.0 and
/// Settings' "Chips on checklist items" on, as board 7 draws them: an
/// unticked item has a plan (an indented `Plan: [[...]]` line under it), or
/// waits on a choice ("needs a choice" in the item or that line); a ticked
/// one landed, its commit named on a line under it. Read from the note's
/// text alone, so drawing starts nothing.
///
/// The tab's needs the workspace's state, which the app's background look
/// hands it (`ItemTab`, kept in memory by app.rs): a tab working on the
/// item, with its phase, or a tab waiting on you, with what it waits on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chip {
    /// A tab working on the item: its number, its phase, if any, and its
    /// color, which its number is drawn in.
    Working(usize, String, Option<Color>),
    /// A tab waiting on you: its number, what it waits on, if known, and
    /// its color.
    Waits(usize, String, Option<Color>),
    Choice,
    Plan,
    /// A ticked item's commit, its short hash.
    Landed(String),
}

/// A tab on a note's item, as the app's background look last found it in
/// the server's `list`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemTab {
    /// The tab's id (`@<n>`), which `o` on the item goes to.
    pub id: String,
    /// The note the tab is linked to (`@tab_note`), empty when none.
    pub note: String,
    /// The ID of the note its item is in, from its branch's `wtnItem`,
    /// empty when unknown.
    pub note_id: String,
    /// The tab's number in its session.
    pub index: usize,
    /// Its Claude waits on you, else it works.
    pub waits: bool,
    /// The loop's phase while working, what it waits on while waiting.
    pub detail: String,
    /// The tab's color, which its number on the chip is drawn in.
    pub color: Option<Color>,
}

impl Chip {
    pub fn text(&self) -> String {
        let with = |head: String, detail: &str| {
            if detail.is_empty() {
                head
            } else {
                format!("{head} · {detail}")
            }
        };
        match self {
            Chip::Working(n, phase, _) => with(format!("⠹ tab {n}"), phase),
            Chip::Waits(n, wait, _) => with(format!("● tab {n} waits on you"), wait),
            Chip::Choice => "needs a choice".into(),
            Chip::Plan => "plan".into(),
            Chip::Landed(hash) => format!("landed {hash}"),
        }
    }

    /// The chip as drawn, a space either side of its text, in runs of
    /// text and style: a tab's number in the tab's color (as words on the
    /// light theme), the rest in `style`; lit under the mouse.
    pub fn runs(&self, lit: bool, light: bool) -> Vec<(String, Style)> {
        let style = if lit {
            Style::new().fg(Color::Indexed(16)).bg(Color::Indexed(81))
        } else {
            self.style(light)
        };
        let (number, color) = match self {
            Chip::Working(n, _, color) | Chip::Waits(n, _, color) => {
                (Some(format!("tab {n}")), *color)
            }
            _ => (None, None),
        };
        let text = format!(" {} ", self.text());
        match (number, color.filter(|_| !lit)) {
            (Some(number), Some(color)) => {
                let at = text.find(&number).unwrap_or(0);
                let end = at + number.len();
                let tinted = style
                    .fg(crate::tabs::color::as_words(color, light))
                    .add_modifier(Modifier::BOLD);
                vec![
                    (text[..at].to_string(), style),
                    (number, tinted),
                    (text[end..].to_string(), style),
                ]
            }
            _ => vec![(text, style)],
        }
    }

    /// Its width on the screen as `runs` draws it.
    pub fn width(&self) -> usize {
        width(&self.text()) + 2
    }

    /// A run of background color: the choice and a waiting tab yellow, a
    /// working tab blue, the plan dim.
    pub fn style(&self, light: bool) -> Style {
        match self {
            Chip::Working(..) => Style::new().fg(Color::Indexed(4)).bg(if light {
                Color::Rgb(0xd8, 0xe6, 0xf8)
            } else {
                Color::Rgb(0x1e, 0x2a, 0x3c)
            }),
            Chip::Choice | Chip::Waits(..) => Style::new().fg(Color::Indexed(3)).bg(if light {
                Color::Rgb(0xf5, 0xe9, 0xb8)
            } else {
                Color::Rgb(0x3a, 0x32, 0x20)
            }),
            Chip::Plan => Style::new()
                .fg(Color::Indexed(if light { 242 } else { 245 }))
                .bg(Color::Indexed(if light { 253 } else { 237 })),
            Chip::Landed(_) => landed_style(light),
        }
    }
}

/// A checklist item's first line, its marker taken off: what wtn records
/// as the item a branch is for, and a tab's `@tab_title`.
pub fn item_text(line: &str, kind: Kind) -> &str {
    match kind {
        Kind::Check { indent, marker, .. } => line.get(indent + marker..).unwrap_or("").trim(),
        _ => "",
    }
}

/// The "landed <hash>" chip's run of background color: green.
pub fn landed_style(light: bool) -> Style {
    Style::new().fg(Color::Indexed(2)).bg(if light {
        Color::Rgb(0xd6, 0xf0, 0xd6)
    } else {
        Color::Rgb(0x1e, 0x33, 0x22)
    })
}

/// Each line's "landed <hash>" chip: a ticked item's, the first commit
/// hash its indented lines name (a word of 7 to 40 hex digits holding a
/// digit and a letter, as git's short hashes are); none for any other line.
pub fn landed<S: AsRef<str>>(lines: &[S], kinds: &[Kind]) -> Vec<Option<String>> {
    let is_hash = |w: &str| {
        (7..=40).contains(&w.len())
            && w.bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            && w.bytes().any(|c| c.is_ascii_digit())
            && w.bytes().any(|c| c.is_ascii_alphabetic())
    };
    let mut out = vec![None; lines.len()];
    for (i, k) in kinds.iter().enumerate() {
        let Kind::Check {
            indent, done: true, ..
        } = *k
        else {
            continue;
        };
        for (line, k) in lines.iter().zip(kinds).skip(i + 1) {
            let line = line.as_ref();
            if line.bytes().all(is_space)
                || indent_of(line) <= indent
                || matches!(k, Kind::Check { .. })
            {
                break;
            }
            let hash = line
                .split(|c: char| !c.is_ascii_alphanumeric())
                .find(|w| is_hash(w));
            if let Some(h) = hash {
                out[i] = Some(h.chars().take(7).collect());
                break;
            }
        }
    }
    out
}

/// Each line's chips: an unticked item's, from it and the lines indented
/// under it up to the next item, a tab working on it or waiting on you
/// first (`tabs`, the note's own tabs keyed by the item's first line); none
/// for any other line.
pub fn chips<S: AsRef<str>>(
    lines: &[S],
    kinds: &[Kind],
    tabs: &HashMap<String, ItemTab>,
) -> Vec<Vec<Chip>> {
    let mut out = vec![Vec::new(); lines.len()];
    for (i, k) in kinds.iter().enumerate() {
        let Kind::Check {
            indent,
            done: false,
            ..
        } = *k
        else {
            continue;
        };
        if let Some(tab) = tabs.get(item_text(lines[i].as_ref(), *k)) {
            out[i].push(if tab.waits {
                Chip::Waits(tab.index, tab.detail.clone(), tab.color)
            } else {
                Chip::Working(tab.index, tab.detail.clone(), tab.color)
            });
        }
        let mut plan = false;
        let mut choice = lines[i].as_ref().to_lowercase().contains("needs a choice");
        for (line, k) in lines.iter().zip(kinds).skip(i + 1) {
            let line = line.as_ref();
            if line.bytes().all(is_space)
                || indent_of(line) <= indent
                || matches!(k, Kind::Check { .. })
            {
                break;
            }
            let text = line.trim_start();
            let text = text.strip_prefix("- ").unwrap_or(text);
            if text.starts_with("Plan:") && text.contains("[[") {
                plan = true;
                choice |= text.to_lowercase().contains("needs a choice");
            }
        }
        if choice {
            out[i].push(Chip::Choice);
        }
        if plan {
            out[i].push(Chip::Plan);
        }
    }
    out
}

/// The end (exclusive) of the item on `row` at `indent`: the lines
/// indented under it, blank lines between included, trailing blank lines
/// left off, as wtn reads an item whole (`worktree_from_note::item_block`).
fn item_end<S: AsRef<str>>(lines: &[S], row: usize, indent: usize) -> usize {
    let mut end = row + 1;
    let mut last = row + 1;
    while end < lines.len() {
        let line = lines[end].as_ref();
        if line.bytes().all(is_space) {
            end += 1;
            continue;
        }
        if indent_of(line) <= indent {
            break;
        }
        end += 1;
        last = end;
    }
    last
}

/// The note an unticked item's plan names: the `[[...]]` on an indented
/// `Plan:` line under it, blank lines between included, past its
/// sub-items; `None` for any other line, or an item with none.
pub fn plan_of<S: AsRef<str>>(lines: &[S], kinds: &[Kind], row: usize) -> Option<String> {
    let Some(Kind::Check {
        indent,
        done: false,
        ..
    }) = kinds.get(row).copied()
    else {
        return None;
    };
    let end = item_end(lines, row, indent);
    // A sub-item and what is under it are passed over: a plan there is
    // the sub-item's.
    let mut sub: Option<usize> = None;
    for (line, k) in lines.iter().zip(kinds).take(end).skip(row + 1) {
        let line = line.as_ref();
        if line.bytes().all(is_space) {
            continue;
        }
        if sub.is_some_and(|s| indent_of(line) > s) {
            continue;
        }
        sub = None;
        if let Kind::Check { indent, .. } = k {
            sub = Some(*indent);
            continue;
        }
        let text = line.trim_start();
        let text = text.strip_prefix("- ").unwrap_or(text);
        if let Some(rest) = text.strip_prefix("Plan:")
            && let Some((_, after)) = rest.split_once("[[")
            && let Some((title, _)) = after.split_once("]]")
            && !title.trim().is_empty()
        {
            return Some(title.trim().to_string());
        }
    }
    None
}

/// Whether a heading under the title names parked work, which UX 2.0
/// shows folded to one line with its count of items: the sections wtn and
/// autopilot leave out (`worktree_from_note::parked_heading`: Defered,
/// Later, Backlog...).
pub fn parked(line: &str, kind: Kind) -> bool {
    matches!(kind, Kind::Heading { .. } | Kind::Subheading { .. })
        && crate::cmd::worktree_from_note::parked_heading(line)
}

/// The checklist items at the top level of the rows `start..=end`.
pub fn items_in(kinds: &[Kind], start: usize, end: usize) -> usize {
    kinds
        .iter()
        .take(end + 1)
        .skip(start)
        .filter(|k| matches!(k, Kind::Check { indent: 0, .. }))
        .count()
}

/// Where "Add an item" puts a new one: in the note's top list (the rows
/// before its first `## ` or `### ` section), after its last unticked top-level item
/// and what is indented under it; with none, after the title (and a blank
/// line under it).
pub fn new_item_row<S: AsRef<str>>(lines: &[S], kinds: &[Kind]) -> usize {
    let top_end = kinds
        .iter()
        .position(|k| matches!(k, Kind::Heading { .. } | Kind::Subheading { .. }))
        .unwrap_or(kinds.len());
    let last = (0..top_end).rev().find(|&r| {
        matches!(
            kinds[r],
            Kind::Check {
                indent: 0,
                done: false,
                ..
            }
        )
    });
    match last {
        Some(r) => item_end(&lines[..top_end], r, 0),
        None => {
            let title = kinds
                .iter()
                .position(|k| matches!(k, Kind::Title { .. }))
                .map_or(0, |t| t + 1);
            let mut at = title.min(top_end);
            if at < top_end && lines[at].as_ref().bytes().all(is_space) {
                at += 1;
            }
            at
        }
    }
}

/// Pushes a conceal for table.rs.
pub fn conceal_with(
    look: &mut Look,
    s: usize,
    e: usize,
    with: &'static str,
    hl: Hl,
    priority: u16,
) {
    look.conceal(s, e, Some(with));
    look.hl(s, e, hl, priority);
}

/// Pushes a highlight for table.rs.
pub fn highlight(look: &mut Look, s: usize, e: usize, hl: Hl, priority: u16) {
    look.hl(s, e, hl, priority);
}

/// One cell a line shows: its text (a grapheme, or what stands for markup
/// hidden), its style, and the byte of the line it came from; `None` for
/// text put before the line.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub text: String,
    pub style: Style,
    pub col: Option<usize>,
    /// The bytes of the line it stands for.
    pub len: usize,
    pub width: usize,
}

/// A line laid out as the cells it shows. `raw` shows its markup, as the
/// line being typed in shows it; `extra` adds highlights (the search's
/// matches, a selection) over the look's.
pub fn cells(
    line: &str,
    look: &Look,
    raw: bool,
    extra: &[(usize, usize, Hl)],
    light: bool,
) -> Vec<Cell> {
    let mut hls: Vec<(usize, usize, Hl, u16)> = look.hls.clone();
    hls.extend(extra.iter().map(|&(s, e, h)| (s, e, h, 300)));
    hls.sort_by_key(|h| h.3);
    let style_at = |b: usize| {
        let mut style = Style::new();
        for (s, e, hl, _) in &hls {
            if b >= *s && b < *e {
                style = style.patch(hl.style(light));
            }
        }
        style
    };
    let mut out = Vec::new();
    if let Some((text, hl)) = look.before {
        for g in text.graphemes(true) {
            out.push(Cell {
                text: g.to_string(),
                style: hl.style(light),
                col: None,
                len: 0,
                width: width(g),
            });
        }
    }
    // Past the end of a range shown as one character.
    let mut skip_to = 0;
    for (at, g) in line.grapheme_indices(true) {
        if at < skip_to {
            continue;
        }
        if !raw
            && let Some(&(s, e, with)) = look
                .conceal
                .iter()
                .filter(|(s, e, _)| at >= *s && at < *e)
                .max_by_key(|(s, _, w)| (w.is_some(), *s))
        {
            // A range with a character in its place shows it once, at its
            // start, for the whole range; one without shows nothing.
            if let Some(with) = with
                && at == s
            {
                out.push(Cell {
                    text: with.to_string(),
                    style: style_at(s),
                    col: Some(s),
                    len: e - s,
                    width: width(with),
                });
                skip_to = e;
            }
            continue;
        }
        let style = style_at(at);
        if g == "\t" {
            // A tab as the two spaces `tabstop` gives it.
            out.push(Cell {
                text: "  ".into(),
                style,
                col: Some(at),
                len: 1,
                width: 2,
            });
        } else if g.chars().all(char::is_control) {
            // Any other control character as ^M and its like.
            let c = g.chars().next().unwrap_or('?') as u32;
            let shown = format!("^{}", char::from_u32((c + 64) % 128).unwrap_or('?'));
            out.push(Cell {
                text: shown,
                style: style.patch(Hl::Fence.style(light)),
                col: Some(at),
                len: g.len(),
                width: 2,
            });
        } else {
            out.push(Cell {
                text: g.to_string(),
                style,
                col: Some(at),
                len: g.len(),
                width: width(g).max(1),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_lines_style_is_read_from_how_it_starts_and_the_first_line_is_the_title() {
        let lines = [
            "Shopping",
            "",
            "## Today",
            "### Later",
            "- [ ] eggs",
            "  - [x] milk",
            "* one",
            "- two",
            "1. three",
            "> quote",
            "```",
            "# not a heading",
            "```",
            "| a | b |",
            "plain",
        ];
        let ks = kinds(&lines);
        let names: Vec<&str> = ks.iter().map(Kind::name).collect();
        assert_eq!(
            names,
            [
                "title",
                "body",
                "heading",
                "subheading",
                "check",
                "check",
                "bullet",
                "dash",
                "number",
                "quote",
                "fence",
                "code",
                "fence",
                "table",
                "body"
            ]
        );
        assert!(!ks[4].done());
        assert_eq!(
            ks[5],
            Kind::Check {
                indent: 2,
                done: true,
                marker: 5
            },
            "a ticked item, indented"
        );
    }

    #[test]
    fn inline_styles_are_found_with_their_markup_and_code_and_snake_case_left_alone() {
        let line = "**b** *i* <u>u</u> ~~s~~ ==h== `**c**` [[Trip]] [site](https://x.co) https://a.b/c. #tag snake_case _it_ a#b";
        let spans = inline(line, 0);
        let find = |hl: Hl, text: &str| {
            spans
                .iter()
                .find(|s| s.hl == hl && &line[s.s..s.e] == text)
                .unwrap_or_else(|| panic!("{hl:?} {text}"))
        };
        let hidden = |s: &Span| {
            s.conceal
                .iter()
                .map(|(a, b)| &line[*a..*b])
                .collect::<Vec<_>>()
        };
        assert_eq!(hidden(find(Hl::Bold, "b")), ["**", "**"]);
        assert_eq!(hidden(find(Hl::Italic, "i")), ["*", "*"]);
        assert_eq!(hidden(find(Hl::Underline, "u")), ["<u>", "</u>"]);
        assert_eq!(hidden(find(Hl::Strike, "s")), ["~~", "~~"]);
        assert_eq!(hidden(find(Hl::Highlight, "h")), ["==", "=="]);
        assert_eq!(
            hidden(find(Hl::Mono, "**c**")),
            ["`", "`"],
            "nothing inside code is styled"
        );
        assert!(
            !spans
                .iter()
                .any(|s| s.hl == Hl::Bold && &line[s.s..s.e] == "c")
        );
        assert_eq!(find(Hl::NoteLink, "Trip").note.as_deref(), Some("Trip"));
        let site = find(Hl::Link, "site");
        assert_eq!(site.url.as_deref(), Some("https://x.co"));
        assert_eq!(hidden(site), ["[", "](https://x.co)"]);
        assert_eq!(
            find(Hl::Link, "https://a.b/c").url.as_deref(),
            Some("https://a.b/c"),
            "a full stop after a link isn't part of it"
        );
        assert_eq!(find(Hl::Tag, "#tag").tag.as_deref(), Some("tag"));
        find(Hl::Italic, "it");
        assert_eq!(
            spans.iter().filter(|s| s.hl == Hl::Italic).count(),
            2,
            "snake_case isn't italic"
        );
        assert_eq!(
            spans.iter().filter(|s| s.hl == Hl::Tag).count(),
            1,
            "a#b isn't a tag"
        );
    }

    #[test]
    fn nested_styles_each_find_their_markers() {
        let line = "alpha ***beta*** gamma";
        let spans = inline(line, 0);
        assert!(
            spans
                .iter()
                .any(|s| s.hl == Hl::Bold && &line[s.s..s.e] == "beta")
        );
        assert!(spans.iter().any(|s| s.hl == Hl::Italic));
        let image = "  ![shot](.attachments/Bugs/shot.png)";
        let spans = inline(image, 0);
        assert_eq!(
            spans[0].image.as_deref(),
            Some(".attachments/Bugs/shot.png")
        );
        assert_eq!(&image[spans[0].s..spans[0].e], "shot");
    }

    #[test]
    fn a_ticked_items_lines_are_struck_through_and_markup_hides() {
        let lines = [
            "# List", "- [ ] b", "", "- [x] a", "- [x] c", "  why c", "  - more", "", "after",
        ];
        let ks = kinds(&lines);
        let d = Drawing {
            strike_checked: true,
            note_exists: &|_| true,
        };
        let looks = looks(&lines, &ks, &d);
        let struck: Vec<usize> = (0..lines.len())
            .filter(|i| looks[*i].has(Hl::CheckedStrike))
            .collect();
        assert_eq!(
            struck,
            [3, 4, 5, 6],
            "each ticked item and the lines under it, not the gap or what follows"
        );
        let shown = |i: usize, raw: bool| {
            cells(lines[i], &looks[i], raw, &[], false)
                .iter()
                .map(|c| c.text.clone())
                .collect::<String>()
        };
        assert_eq!(shown(0, false), "▾ List");
        assert_eq!(shown(1, false), "☐ b");
        assert_eq!(shown(3, false), "☑ a");
        assert_eq!(shown(6, false), "  – more");
        assert_eq!(
            shown(1, true),
            "- [ ] b",
            "the line typed in shows its markup"
        );
        let bold = ["# T", "say **hello** world"];
        let looks = looks_of(&bold);
        let cells = cells(bold[1], &looks[1], false, &[], false);
        assert_eq!(
            cells.iter().map(|c| c.text.as_str()).collect::<String>(),
            "say hello world"
        );
        let h = cells.iter().find(|c| c.text == "h").unwrap();
        assert_eq!(h.col, Some(6));
        assert!(h.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn an_unticked_items_plan_and_choice_are_its_chips() {
        let lines = [
            "# Work",
            "- [ ] stable IDs",
            "  Plan: [[Stable IDs]]",
            "- [ ] a condensed sidebar",
            "  Plan: [[Condensed sidebar]], needs a choice: A or B",
            "- [ ] needs a choice between two looks",
            "- [ ] no plan yet",
            "  some words",
            "- [x] landed",
            "  Plan: [[Landed]]",
            "Plan: [[Not under an item]]",
        ];
        let ks = kinds(&lines);
        let got = chips(&lines, &ks, &HashMap::new());
        assert_eq!(got[1], [Chip::Plan]);
        assert_eq!(got[3], [Chip::Choice, Chip::Plan]);
        assert_eq!(got[5], [Chip::Choice]);
        assert!(got[6].is_empty(), "no Plan: line, no chip");
        assert!(got[8].is_empty(), "a ticked item gets nothing new");
        assert!(got[10].is_empty(), "a plan under no item");
        assert_eq!(Chip::Plan.text(), "plan");
        assert_eq!(Chip::Choice.style(false).fg, Some(Color::Indexed(3)));
    }

    #[test]
    fn a_tab_on_an_item_is_its_first_chip_and_a_ticked_items_commit_is_landed() {
        let lines = [
            "# Work",
            "- [ ] show uncommitted changes in M-d",
            "  Plan: [[M-d uncommitted]]",
            "  - [ ] give each note a stable ID",
            "- [ ] nobody on it",
            "- [x] sealed review of community PRs",
            "  landed in 2505ca4f, see the PR",
            "- [x] ticked with words only",
            "  defaced facade 1234567",
        ];
        let ks = kinds(&lines);
        let tabs = HashMap::from([
            (
                "show uncommitted changes in M-d".to_string(),
                ItemTab {
                    id: "@3".into(),
                    note: String::new(),
                    note_id: String::new(),
                    index: 3,
                    waits: false,
                    detail: "review round 1".into(),
                    color: Some(Color::Rgb(0x9a, 0xd0, 0xf5)),
                },
            ),
            (
                "give each note a stable ID".to_string(),
                ItemTab {
                    id: "@2".into(),
                    note: String::new(),
                    note_id: String::new(),
                    index: 2,
                    waits: true,
                    detail: "git push".into(),
                    color: None,
                },
            ),
        ]);
        let got = chips(&lines, &ks, &tabs);
        assert_eq!(
            got[1],
            [
                Chip::Working(
                    3,
                    "review round 1".into(),
                    Some(Color::Rgb(0x9a, 0xd0, 0xf5))
                ),
                Chip::Plan
            ]
        );
        assert_eq!(got[3], [Chip::Waits(2, "git push".into(), None)]);
        assert!(got[4].is_empty());
        assert_eq!(got[1][0].text(), "⠹ tab 3 · review round 1");
        assert_eq!(got[3][0].text(), "● tab 2 waits on you · git push");
        assert_eq!(Chip::Working(1, String::new(), None).text(), "⠹ tab 1");
        assert_eq!(got[1][0].style(false).fg, Some(Color::Indexed(4)));
        assert_eq!(got[3][0].style(false).fg, Some(Color::Indexed(3)));
        // The tab's number in its color, the rest in the state's.
        let runs = got[1][0].runs(false, false);
        let texts: Vec<&str> = runs.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(texts, [" ⠹ ", "tab 3", " · review round 1 "]);
        assert_eq!(runs[1].1.fg, Some(Color::Rgb(0x9a, 0xd0, 0xf5)));
        assert_eq!(runs[2].1.fg, Some(Color::Indexed(4)));
        assert_eq!(got[3][0].runs(false, false).len(), 1, "no color known");
        assert_eq!(got[1][0].width(), width(&got[1][0].text()) + 2);
        let landed = landed(&lines, &ks);
        assert_eq!(landed[5].as_deref(), Some("2505ca4"));
        assert_eq!(landed[7], None, "words of hex letters or digits alone");
        assert_eq!(landed[1], None, "an unticked item lands nothing");
    }

    #[test]
    fn an_items_plan_parked_sections_and_where_a_new_item_goes() {
        let lines = [
            "# Work",
            "",
            "- [ ] with a plan",
            "  After: Kit",
            "  Plan: [[Plan - Kit]], needs a choice",
            "- [ ] no plan",
            "  - [ ] Plan: [[Not this one]]",
            "",
            "- [x] done",
            "## Defered",
            "- [ ] parked one",
            "- [ ] parked two",
            "  - [ ] under it",
            "## Shipped",
        ];
        let ks = kinds(&lines);
        assert_eq!(plan_of(&lines, &ks, 2).as_deref(), Some("Plan - Kit"));
        assert_eq!(plan_of(&lines, &ks, 5), None, "a sub-item is no plan");
        assert_eq!(plan_of(&lines, &ks, 8), None, "a ticked item");
        assert_eq!(plan_of(&lines, &ks, 3), None, "not an item");
        assert!(parked(lines[9], ks[9]));
        assert!(!parked(lines[13], ks[13]));
        assert!(!parked("Defered", Kind::Body));
        assert_eq!(items_in(&ks, 9, 13), 2);
        assert_eq!(new_item_row(&lines, &ks), 7);
        let bare = ["# Fresh", "", "Some words"];
        assert_eq!(new_item_row(&bare, &kinds(&bare)), 2);
        let sections = ["# Fresh", "## Defered", "- [ ] parked"];
        assert_eq!(new_item_row(&sections, &kinds(&sections)), 1);
        let sub = ["# Work", "- [ ] Active", "### Later", "- [ ] Deferred"];
        assert_eq!(new_item_row(&sub, &kinds(&sub)), 2);
        // An item's lines past a blank line under it are still its own.
        let gap = [
            "# T",
            "- [ ] First",
            "  - detail",
            "",
            "  Plan: [[Plan - First]]",
            "",
            "- [x] done",
        ];
        let gk = kinds(&gap);
        assert_eq!(plan_of(&gap, &gk, 1).as_deref(), Some("Plan - First"));
        let nested = [
            "- [ ] Parent",
            "  - [ ] Child",
            "    Plan: [[Plan - Child]]",
            "  Plan: [[Plan - Parent]]",
        ];
        let nk = kinds(&nested);
        assert_eq!(plan_of(&nested, &nk, 0).as_deref(), Some("Plan - Parent"));
        assert_eq!(plan_of(&nested, &nk, 1).as_deref(), Some("Plan - Child"));
        assert_eq!(new_item_row(&gap, &gk), 5);
    }

    fn looks_of(lines: &[&str]) -> Vec<Look> {
        let ks = kinds(lines);
        looks(
            lines,
            &ks,
            &Drawing {
                strike_checked: true,
                note_exists: &|_| true,
            },
        )
    }
}
