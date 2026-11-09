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
//! keeps (src/photos), cropped to a circle two columns wide; until it is
//! had, and wherever it can't be, it is their initial on a color of their
//! own.

use ratatui::style::{Color, Modifier};

use super::markdown::{
    self, Doc, Images, Line, Markdown, PictureRow, Placement, Span, Style, Wanted,
};
use crate::photos::Who;
use crate::ui::Theme;
use crate::ui::kit::{self, Mark, Tone};

/// The narrowest pane the panel is drawn beside the column in.
pub const PANEL_MIN: usize = 100;

/// The panel's width in a pane `cols` wide: about a third, as the board
/// draws it.
pub fn panel_width(cols: usize) -> usize {
    (cols * 3 / 10).clamp(36, 52)
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
    /// Someone's photo, two columns wide: who (none for a team), and the
    /// name whose initial stands in for it.
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
        let term = format!(
            "{} {}",
            std::env::var("TERM").unwrap_or_default(),
            std::env::var("TERM_PROGRAM").unwrap_or_default()
        )
        .to_lowercase();
        Look {
            theme: if crate::status::theme::light() {
                Theme::LIGHT
            } else {
                Theme::DARK
            },
            caps: kit::caps(&term),
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
                let doc = Markdown::parse(markdown, place).layout_with(width, images);
                let from = doc
                    .lines
                    .iter()
                    .position(|l| !l.plain().trim().is_empty() || l.picture.is_some());
                let to = doc
                    .lines
                    .iter()
                    .rposition(|l| !l.plain().trim().is_empty() || l.picture.is_some());
                let (Some(from), Some(to)) = (from, to) else {
                    continue;
                };
                let offset = out.links.len();
                out.links.extend(doc.links);
                for p in doc.placed {
                    if !out.placed.iter().any(|q| q.id == p.id) {
                        out.placed.push(p);
                    }
                }
                for mut line in doc.lines.into_iter().take(to + 1).skip(from) {
                    for s in &mut line.spans {
                        s.link = s.link.map(|l| l + offset);
                    }
                    if let Some(p) = &mut line.picture {
                        p.link = p.link.map(|l| l + offset);
                    }
                    if *indent > 0 {
                        line.spans
                            .insert(0, Span::new(" ".repeat(*indent), Style::default()));
                    }
                    let (block, at, n) = line.anchor;
                    line.anchor = (base + block, at, n);
                    out.lines.push(line);
                }
            }
        }
    }
    out
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

/// A person's color for their initial, from their name.
fn color_of(name: &str) -> u8 {
    const COLORS: [u8; 8] = [67, 96, 66, 137, 61, 101, 72, 131];
    let h = name.bytes().fold(2_166_136_261u32, |h, b| {
        (h ^ b as u32).wrapping_mul(16_777_619)
    });
    COLORS[h as usize % COLORS.len()]
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
            Piece::Avatar(who, name) => spans.push(avatar(who.as_ref(), name, out, images)),
        }
    }
    spans
}

/// Someone's photo, where it is ready, else their initial on their color.
fn avatar(who: Option<&Who>, name: &str, out: &mut Doc, images: Option<&Images>) -> Span {
    let ready = who.and_then(|w| {
        let key = markdown::photo_key(w);
        images
            .and_then(|i| i.ready.get(&key))
            .map(|(id, _)| (key, *id))
    });
    if let Some((key, id)) = ready {
        if !out.placed.iter().any(|p| p.id == id) {
            out.placed.push(Placement {
                key,
                id,
                cols: 2,
                rows: 1,
            });
        }
        let mut span = Span::new("  ", Style::default());
        span.picture = Some(PictureRow {
            id,
            row: 0,
            cols: 2,
            link: None,
        });
        return span;
    }
    let initial: String = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".into());
    Span::new(
        format!("{initial} "),
        Style {
            fg: Some(255),
            bg: Some(color_of(name)),
            bold: true,
            ..Style::default()
        },
    )
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
    fn when_says_how_long_ago() {
        let now: jiff::Timestamp = "2026-11-08T12:00:00Z".parse().unwrap();
        assert_eq!(when("2026-11-08T11:59:30Z", now), "just now");
        assert_eq!(when("2026-11-08T10:00:00Z", now), "2h ago");
        assert_eq!(when("2026-11-07T10:00:00Z", now), "yesterday");
        assert_eq!(when("2026-11-05T12:00:00Z", now), "3 days ago");
        assert_eq!(when("2026-01-05T12:00:00Z", now), "2026-01-05");
    }
}
