//! The PR (M-v) and the ticket (M-i) as UX 2.0's boards 13 and 14 draw
//! them, in Claude's place: a column read top to bottom (the head, the
//! description through Markdown, what follows it: the ticket's sub-issues,
//! the comments with people's photos) and beside it a panel of what GitHub
//! and Linear keep at the right of the page (the PR's checks, reviewers,
//! open review threads and files; the ticket's facts, pull requests and
//! related tickets). New with UX 2.0, with no tmux-version file behind it:
//! pr-view.js laid both out as one Markdown text, which the views still do
//! with UX 2.0 off.
//!
//! A board is plain data (`Board`, its `Part`s and `Piece`s), which
//! `super::pr` and `super::ticket` build from what GitHub and Linear
//! answered, and `lay_out` turns into the pager's lines for a width, the
//! kit's atoms (src/ui/kit.rs) for its chips, marks and labels, so a state
//! has the mark and tone it has everywhere. The pager (`super::pager`) puts
//! the panel beside the column where the pane is wide enough
//! (`PANEL_MIN`), fixed while the column scrolls, its lists cut short by
//! the builders with "N more"; a section that doesn't fit in the pane's
//! height beside the column follows the column instead, as all of them do
//! in a narrower pane, so none is out of reach.
//!
//! An avatar is someone's photo where the view draws pictures
//! (`super::pictures`, with Settings' `photos` on), from the cache Photos
//! keeps (src/photos), cropped to a circle; until it is had, and wherever
//! it can't be, it is the kit's text form, their initial on a color of
//! their own (`kit::avatar`, board 0d). It is Large (`Part::Said`, 4 by 2)
//! beside what someone wrote or did, Small (`Piece::Avatar`, 2 by 1) in a
//! run of names.

use ratatui::style::{Color, Modifier};

use super::markdown::{
    self, Doc, Images, Line, Markdown, PictureRow, Placement, Span, Style, Wanted,
};
use crate::photos::Who;
use crate::ui::Theme;
use crate::ui::kit::{self, Mark, Tone};

/// The narrowest pane the panel is drawn beside the column in.
pub const PANEL_MIN: usize = 100;

/// The panel's width in a pane `cols` wide: its share of the board's 160
/// columns (48), kept on a screen of any width as board 45 says, never
/// under 36.
pub fn panel_width(cols: usize) -> usize {
    kit::beside(cols, 48, 36, 0)
}

/// The PR or the ticket, as its board lays it out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Board {
    /// The column, read top to bottom.
    pub main: Vec<Part>,
    /// What sits beside the column.
    pub panel: Vec<Part>,
    /// Where a diagram that can't be drawn here is shown properly.
    pub place: String,
    /// The footer's keys and what each does.
    pub keys: Vec<(String, String)>,
}

/// One piece of a board's column or panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    Row(Vec<Piece>),
    /// A row with something at its right edge: a file's lines.
    Ends(Vec<Piece>, Vec<Piece>),
    /// Pieces flowed into as many rows as they take: the checks.
    Flow(Vec<Vec<Piece>>),
    /// A section's name, in capitals, and its count at the right.
    Label(String, Option<String>),
    /// Markdown, `indent` columns in from the margin.
    Text {
        markdown: String,
        indent: usize,
    },
    /// What someone wrote or did, `indent` columns in (board 0d): their
    /// Large avatar, `head` (their name, when) beside its first row and
    /// `body`'s Markdown from its second, both `kit::SAID_INDENT` past
    /// it; with no body, the second row is left blank.
    Said {
        who: Option<Who>,
        name: String,
        head: Vec<Piece>,
        body: String,
        indent: usize,
    },
    Rule,
    Blank,
}

/// A piece of a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    Text(String, Tone),
    /// A name: bold.
    Name(String, Tone),
    Chip(String, Tone),
    Mark(Mark),
    /// Someone's Small avatar (`kit::avatar`), two columns wide: who (none
    /// for a team), and the name whose initial stands in for it.
    Avatar(Option<Who>, String),
    /// Text that opens a link.
    Link(String, Tone, String),
}

