//! UX 2.0's kit of atoms: the pieces of text every part draws with in
//! place of its own, as board 0 of the UX 2.0 canvas ("The kit") draws
//! them. New with UX 2.0, with no tmux-version file behind it.
//!
//! Tones (`Tone`) and surfaces (`Surface`) are the colors, one mark per
//! state (`mark`), and the atoms: a section's `label`, the `rule`s, a
//! list's `row`, a `chip`, a stage's `cell`, a `keycap`, a `card` (a box),
//! an `open_card` (the Tabs view's card: a top rule and its tone's bar), a
//! `board_card` (a board's card, as tall as what it holds) and the `board`
//! they make (board 0c, "The kit's board": columns of cards, the PRs,
//! Tickets and Autopilot boards' one look), a box's
//! `option`, the `pane` beside a list, an `input`, a calendar's `event`,
//! a two-row bar's `bar_name`, the key `hints` of every footer and bar
//! (the key in the accent, what it does dim, two spaces between), and a
//! log's line that says why it failed, `marked`; and a person's `avatar`
//! (board 0d), `Large` (4 by 2) beside what they wrote, their name and
//! words `SAID_INDENT` columns in, or `Small` (2 by 1) in a run of names,
//! its initial on the color `avatar_color` gives their name whatever
//! service they are from, under the photo the image atom draws.
//!
//! A row's parts are laid out by `columns` (board 45, "Layout: how a view
//! fills a screen of any size"): fixed columns at their natural width up
//! to a cap, stretching ones sharing the rest by weight (wrapped text no
//! wider than `WRAP_MAX`), right-edge ones flush right, and below what
//! they need, columns dropped in the order the view names; `beside` scales
//! a pane beside a board with the screen, and `place` puts spans in a
//! column.
//!
//! Objects are rounded and structure is square: a chip, a cell, a keycap
//! and an input end in half circles (U+E0B6 and U+E0B4) where the terminal
//! draws them itself (Ghostty, `caps`), and in a space of their fill
//! elsewhere, which the terminal would take from a font that may not have
//! them; either way the object keeps its width, so nothing moves. Boxes
//! and a board's cards have arc corners; the Tabs view's cards are open, a
//! rule along the top and a bar down the left; rules, bars, chosen rows, tables and the calendar's
//! events stay square.
//!
//! Every atom gives one row (`Vec<Span>`) or several (`Vec<Vec<Span>>`), so
//! the sidebar's and mission control's line builders take them, and a
//! `Buffer` through `ui::draw::put_line`. Text is measured with
//! `ui::draw::width`.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use super::Theme;
use super::draw::{cut, width};
use crate::mux::compose::colors::ACCENT;
use crate::sidebar::text::{Badge, SPINNER};

/// The half circle that opens a rounded object, and the one that closes it.
pub const CAP_LEFT: &str = "\u{E0B6}";
pub const CAP_RIGHT: &str = "\u{E0B4}";

/// Whether a terminal, by the name it attached with (`TERM`), draws the
/// half circles itself: Ghostty does, so they never depend on a font.
pub fn caps(term: &str) -> bool {
    term.contains("ghostty")
}

fn light(theme: &Theme) -> bool {
    *theme == Theme::LIGHT
}

/// A color by the theme: the light one, the canvas's truecolor one (as
/// `0xRRGGBB`) for `Theme::DARK_TRUE`, else the dark one.
fn ix(dark: u8, lightc: u8, canvas: u32, theme: &Theme) -> Color {
    if *theme == Theme::DARK_TRUE {
        let [_, r, g, b] = canvas.to_be_bytes();
        Color::Rgb(r, g, b)
    } else {
        Color::Indexed(if light(theme) { lightc } else { dark })
    }
}

/// The accent, the canvas's for `Theme::DARK_TRUE`.
fn accent(theme: &Theme) -> Color {
    if *theme == Theme::DARK_TRUE {
        Color::Rgb(0x5f, 0xd7, 0xff)
    } else {
        ACCENT
    }
}

/// Dark text on a tone's color (a lit key, a lit group's name).
fn on_color(theme: &Theme) -> Color {
    ix(232, 232, 0x11131a, theme)
}

/// What a piece of text says by its color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Text,
    Dim,
    Faint,
    Accent,
    Needs,
    Working,
    Landing,
    Done,
    Failed,
}

impl Tone {
    /// Its color as text.
    pub fn fg(self, theme: &Theme) -> Color {
        match self {
            Tone::Text => theme.text,
            Tone::Dim => theme.dim,
            Tone::Faint => theme.faint,
            Tone::Accent => accent(theme),
            Tone::Needs => ix(220, 136, 0xf2c56b, theme),
            Tone::Working => ix(111, 25, 0x82aaff, theme),
            Tone::Landing => ix(179, 130, 0xc49bff, theme),
            Tone::Done => ix(114, 28, 0x7fd18b, theme),
            Tone::Failed => ix(160, 160, 0xff8a94, theme),
        }
    }

    /// Its tint: a background in its hue, for chips and cells; the
    /// neutral one for the greys.
    pub fn tint(self, theme: &Theme) -> Color {
        match self {
            Tone::Text | Tone::Dim | Tone::Faint | Tone::Accent => ix(237, 254, 0x2a2e3d, theme),
            Tone::Needs => ix(58, 230, 0x3a3220, theme),
            Tone::Working => ix(24, 153, 0x20304f, theme),
            Tone::Landing => ix(94, 223, 0x2a2440, theme),
            Tone::Done => ix(22, 194, 0x1d3a2a, theme),
            Tone::Failed => ix(52, 224, 0x3a1d24, theme),
        }
    }

    pub fn style(self, theme: &Theme) -> Style {
        Style::new().fg(self.fg(theme))
    }

    /// Bold, which is for names.
    pub fn bold(self, theme: &Theme) -> Style {
        self.style(theme).add_modifier(Modifier::BOLD)
    }
}

/// What a region is drawn on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// The pane's own background.
    Base,
    Sunken,
    Panel,
    Raised,
    /// The chosen row's.
    Chosen,
}

impl Surface {
    pub fn color(self, theme: &Theme) -> Color {
        match self {
            Surface::Base => Color::Reset,
            Surface::Sunken => ix(234, 255, 0x11121a, theme),
            Surface::Panel => ix(235, 254, 0x20243a, theme),
            Surface::Raised => ix(236, 253, 0x1f2232, theme),
            Surface::Chosen => theme.highlight,
        }
    }
}

/// The color of the lines between regions.
pub fn border(theme: &Theme) -> Color {
    ix(238, 248, 0x3a3f52, theme)
}

/// A state, which has one mark wherever it is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Needs,
    Working,
    Landing,
    Queued,
    Idle,
    Failed,
    Passed,
    Pending,
    Reminder,
    Later,
    Mention,
    Note,
    Comment,
    /// The computer held awake while tabs work (src/awake).
    Awake,
    /// A tab selected in the sidebar's select mode.
    Selected,
}

impl Mark {
    /// The mark a tab's badge state is shown with.
    pub fn of_badge(b: Badge) -> Mark {
        match b {
            Badge::Waiting | Badge::Pinned => Mark::Needs,
            Badge::Done | Badge::Merged => Mark::Passed,
            Badge::Working
            | Badge::Background
            | Badge::Review
            | Badge::Ci
            | Badge::Queue
            | Badge::Deploy
            | Badge::Scheduled
            | Badge::Monitor => Mark::Working,
            Badge::Queued => Mark::Queued,
            Badge::Landing => Mark::Landing,
            Badge::Failing => Mark::Failed,
            Badge::Reminder => Mark::Reminder,
            Badge::Later => Mark::Later,
            Badge::Idle => Mark::Idle,
        }
    }

    /// Its glyph (the spinner's first frame for working) and tone.
    pub fn look(self) -> (&'static str, Tone) {
        match self {
            Mark::Needs => ("●", Tone::Needs),
            Mark::Working => (SPINNER[0], Tone::Working),
            Mark::Landing | Mark::Queued => ("▲", Tone::Landing),
            Mark::Idle => ("○", Tone::Dim),
            Mark::Failed => ("✕", Tone::Failed),
            Mark::Passed => ("✓", Tone::Done),
            Mark::Pending => ("◌", Tone::Needs),
            Mark::Reminder => ("◷", Tone::Needs),
            Mark::Later => ("◷", Tone::Dim),
            Mark::Mention => ("@", Tone::Working),
            Mark::Note => ("✎", Tone::Landing),
            Mark::Comment => ("◇", Tone::Working),
            Mark::Awake => ("☕", Tone::Dim),
            Mark::Selected => ("◆", Tone::Accent),
        }
    }
}

