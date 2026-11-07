//! Mission control's screen (lib/sidebar/render.js's `missionLines`,
//! `missionGrid`, `card` and `boardLines`): a plain function from what a
//! client's mission control holds to the lines to draw, each exactly the
//! screen's width, and where a click on each part lands, so the tests can
//! read it as text.
//!
//! At the top a bar naming the views (the tabs, then each view, with how
//! many items each has), the counts of tabs waiting, finished and working,
//! and the filter; at the bottom the keys, or what the last action did. In
//! between, the view in front:
//!
//! - the tabs as cards in stages (`grid`): each row of the screen a shelf
//!   of stage boxes side by side, a stage that doesn't fit in the room left
//!   starting the next shelf, each card the tab's badge, number and name in
//!   its top edge, what it is doing, and its latest turn: your prompt, then
//!   the end of what Claude said or the tool call it is stopped on;
//! - a board (the PR and ticket boards): a column for each place, side by
//!   side, each item two lines;
//! - a table (Resources): its columns' names, its rows, and a totals row;
//! - any other view, one row per item, a heading over each group, Settings'
//!   rows as name, value and what the option does.

use std::sync::LazyLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use regex::Regex;

use super::views::{self, View, ViewId};
use super::{Focus, HitBox, Mission, Spot};
use crate::items::{Item, State};
use crate::sidebar::model::{self, Rules, TabModel, TabState};
use crate::sidebar::render::Line;
use crate::sidebar::text::{self, Badge, SPINNER, width};
use crate::ui::Theme;

/// A box's border takes a column each side.
const BORDER: usize = 2;
/// The edge of mission control's boxes and its name.
const ACCENT: Color = Color::Indexed(81);
const YELLOW: Color = Color::Indexed(220);
/// An error, or why a view has nothing.
const RED: Color = Color::Indexed(167);
/// Easy mode's close button in the bar.
const CLOSE: &str = " × ";

fn fg(c: Color) -> Style {
    Style::new().fg(c)
}

fn bold(c: Color) -> Style {
    fg(c).add_modifier(Modifier::BOLD)
}

/// Spans being put together into a line.
#[derive(Default)]
struct Row {
    spans: Vec<Span<'static>>,
}

impl Row {
    fn put(&mut self, text: impl Into<String>, style: Style) -> &mut Row {
        let text = text.into();
        if !text.is_empty() {
            self.spans.push(Span::styled(text, style));
        }
        self
    }

    fn width(&self) -> usize {
        spans_width(&self.spans)
    }

    /// The spans cut to `cols` (an ellipsis in place of what didn't fit) and
    /// padded with spaces in `pad` to fill them.
    fn fitted(self, cols: usize, pad: Style) -> Vec<Span<'static>> {
        fit(self.spans, cols, pad)
    }

    fn line(self, cols: usize, fill: Option<Color>) -> Line {
        Line {
            spans: self.fitted(cols, Style::new()),
            fill,
        }
    }
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| width(&s.content)).sum()
}

/// Spans cut to `cols` columns and padded to fill them.
fn fit(spans: Vec<Span<'static>>, cols: usize, pad: Style) -> Vec<Span<'static>> {
    let total = spans_width(&spans);
    let mut out = Vec::new();
    let mut used = 0;
    if total <= cols {
        out = spans;
        used = total;
    } else if cols > 0 {
        for span in spans {
            let w = width(&span.content);
            if used + w < cols {
                used += w;
                out.push(span);
                continue;
            }
            let mut kept = String::new();
            let mut before = 0;
            for c in span.content.chars() {
                let cw = text::width_after(c, before);
                if used + cw > cols - 1 {
                    break;
                }
                kept.push(c);
                used += cw;
                before = cw;
            }
            kept.push('…');
            used += 1;
            out.push(Span::styled(kept, span.style));
            break;
        }
    }
    if used < cols {
        out.push(Span::styled(" ".repeat(cols - used), pad));
    }
    out
}

/// A badge: a colored pill three columns wide, the spinner turning while
/// Claude works; an idle tab's mark faint, with no pill (the sidebar's).
fn badge(badge: Badge, frame: usize, theme: &Theme) -> Span<'static> {
    let look = badge.look();
    let Some(color) = look.color else {
        return Span::styled(format!(" {} ", look.mark), fg(theme.faint));
    };
    let mark = if badge.spins() {
        SPINNER[frame % SPINNER.len()]
    } else {
        look.mark
    };
    let after = if width(mark) > 1 { "" } else { " " };
    Span::styled(
        format!(" {mark}{after}"),
        Style::new()
            .bg(Color::Indexed(color))
            .fg(Color::Indexed(look.ink.unwrap_or(255)))
            .add_modifier(Modifier::BOLD),
    )
}

/// An item's mark: needing you yellow, under way blue, the rest faint.
fn item_mark(item: &Item, theme: &Theme) -> Span<'static> {
    match item.state {
        State::Attention => Span::styled("●", bold(YELLOW)),
        State::Active => Span::styled("●", bold(Color::Indexed(67))),
        State::Idle => Span::styled("○", fg(theme.faint)),
    }
}

/// The counts beside the views: failing builds, then how many wait on you,
/// have finished and are working (a loop's too).
fn tally(tabs: &[TabModel], r: &Rules, theme: &Theme) -> Vec<Span<'static>> {
    let mut items: Vec<(String, Style)> = Vec::new();
    let failing = tabs.iter().filter(|t| model::build_failing(t, r)).count();
    if failing > 0 {
        items.push((
            format!("{} {failing}", Badge::Failing.look().mark),
            bold(Color::Indexed(160)),
        ));
    }
    for (b, n) in [
        (
            Badge::Waiting,
            tabs.iter().filter(|t| t.state == TabState::Waiting).count(),
        ),
        (
            Badge::Done,
            tabs.iter().filter(|t| t.state == TabState::Done).count(),
        ),
        (
            Badge::Working,
            tabs.iter().filter(|t| t.state.busy()).count(),
        ),
    ] {
        if n == 0 {
            continue;
        }
        let mark = if b == Badge::Working {
            "⋯"
        } else {
            b.look().mark
        };
        items.push((format!("{mark} {n}"), bold(b.color().unwrap_or(theme.text))));
    }
    let mut out = Vec::new();
    for (i, (t, style)) in items.into_iter().enumerate() {
        if i > 0 {
            out.push(Span::styled("  ", fg(theme.faint)));
        }
        out.push(Span::styled(t, style));
    }
    out
}

// ── The grid of cards ───────────────────────────────────────────────────────

/// A stage's box on a shelf: its stage, its cards (places in `Grid::order`),
/// how many across and how many rows of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub stage: model::Stage,
    pub count: usize,
    pub cards: Vec<usize>,
    pub across: usize,
    pub rows: usize,
}

/// A row of stage boxes, as tall as its tallest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shelf {
    pub rows: usize,
    pub blocks: Vec<Block>,
}

/// The tabs laid out in stages: their order (indices into the tabs given),
/// the shelves, and the rows of cards the keys move through, each a
/// shelf's row, every box taking part in each, a shorter box's card
/// spanning the rows its height covers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grid {
    pub order: Vec<usize>,
    pub shelves: Vec<Shelf>,
    pub rows: Vec<Vec<usize>>,
}