/// How the board is drawn here: the theme, and whether the terminal draws
/// the kit's half circles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    pub theme: Theme,
    pub caps: bool,
}

impl Look {
    /// The look of the terminal the pane is in: the multiplexer's panes
    /// keep the terminal the workspace started in as `TERM_PROGRAM`.
    pub fn here() -> Look {
        Look {
            theme: if crate::status::theme::light() {
                Theme::LIGHT
            } else {
                Theme::DARK
            },
            caps: crate::mux::graphics::apps_round_ends(),
        }
    }
}

impl Default for Look {
    fn default() -> Look {
        Look {
            theme: Theme::DARK,
            caps: false,
        }
    }
}

/// The columns either side of what a board draws, as the Markdown's.
const MARGIN: usize = 2;
/// How far apart the anchors of two parts are, more than a part's blocks.
const PART: usize = 1 << 24;

impl Board {
    /// What it has that the view draws as pictures: the Markdown's pictures
    /// and diagrams, and the photos of everyone on it.
    pub fn wanted(&self, place: &str) -> Vec<Wanted> {
        let mut out = Vec::new();
        for part in self.main.iter().chain(&self.panel) {
            match part {
                Part::Text { markdown, .. } => {
                    out.extend(Markdown::parse(markdown, place).wanted())
                }
                Part::Said {
                    who, head, body, ..
                } => {
                    out.extend(who.iter().map(|w| Wanted::Photo(w.clone())));
                    out.extend(photos(head));
                    out.extend(Markdown::parse(body, place).wanted());
                }
                Part::Row(pieces) | Part::Ends(pieces, _) => out.extend(photos(pieces)),
                Part::Flow(groups) => groups.iter().for_each(|g| out.extend(photos(g))),
                _ => {}
            }
        }
        let mut seen = std::collections::HashSet::new();
        out.retain(|w| seen.insert(w.clone()));
        out
    }
}

fn photos(pieces: &[Piece]) -> Vec<Wanted> {
    pieces
        .iter()
        .filter_map(|p| match p {
            Piece::Avatar(Some(who), _) => Some(Wanted::Photo(who.clone())),
            _ => None,
        })
        .collect()
}