/// Whether a tab's badge is drawn turning: the spinner's own states, and
/// with UX 2.0 every state the kit marks as working (a loop between its
/// ticks, Monitor watches), so whatever draws it keeps the spinner going.
pub fn spins(b: Badge, ux2: bool) -> bool {
    b.spins() || (ux2 && Mark::of_badge(b) == Mark::Working)
}

/// A state's mark in its tone; working's turns with the spinner's frame.
pub fn mark(m: Mark, frame: usize, theme: &Theme) -> Span<'static> {
    let (glyph, tone) = m.look();
    let glyph = if m == Mark::Working {
        SPINNER[frame % SPINNER.len()]
    } else {
        glyph
    };
    Span::styled(glyph, tone.bold(theme))
}

/// How many columns spans take.
pub fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| width(&s.content)).sum()
}

/// Spans cut or padded to exactly `cols` columns, the padding in `pad`.
pub fn fit(spans: Vec<Span<'static>>, cols: usize, pad: Style) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut used = 0;
    for span in spans {
        let w = width(&span.content);
        if used + w <= cols {
            used += w;
            out.push(span);
            continue;
        }
        let kept = cut(&span.content, cols - used);
        used += width(&kept);
        out.push(Span::styled(kept, span.style));
        break;
    }
    if used < cols {
        out.push(Span::styled(" ".repeat(cols - used), pad));
    }
    out
}

/// A row's column, as board 45 ("Layout: how a view fills a screen of any
/// size") sorts them: one at its natural width up to a cap, one stretching
/// into what the fixed ones leave by its weight, or one flush right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Col {
    Fixed {
        min: usize,
        natural: usize,
        cap: usize,
    },
    /// A wrapped-text column's `max` is `WRAP_MAX`.
    Stretch {
        weight: usize,
        min: usize,
        max: Option<usize>,
    },
    Right {
        natural: usize,
        cap: usize,
    },
}

/// The widest wrapped text reads well at.
pub const WRAP_MAX: usize = 100;

impl Col {
    fn least(&self) -> usize {
        match *self {
            Col::Fixed { min, natural, cap } => natural.min(cap).max(min),
            Col::Right { natural, cap } => natural.min(cap),
            Col::Stretch { min, .. } => min,
        }
    }
}

/// Each column's x and width in a row `width` wide, `gap` columns between
/// two, `None` for one dropped: fixed and right-edge columns take their
/// natural width up to their cap, stretching ones share what is left by
/// weight, each within its least and most, and right-edge ones sit flush
/// right. When the fixed columns and the stretching ones' least don't fit,
/// columns are dropped in `drop`'s order (indexes into `cols`) until they do.
pub fn columns(
    cols: &[Col],
    width: usize,
    gap: usize,
    drop: &[usize],
) -> Vec<Option<(usize, usize)>> {
    let mut live = vec![true; cols.len()];
    let needed = |live: &[bool]| {
        let kept: Vec<&Col> = cols
            .iter()
            .zip(live)
            .filter(|(_, l)| **l)
            .map(|(c, _)| c)
            .collect();
        kept.iter().map(|c| c.least()).sum::<usize>() + gap * kept.len().saturating_sub(1)
    };
    for &i in drop {
        if needed(&live) <= width {
            break;
        }
        if let Some(l) = live.get_mut(i) {
            *l = false;
        }
    }

    let mut widths: Vec<usize> = cols.iter().map(Col::least).collect();
    let mut left = width.saturating_sub(needed(&live));
    let mut growing: Vec<usize> = (0..cols.len())
        .filter(|&i| live[i] && matches!(cols[i], Col::Stretch { weight, .. } if weight > 0))
        .collect();
    // Share what is left by weight; a column its share takes past its most
    // stops there, and the rest is shared again among the others.
    while left > 0 && !growing.is_empty() {
        let total: usize = growing
            .iter()
            .map(|&i| match cols[i] {
                Col::Stretch { weight, .. } => weight,
                _ => 0,
            })
            .sum();
        let rooms: Vec<usize> = growing
            .iter()
            .map(|&i| match cols[i] {
                Col::Stretch { max: Some(max), .. } => max.saturating_sub(widths[i]),
                _ => usize::MAX,
            })
            .collect();
        let shares: Vec<usize> = growing
            .iter()
            .map(|&i| match cols[i] {
                Col::Stretch { weight, .. } => left * weight / total,
                _ => 0,
            })
            .collect();
        let full: Vec<usize> = (0..growing.len())
            .filter(|&g| shares[g] >= rooms[g])
            .collect();
        if full.is_empty() {
            let given: usize = shares.iter().sum();
            for (g, &i) in growing.iter().enumerate() {
                widths[i] += shares[g];
            }
            // What rounding left over goes a cell each to those that can
            // still grow, the last first, within their most.
            let mut over = left - given;
            for g in (0..growing.len()).rev() {
                let more = over.min(rooms[g] - shares[g]).min(1);
                widths[growing[g]] += more;
                over -= more;
            }
            break;
        }
        for &g in full.iter().rev() {
            left -= rooms[g];
            widths[growing[g]] += rooms[g];
            growing.remove(g);
        }
    }

    let mut out = vec![None; cols.len()];
    let mut x = 0;
    for i in (0..cols.len()).filter(|&i| live[i] && !matches!(cols[i], Col::Right { .. })) {
        out[i] = Some((x, widths[i]));
        x += widths[i] + gap;
    }
    let mut right = width;
    for i in (0..cols.len())
        .rev()
        .filter(|&i| live[i] && matches!(cols[i], Col::Right { .. }))
    {
        right = right.saturating_sub(widths[i]);
        out[i] = Some((right, widths[i]));
        right = right.saturating_sub(gap);
    }
    out
}

/// The widest of a column's texts, its natural width.
pub fn natural<'a>(texts: impl IntoIterator<Item = &'a str>) -> usize {
    texts.into_iter().map(width).max().unwrap_or(0)
}

/// The width of a pane beside a board on a screen `width` wide, whose
/// share at 160 columns is `share`: scaled with the screen, never under
/// `min`, and 0 (dropped) when what it leaves the board would be under
/// `board_min`.
pub fn beside(width: usize, share: usize, min: usize, board_min: usize) -> usize {
    let pane = (width * share / 160).max(min);
    if width.saturating_sub(pane) < board_min {
        0
    } else {
        pane
    }
}

/// `spans` put on `line` at a column from `columns`: `line` padded to its x,
/// then the spans cut with … or padded to its width.
pub fn place(line: &mut Vec<Span<'static>>, spans: Vec<Span<'static>>, (x, w): (usize, usize)) {
    let used = spans_width(line);
    if used < x {
        line.push(Span::raw(" ".repeat(x - used)));
    }
    line.extend(fit(spans, w, Style::default()));
}

/// A section's name in capitals, bold, in its tone, with its count (when
/// it has one) a column short of the right of `cols` in the same tone; a
/// space before it.
pub fn label(
    text: &str,
    tone: Tone,
    count: Option<usize>,
    cols: usize,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let title = text.to_uppercase();
    let mut out = vec![
        Span::raw(" "),
        Span::styled(title.clone(), tone.bold(theme)),
    ];
    if let Some(n) = count {
        let n = n.to_string();
        let gap = cols.saturating_sub(width(&title) + width(&n) + 2).max(1);
        out.push(Span::raw(" ".repeat(gap)));
        out.push(Span::styled(n, tone.style(theme)));
    }
    fit(out, cols, Style::new())
}

/// The line between regions, `cols` wide.
pub fn rule(cols: usize, theme: &Theme) -> Span<'static> {
    Span::styled("─".repeat(cols), Style::new().fg(border(theme)))
}

/// The dashed line under a table's column heads, and only there.
pub fn head_rule(cols: usize, theme: &Theme) -> Span<'static> {
    Span::styled("┄".repeat(cols), Style::new().fg(border(theme)))
}

/// The line between panes side by side.
pub fn rule_v(theme: &Theme) -> Span<'static> {
    Span::styled("│", Style::new().fg(border(theme)))
}

/// The keys of a footer or bar: each key in the accent, what it does dim,
/// two spaces between pairs.
pub fn hints(pairs: &[(&str, &str)], theme: &Theme) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    for (i, (key, what)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push(Span::raw("  "));
        }
        out.push(Span::styled(key.to_string(), Tone::Accent.style(theme)));
        if !what.is_empty() {
            out.push(Span::styled(format!(" {what}"), Tone::Dim.style(theme)));
        }
    }
    out
}

