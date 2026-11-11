//! UX 2.0's kit of atoms: the pieces of text every part draws with in
//! place of its own, as board 0 of the UX 2.0 canvas ("The kit") draws
//! them. New with UX 2.0, with no tmux-version file behind it.
//!
//! Tones (`Tone`) and surfaces (`Surface`) are the colors, one mark per
//! state (`mark`), and the atoms: a section's `label`, the `rule`s, a
//! list's `row`, a `chip`, a stage's `cell`, a `keycap`, a `card`, a box's
//! `option`, the `pane` beside a list, an `input`, a calendar's `event`,
//! a two-row bar's `bar_name`, and the key `hints` of every footer and bar
//! (the key in the accent, what it does dim, two spaces between).
//!
//! Objects are rounded and structure is square: a chip, a cell, a keycap
//! and an input end in half circles (U+E0B6 and U+E0B4) where the terminal
//! draws them itself (Ghostty, `caps`), and in a space of their fill
//! elsewhere, which the terminal would take from a font that may not have
//! them; either way the object keeps its width, so nothing moves. Cards
//! and boxes have arc corners; rules, bars, chosen rows, tables and the
//! calendar's events stay square.
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

fn ix(dark: u8, lightc: u8, theme: &Theme) -> Color {
    Color::Indexed(if light(theme) { lightc } else { dark })
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
            Tone::Accent => ACCENT,
            Tone::Needs => ix(220, 136, theme),
            Tone::Working => ix(111, 25, theme),
            Tone::Landing => ix(179, 130, theme),
            Tone::Done => ix(114, 28, theme),
            Tone::Failed => ix(160, 160, theme),
        }
    }

    /// Its tint: a background in its hue, for chips and cells; the
    /// neutral one for the greys.
    pub fn tint(self, theme: &Theme) -> Color {
        match self {
            Tone::Text | Tone::Dim | Tone::Faint | Tone::Accent => ix(237, 254, theme),
            Tone::Needs => ix(58, 230, theme),
            Tone::Working => ix(24, 153, theme),
            Tone::Landing => ix(94, 223, theme),
            Tone::Done => ix(22, 194, theme),
            Tone::Failed => ix(52, 224, theme),
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
            Surface::Sunken => ix(234, 255, theme),
            Surface::Panel => ix(235, 254, theme),
            Surface::Raised => ix(236, 253, theme),
            Surface::Chosen => theme.highlight,
        }
    }
}

/// The color of the lines between regions.
pub fn border(theme: &Theme) -> Color {
    ix(238, 248, theme)
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

fn spans_width(spans: &[Span<'_>]) -> usize {
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
                .fg(Color::Indexed(232))
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
            ACCENT,
            Style::new()
                .fg(Color::Indexed(232))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn text(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    const T: Theme = Theme::DARK;

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
}