/// Lays `parts` out for a column `cols` wide, each picture and photo
/// `images` has ready drawn as one.
pub fn lay_out(
    parts: &[Part],
    cols: usize,
    place: &str,
    images: Option<&Images>,
    look: &Look,
) -> Doc {
    let mut out = Doc::default();
    let inner = cols.saturating_sub(2 * MARGIN).max(10);
    let margin = || Span::new(" ".repeat(MARGIN), Style::default());
    for (i, part) in parts.iter().enumerate() {
        let base = i * PART;
        let row = |spans: Vec<Span>, out: &mut Doc, at: usize| {
            let mut all = vec![margin()];
            all.extend(spans);
            out.lines.push(Line::of(all, (base, at, 1)));
        };
        match part {
            Part::Blank => row(Vec::new(), &mut out, 0),
            Part::Rule => row(
                vec![Span::new(
                    "─".repeat(inner),
                    Style::fg(color(kit::border(&look.theme))),
                )],
                &mut out,
                0,
            ),
            Part::Row(pieces) => {
                // A fact's key, or a reply's indent, before the rest: its
                // lines after the first start under the rest.
                let hang = match pieces.first() {
                    Some(Piece::Text(t, _)) if pieces.len() > 1 && t.ends_with(' ') => {
                        let next = match &pieces[1] {
                            Piece::Text(n, _) => n.len() - n.trim_start_matches(' ').len(),
                            _ => 0,
                        };
                        markdown::width(t) + next
                    }
                    _ => 0,
                };
                let spans = spans_of(pieces, &mut out, images, look);
                for (at, line) in wrap(spans, inner, hang).into_iter().enumerate() {
                    row(line, &mut out, at);
                }
            }
            Part::Label(name, count) => {
                let left = vec![Piece::Name(name.to_uppercase(), Tone::Dim)];
                let right = count
                    .iter()
                    .map(|c| Piece::Text(c.clone(), Tone::Dim))
                    .collect::<Vec<_>>();
                let spans = ends(&left, &right, inner, &mut out, images, look);
                row(spans, &mut out, 0);
            }
            Part::Ends(left, right) => {
                let spans = ends(left, right, inner, &mut out, images, look);
                row(spans, &mut out, 0);
            }
            Part::Flow(groups) => {
                // Each row as many groups as fit, a group wider than a row
                // wrapped on rows of its own, under its text.
                let mut lines: Vec<Vec<Span>> = Vec::new();
                let mut line: Vec<Span> = Vec::new();
                let mut used = 0;
                for group in groups {
                    let spans = spans_of(group, &mut out, images, look);
                    let w: usize = spans.iter().map(Span::width).sum();
                    if used > 0 && used + 3 + w > inner {
                        lines.push(std::mem::take(&mut line));
                        used = 0;
                    }
                    if w > inner {
                        lines.extend(wrap(spans, inner, 2));
                        continue;
                    }
                    if used > 0 {
                        line.push(Span::new("   ", Style::default()));
                        used += 3;
                    }
                    used += w;
                    line.extend(spans);
                }
                if !line.is_empty() {
                    lines.push(line);
                }
                for (at, line) in lines.into_iter().enumerate() {
                    row(line, &mut out, at);
                }
            }
            Part::Text { markdown, indent } => {
                let width = (inner + 2 * MARGIN).saturating_sub(*indent);
                for mut line in text_lines(markdown, width, place, images, &mut out, base) {
                    if *indent > 0 {
                        line.spans
                            .insert(0, Span::new(" ".repeat(*indent), Style::default()));
                    }
                    out.lines.push(line);
                }
            }
            Part::Said {
                who,
                name,
                head,
                body,
                indent,
            } => {
                let face = avatar(who.as_ref(), name, kit::AvatarSize::Large, &mut out, images);
                let mut rows = face.into_iter();
                let past = indent + kit::SAID_INDENT;
                // The avatar's rows, each beside the next line: the head's,
                // wrapped (a review thread's place kept whole), then the
                // body's; a row with nothing beside it on a line of its own.
                let beside = |rows: &mut std::vec::IntoIter<Vec<Span>>| {
                    let mut s = vec![Span::new(" ".repeat(indent + MARGIN), Style::default())];
                    match rows.next() {
                        Some(row) => s.extend(row),
                        None => s.push(Span::new(
                            " ".repeat(kit::AvatarSize::Large.cols()),
                            Style::default(),
                        )),
                    }
                    s.push(Span::new(" ", Style::default()));
                    s
                };
                let head = spans_of(head, &mut out, images, look);
                for (at, line) in wrap(head, inner.saturating_sub(past), 0)
                    .into_iter()
                    .enumerate()
                {
                    let mut all = beside(&mut rows);
                    all.extend(line);
                    out.lines.push(Line::of(all, (base, at, 1)));
                }
                let width = (inner + 2 * MARGIN).saturating_sub(past);
                let mut lines = text_lines(body, width, place, images, &mut out, base);
                let mut first_beside = false;
                if rows.len() > 0 {
                    if lines.first_mut().is_some_and(|l| drop_margin(&mut l.spans)) {
                        lines[0].spans.splice(0..0, beside(&mut rows));
                        first_beside = true;
                    } else {
                        out.lines
                            .push(Line::of(beside(&mut rows), (base, 1 << 16, 1)));
                    }
                }
                for (n, mut line) in lines.into_iter().enumerate() {
                    if !(first_beside && n == 0) {
                        line.spans
                            .insert(0, Span::new(" ".repeat(past), Style::default()));
                    }
                    out.lines.push(line);
                }
            }
        }
    }
    out
}

/// Takes the Markdown's margin, its first `MARGIN` columns of spaces,
/// off a line's start; false, leaving it as it was, where they aren't spaces.
fn drop_margin(spans: &mut Vec<Span>) -> bool {
    let blank: String = spans
        .iter()
        .take_while(|s| s.picture.is_none())
        .flat_map(|s| s.text.chars())
        .take(MARGIN)
        .collect();
    if blank != " ".repeat(MARGIN) {
        return false;
    }
    let mut left = MARGIN;
    while left > 0 {
        let first = &mut spans[0];
        let n = first.text.len().min(left);
        first.text.drain(..n);
        left -= n;
        if first.text.is_empty() {
            spans.remove(0);
        }
    }
    true
}