/// A card's width: a row of `across` cards fits even when every card is in
/// a box, and border, of its own.
pub fn card_width(cols: usize, config: (u16, u16)) -> usize {
    let across = (cols / (config.0.max(1) as usize)).max(1);
    (cols / across).saturating_sub(BORDER).max(20)
}

/// Mission control's tabs laid out in stages, packed: each row of the screen
/// a shelf of stage boxes side by side. A box takes its cards' width and two
/// columns of border, and a shelf is `cols` wide. A stage that fits in the
/// room left on the shelf goes there, in one row of cards; one that doesn't
/// starts the next shelf, as wide as it needs up to the shelf, in as many
/// rows of cards as it takes.
pub fn grid(tabs: &[TabModel], r: &Rules, cols: usize, card: usize) -> Grid {
    let mut order = Vec::new();
    let mut shelves: Vec<Shelf> = Vec::new();
    let card = card.max(1);
    let fit = (cols.saturating_sub(BORDER) / card).max(1);
    let mut used = 0;
    for staged in model::staged(tabs, r) {
        let n = staged.tabs.len();
        let room = if shelves.is_empty() {
            0
        } else {
            cols.saturating_sub(used + BORDER) / card
        };
        if shelves.is_empty() || room < n {
            shelves.push(Shelf {
                rows: 0,
                blocks: Vec::new(),
            });
            used = 0;
        }
        let across = n.min(fit);
        let rows = n.div_ceil(across);
        let cards: Vec<usize> = staged
            .tabs
            .iter()
            .map(|t| {
                let at = tabs.iter().position(|x| x.id == t.id).unwrap_or(0);
                order.push(at);
                order.len() - 1
            })
            .collect();
        let shelf = shelves.last_mut().expect("a shelf was pushed");
        shelf.blocks.push(Block {
            stage: staged.stage,
            count: n,
            cards,
            across,
            rows,
        });
        shelf.rows = shelf.rows.max(rows);
        used += BORDER + across * card;
    }
    let rows = shelves
        .iter()
        .flat_map(|s| {
            (0..s.rows).map(move |i| {
                s.blocks
                    .iter()
                    .flat_map(|b| {
                        let row = (b.rows - 1).min(i * b.rows / s.rows);
                        let start = (row * b.across).min(b.cards.len());
                        let end = (start + b.across).min(b.cards.len());
                        b.cards[start..end].to_vec()
                    })
                    .collect::<Vec<usize>>()
            })
        })
        .collect();
    Grid {
        order,
        shelves,
        rows,
    }
}

/// A link in a card as the few words that name it: a Markdown link as its
/// text, a ticket's or PR's address as its ID or number, any other address
/// as its host, since a card is too narrow for addresses.
pub fn unlinked(text: &str) -> String {
    static MD: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\[([^\]\n]+)\]\(<?[^)\s>]+>?\)").expect("a regex"));
    static ANGLED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"<(https?://[^>\s]+)>").expect("a regex"));
    static BARE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"https?://[^\s)>\]]*[^\s)>\].,;:!?'"]"#).expect("a regex"));
    let text = MD.replace_all(text, "$1");
    let text = ANGLED.replace_all(&text, |c: &regex::Captures| short_url(&c[1]));
    BARE.replace_all(&text, |c: &regex::Captures| short_url(&c[0]))
        .into_owned()
}

fn short_url(url: &str) -> String {
    static TICKET: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"linear\.app/[^/]+/issue/([A-Za-z][A-Za-z0-9]*-\d+)").expect("a regex")
    });
    static PR: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"github\.com/[^/]+/[^/]+/(?:pull|issues)/(\d+)").expect("a regex")
    });
    static HOST: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^https?://(?:www\.)?([^/?#]+)").expect("a regex"));
    if let Some(m) = TICKET.captures(url) {
        return m[1].to_uppercase();
    }
    if let Some(m) = PR.captures(url) {
        return format!("#{}", &m[1]);
    }
    HOST.captures(url)
        .map_or_else(|| url.to_string(), |m| m[1].to_string())
}

/// What a card shows under its edge, with a reply of at most `room` lines
/// (the end of a longer one).
fn card_body(
    tab: &TabModel,
    inner: usize,
    room: usize,
    r: &Rules,
    theme: &Theme,
) -> Vec<Vec<Span<'static>>> {
    let mut body: Vec<Vec<Span<'static>>> = Vec::new();
    let b = model::badge_state(tab, r);
    let color = b.color().unwrap_or(theme.dim);
    let mut facts = vec![model::state_words(tab, r)];
    facts.extend(tab.pr.map(|p| format!("#{p}")));
    facts.extend(model::pr_facts(tab));
    facts.extend(tab.claude.as_ref().and_then(|c| c.name.clone()));
    if tab.notes_here {
        facts.push("✎ Notes here".into());
    }
    let facts: Vec<String> = facts.into_iter().filter(|f| !f.is_empty()).collect();
    body.push(vec![Span::styled(facts.join(" · "), fg(color))]);
    let turn = tab.claude.as_ref().and(tab.turn.as_ref());
    let Some(turn) = turn else {
        let place = tab
            .path
            .as_ref()
            .map(crate::paths::tilde)
            .unwrap_or_default();
        body.push(vec![Span::styled(place, fg(theme.dim))]);
        return body;
    };
    for (i, line) in text::wrap(
        &unlinked(&text::plain(&turn.prompt)),
        inner.saturating_sub(2),
    )
    .into_iter()
    .take(2)
    .enumerate()
    {
        let lead = if i > 0 { "  " } else { "› " };
        body.push(vec![Span::styled(format!("{lead}{line}"), fg(theme.dim))]);
    }
    body.push(Vec::new());
    // A turn stopped on a tool call is asking to run it, under the reply.
    let asking: Vec<String> = match &turn.pending {
        Some(p) if tab.state == TabState::Waiting => text::wrap(&format!("Asking: {p}"), inner)
            .into_iter()
            .take(2)
            .collect(),
        _ => Vec::new(),
    };
    let reply = match &turn.reply {
        Some(reply) => unlinked(&text::plain(reply)),
        None if tab.state == TabState::Working => {
            if turn.tools > 0 {
                format!(
                    "(working, {} tool call{} so far)",
                    turn.tools,
                    if turn.tools == 1 { "" } else { "s" }
                )
            } else {
                "(working)".into()
            }
        }
        None => "(no reply yet)".into(),
    };
    let left = room
        .saturating_sub(body.len())
        .saturating_sub(if asking.is_empty() {
            0
        } else {
            asking.len() + 1
        });
    let lines = text::layout(&reply, inner);
    // The end of a long reply is where its question or conclusion is.
    let kept: Vec<String> = if lines.len() > left {
        std::iter::once("…".to_string())
            .chain(
                lines[lines.len() - left.saturating_sub(1).min(lines.len())..]
                    .iter()
                    .cloned(),
            )
            .collect()
    } else {
        lines
    };
    for line in kept {
        body.push(vec![Span::styled(line, fg(theme.text))]);
    }
    if !asking.is_empty() {
        body.push(Vec::new());
        for line in asking {
            body.push(vec![Span::styled(line, bold(YELLOW))]);
        }
    }
    body
}