/// A list's line, `cols` wide: a `▌` in the accent at its left when it is
/// the chosen one, filled with the Chosen surface edge to edge; a space in
/// the bar's place otherwise.
pub fn row(
    spans: Vec<Span<'static>>,
    cols: usize,
    chosen: bool,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let bar = if chosen { "▌" } else { " " };
    let mut line = vec![Span::styled(bar, Tone::Accent.style(theme))];
    line.extend(spans);
    let line = fit(line, cols, Style::new());
    if !chosen {
        return line;
    }
    let bg = Surface::Chosen.color(theme);
    line.into_iter()
        .map(|s| {
            let style = s.style.bg(bg);
            Span::styled(s.content, style)
        })
        .collect()
}

/// The ends of a rounded object filled with `fill`: the half circles in
/// the fill's color where the terminal draws them, else a space of it.
fn ends(fill: Color, caps: bool) -> (Span<'static>, Span<'static>) {
    if caps {
        let edge = Style::new().fg(fill);
        (Span::styled(CAP_LEFT, edge), Span::styled(CAP_RIGHT, edge))
    } else {
        let space = Style::new().bg(fill);
        (Span::styled(" ", space), Span::styled(" ", space))
    }
}

/// Spans on `fill` between its two ends.
fn rounded(inner: Vec<Span<'static>>, fill: Color, caps: bool) -> Vec<Span<'static>> {
    let (l, r) = ends(fill, caps);
    let mut out = vec![l];
    out.extend(inner.into_iter().map(|s| {
        let style = s.style.bg(fill);
        Span::styled(s.content, style)
    }));
    out.push(r);
    out
}

/// A chip: the text in its tone on its tint between its ends, and a
/// column after it.
pub fn chip(text: &str, tone: Tone, caps: bool, theme: &Theme) -> Vec<Span<'static>> {
    let mut out = rounded(
        vec![Span::styled(text.to_string(), tone.style(theme))],
        tone.tint(theme),
        caps,
    );
    out.push(Span::raw(" "));
    out
}

/// A meter of `cells` cells, as many filled as `used` percent reaches
/// (rounded), filled ones in `tone` and the rest faint.
pub fn meter(used: u8, cells: usize, tone: Tone, theme: &Theme) -> Vec<Span<'static>> {
    let filled = ((used.min(100) as usize * cells + 50) / 100).min(cells);
    vec![
        Span::styled("▰".repeat(filled), tone.style(theme)),
        Span::styled("▱".repeat(cells - filled), Tone::Faint.style(theme)),
    ]
}

/// A stage's cell, `cols` wide with its ends: the text centered on its
/// tone's tint, bold while working or failed; a waiting one dim on the
/// neutral tint (`Tone::Dim`).
pub fn cell(text: &str, cols: usize, tone: Tone, caps: bool, theme: &Theme) -> Vec<Span<'static>> {
    let inner = cols.saturating_sub(2);
    let style = if matches!(tone, Tone::Working | Tone::Failed) {
        tone.bold(theme)
    } else {
        tone.style(theme)
    };
    rounded(
        vec![Span::styled(centered(text, inner), style)],
        tone.tint(theme),
        caps,
    )
}

/// The dim note on the row under a cell, centered under it.
pub fn cell_note(text: &str, cols: usize, theme: &Theme) -> Span<'static> {
    Span::styled(centered(text, cols), Tone::Dim.style(theme))
}

/// `text` centered in `cols` columns.
pub fn centered(text: &str, cols: usize) -> String {
    let text = cut(text, cols);
    let left = cols.saturating_sub(width(&text)) / 2;
    let right = cols.saturating_sub(width(&text) + left);
    format!("{}{text}{}", " ".repeat(left), " ".repeat(right))
}

/// A key and what it does. Unlit, the key in the accent bold on the panel
/// between its ends, then what it does dim; lit (what it opens is in
/// front), one pill: the key dark and bold on the accent, then what it
/// does bright and bold on the Chosen surface.
pub fn keycap(key: &str, what: &str, lit: bool, caps: bool, theme: &Theme) -> Vec<Span<'static>> {
    let accent = Tone::Accent.fg(theme);
    if !lit {
        let mut out = rounded(
            vec![Span::styled(key.to_string(), Tone::Accent.bold(theme))],
            Surface::Panel.color(theme),
            caps,
        );
        out.push(Span::styled(format!(" {what} "), Tone::Dim.style(theme)));
        return out;
    }
    let chosen = Surface::Chosen.color(theme);
    let (l, _) = ends(accent, caps);
    let (_, r) = ends(chosen, caps);
    vec![
        l,
        Span::styled(
            key.to_string(),
            Style::new()
                .bg(accent)
                .fg(on_color(theme))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {what}"),
            Style::new()
                .bg(chosen)
                .fg(theme.bright)
                .add_modifier(Modifier::BOLD),
        ),
        r,
        Span::raw(" "),
    ]
}

/// Which row of a two-row bar a name is on: mission control's groups over
/// the views of the one in front.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarRow {
    Groups,
    Views,
}

/// A name in a two-row bar, two columns wider than its text whatever its
/// look, so nothing moves as the one in front changes. The group in front
/// is dark and bold on the accent between its ends; the view in front
/// bold in the accent and underlined on the Chosen surface between its
/// ends; any other dim between spaces. `ring` (Tab's focus, or the mouse
/// over one not in front) draws it bright, bold and reversed.
pub fn bar_name(
    text: &str,
    row: BarRow,
    lit: bool,
    ring: bool,
    caps: bool,
    theme: &Theme,
) -> Vec<Span<'static>> {
    if ring {
        let style = Style::new()
            .fg(theme.bright)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED);
        return vec![Span::styled(format!(" {text} "), style)];
    }
    if !lit {
        return vec![Span::styled(format!(" {text} "), Tone::Dim.style(theme))];
    }
    let (fill, style) = match row {
        BarRow::Groups => (
            accent(theme),
            Style::new()
                .fg(on_color(theme))
                .add_modifier(Modifier::BOLD),
        ),
        BarRow::Views => (
            Surface::Chosen.color(theme),
            Tone::Accent.bold(theme).add_modifier(Modifier::UNDERLINED),
        ),
    };
    rounded(vec![Span::styled(text.to_string(), style)], fill, caps)
}

/// The color of a card's edge: the accent when chosen, `hover` under the
/// mouse, the border's otherwise.
pub fn card_edge(chosen: bool, hover: Option<Color>, theme: &Theme) -> Color {
    if chosen {
        Tone::Accent.fg(theme)
    } else {
        hover.unwrap_or(theme.faint)
    }
}

/// A card, `cols` wide and `rows` tall: a box with arc corners in `edge`,
/// its title on the top edge at the left and `right` at its right, and the
/// caller's lines inside, padded to its width and cut to its height.
pub fn card(
    cols: usize,
    rows: usize,
    title: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    inside: &[Vec<Span<'static>>],
    edge: Color,
) -> Vec<Vec<Span<'static>>> {
    let e = Style::new().fg(edge);
    let inner = cols.saturating_sub(4);
    // `╭ title ─── right ─╮`: five columns and the right's two spaces
    // around the words, so a narrow card keeps its title (a PR's #12345 in
    // a card 14 wide).
    let mut top = vec![Span::styled("╭", e), Span::raw(" ")];
    let right_w = if right.is_empty() {
        0
    } else {
        spans_width(&right) + 2
    };
    top.extend(fit(title, cols.saturating_sub(5 + right_w), Style::new()));
    while top
        .last()
        .is_some_and(|s| s.content.trim().is_empty() && s.style == Style::new())
    {
        top.pop();
    }
    top.push(Span::raw(" "));
    let used = spans_width(&top) + right_w + 2;
    top.push(Span::styled("─".repeat(cols.saturating_sub(used)), e));
    if !right.is_empty() {
        top.push(Span::raw(" "));
        top.extend(right);
        top.push(Span::raw(" "));
    }
    top.push(Span::styled("─╮", e));
    let mut out = vec![fit(top, cols, Style::new())];
    let body = rows.saturating_sub(2);
    for i in 0..body {
        let line = inside.get(i).cloned().unwrap_or_default();
        let mut row = vec![Span::styled("│", e), Span::raw(" ")];
        row.extend(fit(line, inner, Style::new()));
        row.push(Span::raw(" "));
        row.push(Span::styled("│", e));
        out.push(row);
    }
    if rows >= 2 {
        out.push(vec![Span::styled(
            format!("╰{}╯", "─".repeat(cols.saturating_sub(2))),
            e,
        )]);
    }
    out
}