/// `markdown` laid out `width` wide, its blank ends dropped, its links and
/// pictures taken into `out` and its lines anchored in the part at `base`.
fn text_lines(
    markdown: &str,
    width: usize,
    place: &str,
    images: Option<&Images>,
    out: &mut Doc,
    base: usize,
) -> Vec<Line> {
    let doc = Markdown::parse(markdown, place).layout_with(width, images);
    let shown = |l: &Line| !l.plain().trim().is_empty() || l.picture.is_some();
    let (Some(from), Some(to)) = (
        doc.lines.iter().position(shown),
        doc.lines.iter().rposition(shown),
    ) else {
        return Vec::new();
    };
    let offset = out.links.len();
    out.links.extend(doc.links);
    for p in doc.placed {
        if !out.placed.iter().any(|q| q.id == p.id) {
            out.placed.push(p);
        }
    }
    doc.lines
        .into_iter()
        .take(to + 1)
        .skip(from)
        .map(|mut line| {
            for s in &mut line.spans {
                s.link = s.link.map(|l| l + offset);
            }
            if let Some(p) = &mut line.picture {
                p.link = p.link.map(|l| l + offset);
            }
            let (block, at, n) = line.anchor;
            line.anchor = (base + block, at, n);
            line
        })
        .collect()
}

/// A row with `right` at its right edge, `left` cut to the room left.
fn ends(
    left: &[Piece],
    right: &[Piece],
    inner: usize,
    out: &mut Doc,
    images: Option<&Images>,
    look: &Look,
) -> Vec<Span> {
    let right = spans_of(right, out, images, look);
    let rw: usize = right.iter().map(Span::width).sum();
    let room = if rw == 0 {
        inner
    } else {
        inner.saturating_sub(rw + 1)
    };
    let mut spans = cut_start(spans_of(left, out, images, look), room);
    let lw: usize = spans.iter().map(Span::width).sum();
    if rw > 0 {
        spans.push(Span::new(
            " ".repeat(inner.saturating_sub(lw + rw).max(1)),
            Style::default(),
        ));
        spans.extend(right);
    }
    spans
}

/// Spans wrapped at spaces into lines `cols` wide, the lines after the
/// first starting `hang` columns in; a word wider than a line broken where
/// the line ends, and a picture kept whole.
fn wrap(spans: Vec<Span>, cols: usize, hang: usize) -> Vec<Vec<Span>> {
    use unicode_segmentation::UnicodeSegmentation;
    let total: usize = spans.iter().map(Span::width).sum();
    if total <= cols {
        return vec![spans];
    }
    let hang = if hang * 2 > cols { 0 } else { hang };
    let mut lines: Vec<Vec<Span>> = Vec::new();
    let mut line: Vec<Span> = Vec::new();
    let mut used = 0;
    let next_line = |line: &mut Vec<Span>, used: &mut usize, lines: &mut Vec<Vec<Span>>| {
        while line
            .last()
            .is_some_and(|s| s.picture.is_none() && s.text.trim().is_empty())
        {
            line.pop();
        }
        lines.push(std::mem::take(line));
        line.push(Span::new(" ".repeat(hang), Style::default()));
        *used = hang;
    };
    for span in spans {
        if span.picture.is_some() {
            if used + span.width() > cols && used > hang {
                next_line(&mut line, &mut used, &mut lines);
            }
            used += span.width();
            line.push(span);
            continue;
        }
        for word in words(&span.text) {
            let w = markdown::width(word);
            let blank = word.trim().is_empty();
            if used + w > cols && used > hang {
                next_line(&mut line, &mut used, &mut lines);
                if blank {
                    continue;
                }
            }
            let mut piece = String::new();
            for g in word.graphemes(true) {
                let gw = markdown::width(g);
                if used + gw > cols && used > hang {
                    line.push(Span {
                        text: std::mem::take(&mut piece),
                        ..span.clone()
                    });
                    next_line(&mut line, &mut used, &mut lines);
                }
                piece.push_str(g);
                used += gw;
            }
            line.push(Span {
                text: piece,
                ..span.clone()
            });
        }
    }
    if line
        .iter()
        .any(|s| s.picture.is_some() || !s.text.trim().is_empty())
    {
        lines.push(line);
    }
    lines
}