/// How tall a tab's card would need to be to show all of it.
fn card_needs(tab: &TabModel, w: usize, r: &Rules, theme: &Theme) -> usize {
    card_body(tab, w.saturating_sub(4), usize::MAX, r, theme).len() + 2
}

/// A tab's card, `w` wide and `h` tall: its mark and name in its top edge,
/// what it is doing, and its latest turn.
#[allow(clippy::too_many_arguments)]
fn card(
    tab: &TabModel,
    w: usize,
    h: usize,
    chosen: bool,
    hovered: bool,
    frame: usize,
    r: &Rules,
    theme: &Theme,
) -> Vec<Vec<Span<'static>>> {
    let inner = w.saturating_sub(4);
    let color = tab.color.map(Color::from).unwrap_or(theme.bright);
    let edge = if chosen {
        bold(color)
    } else if hovered {
        fg(color)
    } else {
        fg(theme.faint)
    };
    let index = tab.index.to_string();
    let name = text::cut(&tab.name, inner.saturating_sub(8 + width(&index)));
    let name_style = if chosen {
        bold(theme.bright)
    } else {
        fg(theme.text)
    };
    let mut top = vec![
        Span::styled("╭─", edge),
        Span::raw(" "),
        badge(model::badge_state(tab, r), frame, theme),
        Span::raw(" "),
        Span::styled(index, fg(theme.dim)),
        Span::raw(" "),
        Span::styled(name, name_style),
        Span::raw(" "),
    ];
    let used = spans_width(&top);
    top.push(Span::styled("─".repeat(w.saturating_sub(used + 2)), edge));
    top.push(Span::styled("─╮", edge));
    let mut out = vec![fit(top, w, Style::new())];
    let body = card_body(tab, inner, h.saturating_sub(2), r, theme);
    for line in body.into_iter().take(h.saturating_sub(2)) {
        let mut row = vec![Span::styled("│", edge), Span::raw(" ")];
        row.extend(fit(line, inner, Style::new()));
        row.push(Span::raw(" "));
        row.push(Span::styled("│", edge));
        out.push(row);
    }
    while out.len() < h.saturating_sub(1) {
        out.push(vec![
            Span::styled("│", edge),
            Span::raw(" ".repeat(w.saturating_sub(2))),
            Span::styled("│", edge),
        ]);
    }
    out.push(vec![Span::styled(
        format!("╰{}╯", "─".repeat(w.saturating_sub(2))),
        edge,
    )]);
    out
}

/// The tabs' cards in their stages' boxes, scrolled to keep the chosen card
/// in view, and where each card's cells are.
fn cards(
    m: &Mission,
    cols: usize,
    body: usize,
    top_row: usize,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
) {
    let snap = &m.snap;
    let theme = &snap.theme;
    let r = &snap.rules;
    let cw = card_width(cols, snap.card);
    let g = grid(&snap.tabs, r, cols, cw);
    if g.order.is_empty() {
        lines.push(
            Row::default()
                .put("  No tabs.", fg(theme.dim))
                .take()
                .line(cols, None),
        );
        return;
    }
    let card_rows: usize = g.shelves.iter().map(|s| s.rows).sum();
    // Cards are as tall as their shelf's fullest card needs, at least six
    // lines and at most what would fill the screen, so a shelf of quiet
    // tabs stays short rather than stretching empty cards down the screen.
    let most = 6usize
        .max(snap.card.1 as usize)
        .max(30.min(body.saturating_sub(BORDER * g.shelves.len()) / card_rows.max(1)));
    let mut drawn: Vec<Vec<Span<'static>>> = Vec::new();
    // Each card's lines in `drawn`, and its columns.
    let mut at: Vec<(usize, usize, usize, usize, usize)> = Vec::new();
    let (mut cursor_top, mut cursor_bottom) = (0, 0);
    for shelf in &g.shelves {
        let top = drawn.len();
        let tallest = shelf
            .blocks
            .iter()
            .flat_map(|b| b.cards.iter())
            .map(|&i| card_needs(&snap.tabs[g.order[i]], cw, r, theme))
            .max()
            .unwrap_or(6);
        let height = shelf.rows * most.min(tallest.max(6));
        let mut box_x = 0;
        let mut boxes: Vec<Vec<Vec<Span<'static>>>> = Vec::new();
        for block in &shelf.blocks {
            let x = box_x;
            let inner = block.across * cw;
            box_x += inner + BORDER;
            let row_height = |row: usize| {
                if row < block.rows - 1 {
                    height / block.rows
                } else {
                    height - (height / block.rows) * (block.rows - 1)
                }
            };
            let mut row_top = top + 1;
            let mut body_lines: Vec<Vec<Span<'static>>> = Vec::new();
            for row in 0..block.rows {
                let h = row_height(row);
                let start = (row * block.across).min(block.cards.len());
                let end = (start + block.across).min(block.cards.len());
                let row_cards: Vec<Vec<Vec<Span<'static>>>> = block.cards[start..end]
                    .iter()
                    .enumerate()
                    .map(|(c, &i)| {
                        at.push((
                            row_top,
                            row_top + h - 1,
                            x + 1 + c * cw,
                            x + (c + 1) * cw,
                            i,
                        ));
                        card(
                            &snap.tabs[g.order[i]],
                            cw,
                            h,
                            i as isize == m.cursor,
                            m.hover == Some(Spot::Index(i as isize)),
                            snap.frame,
                            r,
                            theme,
                        )
                    })
                    .collect();
                for line in 0..h {
                    let mut spans: Vec<Span<'static>> = Vec::new();
                    for c in &row_cards {
                        spans.extend(c.get(line).cloned().unwrap_or_default());
                    }
                    body_lines.push(fit(spans, inner, Style::new()));
                }
                row_top += h;
            }
            let color = fg(block.stage.color().color().unwrap_or(theme.dim));
            let title = format!(" {} {} ", block.stage.title().to_uppercase(), block.count);
            let shown = text::cut(&title, inner.saturating_sub(2));
            let mut lines_of = vec![vec![
                Span::styled("╭─", color),
                Span::styled(shown.clone(), color.add_modifier(Modifier::BOLD)),
                Span::styled(
                    format!("{}╮", "─".repeat(inner.saturating_sub(1 + width(&shown)))),
                    color,
                ),
            ]];
            for line in body_lines {
                let mut spans = vec![Span::styled("│", color)];
                spans.extend(line);
                spans.push(Span::styled("│", color));
                lines_of.push(spans);
            }
            lines_of.push(vec![Span::styled(
                format!("╰{}╯", "─".repeat(inner)),
                color,
            )]);
            boxes.push(lines_of);
        }
        for line in 0..height + BORDER {
            let mut spans: Vec<Span<'static>> = Vec::new();
            for b in &boxes {
                spans.extend(b.get(line).cloned().unwrap_or_default());
            }
            drawn.push(fit(spans, cols, Style::new()));
        }
        if shelf
            .blocks
            .iter()
            .any(|b| b.cards.iter().any(|&i| i as isize == m.cursor))
        {
            cursor_top = top;
            cursor_bottom = drawn.len();
        }
    }
    let start = cursor_top.min(cursor_bottom.saturating_sub(body));
    for spans in drawn.iter().skip(start).take(body) {
        lines.push(Line {
            spans: spans.clone(),
            fill: None,
        });
    }
    for (top, bottom, x0, x1, index) in at {
        for line in top.max(start)..=bottom.min(start + body - 1) {
            hits.push(HitBox {
                y: (line - start + top_row) as u16,
                x0: x0 as u16,
                x1: x1.saturating_sub(1) as u16,
                spot: Spot::Index(index as isize),
            });
        }
    }
}

