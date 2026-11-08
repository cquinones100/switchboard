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