/// A text as its runs of spaces and of everything else, in order: what
/// wraps as one, broken only at a space.
fn words(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut from = 0;
    let mut space = None;
    for (i, c) in text.char_indices() {
        let is = c == ' ';
        if space.is_some_and(|s| s != is) {
            out.push(&text[from..i]);
            from = i;
        }
        space = Some(is);
    }
    if from < text.len() {
        out.push(&text[from..]);
    }
    out
}

/// Spans cut from their start to `cols` columns, an ellipsis where text
/// was cut, so a path keeps its file's name.
fn cut_start(spans: Vec<Span>, cols: usize) -> Vec<Span> {
    let total: usize = spans.iter().map(Span::width).sum();
    if total <= cols || spans.iter().any(|s| s.picture.is_some()) {
        return cut(spans, cols);
    }
    let mut drop = total + 1 - cols;
    let mut out = vec![Span::new("…", spans[0].style)];
    for span in spans {
        let w = span.width();
        if drop >= w {
            drop -= w;
            continue;
        }
        let mut skipped = 0;
        let kept: String = span
            .text
            .chars()
            .skip_while(|c| {
                let skip = skipped < drop;
                skipped += markdown::width(&c.to_string());
                skip
            })
            .collect();
        drop = 0;
        out.push(Span { text: kept, ..span });
    }
    out
}

/// Spans cut to `cols` columns, an ellipsis where text was cut.
fn cut(spans: Vec<Span>, cols: usize) -> Vec<Span> {
    let total: usize = spans.iter().map(Span::width).sum();
    if total <= cols {
        return spans;
    }
    let mut out = Vec::new();
    let mut used = 0;
    for span in spans {
        let w = span.width();
        if used + w < cols {
            used += w;
            out.push(span);
            continue;
        }
        if span.picture.is_none() {
            let kept = crate::ui::draw::cut(&span.text, cols.saturating_sub(used + 1));
            out.push(Span {
                text: format!("{}…", kept.trim_end()),
                ..span
            });
        } else {
            out.push(Span::new("…", Style::default()));
        }
        break;
    }
    out
}

/// A 256-color index from the kit's colors, which are all of them.
fn color(c: Color) -> u8 {
    match c {
        Color::Indexed(n) => n,
        _ => markdown::TEXT,
    }
}

/// The kit's spans as the pager's.
fn from_kit(spans: Vec<ratatui::text::Span<'static>>) -> Vec<Span> {
    spans
        .into_iter()
        .map(|s| {
            let style = Style {
                fg: match s.style.fg {
                    Some(Color::Indexed(n)) => Some(n),
                    _ => None,
                },
                bg: match s.style.bg {
                    Some(Color::Indexed(n)) => Some(n),
                    _ => None,
                },
                bold: s.style.add_modifier.contains(Modifier::BOLD),
                ..Style::default()
            };
            Span::new(s.content.into_owned(), style)
        })
        .collect()
}

fn tone(t: Tone, bold: bool, look: &Look) -> Style {
    Style {
        fg: Some(color(t.fg(&look.theme))),
        bold,
        ..Style::default()
    }
}

fn spans_of(pieces: &[Piece], out: &mut Doc, images: Option<&Images>, look: &Look) -> Vec<Span> {
    let mut spans = Vec::new();
    for piece in pieces {
        match piece {
            Piece::Text(text, t) => spans.push(Span::new(text.clone(), tone(*t, false, look))),
            Piece::Name(text, t) => spans.push(Span::new(text.clone(), tone(*t, true, look))),
            Piece::Chip(text, t) => {
                spans.extend(from_kit(kit::chip(text, *t, look.caps, &look.theme)))
            }
            Piece::Mark(m) => spans.extend(from_kit(vec![kit::mark(*m, 0, &look.theme)])),
            Piece::Link(text, t, url) => {
                let at = out.links.len();
                out.links.push(url.clone());
                let mut span = Span::new(text.clone(), tone(*t, false, look));
                span.link = Some(at);
                spans.push(span);
            }
            Piece::Avatar(who, name) => spans.extend(
                avatar(who.as_ref(), name, kit::AvatarSize::Small, out, images)
                    .into_iter()
                    .flatten(),
            ),
        }
    }
    spans
}