/// A card drawn open, as the canvas draws the Tabs view's (board 3), `cols`
/// wide and `rows` tall: its title and `right` on a rule along its top in
/// the faint color, laid out by `columns`, a bar (`▎`) of its tone down the
/// left of every row under it, and no other edge. The caller's lines sit
/// a space after the bar, `cols - 3` wide, so its right column stays empty.
/// Chosen, the rule and the bar are the accent and every row has the
/// Chosen surface; under the mouse, they are `hover`.
#[allow(clippy::too_many_arguments)]
pub fn open_card(
    cols: usize,
    rows: usize,
    title: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    inside: &[Vec<Span<'static>>],
    tone: Tone,
    chosen: bool,
    hover: Option<Color>,
    theme: &Theme,
) -> Vec<Vec<Span<'static>>> {
    let (rule_color, bar_color) = match (chosen, hover) {
        (true, _) => (Tone::Accent.fg(theme), Tone::Accent.fg(theme)),
        (false, Some(h)) => (h, h),
        (false, None) => (theme.faint, tone.fg(theme)),
    };
    let e = Style::new().fg(rule_color);
    // `─ title ─── right ─`: the words between a column of rule at each
    // end, a space either side of the rule between them.
    let right_w = spans_width(&right);
    let at = columns(
        &[
            Col::Stretch {
                weight: 1,
                min: 1,
                max: None,
            },
            Col::Right {
                natural: right_w,
                cap: right_w,
            },
        ],
        cols.saturating_sub(4),
        2,
        &[],
    );
    let mut top = vec![Span::styled("─", e), Span::raw(" ")];
    if let Some(Some((_, w))) = at.first() {
        top.extend(fit(title, *w, Style::new()));
        while top
            .last()
            .is_some_and(|s| s.content.trim().is_empty() && s.style == Style::new())
        {
            top.pop();
        }
    }
    top.push(Span::raw(" "));
    let fill_to = match at.get(1) {
        Some(Some((x, _))) if right_w > 0 => 2 + x - 1,
        _ => cols,
    };
    let used = spans_width(&top);
    top.push(Span::styled("─".repeat(fill_to.saturating_sub(used)), e));
    if right_w > 0 {
        top.push(Span::raw(" "));
        top.extend(right);
        top.push(Span::raw(" "));
        top.push(Span::styled("─", e));
    }
    let mut out = vec![fit(top, cols, Style::new())];
    let bar = Style::new().fg(bar_color);
    let inner = cols.saturating_sub(3);
    for i in 0..rows.saturating_sub(1) {
        let line = inside.get(i).cloned().unwrap_or_default();
        let mut row = vec![Span::styled("▎", bar), Span::raw(" ")];
        row.extend(fit(line, inner, Style::new()));
        row.push(Span::raw(" "));
        out.push(fit(row, cols, Style::new()));
    }
    if !chosen {
        return out;
    }
    let bg = Surface::Chosen.color(theme);
    out.into_iter()
        .map(|line| {
            line.into_iter()
                .map(|s| {
                    let style = if s.style.bg.is_none() {
                        s.style.bg(bg)
                    } else {
                        s.style
                    };
                    Span::styled(s.content, style)
                })
                .collect()
        })
        .collect()
}

/// How a board's card shows its title: live, a past one's (dim, under the
/// board), or a done one's (faint and struck through).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Title {
    Live,
    Past,
    Struck,
}

/// One card of a board (board 0c, "The kit's board"): on its top edge a
/// mark and a name at the left (already styled) and a short state at the
/// right; in it the title on two lines at most, its fact lines, and where
/// it has stages, a meter of how far they are, in its tone, last.
#[derive(Clone, Debug)]
pub struct BoardCard {
    pub mark: Option<Span<'static>>,
    pub name: Span<'static>,
    pub state: Vec<Span<'static>>,
    pub title: String,
    pub title_look: Title,
    pub facts: Vec<Vec<Span<'static>>>,
    pub meter: Option<(f32, Tone)>,
    pub chosen: bool,
    pub hovered: bool,
}

impl BoardCard {
    /// How many rows it takes `cols` wide: its edges, its title's lines,
    /// its facts and its meter.
    pub fn height(&self, cols: usize) -> usize {
        2 + title_lines(&self.title, cols.saturating_sub(4)).len()
            + self.facts.len()
            + usize::from(self.meter.is_some())
    }

    /// The row of its meter, from its top edge, when it has one.
    pub fn meter_row(&self, cols: usize) -> Option<usize> {
        self.meter.map(|_| self.height(cols) - 2)
    }
}

/// A title wrapped to two lines at most of `cols`, by words, the second
/// ending in … when the rest doesn't fit; one line when it fits on one.
pub fn title_lines(title: &str, cols: usize) -> Vec<String> {
    let wrapped = crate::sidebar::text::wrap(title, cols);
    let first = cut(wrapped.first().map_or("", String::as_str), cols);
    match wrapped.get(1..).filter(|rest| !rest.is_empty()) {
        Some(rest) => vec![first, cut(&rest.join(" "), cols)],
        None => vec![first],
    }
}

/// A board's card, `cols` wide and as tall as what it holds
/// (`BoardCard::height`): the kit's `card`, its edge in the border's color,
/// the accent when chosen and dim under the mouse; its title bold when
/// chosen.
pub fn board_card(card: &BoardCard, cols: usize, theme: &Theme) -> Vec<Vec<Span<'static>>> {
    let inner = cols.saturating_sub(4);
    let edge = if card.chosen {
        Tone::Accent.fg(theme)
    } else if card.hovered {
        theme.dim
    } else {
        border(theme)
    };
    let mut top = Vec::new();
    if let Some(mark) = &card.mark {
        top.push(mark.clone());
        top.push(Span::raw(" "));
    }
    top.push(card.name.clone());
    let title_style = match card.title_look {
        Title::Live if card.chosen => Style::new().fg(theme.bright).add_modifier(Modifier::BOLD),
        Title::Live => Tone::Text.style(theme),
        Title::Past => Tone::Dim.style(theme),
        Title::Struck => Tone::Faint.style(theme).add_modifier(Modifier::CROSSED_OUT),
    };
    let mut inside: Vec<Vec<Span<'static>>> = title_lines(&card.title, inner)
        .into_iter()
        .map(|line| vec![Span::styled(line, title_style)])
        .collect();
    inside.extend(card.facts.iter().cloned());
    if let Some((frac, tone)) = card.meter {
        let used = (frac.clamp(0.0, 1.0) * 100.0).round() as u8;
        inside.push(meter(used, inner, tone, theme));
    }
    self::card(
        cols,
        card.height(cols),
        top,
        card.state.clone(),
        &inside,
        edge,
    )
}

/// A board's column: its name, the tone of where its cards stand, and
/// its cards.
#[derive(Clone, Debug)]
pub struct BoardColumn {
    pub name: String,
    pub tone: Tone,
    pub cards: Vec<BoardCard>,
}

/// Where a card of a board was drawn, from the board's top-left corner:
/// its column and place in it, and its box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    pub column: usize,
    pub card: usize,
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

/// A board drawn: its lines, the cards it shows, and on a screen too
/// narrow for every column, the "more" markers on its top row that lead to
/// a card (each its column and width), which the caller makes a click and
/// a Tab stop.
#[derive(Clone, Debug, Default)]
pub struct Board {
    pub lines: Vec<Vec<Span<'static>>>,
    pub placed: Vec<Placed>,
    pub more_left: Option<(usize, usize)>,
    pub more_right: Option<(usize, usize)>,
}

/// The narrowest a board's column is drawn.
pub const BOARD_COLUMN_MIN: usize = 16;