// ── Boards, tables and lists ────────────────────────────────────────────────

/// A board: a column for each place an item can stand, side by side, each
/// item two lines, its title and where it is; the column with the chosen
/// item scrolls to keep it in view.
#[allow(clippy::too_many_arguments)]
fn board(
    view: &View,
    columns: &[&str],
    cols: usize,
    body: usize,
    m: &Mission,
    theme: &Theme,
    top_row: usize,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
) {
    let col_w = (cols / columns.len().max(1)).max(8);
    let mut drawn: Vec<Vec<Vec<Span<'static>>>> = Vec::new();
    for (ci, column) in columns.iter().enumerate() {
        let items: Vec<(usize, &Item)> = view
            .data
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| i.group.as_deref() == Some(column))
            .collect();
        let color = match *column {
            "Merged" => Badge::Merged.color(),
            "In merge queue" => Badge::Queued.color(),
            "Approved" | "Done" => Badge::Done.color(),
            "Draft" => Some(theme.dim),
            _ => Badge::Working.color(),
        }
        .unwrap_or(theme.text);
        let mut out = vec![
            fit(
                vec![Span::styled(
                    format!(" {} {}", column.to_uppercase(), items.len()),
                    bold(color),
                )],
                col_w,
                Style::new(),
            ),
            fit(
                vec![Span::styled(
                    format!(" {}", "─".repeat(col_w.saturating_sub(2))),
                    fg(color),
                )],
                col_w,
                Style::new(),
            ),
        ];
        let room = body.saturating_sub(2) / 2;
        let chosen_at = items.iter().position(|(i, _)| *i as isize == m.cursor);
        let first = match chosen_at {
            Some(c) if c >= room => c + 1 - room,
            _ => 0,
        };
        for (shown, (index, item)) in items.iter().enumerate().skip(first).take(room) {
            let chosen = *index as isize == m.cursor;
            let lit = !chosen && m.hover == Some(Spot::Index(*index as isize));
            let bg = if chosen {
                Some(theme.highlight)
            } else if lit {
                Some(theme.panel)
            } else {
                None
            };
            let with_bg = |s: Style| match bg {
                Some(c) => s.bg(c),
                None => s,
            };
            let title = if chosen {
                bold(theme.bright)
            } else {
                fg(theme.text)
            };
            let mark = item_mark(item, theme);
            let first_line = fit(
                vec![
                    Span::styled(" ", with_bg(Style::new())),
                    Span::styled(mark.content.clone(), with_bg(mark.style)),
                    Span::styled(" ", with_bg(Style::new())),
                    Span::styled(
                        text::cut(&item.title, col_w.saturating_sub(5)),
                        with_bg(title),
                    ),
                ],
                col_w.saturating_sub(1),
                with_bg(Style::new()),
            );
            let second = fit(
                vec![Span::styled(
                    format!("   {}", text::cut(&item.detail, col_w.saturating_sub(5))),
                    with_bg(fg(theme.dim)),
                )],
                col_w.saturating_sub(1),
                with_bg(Style::new()),
            );
            let y = top_row + 2 + (shown - first) * 2;
            for dy in 0..2 {
                hits.push(HitBox {
                    y: (y + dy) as u16,
                    x0: (ci * col_w) as u16,
                    x1: ((ci + 1) * col_w).saturating_sub(1) as u16,
                    spot: Spot::Index(*index as isize),
                });
            }
            let mut a = first_line;
            a.push(Span::raw(" "));
            let mut b = second;
            b.push(Span::raw(" "));
            out.push(a);
            out.push(b);
        }
        if items.is_empty() {
            out.push(fit(
                vec![Span::styled("   none", fg(theme.faint))],
                col_w,
                Style::new(),
            ));
        }
        drawn.push(out);
    }
    for i in 0..body {
        let mut spans = Vec::new();
        for c in &drawn {
            spans.extend(
                c.get(i)
                    .cloned()
                    .unwrap_or_else(|| vec![Span::raw(" ".repeat(col_w))]),
            );
        }
        lines.push(Line {
            spans: fit(spans, cols, Style::new()),
            fill: None,
        });
    }
}

/// When an item happened, shortly, for the column at a row's end; nothing
/// for one still to come (a meeting), whose time its detail says.
fn when(item: &Item, now_secs: i64) -> String {
    match item.time {
        Some(ms) if ms.div_euclid(1000) <= now_secs => {
            text::ago(Some(ms.div_euclid(1000)), now_secs)
        }
        _ => String::new(),
    }
}