/// Someone's avatar as large as `size`, a row a line: their photo where
/// it is ready, else the kit's text form, their initial on their color.
fn avatar(
    who: Option<&Who>,
    name: &str,
    size: kit::AvatarSize,
    out: &mut Doc,
    images: Option<&Images>,
) -> Vec<Vec<Span>> {
    let (cols, rows) = (size.cols() as u16, size.rows() as u16);
    let ready = who.and_then(|w| {
        let key = markdown::photo_key(w);
        images
            .and_then(|i| i.ready.get(&key))
            .map(|(id, _)| (key, *id))
    });
    let Some((key, id)) = ready else {
        return kit::avatar(name, size).into_iter().map(from_kit).collect();
    };
    // The same photo Small and Large is two images, each its own size.
    let id = match size {
        kit::AvatarSize::Small => id,
        kit::AvatarSize::Large => markdown::sized_id(id, cols, rows),
    };
    if !out.placed.iter().any(|p| p.id == id) {
        out.placed.push(Placement {
            key,
            id,
            cols,
            rows,
        });
    }
    (0..rows)
        .map(|row| {
            let mut span = Span::new(" ".repeat(cols as usize), Style::default());
            span.picture = Some(PictureRow {
                id,
                row,
                cols,
                link: None,
            });
            vec![span]
        })
        .collect()
}

/// How long ago an ISO time was, as the boards say it: "just now", "4m
/// ago", "2h ago", "yesterday", "3 days ago", else its day.
pub fn when(iso: &str, now: jiff::Timestamp) -> String {
    let Ok(at) = iso.parse::<jiff::Timestamp>() else {
        return iso.chars().take(10).collect();
    };
    let s = now.as_second() - at.as_second();
    match s {
        ..60 => "just now".into(),
        60..3600 => format!("{}m ago", s / 60),
        3600..86_400 => format!("{}h ago", s / 3600),
        86_400..172_800 => "yesterday".into(),
        172_800..2_592_000 => format!("{} days ago", s / 86_400),
        _ => iso.chars().take(10).collect(),
    }
}

/// A panel's parts in its sections, each from its label (and what comes
/// before the first label as one of its own), the blank lines between
/// them kept with the section they end.
pub fn sections(parts: &[Part]) -> Vec<&[Part]> {
    let mut out = Vec::new();
    let mut from = 0;
    for (i, part) in parts.iter().enumerate() {
        if matches!(part, Part::Label(..)) && i > from {
            out.push(&parts[from..i]);
            from = i;
        }
    }
    if from < parts.len() {
        out.push(&parts[from..]);
    }
    out
}

/// The color of the line between the column and the panel.
pub fn rule_color(look: &Look) -> u8 {
    color(kit::border(&look.theme))
}

/// The color of the text, a scroll bar's thumb among it.
pub fn text_color(look: &Look) -> u8 {
    color(look.theme.text)
}

/// "N more", for a list cut short.
pub fn more(n: usize) -> Part {
    Part::Row(vec![Piece::Text(format!("{n} more"), Tone::Dim)])
}