/// The kit's board (board 0c), `cols` wide and at most `rows` tall: the
/// columns share the width a column apart, never under
/// `BOARD_COLUMN_MIN`; each column's head its name in capitals, bold, in
/// its tone, its count right after it; a blank row, then its cards a row
/// apart, from the one that keeps the chosen card in sight, only whole
/// ones; an empty column a dashed slot reading "nothing here". When the
/// columns can't all keep their width, the chosen card's column and its
/// neighbors, with "‹ 1 more" and "2 more ›" on a row over them, each lit
/// when `lit` says so (under the mouse or Tab's focus); a marker whose
/// hidden columns hold no card, or any marker while none of the board's
/// cards is chosen, is only a count, since `h` and `l` pass over empty
/// columns and move from a chosen card.
pub fn board(
    columns: &[BoardColumn],
    cols: usize,
    rows: usize,
    lit: [bool; 2],
    theme: &Theme,
) -> Board {
    let n = columns.len().max(1);
    let fits = ((cols + 1) / (BOARD_COLUMN_MIN + 1)).clamp(1, n);
    let chosen_at = columns
        .iter()
        .position(|c| c.cards.iter().any(|card| card.chosen));
    let chosen_col = chosen_at.unwrap_or(0);
    let start = chosen_col
        .saturating_sub((fits - 1) / 2)
        .min(n.saturating_sub(fits));
    let shown = &columns[start.min(columns.len())..(start + fits).min(columns.len())];
    let mut out = Board::default();
    let mut lines: Vec<Vec<Span<'static>>> = Vec::new();
    if fits < columns.len() {
        let style = |on: bool| {
            if on {
                Tone::Accent.style(theme).add_modifier(Modifier::REVERSED)
            } else {
                Tone::Dim.style(theme)
            }
        };
        let after = columns.len() - start - shown.len();
        // A marker moves the board only from a chosen card toward another,
        // since h and l pass over empty columns and move from a card: one
        // with none past it, or with no card of the board chosen, is a
        // count alone.
        let holds = |hidden: &[BoardColumn]| {
            chosen_at.is_some() && hidden.iter().any(|c| !c.cards.is_empty())
        };
        let mut row = Vec::new();
        if start > 0 {
            let words = format!("‹ {start} more");
            let moves = holds(&columns[..start]);
            if moves {
                out.more_left = Some((0, width(&words)));
            }
            row.push(Span::styled(words, style(moves && lit[0])));
        }
        if after > 0 {
            let words = format!("{after} more ›");
            let w = width(&words);
            let x = cols.saturating_sub(w);
            let moves = holds(&columns[start + shown.len()..]);
            if moves {
                out.more_right = Some((x, w));
            }
            let used = spans_width(&row);
            row.push(Span::raw(" ".repeat(x.saturating_sub(used))));
            row.push(Span::styled(words, style(moves && lit[1])));
        }
        lines.push(fit(row, cols, Style::new()));
    }
    let top = lines.len();
    let k = shown.len().max(1);
    let col_w = (cols.saturating_sub(k - 1) / k).max(1);
    let card_rows = rows.saturating_sub(top + 2);
    let mut drawn: Vec<Vec<Vec<Span<'static>>>> = Vec::new();
    for (ci, column) in shown.iter().enumerate() {
        let x = ci * (col_w + 1);
        let mut col = vec![
            vec![
                Span::styled(column.name.to_uppercase(), column.tone.bold(theme)),
                Span::raw(" "),
                Span::styled(column.cards.len().to_string(), column.tone.style(theme)),
            ],
            Vec::new(),
        ];
        if column.cards.is_empty() && card_rows >= 3 {
            let e = Style::new().fg(border(theme));
            let inner = col_w.saturating_sub(2);
            col.push(vec![Span::styled(format!("╭{}╮", "┄".repeat(inner)), e)]);
            col.push(vec![
                Span::styled("┆", e),
                Span::styled(centered("nothing here", inner), Tone::Faint.style(theme)),
                Span::styled("┆", e),
            ]);
            col.push(vec![Span::styled(format!("╰{}╯", "┄".repeat(inner)), e)]);
        }
        let heights: Vec<usize> = column.cards.iter().map(|c| c.height(col_w)).collect();
        // From the first card that keeps the chosen one whole in sight.
        let span = |from: usize, to: usize| heights[from..=to].iter().sum::<usize>() + (to - from);
        let mut first = 0;
        if let Some(c) = column.cards.iter().position(|c| c.chosen) {
            while first < c && span(first, c) > card_rows {
                first += 1;
            }
        }
        let mut y = 0;
        for (i, card) in column.cards.iter().enumerate().skip(first) {
            let h = heights[i];
            if y + h > card_rows {
                break;
            }
            out.placed.push(Placed {
                column: start + ci,
                card: i,
                x,
                y: top + 2 + y,
                w: col_w,
                h,
            });
            if y > 0 {
                col.push(Vec::new());
            }
            col.extend(board_card(card, col_w, theme));
            y += h + 1;
        }
        drawn.push(col);
    }
    let tallest = drawn.iter().map(Vec::len).max().unwrap_or(0);
    for i in 0..tallest.min(rows.saturating_sub(top)) {
        let mut row = Vec::new();
        for (ci, col) in drawn.iter().enumerate() {
            if ci > 0 {
                row.push(Span::raw(" "));
            }
            row.extend(fit(
                col.get(i).cloned().unwrap_or_default(),
                col_w,
                Style::new(),
            ));
        }
        lines.push(fit(row, cols, Style::new()));
    }
    out.lines = lines;
    out
}

/// A line in a box: the chosen one with a yellow `›` and its text bright
/// and bold, the others in the text color; its key at `key_col`, in the
/// accent when chosen and dim otherwise. The box fills the chosen line
/// with the Chosen surface.
pub fn option(
    text: &str,
    key: Option<char>,
    key_col: usize,
    chosen: bool,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let mut out = if chosen {
        vec![
            Span::raw(" "),
            Span::styled(
                "›",
                Style::new()
                    .fg(super::draw::YELLOW)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                text.to_string(),
                Style::new().fg(theme.bright).add_modifier(Modifier::BOLD),
            ),
        ]
    } else {
        vec![
            Span::raw("   "),
            Span::styled(text.to_string(), Tone::Text.style(theme)),
        ]
    };
    if let Some(k) = key {
        let pad = key_col.saturating_sub(3 + width(text));
        out.push(Span::raw(" ".repeat(pad)));
        let tone = if chosen { Tone::Accent } else { Tone::Dim };
        out.push(Span::styled(k.to_string(), tone.style(theme)));
    }
    out
}

/// What is chosen beside a list or a board: its name bold, a line of facts
/// (dim, but for a mark among them), a rule under them.
pub fn pane(
    title: &str,
    facts: Vec<Span<'static>>,
    cols: usize,
    theme: &Theme,
) -> Vec<Vec<Span<'static>>> {
    vec![
        fit(
            vec![Span::styled(cut(title, cols), Tone::Text.bold(theme))],
            cols,
            Style::new(),
        ),
        fit(facts, cols, Style::new()),
        vec![rule(cols, theme)],
    ]
}

/// A log's line that says why it failed, `cols` wide on the failed tint:
/// a `▶` at its left, its number (right-aligned `num_w` wide) in the
/// failed tone, its text bold; the other lines of the log beside it put a
/// space where the `▶` is.
pub fn marked(
    number: usize,
    num_w: usize,
    text: &str,
    cols: usize,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let tint = Tone::Failed.tint(theme);
    let line = vec![
        Span::styled("▶", Tone::Failed.bold(theme).bg(tint)),
        Span::styled(
            format!("{number:>num_w$} "),
            Tone::Failed.style(theme).bg(tint),
        ),
        Span::styled(text.to_string(), Tone::Text.bold(theme).bg(tint)),
    ];
    fit(line, cols, Style::new().bg(tint))
}

/// A field, `cols` wide with its ends, on the panel: a `›` in the accent,
/// what is typed (or the placeholder faint after the cursor), a `▏` cursor
/// in the accent, and its hints at the right.
pub fn input(
    text: &str,
    placeholder: &str,
    cols: usize,
    keys: &[(&str, &str)],
    theme: &Theme,
    caps: bool,
) -> Vec<Span<'static>> {
    input_beside(text, placeholder, cols, hints(keys, theme), theme, caps)
}

/// A field as `input` draws it, with `right` at its right in place of
/// key hints (the palette's count of rows matched).
pub fn input_beside(
    text: &str,
    placeholder: &str,
    cols: usize,
    right: Vec<Span<'static>>,
    theme: &Theme,
    caps: bool,
) -> Vec<Span<'static>> {
    let inner = cols.saturating_sub(2);
    let mut typed = vec![
        Span::styled("› ", Tone::Accent.bold(theme)),
        Span::styled(text.to_string(), Tone::Text.style(theme)),
        Span::styled("▏", Tone::Accent.style(theme)),
    ];
    if text.is_empty() {
        typed.push(Span::styled(
            placeholder.to_string(),
            Tone::Faint.style(theme),
        ));
    }
    let room = inner.saturating_sub(spans_width(&right) + 1);
    let mut line = fit(typed, room, Style::new());
    if inner > spans_width(&line) + spans_width(&right) {
        line.extend(right);
        line.push(Span::raw(" "));
    }
    rounded(
        fit(line, inner, Style::new()),
        Surface::Panel.color(theme),
        caps,
    )
}

/// A calendar's event, `cols` wide and `rows` tall (as tall as it lasts on
/// a day, one in a month): its tone's tint with a `▌` in its tone down its
/// left, its title bold, the detail dim on its second row. One that is
/// over is on the panel, dim and struck through; the chosen one on the
/// Chosen surface with the bar in the accent. Square.
#[allow(clippy::too_many_arguments)]
pub fn event(
    title: &str,
    detail: &str,
    tone: Tone,
    cols: usize,
    rows: usize,
    over: bool,
    chosen: bool,
    theme: &Theme,
) -> Vec<Vec<Span<'static>>> {
    let bg = if chosen {
        Surface::Chosen.color(theme)
    } else if over {
        Surface::Panel.color(theme)
    } else {
        tone.tint(theme)
    };
    let bar = if chosen { Tone::Accent } else { tone };
    let mut text = if over {
        Tone::Dim.style(theme).add_modifier(Modifier::CROSSED_OUT)
    } else {
        Tone::Text.bold(theme)
    };
    if chosen && !over {
        text = Style::new().fg(theme.bright).add_modifier(Modifier::BOLD);
    }
    let dim = Tone::Dim.style(theme);
    let inner = cols.saturating_sub(1);
    (0..rows.max(1))
        .map(|i| {
            let words = match i {
                0 => Span::styled(title.to_string(), text),
                1 => Span::styled(detail.to_string(), dim),
                _ => Span::raw(""),
            };
            let mut line = vec![Span::styled("▌", bar.style(theme).bg(bg))];
            line.extend(fit(vec![words], inner, Style::new()).into_iter().map(|s| {
                let style = s.style.bg(bg);
                Span::styled(s.content, style)
            }));
            line
        })
        .collect()
}