/// A list: an error or how to set it up, whole; else its rows (a table's
/// with its columns' names over them and its totals under them), scrolled to
/// keep the chosen row in sight.
fn list(
    view: &View,
    cols: usize,
    rows: usize,
    m: &Mission,
    top_row: usize,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
) {
    let snap = &m.snap;
    let theme = &snap.theme;
    let data = &view.data;
    if let Some(setup) = &data.setup {
        for w in text::wrap(setup, cols.saturating_sub(4)) {
            lines.push(
                Row::default()
                    .put(format!("  {w}"), fg(theme.text))
                    .take()
                    .line(cols, None),
            );
        }
        return;
    }
    if let Some(error) = &data.error {
        for w in text::wrap(error, cols.saturating_sub(4)) {
            lines.push(
                Row::default()
                    .put(format!("  {w}"), fg(RED))
                    .take()
                    .line(cols, None),
            );
        }
    }
    if !view.loaded {
        lines.push(
            Row::default()
                .put("  Loading…", fg(theme.dim))
                .take()
                .line(cols, None),
        );
        return;
    }
    if data.items.is_empty() {
        if data.error.is_none() {
            let empty = data.empty.clone().unwrap_or_else(|| match view.id {
                ViewId::WorkMap => views::work_map_empty(),
                _ => "Nothing here.".into(),
            });
            for w in text::wrap(&empty, cols.saturating_sub(4)) {
                lines.push(
                    Row::default()
                        .put(format!("  {w}"), fg(theme.dim))
                        .take()
                        .line(cols, None),
                );
            }
        }
        return;
    }
    let table = view.table();
    let footer = table && !data.footer.is_empty();
    let body = rows.saturating_sub(1 + lines.len());
    let room = body
        .saturating_sub(usize::from(table))
        .saturating_sub(if footer { 2 } else { 0 })
        .max(1);
    let cursor = m.cursor.max(0) as usize;
    let first = (cursor + 1).saturating_sub(room);
    let shown: Vec<(usize, &Item)> = data
        .items
        .iter()
        .enumerate()
        .skip(first)
        .take(room)
        .collect();
    // A table's columns are as wide as their widest cell or name; the first,
    // the name, takes no more than it needs, leaving the chosen row room for
    // its `more` after the figures.
    let more_room = shown
        .iter()
        .map(|(_, i)| i.more.first().map_or(0, |m| width(m) + 2))
        .max()
        .unwrap_or(0)
        .min(40);
    let mut table_widths: Vec<usize> = Vec::new();
    if table {
        table_widths = data
            .columns
            .iter()
            .enumerate()
            .map(|(c, column)| {
                shown
                    .iter()
                    .map(|(_, i)| i.cells.get(c).map_or(0, |x| width(x)))
                    .chain(std::iter::once(data.footer.get(c).map_or(0, |x| width(x))))
                    .chain(std::iter::once(width(&column.heading)))
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let rest: usize = table_widths.iter().skip(1).map(|w| w + 2).sum();
        if let Some(w0) = table_widths.first_mut() {
            *w0 = 8.max((*w0).min(cols.saturating_sub(4 + more_room + rest)));
        }
    }
    let table_row = |cells: &[String], style: &dyn Fn(usize) -> Style| -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        for (c, w) in table_widths.iter().enumerate() {
            if c > 0 {
                spans.push(Span::raw("  "));
            }
            let cell = text::cut(cells.get(c).map_or("", String::as_str), *w);
            let gap = " ".repeat(w.saturating_sub(width(&cell)));
            let right = data.columns.get(c).is_some_and(|col| col.right);
            let text = if right {
                format!("{gap}{cell}")
            } else {
                format!("{cell}{gap}")
            };
            spans.push(Span::styled(text, style(c)));
        }
        spans
    };
    if table {
        let headings: Vec<String> = data.columns.iter().map(|c| c.heading.clone()).collect();
        let mut row = Row::default();
        row.put("   ", Style::new());
        row.spans.extend(table_row(&headings, &|_| bold(theme.dim)));
        lines.push(row.line(cols, None));
    }
    // Rows with cells (the Work map) line up in columns, each as wide as its
    // widest cell on the screen, up to a cap; the title takes what is left.
    let caps = [12usize, usize::MAX, 7, 28, 14];
    let mut widths: Vec<usize> = caps
        .iter()
        .enumerate()
        .map(|(c, cap)| {
            shown
                .iter()
                .filter(|(_, i)| !i.cells.is_empty() && !table)
                .map(|(_, i)| i.cells.get(c).map_or(0, |x| width(x)))
                .max()
                .unwrap_or(0)
                .min(*cap)
        })
        .collect();
    let others: usize = widths
        .iter()
        .enumerate()
        .filter(|(c, _)| *c != 1)
        .map(|(_, w)| w + 2)
        .sum();
    widths[1] = 10.max(widths[1].min(cols.saturating_sub(12 + others)));
    let label_width = shown
        .iter()
        .filter(|(_, i)| i.data.get("setting").is_some() || i.data.get("own").is_some())
        .map(|(_, i)| width(&i.title))
        .max()
        .unwrap_or(0)
        .min(32);
    for (index, item) in shown {
        let chosen = index == cursor && m.cursor >= 0;
        let fill = if chosen {
            Some(theme.highlight)
        } else if m.hover == Some(Spot::Index(index as isize)) {
            Some(theme.panel)
        } else {
            None
        };
        let name = if chosen {
            bold(theme.bright)
        } else {
            fg(theme.text)
        };
        hits.push(HitBox {
            y: (top_row + lines.len()) as u16,
            x0: 0,
            x1: cols.saturating_sub(1) as u16,
            spot: Spot::Index(index as isize),
        });
        let mut row = Row::default();
        if views::is_header(item) {
            let count = item
                .data
                .get("count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            row.put(format!(" {}", item.title.to_uppercase()), bold(theme.dim))
                .put(format!(" {count}"), fg(theme.faint));
            lines.push(row.line(cols, fill));
            continue;
        }
        row.put(" ", Style::new());
        row.spans.push(item_mark(item, theme));
        row.put(" ", Style::new());
        if let Some(value) = item.data.get("value").and_then(serde_json::Value::as_str) {
            let value_style = if item.state == State::Active {
                fg(theme.bright)
            } else {
                fg(theme.dim)
            };
            row.put(text::pad(&item.title, label_width), name)
                .put("  ", Style::new())
                .put(text::pad(value, 13), value_style)
                .put(" ", Style::new())
                .put(item.detail.clone(), fg(theme.dim));
            lines.push(row.line(cols, fill));
            continue;
        }
        if table {
            row.spans.extend(table_row(&item.cells, &|c| {
                if c == 0 { name } else { fg(theme.text) }
            }));
            if chosen && let Some(more) = item.more.first() {
                row.put(format!("  {more}"), fg(theme.dim));
            }
            lines.push(row.line(cols, fill));
            continue;
        }
        if !item.cells.is_empty() {
            for (c, cell) in item.cells.iter().enumerate().take(widths.len()) {
                if c > 0 {
                    row.put("  ", Style::new());
                }
                let style = match c {
                    1 => name,
                    0 => fg(theme.text),
                    _ => fg(theme.dim),
                };
                row.put(text::pad(cell, widths[c]), style);
            }
            if chosen && let Some(more) = item.more.first() {
                row.put(format!("  {more}"), fg(theme.dim));
            }
            lines.push(row.line(cols, fill));
            continue;
        }
        row.put(item.title.clone(), name)
            .put("  ", Style::new())
            .put(item.detail.clone(), fg(theme.dim));
        let at = when(item, snap.now_secs);
        let mut spans = row.fitted(cols.saturating_sub(6), Style::new());
        spans.push(Span::styled(format!("{at:>5} "), fg(theme.faint)));
        lines.push(Line { spans, fill });
    }
    if footer {
        while lines.len() + 3 < rows {
            lines.push(Row::default().line(cols, None));
        }
        lines.push(
            Row::default()
                .put(
                    format!(" {}", "─".repeat(cols.saturating_sub(2))),
                    fg(theme.faint),
                )
                .take()
                .line(cols, None),
        );
        let mut row = Row::default();
        row.put("   ", Style::new());
        row.spans
            .extend(table_row(&data.footer, &|_| bold(theme.text)));
        lines.push(row.line(cols, None));
    }
}

impl Row {
    /// The row as built so far, taken out of a chain of `put`s.
    fn take(&mut self) -> Row {
        Row {
            spans: std::mem::take(&mut self.spans),
        }
    }
}

// ── The whole screen ────────────────────────────────────────────────────────

/// A view's label in the bar: ` Title count `.
fn label(title: &str, count: Option<usize>) -> String {
    match count {
        Some(n) => format!(" {title} {n} "),
        None => format!(" {title} "),
    }
}

/// The keys the footer names for the view in front.
fn hints(m: &Mission, view: Option<&View>) -> String {
    if m.jump.is_some() {
        return "type a view's name  ↵ go there  esc cancel".into();
    }
    let next = if m.snap.tab_focus {
        "[ ] views  tab focus"
    } else {
        "tab next view"
    };
    let Some(view) = view else {
        return format!("hjkl move  ↵ actions (↵↵ go to tab)  / filter  {next}  ? keys  esc close");
    };
    match view.id {
        ViewId::Settings => format!("jk move  ↵ change  / search  {next}  ? keys  esc close"),
        ViewId::Prs | ViewId::Tickets => {
            format!("hl column  jk move  ↵ what to do with it  {next}  ? keys  esc close")
        }
        ViewId::Plugin(_) => format!("jk move  ↵ open  r refresh  {next}  ? keys  esc close"),
        _ if view.data.items.iter().any(views::is_header) => {
            format!("jk move  ↵ actions, or show merged  {next}  ? keys  esc close")
        }
        _ => format!("jk move  ↵ actions  / filter  {next}  ? keys  esc close"),
    }
}

/// Mission control's lines for a screen `cols` by `rows`, and where a click
/// on each part lands.
pub fn screen(m: &Mission, cols: usize, rows: usize) -> (Vec<Line>, Vec<HitBox>) {
    let snap = &m.snap;
    let theme = &snap.theme;
    let mut hits = Vec::new();
    let mut names: Vec<(String, usize)> = vec![("Tabs".into(), snap.tabs.len())];
    names.extend(
        snap.views
            .iter()
            .map(|v| (v.title.clone(), v.data.items.len())),
    );
    let jumped = m.jump.as_ref().and_then(|j| j.matched);
    // A bar too wide for the screen with every count in it keeps only the
    // chosen view's count.
    let prefix = " MISSION CONTROL  ";
    let all_labels: String = names
        .iter()
        .map(|(t, n)| label(t, Some(*n)))
        .collect::<Vec<_>>()
        .join(" ");
    let roomy = width(prefix) + width(&all_labels) + 30 <= cols;
    let labels: Vec<String> = names
        .iter()
        .enumerate()
        .map(|(i, (t, n))| {
            let count = (roomy || i == m.view || jumped == Some(i)).then_some(*n);
            label(t, count)
        })
        .collect();
    // The filter, a view's name being typed, or, with Tab moving focus, a
    // stop of its own even while empty on a view that filters.
    let view = m.view.checked_sub(1).and_then(|i| snap.views.get(i));
    let on_board = view.is_some_and(|v| v.columns().is_some());
    let filter_lit = m.focus == Some(Focus::Filter) || m.hover == Some(Spot::Filter);
    let reverse = |s: Style, on: bool| {
        if on {
            s.add_modifier(Modifier::REVERSED)
        } else {
            s
        }
    };
    let filter_stop = snap.tab_focus && m.jump.is_none() && !on_board;
    let mut filter: Vec<Span<'static>> = Vec::new();
    if let Some(j) = &m.jump {
        filter.push(Span::raw("  "));
        filter.push(Span::styled(format!("view: {}", j.text), fg(YELLOW)));
        filter.push(Span::styled("▏", fg(theme.dim)));
    } else if let Some(f) = &m.filter {
        filter.push(Span::raw("  "));
        filter.push(Span::styled(
            format!("/{}", f.text),
            reverse(fg(YELLOW), filter_lit),
        ));
        if f.typing {
            filter.push(Span::styled("▏", fg(theme.dim)));
        }
    } else if filter_stop {
        filter.push(Span::raw("  "));
        filter.push(Span::styled(
            " / filter ",
            reverse(fg(theme.dim), filter_lit),
        ));
    }
    let counts = tally(&snap.tabs, &snap.rules, theme);
    // With Tab's focus on a view's name or the filter past the screen's
    // edge, the bar scrolls: the names before it give way to a ‹.
    let target = match m.focus {
        Some(Focus::Filter) => Some(labels.len() - 1),
        Some(Focus::View(v)) => Some(v.min(labels.len() - 1)),
        _ => None,
    };
    let end_of = |skip: usize| -> usize {
        let t = target.unwrap_or(0);
        let names: usize = labels[skip..=t.max(skip)]
            .iter()
            .map(|l| width(l) + 1)
            .sum::<usize>();
        width(prefix)
            + if skip > 0 { 2 } else { 0 }
            + names.saturating_sub(1)
            + if m.focus == Some(Focus::Filter) {
                2 + spans_width(&counts) + spans_width(&filter)
            } else {
                0
            }
    };
    let mut skip = 0;
    if let Some(t) = target {
        while skip < t && end_of(skip) > cols {
            skip += 1;
        }
    }
    let mut head = Row::default();
    head.put(" ", Style::new())
        .put("MISSION CONTROL", bold(ACCENT))
        .put("  ", Style::new());
    if skip > 0 {
        head.put("‹ ", fg(theme.dim));
    }
    for (i, l) in labels.iter().enumerate().skip(skip) {
        if i > skip {
            head.put(" ", Style::new());
        }
        let x = head.width();
        hits.push(HitBox {
            y: 0,
            x0: x as u16,
            x1: (x + width(l)).saturating_sub(1) as u16,
            spot: Spot::View(i),
        });
        let ring =
            m.focus == Some(Focus::View(i)) || (m.hover == Some(Spot::View(i)) && i != m.view);
        let style = if ring {
            bold(theme.bright).add_modifier(Modifier::REVERSED)
        } else if jumped == Some(i) {
            Style::new()
                .bg(YELLOW)
                .fg(Color::Indexed(232))
                .add_modifier(Modifier::BOLD)
        } else if i == m.view {
            Style::new()
                .bg(theme.highlight)
                .fg(theme.bright)
                .add_modifier(Modifier::BOLD)
        } else {
            fg(theme.dim)
        };
        head.put(l.clone(), style);
    }
    head.put("  ", Style::new());
    head.spans.extend(counts);
    if !filter.is_empty() && (filter_stop || (m.filter.is_some() && snap.tab_focus)) {
        let x = head.width() + 2;
        hits.push(HitBox {
            y: 0,
            x0: x as u16,
            x1: (head.width() + spans_width(&filter)).saturating_sub(1) as u16,
            spot: Spot::Filter,
        });
    }
    head.spans.extend(filter);
    let mut lines = Vec::new();
    if snap.close && cols > 8 {
        let mut spans = head.fitted(cols - width(CLOSE) - 1, Style::new());
        let lit = m.focus == Some(Focus::Close) || m.hover == Some(Spot::Close);
        spans.push(Span::styled(CLOSE, reverse(bold(theme.bright), lit)));
        spans.push(Span::raw(" "));
        hits.insert(
            0,
            HitBox {
                y: 0,
                x0: (cols - width(CLOSE) - 1) as u16,
                x1: (cols - 2) as u16,
                spot: Spot::Close,
            },
        );
        lines.push(Line { spans, fill: None });
    } else {
        lines.push(head.line(cols, None));
    }
    // While a note is worked continuously, its line leads the tabs, where
    // the cursor's -1 is (src/autopilot).
    if let (None, Some(pilot)) = (view, &snap.pilot) {
        hits.push(HitBox {
            y: lines.len() as u16,
            x0: 0,
            x1: cols.saturating_sub(1) as u16,
            spot: Spot::Index(-1),
        });
        let lit = m.hover == Some(Spot::Index(-1));
        lines.push(crate::autopilot::line::line(
            pilot,
            cols,
            m.cursor == -1,
            lit,
            theme,
        ));
    }
    let body = rows.saturating_sub(1 + lines.len());
    match view {
        None => {
            let top = lines.len();
            cards(m, cols, body, top, &mut lines, &mut hits);
        }
        Some(v) => {
            if let Some(columns) = v.columns() {
                if v.data.setup.is_some() || !v.loaded {
                    list(v, cols, rows, m, 0, &mut lines, &mut hits);
                } else {
                    if let Some(e) = &v.data.error {
                        for w in text::wrap(e, cols.saturating_sub(4)) {
                            lines.push(
                                Row::default()
                                    .put(format!("  {w}"), fg(RED))
                                    .take()
                                    .line(cols, None),
                            );
                        }
                    }
                    let top = lines.len();
                    let room = rows.saturating_sub(1 + top);
                    board(v, columns, cols, room, m, theme, top, &mut lines, &mut hits);
                }
            } else if v.id == ViewId::Calendar && super::calendar::is_day(v) {
                let look = super::calendar::Look {
                    cols,
                    rows: rows.saturating_sub(1 + lines.len()),
                    cursor: usize::try_from(m.cursor).ok(),
                    tz: &snap.tz,
                    now_ms: snap.now_secs * 1000,
                    theme,
                    top_row: lines.len(),
                };
                let (day, at) = super::calendar::day(v, &look);
                lines.extend(day);
                hits.extend(at);
            } else {
                list(v, cols, rows, m, 0, &mut lines, &mut hits);
            }
        }
    }
    lines.truncate(rows.saturating_sub(1));
    while lines.len() < rows.saturating_sub(1) {
        lines.push(Row::default().line(cols, None));
    }
    let foot = match &m.note {
        Some(note) => Row::default()
            .put(format!(" {note}"), fg(theme.bright))
            .take(),
        None => Row::default()
            .put(format!(" {}", hints(m, view)), fg(theme.dim))
            .take(),
    };
    lines.push(foot.line(cols, None));
    hits.retain(|h| (h.y as usize) < rows.saturating_sub(1));
    (lines, hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::ViewData;
    use crate::mission::{Jump, Snapshot};
    use crate::mux::TabId;
    use crate::sidebar::claude::{LoopRun, SessionRef};
    use crate::sidebar::model::tests::rules;
    use crate::sidebar::model::{MergeState, TabLoop};

    fn tab(n: u32, name: &str) -> TabModel {
        let mut t = TabModel::new(TabId(n), n as usize, name);
        t.claude = Some(SessionRef {
            session_id: "s".into(),
            ..SessionRef::default()
        });
        t.path = Some("/work".into());
        t
    }

    fn mission(tabs: Vec<TabModel>, views: Vec<View>) -> Mission {
        let mut m = Mission::new();
        m.snap = Snapshot {
            tabs,
            views,
            rules: rules(),
            theme: Theme::DARK,
            card: (56, 13),
            tab_focus: true,
            close: false,
            now_secs: rules().now_secs(),
            tz: jiff::tz::TimeZone::UTC,
            frame: 0,
            pilot: None,
        };
        m
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.text().trim_end().to_string())
            .collect()
    }

    #[test]
    fn every_line_is_the_screens_width() {
        let m = mission(
            vec![
                tab(1, "one"),
                tab(2, "a very long name ".repeat(9).as_str()),
            ],
            Vec::new(),
        );
        let (lines, _) = screen(&m, 100, 30);
        assert_eq!(lines.len(), 30);
        for l in &lines {
            assert_eq!(width(&l.text()), 100, "{:?}", l.text());
        }
    }

    #[test]
    fn stages_pack_side_by_side_each_block_under_its_own_header() {
        let r = rules();
        let mut tabs: Vec<TabModel> = Vec::new();
        let mut add = |n: u32, name: &str, f: &dyn Fn(&mut TabModel)| {
            let mut t = tab(n, name);
            f(&mut t);
            tabs.push(t);
        };
        add(1, "plain", &|_| {});
        add(2, "queued b", &|t| {
            t.merge = Some(MergeState::Queued);
            t.position = Some(2);
        });
        add(3, "reviewing", &|t| {
            t.state = TabState::Background;
            t.background = 2;
            t.tab_loop = Some(TabLoop::Run(LoopRun {
                kind: "review",
                round: 2,
            }));
        });
        add(4, "busy", &|t| t.state = TabState::Working);
        add(5, "queued a", &|t| {
            t.merge = Some(MergeState::Queued);
            t.position = Some(1);
        });
        add(6, "asking", &|t| t.state = TabState::Waiting);
        add(7, "ci fixing", &|t| {
            t.state = TabState::Background;
            t.background = 1;
            t.tab_loop = Some(TabLoop::Run(LoopRun {
                kind: "ci",
                round: 1,
            }));
        });
        add(8, "landed", &|t| t.merge = Some(MergeState::Merged));
        add(9, "watching the queue", &|t| {
            t.state = TabState::Background;
            t.background = 1;
            t.tab_loop = Some(TabLoop::Run(LoopRun {
                kind: "queue",
                round: 1,
            }));
            t.merge = Some(MergeState::Queued);
            t.position = Some(3);
        });
        // Room for three cards a shelf, each box with its two columns of
        // border: a stage that doesn't fit in the room left starts the next
        // shelf.
        let g = grid(&tabs, &r, 3 * 22, 20);
        let laid: Vec<Vec<(&str, Vec<&str>)>> = g
            .shelves
            .iter()
            .map(|s| {
                s.blocks
                    .iter()
                    .map(|b| {
                        (
                            b.stage.title(),
                            b.cards
                                .iter()
                                .map(|&i| tabs[g.order[i]].name.as_str())
                                .collect(),
                        )
                    })
                    .collect()
            })
            .collect();
        assert_eq!(
            laid,
            vec![
                vec![
                    ("Needs you", vec!["asking"]),
                    ("Working", vec!["busy"]),
                    ("Review loop", vec!["reviewing"])
                ],
                vec![("CI loop", vec!["ci fixing"])],
                vec![(
                    "In merge queue",
                    vec!["queued a", "queued b", "watching the queue"]
                )],
                vec![("Merged", vec!["landed"]), ("Idle", vec!["plain"])],
            ],
            "a loop is its own stage, but a PR's place in the queue or merged comes first; the queue in queue order"
        );
        let many: Vec<TabModel> = (0..5).map(|i| tab(20 + i, &format!("idle {i}"))).collect();
        let big = grid(&many, &r, 3 * 22, 20);
        assert_eq!(big.shelves.len(), 1);
        assert_eq!(
            (
                big.shelves[0].blocks[0].across,
                big.shelves[0].blocks[0].rows
            ),
            (3, 2)
        );
        assert_eq!(
            big.rows.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![3, 2],
            "its rows are the rows the keys move through"
        );
        let m = mission(tabs, Vec::new());
        let (lines, hits) = screen(&m, 3 * 56, 200);
        let text = texts(&lines);
        let tops: Vec<&String> = text.iter().filter(|l| l.starts_with("╭─ ")).collect();
        assert!(
            tops[0].contains("NEEDS YOU 1") && tops[0].contains("WORKING 1"),
            "{tops:?}"
        );
        assert_eq!(
            text.iter().filter(|l| l.starts_with("╰")).count(),
            tops.len(),
            "every box closed"
        );
        assert!(text[0].contains("MISSION CONTROL"));
        assert!(
            text[0].contains("⋯ 4"),
            "the loops count as working: {}",
            text[0]
        );
        assert!(hits.iter().any(|h| h.spot == Spot::Index(0)));
    }

    #[test]
    fn a_shelf_of_quiet_tabs_is_as_short_as_a_card_can_be() {
        let mut quiet = vec![tab(1, "quiet 1"), tab(2, "quiet 2")];
        for t in &mut quiet {
            t.claude = None;
        }
        let m = mission(quiet, Vec::new());
        let (lines, _) = screen(&m, 3 * 56, 60);
        let text = texts(&lines);
        let bottom = text.iter().position(|l| l.starts_with("╰")).unwrap();
        assert_eq!(bottom, 8, "{text:#?}");
        assert!(text[9..59].iter().all(String::is_empty));
        assert!(
            text.iter().any(|l| l.contains("/work")),
            "a card with no Claude shows its directory"
        );
    }

    // test/tmux-sidebar.test.js's "while a note is worked continuously,
    // mission control's Tabs view leads with its line, which the cursor and
    // a click reach above the cards".
    #[test]
    fn the_note_worked_continuously_leads_the_tabs_above_the_cards() {
        let mut m = mission(vec![tab(1, "one"), tab(2, "two")], Vec::new());
        m.snap.pilot = Some(crate::autopilot::Summary {
            note: "Work".into(),
            busy: 2,
            open: 4,
            slots: 3,
            cap: 5,
            next: "Next thing".into(),
            paused: false,
            away: false,
            ready_only: false,
            skipped: 1,
            spend_limit: 0.0,
        });
        m.cursor = -1;
        let (lines, hits) = screen(&m, 120, 30);
        assert_eq!(lines.len(), 30);
        for l in &lines {
            assert_eq!(width(&l.text()), 120, "{:?}", l.text());
        }
        let text = texts(&lines);
        assert!(
            text[1].starts_with(
                " AUTO Work  2 of 3 working · 4 of 5 open · next: Next thing · 1 skipped"
            ),
            "{}",
            text[1]
        );
        assert_eq!(lines[1].fill, Some(Theme::DARK.highlight), "chosen");
        assert!(
            hits.contains(&HitBox {
                y: 1,
                x0: 0,
                x1: 119,
                spot: Spot::Index(-1)
            }),
            "a click on it chooses it"
        );
        assert!(
            hits.iter()
                .filter(|h| matches!(h.spot, Spot::Index(i) if i >= 0))
                .all(|h| h.y >= 2),
            "the cards under it"
        );
    }

    #[test]
    fn links_in_a_card_show_as_their_names() {
        assert_eq!(
            unlinked(
                "Closed [ABC-2204](https://linear.app/acme/issue/ABC-2204/old) after [#4321](https://github.com/acme/app/pull/4321), see <https://acme.slack.com/archives/D0/p1> and https://linear.app/acme/issue/abc-7/x."
            ),
            "Closed ABC-2204 after #4321, see acme.slack.com and ABC-7."
        );
    }

    fn list_view(id: ViewId, title: &str, items: Vec<Item>) -> View {
        View {
            id,
            title: title.into(),
            data: ViewData {
                items,
                at: Some(1),
                ..ViewData::default()
            },
            loaded: true,
        }
    }

    #[test]
    fn a_list_names_its_rows_and_the_bar_its_views_with_their_counts() {
        let items = vec![
            Item::new("a", "First thing")
                .detail("about it")
                .state(State::Attention),
            Item::new("b", "Second thing"),
        ];
        let mut m = mission(
            vec![tab(1, "one")],
            vec![list_view(ViewId::Closed, "Closed", items)],
        );
        m.view = 1;
        let (lines, hits) = screen(&m, 120, 12);
        let text = texts(&lines);
        assert!(
            text[0].contains(" Tabs 1 ") && text[0].contains(" Closed 2 "),
            "{}",
            text[0]
        );
        assert!(text[1].contains("● First thing  about it"));
        assert!(text[2].contains("○ Second thing"));
        assert_eq!(
            lines[1].fill,
            Some(Theme::DARK.highlight),
            "the chosen row lit"
        );
        assert!(text[11].contains("jk move"));
        let tabs_name = hits.iter().find(|h| h.spot == Spot::View(0)).unwrap();
        assert_eq!(tabs_name.y, 0);
        assert!(hits.iter().any(|h| h.spot == Spot::Index(1) && h.y == 2));
        m.jump = Some(Jump {
            text: "clo".into(),
            matched: Some(1),
        });
        let (lines, _) = screen(&m, 120, 12);
        assert!(texts(&lines)[0].contains("view: clo"));
    }

    #[test]
    fn settings_rows_line_up_as_name_value_and_what_it_does() {
        let items = views::settings_items(&[]);
        let mut m = mission(
            Vec::new(),
            vec![list_view(ViewId::Settings, "Settings", items)],
        );
        m.view = 1;
        let (lines, _) = screen(&m, 160, 20);
        let text = texts(&lines);
        assert!(text[1].starts_with(" INTEGRATIONS"), "{text:#?}");
        assert!(
            text[2].contains("GitHub") && (text[2].contains(" on ") || text[2].contains(" off ")),
            "{}",
            text[2]
        );
    }

    #[test]
    fn a_board_lays_its_columns_side_by_side() {
        let item = |n: u32, column: &str| {
            Item::new(n.to_string(), format!("#{n} PR {n}"))
                .detail("open")
                .group(column)
        };
        let items = vec![
            item(1, "Draft"),
            item(2, "In review"),
            item(3, "In review"),
            item(4, "Merged"),
        ];
        let mut m = mission(Vec::new(), vec![list_view(ViewId::Prs, "PRs", items)]);
        m.view = 1;
        let (lines, hits) = screen(&m, 150, 20);
        let text = texts(&lines);
        assert!(
            text[1].contains("DRAFT 1")
                && text[1].contains("IN REVIEW 2")
                && text[1].contains("MERGED 1"),
            "{}",
            text[1]
        );
        assert!(text[3].contains("#1 PR 1") && text[3].contains("#2 PR 2"));
        assert!(text[5].contains("#3 PR 3"));
        assert!(hits.iter().any(|h| h.spot == Spot::Index(3)));
    }

    #[test]
    fn a_table_heads_its_columns_and_totals_them_at_the_bottom() {
        let mut row = Item::new("@1", "1 build").target(crate::items::Target::Tab(TabId(1)));
        row.cells = vec!["1 build".into(), "93.0".into(), "1.2G".into(), "14".into()];
        let mut view = list_view(ViewId::Resources, "Resources", vec![row]);
        view.data.columns = crate::usage::resources::columns();
        view.data.footer = vec!["All tabs".into(), "93.0".into(), "1.2G".into(), "14".into()];
        let mut m = mission(Vec::new(), vec![view]);
        m.view = 1;
        let (lines, _) = screen(&m, 100, 12);
        let text = texts(&lines);
        assert!(
            text[1].contains("Tab") && text[1].contains("% CPU"),
            "{}",
            text[1]
        );
        assert!(text[2].contains("1 build") && text[2].contains("93.0"));
        assert!(text[10].contains("All tabs"), "{text:#?}");
    }
}