/// The footer's keys, as the kit's hints.
pub fn hints(keys: &[(String, String)], look: &Look) -> Line {
    let pairs: Vec<(&str, &str)> = keys.iter().map(|(k, w)| (k.as_str(), w.as_str())).collect();
    let mut spans = vec![Span::new(" ", Style::default())];
    spans.extend(from_kit(kit::hints(&pairs, &look.theme)));
    Line::of(spans, (0, 0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::photos::Service;

    #[test]
    fn the_panel_keeps_its_share_of_a_wide_pane() {
        assert_eq!(panel_width(100), 36);
        assert_eq!(panel_width(160), 48);
        assert_eq!(panel_width(240), 72);
    }

    fn plain(doc: &Doc) -> Vec<String> {
        doc.lines
            .iter()
            .map(|l| l.plain().trim_end().to_string())
            .collect()
    }

    #[test]
    fn rows_labels_and_ends_fill_the_width() {
        let parts = vec![
            Part::Label("Files".into(), Some("6".into())),
            Part::Ends(
                vec![Piece::Text("tests/notes_bench.rs".into(), Tone::Dim)],
                vec![Piece::Text("+96".into(), Tone::Done)],
            ),
            Part::Row(vec![Piece::Text("x".repeat(80), Tone::Text)]),
        ];
        let doc = lay_out(&parts, 40, "", None, &Look::default());
        let lines = plain(&doc);
        assert_eq!(lines[0], format!("  FILES{}6", " ".repeat(30)));
        assert_eq!(
            lines[1],
            format!("  tests/notes_bench.rs{}+96", " ".repeat(13))
        );
        // A word wider than the line is broken where the line ends.
        assert_eq!(lines[2], format!("  {}", "x".repeat(36)));
        assert_eq!(lines[3], format!("  {}", "x".repeat(36)));
        assert_eq!(lines[4], format!("  {}", "x".repeat(8)));
    }

    #[test]
    fn a_long_row_wraps_under_its_key_and_a_long_path_keeps_its_end() {
        let parts = vec![
            Part::Row(vec![
                Piece::Text("Labels     ".into(), Tone::Dim),
                Piece::Text(" notes · reliability · performance".into(), Tone::Text),
            ]),
            Part::Ends(
                vec![Piece::Text(
                    "src/views/a/very/long/path/to/notes_bench.rs".into(),
                    Tone::Dim,
                )],
                vec![Piece::Text("+96".into(), Tone::Done)],
            ),
        ];
        let lines = plain(&lay_out(&parts, 34, "", None, &Look::default()));
        assert_eq!(
            lines,
            vec![
                "  Labels      notes ·",
                "              reliability ·",
                "              performance",
                "  …ng/path/to/notes_bench.rs +96",
            ]
        );
    }

    #[test]
    fn checks_flow_into_rows() {
        let check = |n: &str| {
            vec![
                Piece::Mark(Mark::Passed),
                Piece::Text(format!(" {n}"), Tone::Text),
            ]
        };
        let parts = vec![Part::Flow(vec![
            check("fmt"),
            check("clippy"),
            check("test (linux)"),
        ])];
        let lines = plain(&lay_out(&parts, 30, "", None, &Look::default()));
        assert_eq!(lines, vec!["  ✓ fmt   ✓ clippy", "  ✓ test (linux)"]);
        let long = vec![Part::Flow(vec![
            check("fmt"),
            check("integration tests (macos, aarch64)"),
            check("x"),
        ])];
        let lines = plain(&lay_out(&long, 30, "", None, &Look::default()));
        assert_eq!(
            lines,
            vec![
                "  ✓ fmt",
                "  ✓ integration tests",
                "    (macos, aarch64)",
                "  ✓ x"
            ]
        );
    }

    #[test]
    fn markdown_is_indented_its_links_kept_and_its_blank_ends_dropped() {
        let parts = vec![
            Part::Row(vec![Piece::Link(
                "#1".into(),
                Tone::Accent,
                "https://a".into(),
            )]),
            Part::Text {
                markdown: "See [b](https://b).".into(),
                indent: 3,
            },
        ];
        let doc = lay_out(&parts, 40, "", None, &Look::default());
        assert_eq!(plain(&doc), vec!["  #1", "     See b."]);
        assert_eq!(doc.links, vec!["https://a", "https://b"]);
        assert_eq!(doc.lines[1].link_at(9), Some(1));
    }

    #[test]
    fn an_avatar_is_the_photo_once_it_is_ready_and_the_initial_until_then() {
        let who = Who::new(Service::GitHub, "ana");
        let parts = vec![Part::Row(vec![
            Piece::Avatar(Some(who.clone()), "ana".into()),
            Piece::Name(" ana".into(), Tone::Text),
        ])];
        let board = Board {
            main: parts.clone(),
            panel: Vec::new(),
            place: String::new(),
            keys: Vec::new(),
        };
        assert_eq!(board.wanted(""), vec![Wanted::Photo(who.clone())]);
        let before = lay_out(&parts, 40, "", None, &Look::default());
        assert_eq!(plain(&before), vec!["  A  ana"]);
        assert!(before.placed.is_empty());
        let mut images = Images::default();
        images
            .ready
            .insert(markdown::photo_key(&who), (77, (128, 128)));
        let after = lay_out(&parts, 40, "", Some(&images), &Look::default());
        assert_eq!(after.placed[0].key, markdown::photo_key(&who));
        assert_eq!(after.placed[0].id, 77);
        assert_eq!(after.lines[0].width(), 8);
        let drawn = after.lines[0].ansi(&after.links, 0, 40, &[]);
        assert!(
            drawn.contains("\x1b[38;2;0;0;77m"),
            "the photo's cells: {drawn:?}"
        );
        assert!(drawn.contains("ana"));
    }

    #[test]
    fn what_someone_said_has_a_large_avatar_and_its_words_five_in() {
        let who = Who::new(Service::Linear, "u-ana");
        let said = |body: &str| {
            vec![Part::Said {
                who: Some(who.clone()),
                name: "ana".into(),
                head: vec![Piece::Name("ana".into(), Tone::Text)],
                body: body.into(),
                indent: 0,
            }]
        };
        // Without the photo: the initial on the first row, the words 5 in
        // from the second; with no words, the second row blank.
        let text = lay_out(&said("Looks good."), 40, "", None, &Look::default());
        assert_eq!(plain(&text), vec!["  A    ana", "       Looks good."]);
        let alone = lay_out(&said(""), 40, "", None, &Look::default());
        assert_eq!(alone.lines.len(), 2);
        assert_eq!(alone.lines[1].plain().trim(), "");
        // With it: one 4 by 2 picture over both rows, its own image beside
        // the Small one.
        let mut images = Images::default();
        images
            .ready
            .insert(markdown::photo_key(&who), (77, (128, 128)));
        let doc = lay_out(
            &said("Looks good."),
            40,
            "",
            Some(&images),
            &Look::default(),
        );
        assert_eq!((doc.placed[0].cols, doc.placed[0].rows), (4, 2));
        assert_ne!(doc.placed[0].id, 77);
        let rows: Vec<u16> = doc
            .lines
            .iter()
            .flat_map(|l| l.spans.iter().filter_map(|s| s.picture.map(|p| p.row)))
            .collect();
        assert_eq!(rows, vec![0, 1]);
        assert!(plain(&doc)[1].ends_with("   Looks good."));
        // A head too long for its line wraps, a review thread's place kept
        // whole, the avatar's second row beside its second line.
        let place = vec![Part::Said {
            who: None,
            name: "ana".into(),
            head: vec![
                Piece::Name("ana".into(), Tone::Text),
                Piece::Text("  tests/notes_bench.rs:38".into(), Tone::Dim),
            ],
            body: "Looks good.".into(),
            indent: 0,
        }];
        let narrow = lay_out(&place, 34, "", None, &Look::default());
        assert_eq!(
            plain(&narrow),
            vec![
                "  A    ana",
                "       tests/notes_bench.rs:38",
                "       Looks good."
            ]
        );
    }

    #[test]
    fn when_says_how_long_ago() {
        let now: jiff::Timestamp = "2026-11-08T12:00:00Z".parse().unwrap();
        assert_eq!(when("2026-11-08T11:59:30Z", now), "just now");
        assert_eq!(when("2026-11-08T10:00:00Z", now), "2h ago");
        assert_eq!(when("2026-11-07T10:00:00Z", now), "yesterday");
        assert_eq!(when("2026-11-05T12:00:00Z", now), "3 days ago");
        assert_eq!(when("2026-01-05T12:00:00Z", now), "2026-01-05");
    }
}