/// How large a person's avatar is: `Large` (4 columns by 2 rows) beside
/// what they wrote or did, its name on the first row and their words
/// `SAID_INDENT` columns in; `Small` (2 by 1) in a run of names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvatarSize {
    Small,
    Large,
}

impl AvatarSize {
    pub const fn cols(self) -> usize {
        match self {
            AvatarSize::Small => 2,
            AvatarSize::Large => 4,
        }
    }

    pub const fn rows(self) -> usize {
        match self {
            AvatarSize::Small => 1,
            AvatarSize::Large => 2,
        }
    }
}

/// Where a person's name and words start beside a Large avatar: its four
/// columns and a space.
pub const SAID_INDENT: usize = AvatarSize::Large.cols() + 1;

/// A person's color for their initial, from their name, the same whatever
/// service the person is from.
pub fn avatar_color(name: &str) -> u8 {
    const COLORS: [u8; 8] = [67, 96, 66, 137, 61, 101, 72, 131];
    let h = name.bytes().fold(2_166_136_261u32, |h, b| {
        (h ^ b as u32).wrapping_mul(16_777_619)
    });
    COLORS[h as usize % COLORS.len()]
}

/// Someone's avatar as text, a row a line, `size` wide (board 0d, "The
/// kit: a person"): their initial, bold, on their color (`avatar_color`)
/// across the first row's two columns, the rest blank. It is what shows
/// under the photo (`ui::images::avatar`, a view's picture) and in its
/// place until the photo is had, or wherever it can't be drawn, so the
/// text beside it never moves when photos arrive.
pub fn avatar(name: &str, size: AvatarSize) -> Vec<Vec<Span<'static>>> {
    let mut rows = vec![initial(name, Color::Indexed(avatar_color(name)))];
    rows[0].push(Span::raw(" ".repeat(size.cols() - 2)));
    rows.extend((1..size.rows()).map(|_| vec![Span::raw(" ".repeat(size.cols()))]));
    rows
}

/// A person's initial on `color`, two columns wide: dark on a light
/// color and white on a dark one.
pub fn initial(name: &str, color: Color) -> Vec<Span<'static>> {
    let initial: String = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .filter(|i: &String| width(i) <= 2)
        .unwrap_or_else(|| "?".into());
    let [r, g, b] = super::images::rgb(color);
    let lum = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    let fg = if lum > 140.0 {
        Color::Indexed(232)
    } else {
        Color::Indexed(255)
    };
    let fill = Style::new().bg(color);
    vec![
        Span::styled(initial.clone(), fill.fg(fg).add_modifier(Modifier::BOLD)),
        Span::styled(" ".repeat(2 - width(&initial)), fill),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    const T: Theme = Theme::DARK;

    fn fixed(natural: usize, cap: usize) -> Col {
        Col::Fixed {
            min: 0,
            natural,
            cap,
        }
    }

    fn stretch(weight: usize, min: usize, max: Option<usize>) -> Col {
        Col::Stretch { weight, min, max }
    }

    #[test]
    fn the_tones_are_the_canvas_s_in_truecolor_and_indexed_otherwise() {
        let tones = [
            Tone::Accent,
            Tone::Needs,
            Tone::Working,
            Tone::Landing,
            Tone::Done,
            Tone::Failed,
        ];
        for tone in tones {
            for theme in [Theme::DARK, Theme::LIGHT] {
                assert!(matches!(tone.fg(&theme), Color::Indexed(_)), "{tone:?}");
                assert!(matches!(tone.tint(&theme), Color::Indexed(_)), "{tone:?}");
            }
            assert!(
                matches!(tone.fg(&Theme::DARK_TRUE), Color::Rgb(..)),
                "{tone:?}"
            );
            assert!(
                matches!(tone.tint(&Theme::DARK_TRUE), Color::Rgb(..)),
                "{tone:?}"
            );
        }
        let canvas = Theme::DARK_TRUE;
        assert_eq!(Tone::Needs.fg(&canvas), Color::Rgb(0xf2, 0xc5, 0x6b));
        assert_eq!(Tone::Accent.fg(&canvas), Color::Rgb(0x5f, 0xd7, 0xff));
        assert_eq!(Tone::Done.tint(&canvas), Color::Rgb(0x1d, 0x3a, 0x2a));
        assert_eq!(Surface::Raised.color(&canvas), Color::Rgb(0x1f, 0x22, 0x32));
        assert_eq!(border(&canvas), Color::Rgb(0x3a, 0x3f, 0x52));
        assert_eq!(Surface::Base.color(&canvas), Color::Reset);
    }

    #[test]
    fn fixed_columns_take_their_natural_width_up_to_their_cap() {
        let cols = [
            fixed(10, 16),
            fixed(30, 16),
            Col::Fixed {
                min: 8,
                natural: 3,
                cap: 16,
            },
        ];
        assert_eq!(
            columns(&cols, 200, 1, &[]),
            vec![Some((0, 10)), Some((11, 16)), Some((28, 8))]
        );
    }

    #[test]
    fn stretching_columns_share_what_is_left_by_weight() {
        let cols = [stretch(2, 0, None), stretch(1, 0, None)];
        assert_eq!(
            columns(&cols, 90, 0, &[]),
            vec![Some((0, 60)), Some((60, 30))]
        );
    }

    #[test]
    fn what_rounding_leaves_goes_to_columns_that_can_still_grow() {
        let cols = [
            stretch(1, 0, None),
            stretch(1, 0, None),
            stretch(1, 0, Some(53)),
        ];
        assert_eq!(
            columns(&cols, 160, 1, &[]),
            vec![Some((0, 52)), Some((53, 53)), Some((107, 53))]
        );
    }

    #[test]
    fn a_right_edge_column_sits_flush_right_at_any_width() {
        let cols = [
            fixed(16, 16),
            stretch(1, 10, None),
            Col::Right {
                natural: 8,
                cap: 20,
            },
        ];
        for width in [120, 160, 240] {
            let out = columns(&cols, width, 2, &[]);
            assert_eq!(out[2], Some((width - 8, 8)));
            assert_eq!(out[1], Some((18, width - 16 - 8 - 4)));
        }
    }

    #[test]
    fn a_wrapped_text_column_stops_at_100_and_the_rest_goes_to_another() {
        let cols = [stretch(3, 0, Some(WRAP_MAX)), stretch(1, 0, None)];
        assert_eq!(
            columns(&cols, 240, 0, &[]),
            vec![Some((0, 100)), Some((100, 140))]
        );
        assert_eq!(
            columns(&cols, 80, 0, &[]),
            vec![Some((0, 60)), Some((60, 20))]
        );
    }

    #[test]
    fn columns_are_dropped_in_the_given_order_until_the_rest_fits() {
        let cols = [
            fixed(16, 16),
            fixed(34, 34),
            stretch(1, 24, None),
            fixed(20, 20),
        ];
        let drop = [3, 1, 0];
        assert_eq!(
            columns(&cols, 100, 1, &drop),
            vec![
                Some((0, 16)),
                Some((17, 34)),
                Some((52, 27)),
                Some((80, 20))
            ]
        );
        assert_eq!(
            columns(&cols, 80, 1, &drop),
            vec![Some((0, 16)), Some((17, 34)), Some((52, 28)), None]
        );
        assert_eq!(
            columns(&cols, 50, 1, &drop),
            vec![Some((0, 16)), None, Some((17, 33)), None]
        );
    }

    #[test]
    fn a_pane_beside_scales_with_the_screen_and_never_goes_under_its_least() {
        assert_eq!(beside(160, 50, 34, 60), 50);
        assert_eq!(beside(240, 50, 34, 60), 75);
        assert_eq!(beside(100, 50, 34, 60), 34);
        assert_eq!(beside(90, 50, 34, 60), 0);
    }

    #[test]
    fn natural_is_the_widest_text_and_place_cuts_to_the_column() {
        assert_eq!(natural(["ab", "日本語", "abcd"]), 6);
        let mut line = vec![Span::raw("ab")];
        place(&mut line, vec![Span::raw("hello world")], (4, 6));
        assert_eq!(text(&line), "ab  hello…");
        place(&mut line, vec![Span::raw("x")], (12, 3));
        assert_eq!(text(&line), "ab  hello…  x  ");
    }

    #[test]
    fn a_marked_line_fills_its_width_in_the_failed_tint() {
        let line = marked(42, 4, "FAIL a test", 30, &T);
        assert_eq!(text(&line), format!("▶  42 FAIL a test{}", " ".repeat(13)));
        assert_eq!(spans_width(&line), 30);
        let tint = Tone::Failed.tint(&T);
        assert!(line.iter().all(|s| s.style.bg == Some(tint)), "{line:?}");
        assert!(
            line[2].style.add_modifier.contains(Modifier::BOLD),
            "its text bold"
        );
    }

    #[test]
    fn a_chip_keeps_its_width_with_and_without_caps() {
        let with = chip("3 failing", Tone::Failed, true, &T);
        let without = chip("3 failing", Tone::Failed, false, &T);
        assert_eq!(text(&with), "\u{E0B6}3 failing\u{E0B4} ");
        assert_eq!(text(&without), " 3 failing  ");
        assert_eq!(spans_width(&with), spans_width(&without));
        assert_eq!(with[0].style.fg, Some(Tone::Failed.tint(&T)));
        assert_eq!(without[0].style.bg, Some(Tone::Failed.tint(&T)));
        assert_eq!(with[1].style.bg, Some(Color::Indexed(52)));
    }

    #[test]
    fn caps_follow_the_terminal() {
        assert!(caps("xterm-ghostty"));
        assert!(!caps("xterm-256color"));
        assert!(!caps(""));
    }

    #[test]
    fn a_cell_centers_its_text_in_its_width() {
        let c = cell("review", 12, Tone::Working, false, &T);
        assert_eq!(text(&c), "   review   ");
        assert!(c[1].style.add_modifier.contains(Modifier::BOLD));
        let w = cell("wait", 12, Tone::Dim, true, &T);
        assert_eq!(spans_width(&w), 12);
        assert_eq!(w[1].style.bg, Some(Color::Indexed(237)));
    }

    #[test]
    fn a_card_has_arc_corners_and_its_title_on_the_edge() {
        let lines = card(
            20,
            4,
            vec![Span::raw("● "), Span::raw("api")],
            vec![Span::raw("idle")],
            &[vec![Span::raw("hello")]],
            border(&T),
        );
        assert_eq!(lines.len(), 4);
        let top = text(&lines[0]);
        assert!(top.starts_with("╭ ● api "), "{top:?}");
        assert!(top.ends_with(" idle ─╮"), "{top:?}");
        assert_eq!(width(&top), 20);
        assert_eq!(text(&lines[1]), "│ hello            │");
        assert_eq!(text(&lines[3]), format!("╰{}╯", "─".repeat(18)));
        for l in &lines {
            assert_eq!(spans_width(l), 20);
        }
        // A card 14 wide keeps a PR's number whole beside its checks' mark.
        let narrow = card(
            14,
            4,
            vec![Span::raw("#12345")],
            vec![Span::raw("✓")],
            &[],
            border(&T),
        );
        assert_eq!(text(&narrow[0]), "╭ #12345  ✓ ─╮");
    }

    #[test]
    fn an_open_card_has_a_top_rule_and_its_tone_s_bar_and_no_other_edge() {
        let draw = |chosen: bool, hover: Option<Color>| {
            open_card(
                20,
                4,
                vec![Span::raw("● "), Span::raw("api")],
                vec![Span::raw("idle")],
                &[vec![Span::raw("hello")]],
                Tone::Working,
                chosen,
                hover,
                &T,
            )
        };
        let lines = draw(false, None);
        assert_eq!(lines.len(), 4);
        let top = text(&lines[0]);
        assert_eq!(top, "─ ● api ───── idle ─", "{top:?}");
        assert_eq!(lines[0][0].style.fg, Some(T.faint));
        assert_eq!(text(&lines[1]), format!("▎ hello{}", " ".repeat(13)));
        for l in &lines[1..] {
            assert_eq!(l[0].content, "▎");
            assert_eq!(l[0].style.fg, Some(Tone::Working.fg(&T)));
            assert!(text(l).ends_with(' '), "nothing in its right column");
        }
        for l in &lines {
            assert_eq!(spans_width(l), 20);
            assert!(!text(l).contains(['╭', '╮', '╰', '╯', '│']));
        }
        // Chosen: the rule and the bar in the accent, on the Chosen surface.
        let chosen = draw(true, None);
        assert_eq!(chosen[0][0].style.fg, Some(Tone::Accent.fg(&T)));
        assert_eq!(chosen[2][0].style.fg, Some(Tone::Accent.fg(&T)));
        assert!(
            chosen
                .iter()
                .flatten()
                .all(|s| s.style.bg == Some(Surface::Chosen.color(&T)))
        );
        // Under the mouse, both in the hover color.
        let lit = draw(false, Some(T.text));
        assert_eq!(lit[0][0].style.fg, Some(T.text));
        assert_eq!(lit[3][0].style.fg, Some(T.text));
        // A narrow card keeps a PR's number whole beside its checks' mark.
        let narrow = open_card(
            14,
            2,
            vec![Span::raw("#12345")],
            vec![Span::raw("✓")],
            &[],
            Tone::Dim,
            false,
            None,
            &T,
        );
        assert_eq!(text(&narrow[0]), "─ #12345 ─ ✓ ─");
    }

    fn board_card_of(title: &str, chosen: bool) -> BoardCard {
        BoardCard {
            mark: None,
            name: Span::raw("#1"),
            state: vec![Span::raw("✓")],
            title: title.to_string(),
            title_look: Title::Live,
            facts: vec![vec![Span::raw("w")]],
            meter: None,
            chosen,
            hovered: false,
        }
    }

    fn column(name: &str, cards: Vec<BoardCard>) -> BoardColumn {
        BoardColumn {
            name: name.to_string(),
            tone: Tone::Working,
            cards,
        }
    }

    #[test]
    fn a_board_card_is_as_tall_as_what_it_holds() {
        let long = board_card(&board_card_of("lays the settings view out", false), 18, &T);
        let short = board_card(&board_card_of("Fix it", false), 18, &T);
        assert_eq!(long.len(), 5);
        assert_eq!(short.len(), 4, "a row shorter for a one-line title");
        assert_eq!(text(&short[0]), "╭ #1 ──────── ✓ ─╮");
        assert_eq!(text(&short[1]), "│ Fix it         │");
        assert_eq!(text(&short[2]), "│ w              │");
        assert!(text(&short[3]).starts_with('╰'), "no footer");
        assert_eq!(short[0][0].style.fg, Some(border(&T)));
        let chosen = board_card(&board_card_of("Fix it", true), 18, &T);
        assert_eq!(chosen[0][0].style.fg, Some(Tone::Accent.fg(&T)));
        let title = chosen[1]
            .iter()
            .find(|s| s.content.contains("Fix"))
            .unwrap();
        assert!(title.style.add_modifier.contains(Modifier::BOLD));
        let mut hovered = board_card_of("Fix it", false);
        hovered.hovered = true;
        assert_eq!(board_card(&hovered, 18, &T)[0][0].style.fg, Some(T.dim));
        // A meter is its last line, in its tone.
        let mut stages = board_card_of("Fix it", false);
        stages.meter = Some((0.5, Tone::Working));
        let drawn = board_card(&stages, 18, &T);
        assert_eq!(stages.meter_row(18), Some(3));
        assert!(text(&drawn[3]).contains('▰'), "{:?}", text(&drawn[3]));
        assert!(text(&drawn[4]).starts_with('╰'));
    }

    #[test]
    fn a_board_shares_its_width_a_column_apart_and_counts_each_column() {
        let columns = vec![
            column("Next", vec![board_card_of("Fix it", false)]),
            column(
                "In review",
                vec![board_card_of("Fix it", true), board_card_of("Again", false)],
            ),
            column("Done", Vec::new()),
        ];
        let b = board(&columns, 62, 20, [false; 2], &T);
        let rows: Vec<String> = b.lines.iter().map(|l| text(l)).collect();
        assert!(rows[0].starts_with("NEXT 1"), "{rows:#?}");
        // Three columns of 20 a column apart: 0, 21 and 42.
        assert_eq!(rows[0].find("IN REVIEW 2"), Some(21), "{rows:#?}");
        assert_eq!(rows[0].find("DONE 0"), Some(42), "{rows:#?}");
        assert_eq!(rows[1], " ".repeat(62));
        assert_eq!(rows[2].find("╭ #1"), Some(0));
        assert_eq!(rows[2].chars().nth(21), Some('╭'));
        assert!(rows[3].contains("nothing here"), "{rows:#?}");
        assert!(rows[2].contains('┄'), "the empty column's dashed slot");
        // The second card a row under the first.
        assert_eq!(rows[7].chars().nth(21), Some('╭'), "{rows:#?}");
        assert_eq!(
            b.placed,
            vec![
                Placed {
                    column: 0,
                    card: 0,
                    x: 0,
                    y: 2,
                    w: 20,
                    h: 4
                },
                Placed {
                    column: 1,
                    card: 0,
                    x: 21,
                    y: 2,
                    w: 20,
                    h: 4
                },
                Placed {
                    column: 1,
                    card: 1,
                    x: 21,
                    y: 7,
                    w: 20,
                    h: 4
                },
            ]
        );
        assert!(b.more_left.is_none() && b.more_right.is_none());
        // Short: the cards that keep the chosen one whole in sight.
        let mut later = columns.clone();
        later[1].cards[0].chosen = false;
        later[1].cards[1].chosen = true;
        let b = board(&later, 62, 7, [false; 2], &T);
        assert_eq!(
            b.placed[1],
            Placed {
                column: 1,
                card: 1,
                x: 21,
                y: 2,
                w: 20,
                h: 4
            }
        );
    }

    #[test]
    fn a_narrow_board_shows_the_chosen_column_and_its_neighbors() {
        let columns: Vec<BoardColumn> = ["A", "B", "C", "D", "E"]
            .iter()
            .enumerate()
            .map(|(i, n)| column(n, vec![board_card_of("Fix it", i == 2)]))
            .collect();
        // Under three times 16 and the gaps: two columns.
        let b = board(&columns, 40, 20, [false, true], &T);
        let rows: Vec<String> = b.lines.iter().map(|l| text(l)).collect();
        assert!(rows[0].starts_with("‹ 2 more"), "{rows:#?}");
        assert!(rows[0].ends_with("1 more ›"), "{rows:#?}");
        assert!(
            rows[1].starts_with("C 1") && rows[1].contains("D 1"),
            "{rows:#?}"
        );
        assert_eq!(b.more_left, Some((0, 8)));
        assert_eq!(b.more_right, Some((32, 8)));
        assert!(
            b.lines[0]
                .last()
                .unwrap()
                .style
                .add_modifier
                .contains(Modifier::REVERSED)
        );
        assert!(b.placed.iter().any(|p| p.column == 2 && p.y == 3));
        // Hidden columns with no card: counted, but nothing to move to.
        let mut empty = columns.clone();
        empty[3].cards.clear();
        empty[4].cards.clear();
        let b = board(&empty, 40, 20, [false; 2], &T);
        assert!(text(&b.lines[0]).ends_with("1 more ›"));
        assert!(b.more_right.is_none() && b.more_left.is_some());
        // None of its cards chosen (the cursor in a row under the board):
        // counts alone.
        let mut none = columns.clone();
        none[2].cards[0].chosen = false;
        let b = board(&none, 40, 20, [false; 2], &T);
        assert!(text(&b.lines[0]).ends_with("more ›"));
        assert!(b.more_left.is_none() && b.more_right.is_none());
        // Wide enough for all: no markers.
        let b = board(&columns, 5 * 17 - 1, 20, [false; 2], &T);
        assert!(b.more_left.is_none() && b.more_right.is_none());
        assert!(text(&b.lines[0]).starts_with("A 1"));
    }

    #[test]
    fn hints_light_only_the_keys() {
        let h = hints(&[("jk", "move"), ("esc", "close")], &T);
        assert_eq!(text(&h), "jk move  esc close");
        assert_eq!(h[0].style.fg, Some(Color::Indexed(81)));
        assert_eq!(h[1].style.fg, Some(T.dim));
        assert_eq!(h[3].style.fg, Some(Color::Indexed(81)));
    }

    #[test]
    fn a_chosen_row_fills_the_full_width_with_a_bar() {
        let r = row(vec![Span::raw("api")], 10, true, &T);
        assert_eq!(text(&r), "▌api      ");
        assert!(r.iter().all(|s| s.style.bg == Some(T.highlight)));
        let plain = row(vec![Span::raw("api")], 10, false, &T);
        assert_eq!(text(&plain), " api      ");
        assert!(plain.iter().all(|s| s.style.bg.is_none()));
    }

    #[test]
    fn a_label_capitalizes_and_right_aligns_its_count() {
        let l = label("Needs you", Tone::Needs, Some(3), 20, &T);
        assert_eq!(text(&l), " NEEDS YOU        3 ");
        assert!(l[1].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(l[1].style.fg, Some(Color::Indexed(220)));
    }

    #[test]
    fn each_state_has_one_mark() {
        let marks = [
            Mark::Needs,
            Mark::Working,
            Mark::Landing,
            Mark::Idle,
            Mark::Failed,
            Mark::Passed,
            Mark::Pending,
            Mark::Reminder,
            Mark::Mention,
            Mark::Note,
            Mark::Comment,
            Mark::Awake,
            Mark::Selected,
        ];
        let glyphs: Vec<&str> = marks.iter().map(|m| m.look().0).collect();
        let mut unique = glyphs.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), glyphs.len());
        assert_eq!(mark(Mark::Needs, 0, &T).content, "●");
        assert_eq!(mark(Mark::Working, 1, &T).content, SPINNER[1]);
        assert_eq!(Mark::of_badge(Badge::Waiting), Mark::Needs);
        assert_eq!(Mark::of_badge(Badge::Review), Mark::Working);
        assert_eq!(Mark::of_badge(Badge::Merged), Mark::Passed);
        assert!(spins(Badge::Scheduled, true) && !spins(Badge::Scheduled, false));
        assert!(spins(Badge::Working, false) && !spins(Badge::Idle, true));
    }

    #[test]
    fn a_keycap_lit_and_unlit_keeps_its_ends() {
        let unlit = keycap("M-d", "Diff", false, false, &T);
        assert_eq!(text(&unlit), " M-d  Diff ");
        let lit = keycap("M-d", "Diff", true, true, &T);
        assert_eq!(text(&lit), "\u{E0B6}M-d Diff\u{E0B4} ");
        assert_eq!(spans_width(&lit), spans_width(&unlit));
        assert_eq!(lit[1].style.bg, Some(Color::Indexed(81)));
    }

    #[test]
    fn an_option_points_at_the_chosen_one() {
        assert_eq!(
            text(&option("Open", Some('o'), 10, true, &T)),
            " › Open   o"
        );
        assert_eq!(text(&option("Open", None, 10, false, &T)), "   Open");
    }

    #[test]
    fn an_input_keeps_its_width() {
        let i = input("", "Search", 30, &[("esc", "close")], &T, false);
        assert_eq!(spans_width(&i), 30);
        assert!(text(&i).contains("› ▏Search"));
        assert!(text(&i).ends_with("esc close  "));
    }

    #[test]
    fn an_event_is_square_with_its_bar() {
        let e = event("Standup", "10:00", Tone::Working, 12, 3, false, false, &T);
        assert_eq!(e.len(), 3);
        assert_eq!(text(&e[0]), "▌Standup    ");
        assert_eq!(text(&e[1]), "▌10:00      ");
        let over = event("Standup", "", Tone::Working, 12, 1, true, false, &T);
        assert!(
            over[0][1]
                .style
                .add_modifier
                .contains(Modifier::CROSSED_OUT)
        );
    }

    #[test]
    fn a_pane_has_its_rule() {
        let p = pane("api", vec![Span::raw("main · 2m")], 10, &T);
        assert_eq!(text(&p[0]), "api       ");
        assert_eq!(text(&p[1]), "main · 2m ");
        assert_eq!(text(&p[2]), "─".repeat(10));
    }

    #[test]
    fn a_large_avatar_is_four_by_two_and_a_small_one_two_by_one() {
        let large = avatar("ana", AvatarSize::Large);
        assert_eq!(large.len(), 2);
        assert!(large.iter().all(|r| spans_width(r) == 4));
        assert_eq!(text(&large[0]), "A   ");
        assert_eq!(text(&large[1]), "    ");
        let color = Color::Indexed(avatar_color("ana"));
        assert_eq!(large[0][0].style.bg, Some(color));
        assert!(large[0][0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(large[1][0].style.bg, None);
        let small = avatar("ana", AvatarSize::Small);
        assert_eq!(small.len(), 1);
        assert_eq!(text(&small[0]), "A ");
        assert_eq!(text(&avatar("", AvatarSize::Small)[0]), "? ");
        assert_eq!(SAID_INDENT, 5);
    }
}
