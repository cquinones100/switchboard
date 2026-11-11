//! Mission control's screen (lib/sidebar/render.js's `missionLines`,
//! `missionGrid`, `card` and `boardLines`): a plain function from what a
//! client's mission control holds to the lines to draw, each exactly the
//! screen's width, and where a click on each part lands, so the tests can
//! read it as text.
//!
//! At the top a bar naming the views (the tabs, then each view, with how
//! many items each has), the counts of tabs waiting, finished and working,
//! and the filter; with UX 2.0 two rows and a rule (`grouped_bar`), the
//! groups of views by feature over the views of the one in front; at the
//! bottom the keys, or what the last action did. In
//! between, the view in front:
//!
//! - the tabs as cards in stages (`grid`): each row of the screen a shelf
//!   of stage boxes side by side, a stage that doesn't fit in the room left
//!   starting the next shelf, each card the tab's badge, number and name in
//!   its top edge, what it is doing, and its latest turn: your prompt, then
//!   the end of what Claude said or the tool call it is stopped on;
//! - a board (the PR and ticket boards): a column for each place, side by
//!   side, each item two lines;
//! - with UX 2.0, the Autopilot view as a board of cards (`pilot_board`),
//!   each with a meter placed as an image over the lines (`Image`);
//! - a table (Resources): its columns' names, its rows, and a totals row;
//!   with UX 2.0 and room, the board `resources_board` draws instead;
//! - any other view, one row per item, a heading over each group, Settings'
//!   rows as name, value and what the option does; with UX 2.0 and room,
//!   Settings as the board `settings_board` draws.

use std::sync::LazyLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use regex::Regex;

use super::views::{self, Scope, View, ViewId};
use super::{Focus, HitBox, Mission, Spot};
use crate::items::{Item, State};
use crate::sidebar::model::{self, Rules, TabModel, TabState};
use crate::sidebar::render::Line;
use crate::sidebar::text::{self, Badge, SPINNER, width};
use crate::ui::Theme;
use crate::ui::kit::{self, Col, Mark, Tone};
use crate::views::markdown::{Images, PictureRow};

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
    for staged in model::grouped(tabs, None, r) {
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

/// A tab's card as UX 2.0 draws it, `w` wide and `h` tall: a rounded box,
/// its state's mark, number and name on the top edge and its state at the
/// edge's right end, the last lines of its screen inside in the pane's own
/// colors, down to a footer line with its branch dimmed and its PR's checks
/// as marks.
#[allow(clippy::too_many_arguments)]
fn card_ux2(
    tab: &TabModel,
    screen: Option<&super::Screen>,
    w: usize,
    h: usize,
    chosen: bool,
    hovered: bool,
    frame: usize,
    r: &Rules,
    theme: &Theme,
) -> Vec<Vec<Span<'static>>> {
    let inner = w.saturating_sub(4);
    let state = model::badge_state(tab, r);
    let mark = Mark::of_badge(state);
    let edge = kit::card_edge(
        chosen,
        hovered.then(|| tab.color.map(Color::from).unwrap_or(theme.bright)),
        theme,
    );
    let words = text::cut(&model::state_words(tab, r), inner / 3);
    let right = if words.is_empty() {
        Vec::new()
    } else {
        vec![Span::styled(words, mark.look().1.style(theme))]
    };
    let title = vec![
        kit::mark(mark, frame, theme),
        Span::raw(" "),
        Span::styled(tab.index.to_string(), bold(theme.bright)),
        Span::raw(" "),
        Span::styled(tab.name.clone(), bold(theme.bright)),
    ];
    // The screen's last lines, down to the footer.
    let room = h.saturating_sub(3);
    let lines: &[Vec<Span<'static>>] = screen.map_or(&[], |s| &s.lines[..]);
    let shown = &lines[lines.len().saturating_sub(room)..];
    let mut inside: Vec<Vec<Span<'static>>> = vec![Vec::new(); room - shown.len()];
    inside.extend(shown.iter().cloned());
    let branch = screen.and_then(|s| s.branch.clone()).unwrap_or_default();
    let mut footer = vec![Span::styled(branch, fg(theme.dim))];
    // The PR's checks as the kit's marks after the branch, failing first.
    if let Some(c) = tab.checks {
        for (mark, n) in [
            (Mark::Failed, c.fail),
            (Mark::Passed, c.pass),
            (Mark::Pending, c.pending),
        ] {
            if n > 0 {
                let (glyph, tone) = mark.look();
                footer.push(Span::styled(format!("  {glyph}{n}"), tone.style(theme)));
            }
        }
    }
    inside.push(footer);
    kit::card(w, h, title, right, &inside, edge)
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
            .map(|&i| {
                let tab = &snap.tabs[g.order[i]];
                if r.ux2 {
                    // A screen fills whatever room the card has.
                    usize::MAX
                } else {
                    card_needs(tab, cw, r, theme)
                }
            })
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
                        let tab = &snap.tabs[g.order[i]];
                        let chosen = i as isize == m.cursor;
                        let hovered = m.hover == Some(Spot::Index(i as isize));
                        if r.ux2 {
                            card_ux2(
                                tab,
                                snap.screens.get(&tab.id),
                                cw,
                                h,
                                chosen,
                                hovered,
                                snap.frame,
                                r,
                                theme,
                            )
                        } else {
                            card(tab, cw, h, chosen, hovered, snap.frame, r, theme)
                        }
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
            // With UX 2.0, the stage's tone in the kit, as the sidebar's
            // heading for it wears.
            let color = if r.ux2 {
                Mark::of_badge(block.stage.color()).look().1.style(theme)
            } else {
                fg(block.stage.color().color().unwrap_or(theme.dim))
            };
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

/// UX 2.0's PR and ticket boards, one family as boards 10 and 19 draw them:
/// a column per place an item stands, its name a label in the state's tone
/// with its count, each item a rounded card (its number or ID in the accent
/// on the top edge, its checks' mark or its priority at the edge's right
/// end, its title on two lines, then a dim line about it); the chosen
/// card's edge in the accent. The views' last group (the PRs merged and the
/// tickets canceled in the last week) is a row of the same cards, faint,
/// under the columns, each saying when. On a screen `SIDE_AT` wide, the
/// chosen item's description sits beside them, through Markdown, and the
/// diagrams it draws as pictures come back, for the frame to place.
#[allow(clippy::too_many_arguments)]
fn board_cards(
    view: &View,
    columns: &[&str],
    cols: usize,
    body: usize,
    m: &Mission,
    theme: &Theme,
    top_row: usize,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
) -> Vec<Picture> {
    // A card's five lines and the blank one under it.
    const TALL: usize = 6;
    // The row's rule, label and blank line, and a card.
    const WEEK_ROWS: usize = 3 + TALL - 1;
    let tickets = view.id == ViewId::Tickets;
    let (columns, week) = columns.split_at(columns.len().saturating_sub(1));
    let in_group = |group: &str| -> Vec<(usize, &Item)> {
        view.data
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| i.group.as_deref() == Some(group))
            .collect()
    };
    let week_items = week.first().map(|w| in_group(w)).unwrap_or_default();
    let side_w = beside_w(cols, SIDE_AT, SIDE_W);
    let chosen_item = usize::try_from(m.cursor)
        .ok()
        .and_then(|c| view.data.items.get(c));
    let images = ready_diagrams(m).filter(|_| side_w > 0);
    let about = chosen_item.filter(|_| side_w > 0).map(|item| {
        if tickets {
            ticket_about(item, side_w, theme, images.as_ref())
        } else {
            pr_about(item, side_w, theme, images.as_ref())
        }
    });
    let cols = cols - side_w;
    if let Some(about) = &about {
        side_hits(
            about.last_top(body),
            cols + 2,
            side_w - 2,
            top_row,
            body,
            hits,
        );
    }
    let about = about
        .map(|a| {
            let top = m.side_top().min(a.last_top(body));
            a.window(top, body)
        })
        .unwrap_or_default();
    // Each picture where the pane's lines put it on the screen.
    let pictures = about
        .pictures
        .iter()
        .map(|p| Picture {
            x: p.x + (cols + 2) as u16,
            y: p.y + top_row as u16,
            ..p.clone()
        })
        .collect();
    let about = about.lines;
    // The week's row when it has anything and the columns keep room for a
    // card.
    let week_h = if !week_items.is_empty() && body >= 2 + TALL + WEEK_ROWS {
        WEEK_ROWS
    } else {
        0
    };
    let board_h = body - week_h;
    // As many columns as fit 12 wide; with more, the ones around the chosen
    // item's, so h and l bring the rest into sight, an arrow on the first
    // and last heading saying more are off to that side.
    let fit_n = (cols / 12).clamp(1, columns.len().max(1));
    let chosen_col = chosen_item.and_then(|item| {
        columns
            .iter()
            .position(|c| item.group.as_deref() == Some(*c))
    });
    let start = chosen_col
        .map_or(0, |c| c.saturating_sub(fit_n - 1))
        .min(columns.len().saturating_sub(fit_n));
    let more_left = start > 0;
    let more_right = start + fit_n < columns.len();
    let columns = &columns[start..(start + fit_n).min(columns.len())];
    let col_w = (cols / columns.len().max(1)).max(12);
    let w = col_w - 1;
    let face = |item: &Item| {
        if tickets {
            ticket_face(item, m, theme)
        } else {
            pr_face(item, m, theme)
        }
    };
    let mut drawn: Vec<Vec<Vec<Span<'static>>>> = Vec::new();
    for (ci, column) in columns.iter().enumerate() {
        let items = in_group(column);
        let right = if ci + 1 == columns.len() && more_right {
            2
        } else {
            0
        };
        let mut heading = kit::label(
            column,
            column_tone(column),
            Some(items.len()),
            w.saturating_sub(right),
            theme,
        );
        if ci == 0 && more_left {
            heading[0] = Span::styled("‹", fg(theme.dim));
        }
        if right > 0 {
            heading.push(Span::styled(" ›", fg(theme.dim)));
        }
        let mut out = vec![
            fit(heading, col_w, Style::new()),
            vec![Span::raw(" ".repeat(col_w))],
        ];
        let room = board_h.saturating_sub(2) / TALL;
        let chosen_at = items.iter().position(|(i, _)| *i as isize == m.cursor);
        let first = match chosen_at {
            Some(c) if c >= room => c + 1 - room,
            _ => 0,
        };
        for (shown, (index, item)) in items.iter().enumerate().skip(first).take(room) {
            let chosen = *index as isize == m.cursor;
            let lit = !chosen && m.hover == Some(Spot::Index(*index as isize));
            let card = board_card(&face(item), w, chosen, lit, false, theme);
            out.extend(card.into_iter().map(|mut line| {
                line.push(Span::raw(" "));
                line
            }));
            out.push(vec![Span::raw(" ".repeat(col_w))]);
            let y = top_row + 2 + (shown - first) * TALL;
            for dy in 0..TALL - 1 {
                hits.push(HitBox {
                    y: (y + dy) as u16,
                    x0: (ci * col_w) as u16,
                    x1: (ci * col_w + w).saturating_sub(1) as u16,
                    spot: Spot::Index(*index as isize),
                });
            }
        }
        if items.is_empty() {
            out.push(fit(
                vec![Span::styled(" none", fg(theme.faint))],
                col_w,
                Style::new(),
            ));
        }
        drawn.push(out);
    }
    let mut rows: Vec<Vec<Span<'static>>> = (0..board_h)
        .map(|i| {
            let mut spans = Vec::new();
            for c in &drawn {
                spans.extend(
                    c.get(i)
                        .cloned()
                        .unwrap_or_else(|| vec![Span::raw(" ".repeat(col_w))]),
                );
            }
            fit(spans, cols, Style::new())
        })
        .collect();
    if week_h > 0 {
        let title = if tickets {
            "Closed this week"
        } else {
            "Merged this week"
        };
        rows.push(vec![kit::head_rule(cols, theme)]);
        rows.push(kit::label(
            title,
            Tone::Dim,
            Some(week_items.len()),
            cols.saturating_sub(1),
            theme,
        ));
        rows.push(Vec::new());
        // As many cards as fit, from the one that keeps the chosen in sight.
        let across = (cols / col_w).max(1);
        let chosen_at = week_items.iter().position(|(i, _)| *i as isize == m.cursor);
        let first = chosen_at.map_or(0, |c| (c + 1).saturating_sub(across));
        let shown: Vec<&(usize, &Item)> = week_items.iter().skip(first).take(across).collect();
        let cards: Vec<Vec<Vec<Span<'static>>>> = shown
            .iter()
            .map(|(index, item)| {
                let chosen = *index as isize == m.cursor;
                let lit = !chosen && m.hover == Some(Spot::Index(*index as isize));
                let mut face = face(item);
                face.meta = vec![Span::styled(
                    week_words(item, tickets, m.snap.now_secs),
                    fg(theme.faint),
                )];
                board_card(&face, w, chosen, lit, true, theme)
            })
            .collect();
        let y = top_row + board_h + 3;
        for (c, (index, _)) in shown.iter().enumerate() {
            for dy in 0..TALL - 1 {
                hits.push(HitBox {
                    y: (y + dy) as u16,
                    x0: (c * col_w) as u16,
                    x1: (c * col_w + w).saturating_sub(1) as u16,
                    spot: Spot::Index(*index as isize),
                });
            }
        }
        for line in 0..TALL - 1 {
            let mut spans = Vec::new();
            for card in &cards {
                spans.extend(card.get(line).cloned().unwrap_or_default());
                spans.push(Span::raw(" "));
            }
            rows.push(fit(spans, cols, Style::new()));
        }
    }
    for (i, row) in rows.into_iter().enumerate() {
        let mut spans = fit(row, cols, Style::new());
        if side_w > 0 {
            spans.push(Span::styled("│ ", fg(theme.faint)));
            spans.extend(fit(
                about.get(i).cloned().unwrap_or_default(),
                side_w - 2,
                Style::new(),
            ));
        }
        lines.push(Line { spans, fill: None });
    }
    pictures
}

/// A board column's tone: where a PR or ticket stands, as the kit marks it.
fn column_tone(column: &str) -> Tone {
    match column {
        "In progress" => Tone::Needs,
        "In review" => Tone::Working,
        "Changes asked" => Tone::Failed,
        "Approved" | "Done" => Tone::Done,
        "In merge queue" => Tone::Landing,
        _ => Tone::Dim,
    }
}

/// What a board's card says of an item: its number or ID on the top edge,
/// what sits at the edge's right end, its title, and the line under it.
struct Face {
    label: String,
    right: Vec<Span<'static>>,
    title: String,
    meta: Vec<Span<'static>>,
    /// A ticket done: its ID and title faint, the title struck through.
    done: bool,
}

/// A card of a board, `w` wide: the face's label in the accent (faint for
/// a done ticket or the week's row), its title on two lines, its meta.
fn board_card(
    face: &Face,
    w: usize,
    chosen: bool,
    lit: bool,
    faint: bool,
    theme: &Theme,
) -> Vec<Vec<Span<'static>>> {
    let inner = w.saturating_sub(4);
    let edge = kit::card_edge(chosen, lit.then_some(theme.text), theme);
    let label = if faint || face.done {
        fg(theme.faint)
    } else {
        bold(ACCENT)
    };
    let title = if faint {
        fg(theme.dim)
    } else if face.done {
        fg(theme.faint).add_modifier(Modifier::CROSSED_OUT)
    } else if chosen {
        bold(theme.bright)
    } else {
        fg(theme.text)
    };
    let [a, b] = two_lines(&face.title, inner);
    kit::card(
        w,
        TALL_CARD,
        vec![Span::styled(face.label.clone(), label)],
        face.right.clone(),
        &[
            vec![Span::styled(a, title)],
            vec![Span::styled(b, title)],
            face.meta.clone(),
        ],
        edge,
    )
}

/// A card's five lines: its edges, two of title and one about it.
const TALL_CARD: usize = 5;

/// A title on two lines of `cols`, by words, the second ending in … when
/// the rest doesn't fit.
fn two_lines(title: &str, cols: usize) -> [String; 2] {
    let wrapped = text::wrap(title, cols);
    let first = wrapped.first().cloned().unwrap_or_default();
    let rest = wrapped.get(1..).map(|r| r.join(" ")).unwrap_or_default();
    [text::cut(&first, cols), text::cut(&rest, cols)]
}

/// The tab working on an item, as a card says it: `tab 3` and its mark.
fn tab_words(tab: &TabModel, m: &Mission, theme: &Theme) -> Vec<Span<'static>> {
    let mark = Mark::of_badge(model::badge_state(tab, &m.snap.rules));
    vec![
        Span::styled(format!("tab {} · ", tab.index), fg(theme.dim)),
        kit::mark(mark, m.snap.frame, theme),
    ]
}

/// A PR's card: its number, its checks' mark, its title without the
/// number, and the tab working on it, else its reviewers, else its
/// repository, review and place in the queue.
fn pr_face(item: &Item, m: &Mission, theme: &Theme) -> Face {
    let number = item.data["pr"]["number"].as_u64();
    let title = item
        .title
        .split_once(' ')
        .filter(|(n, _)| n.starts_with('#'))
        .map_or(item.title.as_str(), |(_, t)| t);
    let repo = item.data["pr"]["repo"].as_str().unwrap_or("");
    let tab = number.and_then(|n| m.pr_tab(n, repo));
    let queued = item.group.as_deref() == Some("In merge queue");
    let meta = match (tab, item.data["reviewers"].as_str()) {
        (Some(t), _) => tab_words(t, m, theme),
        (None, Some(r)) if !queued && !r.is_empty() => {
            vec![Span::styled(r.to_string(), fg(theme.dim))]
        }
        _ => vec![Span::styled(
            item.data["meta"].as_str().unwrap_or("").to_string(),
            fg(theme.dim),
        )],
    };
    Face {
        label: format!("#{}", item.data["pr"]["number"]),
        right: checks_mark(item)
            .map(|c| vec![kit::mark(c, 0, theme)])
            .unwrap_or_default(),
        title: title.to_string(),
        meta,
        done: false,
    }
}

/// A PR's checks as the kit's mark.
fn checks_mark(item: &Item) -> Option<Mark> {
    match item.data["checks"].as_str() {
        Some("success") => Some(Mark::Passed),
        Some("failure") => Some(Mark::Failed),
        Some("pending") => Some(Mark::Pending),
        _ => None,
    }
}

/// A ticket's priority as marks in their tone: `!!!` urgent, `!!` high,
/// `!` medium, nothing for low or none.
fn priority_marks(item: &Item, theme: &Theme) -> Option<Span<'static>> {
    let (marks, tone) = match item.data["priority"].as_u64() {
        Some(1) => ("!!!", Tone::Failed),
        Some(2) => ("!!", Tone::Needs),
        Some(3) => ("!", Tone::Dim),
        _ => return None,
    };
    Some(Span::styled(marks, tone.bold(theme)))
}

/// A ticket's card: its ID, its priority, its title, and when it was done
/// (then the tab still open on it), else the tab working on it, else its
/// project and estimate.
fn ticket_face(item: &Item, m: &Mission, theme: &Theme) -> Face {
    let d = &item.data;
    let done = item.group.as_deref() == Some("Done");
    let tab = d["tab"]
        .as_u64()
        .and_then(|n| m.snap.tabs.iter().find(|t| t.index as u64 == n));
    let meta = match tab {
        _ if done => {
            let mut words = vec![Span::styled(
                week_words(item, true, m.snap.now_secs),
                fg(theme.faint),
            )];
            if let Some(t) = tab {
                words.push(Span::styled(" · ", fg(theme.faint)));
                words.extend(tab_words(t, m, theme));
            }
            words
        }
        Some(t) => tab_words(t, m, theme),
        None => vec![Span::styled(
            [
                d["project"].as_str().unwrap_or("").to_string(),
                estimate(d).unwrap_or_default(),
            ]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · "),
            fg(theme.dim),
        )],
    };
    let label = d["ticket"].as_str().unwrap_or("").to_string();
    let title = d["title"].as_str().map(str::to_string).unwrap_or_else(|| {
        item.title
            .strip_prefix(&label)
            .unwrap_or(&item.title)
            .trim()
            .to_string()
    });
    Face {
        label,
        right: priority_marks(item, theme).into_iter().collect(),
        title,
        meta,
        done,
    }
}

/// A ticket's estimate, `2 pts`.
fn estimate(d: &serde_json::Value) -> Option<String> {
    let n = d["estimate"].as_f64()?;
    let n = if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        n.to_string()
    };
    Some(format!("{n} {}", if n == "1" { "pt" } else { "pts" }))
}

/// What a card in the week's row says: when, and who merged the PR or how
/// the ticket was closed; a done ticket's card says when alone.
fn week_words(item: &Item, tickets: bool, now_secs: i64) -> String {
    let when = item
        .time
        .map(|ms| format!("{} ago", text::ago(Some(ms.div_euclid(1000)), now_secs)))
        .unwrap_or_default();
    let who = if !tickets {
        item.data["mergedBy"].as_str().unwrap_or("").to_string()
    } else if item.group.as_deref() == Some("Closed") {
        item.data["state"].as_str().unwrap_or("").to_lowercase()
    } else {
        String::new()
    };
    [when, who]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The diagrams ready to draw as pictures in the pane beside a board,
/// where the client draws them.
fn ready_diagrams(m: &Mission) -> Option<Images> {
    m.snap
        .cell
        .filter(|_| !m.snap.diagrams.is_empty())
        .map(|cell| Images {
            cell: (cell.0 as u32, cell.1 as u32),
            rows: crate::ui::images::MARKS.len(),
            ready: m
                .snap
                .diagrams
                .iter()
                .map(|(key, diagram)| {
                    let id = crate::ui::images::id_of(&format!("mission/diagram/{key}"));
                    (key.clone(), (id, diagram.image.dimensions()))
                })
                .collect(),
        })
}

/// How wide a screen shows the chosen item's description beside the board,
/// and how many columns it takes there at 160, its share of a screen of
/// any width (board 45).
const SIDE_AT: usize = 140;
const SIDE_W: usize = 50;
/// The columns a diagram in the description is drawn for: the pane's at
/// 160, which a wider pane shows as drawn.
pub const ABOUT_COLS: usize = SIDE_W - 3;

/// The pane beside a board on a screen `cols` wide: none under `at`, else
/// its share `share` of 160 scaled to the screen, never under 34.
fn beside_w(cols: usize, at: usize, share: usize) -> usize {
    if cols < at {
        0
    } else {
        kit::beside(cols, share, 34, 0)
    }
}

/// A diagram the pane beside a board places as an image once its lines are
/// drawn (`ui::images::picture_cut`): its key among the snapshot's
/// diagrams, the id its layout gave it, where the corner of what shows of
/// it is, its size in cells, the first of its rows that shows (past those
/// scrolled above the pane's top), and how many show above the pane's
/// bottom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub key: String,
    pub id: u32,
    pub x: u16,
    pub y: u16,
    pub cols: u16,
    pub rows: u16,
    pub from: u16,
    pub shown: u16,
}

/// The pane beside a board: its lines, and the diagrams placed over them,
/// each by its place among the lines.
#[derive(Clone, Debug, Default)]
struct About {
    lines: Vec<Vec<Span<'static>>>,
    pictures: Vec<Picture>,
}

impl About {
    /// The `body` lines from `top` that the pane shows, scrolled there by
    /// `J` and `K` (`Mission::side_top`), each picture cut at the pane's
    /// top and bottom to the rows of it that show.
    fn window(self, top: usize, body: usize) -> About {
        let end = top + body;
        let pictures = self
            .pictures
            .into_iter()
            .filter_map(|p| {
                let (start, stop) = (p.y as usize, p.y as usize + p.rows as usize);
                let (first, last) = (start.max(top), stop.min(end));
                (first < last).then(|| Picture {
                    y: (first - top) as u16,
                    from: (first - start) as u16,
                    shown: (last - first) as u16,
                    ..p
                })
            })
            .collect();
        About {
            lines: self.lines.into_iter().skip(top).take(body).collect(),
            pictures,
        }
    }

    /// The most the pane's first line shown can be, its last line then at
    /// its bottom.
    fn last_top(&self, body: usize) -> usize {
        self.lines.len().saturating_sub(body)
    }
}

/// The description the board draws beside its columns now, and the place
/// its Markdown is read for (`Markdown::parse`): the chosen PR's or
/// ticket's, or the plan of the Autopilot board's chosen item, on a screen
/// wide enough for the pane.
pub fn description_beside(m: &Mission) -> Option<(&str, &'static str)> {
    let view = m.current()?;
    let (field, place, side_at) = match view.id {
        ViewId::Prs => ("body", crate::views::pr::PLACE, SIDE_AT),
        ViewId::Tickets => ("description", crate::views::ticket::PLACE, SIDE_AT),
        ViewId::Autopilot => ("plan", PLAN_PLACE, PILOT_SIDE_AT),
        _ => return None,
    };
    let is_a_board = view.columns().is_some() || view.id == ViewId::Autopilot;
    let board_with_the_pane = m.snap.rules.ux2
        && is_a_board
        && view.data.setup.is_none()
        && view.loaded
        && m.size.0 as usize >= side_at;
    if !board_with_the_pane {
        return None;
    }
    let text = m.chosen_item()?.data[field].as_str()?;
    Some((text, place))
}

/// Markdown laid out `cols` wide for the pane beside a board, as the PR
/// and ticket views lay it out but without their margin or the blank lines
/// at its ends. Each Mermaid diagram `images` has ready is the rows its
/// picture takes at the pane's width, blank for the frame to place it
/// over, however tall, since the pane cuts it as it cuts the text; one not
/// ready, and every one without `images`, is its text drawing.
fn markdown_lines(
    text: &str,
    place: &str,
    cols: usize,
    theme: &Theme,
    images: Option<&Images>,
) -> About {
    type Row = (Vec<Span<'static>>, Option<PictureRow>);
    let doc = crate::views::markdown::Markdown::parse(text, place).layout_with(cols + 4, images);
    let mut rows: Vec<Row> = doc
        .lines
        .iter()
        .map(|l| {
            let mut margin = 2;
            let mut spans = Vec::new();
            for s in &l.spans {
                let mut t = s.text.as_str();
                while margin > 0 && t.starts_with(' ') {
                    t = &t[1..];
                    margin -= 1;
                }
                if !t.is_empty() {
                    let mut style = s.style.ratatui();
                    // Its plain text in the theme's, so a light terminal reads it.
                    if style.fg == Some(Color::Indexed(crate::views::markdown::TEXT)) {
                        style.fg = Some(theme.text);
                    }
                    spans.push(Span::styled(t.to_string(), style));
                }
            }
            (spans, l.picture)
        })
        .collect();
    let blank = |(spans, picture): &Row| {
        picture.is_none() && spans.iter().all(|s| s.content.trim().is_empty())
    };
    let first = rows.iter().position(|r| !blank(r)).unwrap_or(rows.len());
    rows.drain(..first);
    while rows.last().is_some_and(blank) {
        rows.pop();
    }
    let mut about = About::default();
    for (y, (mut spans, picture)) in rows.into_iter().enumerate() {
        if let Some(row) = picture {
            let x: usize = spans.iter().map(|s| width(&s.content)).sum();
            if row.row == 0
                && let Some(placed) = doc.placed.iter().find(|p| p.id == row.id)
            {
                about.pictures.push(Picture {
                    key: placed.key.clone(),
                    id: row.id,
                    x: x as u16,
                    y: y as u16,
                    cols: row.cols,
                    rows: placed.rows,
                    from: 0,
                    shown: placed.rows,
                });
            }
            spans.push(Span::raw(" ".repeat(row.cols as usize)));
        }
        about.lines.push(spans);
    }
    about
}

/// The pane beside a board's rows, `body` of them from `top_row`, `w`
/// wide from `x`, for the wheel over it to scroll it, each holding the
/// most its first line shown can be.
fn side_hits(last: usize, x: usize, w: usize, top_row: usize, body: usize, hits: &mut Vec<HitBox>) {
    hits.extend((0..body).map(|i| HitBox {
        y: (top_row + i) as u16,
        x0: x as u16,
        x1: (x + w).saturating_sub(1) as u16,
        spot: Spot::Side(last),
    }));
}

/// A description under the pane's head, or a faint line saying it has
/// none, a picture with it, whole: the board shows the part of it its
/// height has room for (`About::window`).
fn with_description(
    head: Vec<Vec<Span<'static>>>,
    description: &str,
    place: &str,
    inner: usize,
    theme: &Theme,
    images: Option<&Images>,
) -> About {
    let described = if description.trim().is_empty() {
        About {
            lines: vec![vec![Span::styled("No description.", fg(theme.faint))]],
            pictures: Vec::new(),
        }
    } else {
        markdown_lines(description, place, inner, theme, images)
    };
    let under = head.len();
    let mut lines = head;
    lines.extend(described.lines);
    let pictures = described
        .pictures
        .into_iter()
        .map(|p| Picture {
            y: p.y + under as u16,
            ..p
        })
        .collect();
    About { lines, pictures }
}

/// The chosen PR's description, as the board draws it beside the columns:
/// its number and title bold, a dim line of its author, commits, lines
/// changed and checks, a rule, then its body through Markdown.
fn pr_about(item: &Item, side_w: usize, theme: &Theme, images: Option<&Images>) -> About {
    let inner = side_w.saturating_sub(3);
    let d = &item.data;
    let mut facts: Vec<String> = Vec::new();
    if let Some(a) = d["author"].as_str().filter(|a| !a.is_empty()) {
        facts.push(a.to_string());
    }
    if let Some(n) = d["commits"].as_u64() {
        facts.push(format!("{n} commit{}", if n == 1 { "" } else { "s" }));
    }
    if let (Some(a), Some(r)) = (d["additions"].as_u64(), d["deletions"].as_u64()) {
        facts.push(format!("+{a} −{r}"));
    }
    let mut fact_line = vec![Span::styled(facts.join(" · "), fg(theme.dim))];
    if let Some(mark) = checks_mark(item) {
        if !facts.is_empty() {
            fact_line.push(Span::styled(" · ", fg(theme.dim)));
        }
        fact_line.push(kit::mark(mark, 0, theme));
        fact_line.push(Span::styled(" checks", fg(theme.dim)));
    }
    let head = kit::pane(&item.title, fact_line, inner, theme);
    with_description(
        head,
        d["body"].as_str().unwrap_or(""),
        crate::views::pr::PLACE,
        inner,
        theme,
        images,
    )
}

/// The chosen ticket's description beside the board: its ID and title
/// bold, a dim line of its state, priority, project, estimate and
/// assignee, a rule, then its description through Markdown.
fn ticket_about(item: &Item, side_w: usize, theme: &Theme, images: Option<&Images>) -> About {
    let inner = side_w.saturating_sub(3);
    let d = &item.data;
    let dim = |s: String| Span::styled(s, fg(theme.dim));
    let mut facts = vec![dim(d["state"].as_str().unwrap_or("").to_string())];
    if let Some(marks) = priority_marks(item, theme) {
        let label = d["priorityLabel"].as_str().unwrap_or("");
        facts.push(dim(" · ".into()));
        facts.push(Span::styled(
            format!("{} {label}", marks.content),
            marks.style,
        ));
    } else if d["priority"].as_u64() == Some(4) {
        facts.push(dim(" · Low".into()));
    }
    let rest: Vec<String> = [
        d["project"].as_str().unwrap_or("").to_string(),
        estimate(d).unwrap_or_default(),
        "you".to_string(),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect();
    facts.push(dim(format!(" · {}", rest.join(" · "))));
    let head = kit::pane(&item.title, facts, inner, theme);
    with_description(
        head,
        d["description"].as_str().unwrap_or(""),
        crate::views::ticket::PLACE,
        inner,
        theme,
        images,
    )
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
    if let Some(counts) = snap.notice_counts.as_ref().filter(|_| snap.rules.ux2)
        && view.id == ViewId::Notifications
    {
        notice_board(view, counts, cols, rows, m, top_row, lines, hits);
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
    if snap.rules.ux2 && view.id == ViewId::WorkMap && !m.picker {
        work_map_rows(view, cols, rows, m, top_row, lines, hits);
        return;
    }
    if snap.rules.ux2 && view.id == ViewId::Landing && cols >= STAGES_AT + 6 * STAGE_STEP {
        stage_rows(view, cols, rows, m, top_row, lines, hits);
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
    // Keys (the Keys view) line up as the key, what it does, then its tags,
    // the key column as wide as the widest key, so none is cut.
    let key_width = shown
        .iter()
        .filter_map(|(_, i)| super::keymap::key_of(i))
        .map(width)
        .max()
        .unwrap_or(0);
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
        if let Some(key) = super::keymap::key_of(item) {
            row.put("   ", Style::new())
                .put(text::pad(key, key_width), bold(theme.bright))
                .put("  ", Style::new())
                .put(item.title.clone(), name)
                .put("  ", Style::new());
            // A key keys.json bound is yours.
            if super::keymap::yours_of(item) {
                for s in kit::chip("yours", Tone::Needs, m.snap.caps, theme) {
                    row.put(s.content.into_owned(), s.style);
                }
            }
            row.put(item.detail.clone(), fg(theme.faint));
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
                .put(" ", Style::new());
            if snap.rules.ux2 && item.data["changed"] == true {
                row.spans
                    .extend(kit::chip("changed", Tone::Needs, snap.caps, theme));
            }
            row.put(item.detail.clone(), fg(theme.dim));
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
        // A kept notification says what it led to and whether the sidebar
        // still shows it in its detail's place.
        let detail = match super::history::notice_of(item) {
            Some((sidebar, led_to)) => [led_to, sidebar]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · "),
            None => item.detail.clone(),
        };
        row.put(item.title.clone(), name)
            .put("  ", Style::new())
            .put(detail, fg(theme.dim));
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

/// With UX 2.0, the Work map as the board draws it: the columns' heads once
/// at the top over a dashed rule, each repository's heading (`▾ name`, where
/// it is and how it lands, its worktrees and how many are open at the
/// right), its rows in columns at fixed places (the worktree's column the
/// first to give way on a narrow screen), its fold line of merged work, and,
/// on a screen of `WORK_PANE_AT` rows or more, the chosen row's pane under
/// the map: its path and tab, its last prompt and its branch counted.
fn work_map_rows(
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
    let items = &view.data.items;
    let cursor = (m.cursor.max(0) as usize).min(items.len().saturating_sub(1));
    let chosen_row = items.get(cursor).filter(|i| !views::is_header(i));
    let pane = if rows >= WORK_PANE_AT {
        chosen_row.map(|i| work_pane(i, m, cols))
    } else {
        None
    };
    // The rule, its two lines, and a window on its fields.
    let pane_h = pane.as_ref().map_or(0, |_| 3 + WORK_PANE_BODY);
    let budget = rows.saturating_sub(1 + lines.len());
    let room = budget.saturating_sub(2 + pane_h).max(1);
    let widths = work_widths(cols);
    // The columns' heads.
    let heads = [
        "worktree",
        "ticket",
        "tab",
        "PR",
        "where it stands",
        "checks",
        "used",
    ];
    let mut head = vec![Span::raw("  ")];
    for (c, h) in heads.iter().enumerate() {
        if widths[c] == 0 {
            continue;
        }
        head.push(Span::styled(
            work_cell(h, widths[c], c == WORK_USED),
            fg(theme.dim),
        ));
    }
    lines.push(Line {
        spans: kit::fit(head, cols, Style::new()),
        fill: None,
    });
    lines.push(Line {
        spans: vec![kit::head_rule(cols, theme)],
        fill: None,
    });
    let first = (cursor + 1).saturating_sub(room);
    for (index, item) in items.iter().enumerate().skip(first).take(room) {
        let chosen = index == cursor && m.cursor >= 0;
        let hovered = !chosen && m.hover == Some(Spot::Index(index as isize));
        hits.push(HitBox {
            y: (top_row + lines.len()) as u16,
            x0: 0,
            x1: cols.saturating_sub(1) as u16,
            spot: Spot::Index(index as isize),
        });
        let spans = if views::is_repo_heading(item) {
            work_heading(item, cols, theme)
        } else if views::is_fold_line(item) {
            let open = item.data["open"] == true;
            let mut s = vec![Span::styled(format!("   {}", item.title), fg(theme.dim))];
            if chosen && !open {
                s.push(Span::styled(" · Enter shows them", fg(theme.faint)));
            }
            s
        } else {
            work_row(item, &widths, chosen, m)
        };
        lines.push(Line {
            spans: kit::row(spans, cols, chosen, theme),
            fill: hovered.then_some(theme.panel),
        });
    }
    if let Some((head, body)) = pane {
        while lines.len() + pane_h < rows.saturating_sub(1) {
            lines.push(Row::default().line(cols, None));
        }
        lines.push(Line {
            spans: vec![kit::rule(cols, theme)],
            fill: None,
        });
        // Its fields scroll with J and K and the wheel over it, as a pane
        // beside a board does.
        let last = body.len().saturating_sub(WORK_PANE_BODY);
        let top = m.side_top().min(last);
        let shown: Vec<Vec<Span<'static>>> = head
            .into_iter()
            .chain(body.into_iter().skip(top).take(WORK_PANE_BODY))
            .collect();
        for l in shown {
            hits.push(HitBox {
                y: (top_row + lines.len()) as u16,
                x0: 0,
                x1: cols.saturating_sub(1) as u16,
                spot: Spot::Side(last),
            });
            let mut spans = vec![Span::raw("  ")];
            spans.extend(l);
            lines.push(Line {
                spans: kit::fit(spans, cols, Style::new()),
                fill: None,
            });
        }
    }
}

/// From what height the Work map shows the chosen row's pane under it, and
/// how many lines of its fields it shows at once.
const WORK_PANE_AT: usize = 30;
const WORK_PANE_BODY: usize = 4;
/// The Work map's columns: the worktree, the ticket, the tab, the PR, where
/// it stands, its checks and when it was last used.
const WORK_USED: usize = 6;

/// Each of the Work map's columns' widths, a space after each included: the
/// ticket's none with the linear setting off, the PR's and the checks' none
/// with the github setting off; the worktree takes what is left, down to
/// 12. On a narrower screen (one beside the sidebar), the others give way
/// in steps until they fit: the tab's and the standing's narrower, the
/// ticket left out (the worktree's branch names it), then the tab's name
/// to a few letters, the rest to their shortest words, and last the checks
/// and when it was used left out.
fn work_widths(cols: usize) -> [usize; 7] {
    let linear = crate::settings::is_on("linear");
    let github = crate::settings::is_on("github");
    let mut w = [
        0,
        if linear { 11 } else { 0 },
        24,
        if github { 8 } else { 0 },
        26,
        if github { 13 } else { 0 },
        6,
    ];
    let rest = |w: &[usize; 7]| w[1..].iter().sum::<usize>();
    let room = cols.saturating_sub(3);
    let steps: [fn(&mut [usize; 7]); 6] = [
        |w| {
            w[2] = 16;
            w[4] = 20;
        },
        |w| w[1] = 0,
        |w| w[2] = 11,
        |w| {
            w[3] = w[3].min(7);
            w[4] = 17;
            w[5] = w[5].min(11);
            w[6] = 5;
        },
        |w| w[5] = 0,
        |w| w[6] = 0,
    ];
    for step in steps {
        if room >= rest(&w) + 12 {
            break;
        }
        step(&mut w);
    }
    w[0] = room.saturating_sub(rest(&w)).max(10);
    w
}

#[cfg(test)]
#[test]
fn the_work_map_s_columns_fit_beside_the_sidebar_on_a_100_column_screen() {
    // 100 columns less the sidebar's 34.
    let w = work_widths(66);
    assert!(2 + w.iter().sum::<usize>() <= 66, "{w:?}");
    assert!(w[3] > 0 && w[5] > 0 && w[6] > 0, "{w:?}");
    assert_eq!(work_widths(160)[2], 24, "a wide screen keeps them whole");
    // The sidebar's narrowest window, 90 columns less its 35.
    let w = work_widths(55);
    assert!(2 + w.iter().sum::<usize>() <= 55, "{w:?}");
}

/// A cell's text cut or padded to `cols` with a space after it, or
/// right-aligned with none (the last column).
fn work_cell(text: &str, cols: usize, right: bool) -> String {
    if right {
        let t = text::cut(text, cols);
        return format!("{}{t}", " ".repeat(cols.saturating_sub(width(&t))));
    }
    let inner = cols.saturating_sub(1);
    format!("{} ", text::pad(&text::cut(text, inner), inner))
}

/// A tone by the name src/mission/views.rs gives it.
fn tone_named(name: &str) -> Tone {
    match name {
        "needs" => Tone::Needs,
        "working" => Tone::Working,
        "landing" => Tone::Landing,
        "done" => Tone::Done,
        "failed" => Tone::Failed,
        "dim" => Tone::Dim,
        "text" => Tone::Text,
        _ => Tone::Faint,
    }
}

/// A repository's heading: `▾ name` bold, where it is and how it lands dim,
/// its worktrees and how many are open at the right.
fn work_heading(item: &Item, cols: usize, theme: &Theme) -> Vec<Span<'static>> {
    let d = &item.data;
    let about: Vec<&str> = [d["place"].as_str(), d["lands"].as_str()]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect();
    let counts = d["counts"].as_str().unwrap_or("").to_string();
    let mut left = vec![Span::styled(format!(" {}", item.title), bold(theme.bright))];
    if !about.is_empty() {
        left.push(Span::styled(
            format!("   {}", about.join(" · ")),
            fg(theme.dim),
        ));
    }
    let right_at = cols.saturating_sub(width(&counts) + 3);
    let mut out = kit::fit(left, right_at, Style::new());
    out.push(Span::raw(" "));
    out.push(Span::styled(counts, fg(theme.dim)));
    out
}

/// A row of the Work map in its columns.
fn work_row(item: &Item, widths: &[usize; 7], chosen: bool, m: &Mission) -> Vec<Span<'static>> {
    let theme = &m.snap.theme;
    let d = &item.data;
    let s = |k: &str| d[k].as_str().unwrap_or("").to_string();
    let name = if chosen {
        bold(theme.bright)
    } else {
        fg(theme.text)
    };
    let mut out = vec![Span::raw(" ")];
    out.push(Span::styled(
        work_cell(&s("worktree"), widths[0], false),
        name,
    ));
    if widths[1] > 0 {
        out.push(Span::styled(
            work_cell(&s("ticket"), widths[1], false),
            fg(theme.dim),
        ));
    }
    let tab = match item.target {
        crate::items::Target::Tab(id) => {
            // Every tab, a filter typed or not.
            let tabs = if m.snap.all_tabs.is_empty() {
                &m.snap.tabs
            } else {
                &m.snap.all_tabs
            };
            tabs.iter().find(|t| t.id == id)
        }
        _ => None,
    };
    match tab {
        Some(t) => {
            let mark = kit::mark(
                Mark::of_badge(model::badge_state(t, &m.snap.rules)),
                m.snap.frame,
                theme,
            );
            let number = format!(" {} ", t.index);
            let color = t.color.map(Color::from).unwrap_or(theme.bright);
            let left = widths[2].saturating_sub(1 + width(&number));
            out.push(mark);
            out.push(Span::styled(number, bold(color)));
            out.push(Span::styled(
                work_cell(&t.name, left, false),
                fg(theme.text),
            ));
        }
        None => out.push(Span::styled(
            work_cell("no tab", widths[2], false),
            fg(theme.faint),
        )),
    }
    if widths[3] > 0 {
        out.push(Span::styled(
            work_cell(&s("pr"), widths[3], false),
            fg(theme.text),
        ));
    }
    out.push(Span::styled(
        work_cell(&s("standing"), widths[4], false),
        tone_named(&s("standingTone")).style(theme),
    ));
    if widths[5] > 0 {
        out.push(Span::styled(
            work_cell(&s("checks"), widths[5], false),
            tone_named(&s("checksTone")).style(theme),
        ));
    }
    out.push(Span::styled(
        work_cell(&s("used"), widths[6], true),
        fg(theme.dim),
    ));
    out
}

/// A pane's lines.
type PaneLines = Vec<Vec<Span<'static>>>;

/// The chosen row's pane under the Work map: its worktree's name and
/// the tab it is open in, then its fields, each wrapped to the width:
/// its path, its last prompt and its branch, with the commits ahead of the default
/// branch and those to push once counted.
fn work_pane(item: &Item, m: &Mission, cols: usize) -> (PaneLines, PaneLines) {
    let theme = &m.snap.theme;
    let d = &item.data;
    let s = |k: &str| d[k].as_str().unwrap_or("").to_string();
    let inner = cols.saturating_sub(4);
    let tab = d["tab"]["index"]
        .as_u64()
        .map_or_else(|| "no tab".into(), |i| format!("open in tab {i}"));
    let facts = vec![Span::styled(tab, fg(theme.dim))];
    let mut head = kit::pane(&s("worktree"), facts, inner, theme);
    head.pop();
    let mut body = Vec::new();
    let mut field = |name: &str, value: &str, tone: Color| {
        for (at, l) in text::wrap(value, inner.saturating_sub(13).max(10))
            .into_iter()
            .enumerate()
        {
            let label = if at == 0 { name } else { "" };
            body.push(vec![
                Span::styled(text::pad(label, 13), fg(theme.dim)),
                Span::styled(l, fg(tone)),
            ]);
        }
    };
    field("path", &s("place"), theme.text);
    let prompt = s("prompt");
    if prompt.trim().is_empty() {
        field("last prompt", "none", theme.faint);
    } else {
        field("last prompt", &prompt.replace('\n', " "), theme.text);
    }
    field("branch", &work_branch(item), theme.text);
    (head, body)
}

/// The pane's branch line: the branch, then, once counted, how far ahead of
/// the default branch it is and what it has to push.
pub(super) fn work_branch(item: &Item) -> String {
    let d = &item.data;
    let branch = d["branch"]
        .as_str()
        .or(d["worktree"].as_str())
        .unwrap_or("")
        .to_string();
    let c = &d["counts"];
    let Some(ahead) = c["ahead"].as_u64() else {
        return branch;
    };
    let commits = |n: u64| format!("{n} commit{}", if n == 1 { "" } else { "s" });
    let push = match c["toPush"].as_u64() {
        Some(0) => "nothing to push".to_string(),
        Some(n) => format!("{} to push", commits(n)),
        None => "no upstream".to_string(),
    };
    format!(
        "{branch} · {} ahead of {} · {push}",
        commits(ahead),
        c["default"].as_str().unwrap_or("")
    )
}

/// UX 2.0's Landing view: where the stage cells start, how wide each is and
/// how far apart they start. A cell's two ends (the kit's half circles)
/// are in its width, leaving 13 columns for its words ("3rd in queue").
const STAGES_AT: usize = 40;
const STAGE_WIDTH: usize = 15;
const STAGE_STEP: usize = 16;
const STAGES: [&str; 6] = ["tests", "review", "leak check", "lock", "push", "applied"];

/// A Landing row's columns from x 2, as board 45 lays them out: the
/// worktree's name at its width on the board, then a cell for each of
/// `stages` stretching into the rest, one column apart.
fn stage_columns(cols: usize, stages: usize) -> Vec<(usize, usize)> {
    let mut wanted = vec![Col::Fixed {
        min: STAGES_AT - 2 - 1,
        natural: STAGES_AT - 2 - 1,
        cap: STAGES_AT - 2 - 1,
    }];
    wanted.extend(std::iter::repeat_n(
        Col::Stretch {
            weight: 1,
            min: STAGE_WIDTH,
            max: None,
        },
        stages,
    ));
    kit::columns(&wanted, cols.saturating_sub(3), 1, &[])
        .into_iter()
        .map(|c| c.map_or((0, 0), |(x, w)| (x + 2, w)))
        .collect()
}

/// A merge queue's stages, as its rows are drawn.
const QUEUE_STAGES: [&str; 5] = ["CI", "approved", "merge queue", "merge", "deploy test"];

/// How far a stage of a landing has got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Done,
    Running,
    Waiting,
    Queued,
    Failed,
    /// Waiting on you (Autopilot's view).
    Asks,
}

impl Stage {
    /// Its text's color, its run's background and whether the text is bold.
    fn tone(self) -> Tone {
        match self {
            Stage::Done => Tone::Done,
            Stage::Running => Tone::Working,
            Stage::Waiting => Tone::Dim,
            Stage::Queued => Tone::Landing,
            Stage::Failed => Tone::Failed,
            Stage::Asks => Tone::Needs,
        }
    }

    /// A stage by the name src/autopilot/view.rs gives its look.
    fn named(look: &str) -> Stage {
        match look {
            "done" => Stage::Done,
            "running" => Stage::Running,
            "queued" => Stage::Queued,
            "failed" => Stage::Failed,
            "asks" => Stage::Asks,
            _ => Stage::Waiting,
        }
    }
}

/// A landing's six stages from what the view holds of it (`data.landing`):
/// a worktree is in the landing queue only once its tests, review and leak
/// check are done (queue-and-clean takes the lock after them), so those are
/// done; a waiter is queued for the lock at its place; the one holding it
/// is at the step its phase names, a failure included (`tests failed …`,
/// `review blocked …`, the words after it shown in the cell), the check
/// after a clean rebase (`checking …`, `check failed`) in the tests' cell. Neither the
/// phase nor the landing's files hold a test count or the review's rounds,
/// so a done cell says only "passed" or "clean".
fn landing_stages(landing: &serde_json::Value) -> [(Stage, String); 6] {
    let done = |s: &str| (Stage::Done, format!("✓ {s}"));
    let running = |s: &str| (Stage::Running, s.to_string());
    let waiting = || (Stage::Waiting, "waiting".to_string());
    let mut out = [
        done("passed"),
        done("clean"),
        done("none"),
        waiting(),
        waiting(),
        waiting(),
    ];
    if landing.get("holding").and_then(serde_json::Value::as_bool) != Some(true) {
        let place = landing
            .get("place")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1);
        out[3] = (Stage::Queued, ordinal(place));
        return out;
    }
    out[3] = done("held");
    let phase = landing
        .get("phase")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let failed = |rest: &str, or: &str| {
        let rest = rest.trim_start_matches([':', ' ']).trim();
        (
            Stage::Failed,
            format!("✕ {}", if rest.is_empty() { or } else { rest }),
        )
    };
    if let Some(rest) = phase.strip_prefix("tests failed") {
        out[0] = failed(rest, "failed");
    } else if phase.starts_with("check failed") {
        out[0] = failed("", "check failed");
    } else if let Some(rest) = phase.strip_prefix("review blocked") {
        out[1] = failed(rest, "blocked");
    } else if phase.contains("testing") {
        out[0] = running("testing…");
    } else if phase.starts_with("checking") {
        out[0] = running("checking…");
    } else if phase.starts_with("reviewing") {
        out[1] = running("reviewing…");
    } else if phase.starts_with("pushing") {
        out[4] = running("pushing…");
    } else if phase.starts_with("applying") {
        out[4] = done("pushed");
        out[5] = running("applying…");
    } else if phase.starts_with("cleaning") {
        out[4] = done("pushed");
        out[5] = done("applied");
    }
    out
}

/// A merge queue row's stages from what the view holds of it
/// (`data.queue.state` and `position`): GitHub lets a PR into the queue only
/// once its checks pass and it is approved, so those are done; the queue
/// cell is its place, its checks running, or a failure when it can't merge;
/// merging is the merge stage running. The deploy test waits, since the row
/// leaves the queue once it merges.
fn queue_stages(queue: &serde_json::Value) -> [(Stage, String); 5] {
    let state = queue
        .get("state")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let position = queue
        .get("position")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let waiting = || (Stage::Waiting, "waiting".to_string());
    let mut out = [
        (Stage::Done, "✓ CI".to_string()),
        (Stage::Done, "✓ approved".to_string()),
        (
            Stage::Queued,
            format!("{} in queue", ordinal(position.max(1))),
        ),
        waiting(),
        waiting(),
    ];
    match state {
        "AWAITING_CHECKS" => out[2] = (Stage::Running, "checks…".into()),
        "UNMERGEABLE" => out[2] = (Stage::Failed, "✕ can't merge".into()),
        "MERGEABLE" => out[2] = (Stage::Done, "✓ ready".into()),
        "LOCKED" => {
            out[2] = (Stage::Done, "✓ ready".into());
            out[3] = (Stage::Running, "merging…".into());
        }
        _ => {}
    }
    out
}

/// `n` with its English ordinal suffix: 1st, 2nd, 11th.
fn ordinal(n: u64) -> String {
    let suffix = match (n % 10, n % 100) {
        (1, m) if m != 11 => "st",
        (2, m) if m != 12 => "nd",
        (3, m) if m != 13 => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// `text` centered in `w` columns.
fn centered(text: &str, w: usize) -> String {
    let text = text::cut(text, w);
    let left = w.saturating_sub(width(&text)) / 2;
    let right = w.saturating_sub(width(&text) + left);
    format!("{}{text}{}", " ".repeat(left), " ".repeat(right))
}

/// UX 2.0's Landing view: each repository's landing queue under its
/// heading and the stages' names, each worktree two lines, its name bold
/// with a cell per stage, then a dim note of where it is under it; the
/// merge queue's rows one line each, as before.
fn stage_rows(
    view: &View,
    cols: usize,
    rows: usize,
    m: &Mission,
    top_row: usize,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
) {
    let theme = &m.snap.theme;
    let items = &view.data.items;
    // The rows each item takes: a heading its line, then its stage names
    // and the dashed rule under them; a landing or a queued PR its row of
    // cells and its note; anything else a line.
    let height = |item: &Item| {
        if views::is_header(item) {
            if item.id.ends_with(":direct") || item.id.ends_with(":queue") {
                3
            } else {
                1
            }
        } else if item.data.get("landing").is_some_and(|l| l.is_object())
            || item.data.get("queue").is_some_and(|q| q.is_object())
        {
            2
        } else {
            1
        }
    };
    // As many as fit, from as far back of the chosen one as they can, so
    // it is always in sight.
    let budget = rows.saturating_sub(1 + lines.len());
    let cursor = (m.cursor.max(0) as usize).min(items.len().saturating_sub(1));
    let mut first = cursor;
    let mut used = items.get(cursor).map_or(0, height);
    while first > 0 && used + height(&items[first - 1]) <= budget {
        first -= 1;
        used += height(&items[first]);
    }
    let mut shown = 0;
    let room = items[first.min(items.len())..]
        .iter()
        .take_while(|i| {
            shown += height(i);
            shown <= budget
        })
        .count()
        .max(1);
    let cursor = m.cursor.max(0) as usize;
    let name_w = STAGES_AT.saturating_sub(4);
    for (index, item) in items.iter().enumerate().skip(first).take(room) {
        let chosen = index == cursor && m.cursor >= 0;
        let fill = if chosen {
            Some(theme.highlight)
        } else if m.hover == Some(Spot::Index(index as isize)) {
            Some(theme.panel)
        } else {
            None
        };
        let mut hit = |lines: &Vec<Line>| {
            hits.push(HitBox {
                y: (top_row + lines.len()) as u16,
                x0: 0,
                x1: cols.saturating_sub(1) as u16,
                spot: Spot::Index(index as isize),
            });
        };
        if views::is_header(item) {
            hit(lines);
            let count = item
                .data
                .get("count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let mut row = Row::default();
            row.put(format!(" {}", item.title), bold(theme.bright))
                .put(format!("   {count}"), fg(theme.dim));
            lines.push(row.line(cols, fill));
            let names: &[&str] = if item.id.ends_with(":direct") {
                &STAGES
            } else if item.id.ends_with(":queue") {
                &QUEUE_STAGES
            } else {
                &[]
            };
            if !names.is_empty() {
                let first = if names.len() == STAGES.len() {
                    " worktree"
                } else {
                    " pull request"
                };
                let at = stage_columns(cols, names.len());
                let mut heads = vec![Span::styled(first, fg(theme.dim))];
                for (name, &col) in names.iter().zip(&at[1..]) {
                    kit::place(
                        &mut heads,
                        vec![Span::styled(centered(name, col.1), fg(theme.dim))],
                        col,
                    );
                }
                lines.push(Line {
                    spans: fit(heads, cols, Style::new()),
                    fill: None,
                });
                lines.push(Line {
                    spans: vec![kit::head_rule(cols, theme)],
                    fill: None,
                });
            }
            continue;
        }
        let pointer = if chosen { "▌" } else { " " };
        let stages: Vec<(Stage, String)> =
            if let Some(landing) = item.data.get("landing").filter(|l| l.is_object()) {
                landing_stages(landing).to_vec()
            } else if let Some(queue) = item.data.get("queue").filter(|q| q.is_object()) {
                queue_stages(queue).to_vec()
            } else {
                Vec::new()
            };
        if stages.is_empty() {
            hit(lines);
            let mut row = Row::default();
            row.put(pointer, fg(ACCENT))
                .put(" ", Style::new())
                .put(item.title.clone(), fg(theme.text))
                .put("  ", Style::new())
                .put(item.detail.clone(), fg(theme.dim));
            lines.push(row.line(cols, fill));
            continue;
        }
        let name = item
            .title
            .split_once(". ")
            .map_or(item.title.as_str(), |(_, n)| n);
        hit(lines);
        let at = stage_columns(cols, stages.len());
        let mut top = Row::default();
        top.put(pointer, fg(ACCENT))
            .put(" ", Style::new())
            .put(text::cut(name, name_w), bold(theme.bright));
        for ((stage, label), &col) in stages.iter().zip(&at[1..]) {
            let cell = kit::cell(label, col.1, stage.tone(), m.snap.caps, theme);
            kit::place(&mut top.spans, cell, col);
        }
        lines.push(top.line(cols, fill));
        hit(lines);
        let mut under = Row::default();
        under
            .put(pointer, fg(ACCENT))
            .put(" ", Style::new())
            .put(text::cut(&item.detail, name_w), fg(theme.dim));
        lines.push(under.line(cols, fill));
    }
}

/// Whether the Keys view is drawn as UX 2.0's board (`keys_board`): with
/// UX 2.0, room for it and a key to show.
pub fn keys_board_shown(m: &Mission, v: &View, cols: usize) -> bool {
    m.snap.rules.ux2
        && v.id == ViewId::Keys
        && cols >= super::keys_board::BOARD_AT
        && v.data.items.iter().any(|i| !views::is_header(i))
        && super::keys_board::fits(&v.data.items, m.size.1 as usize)
}

/// How wide the main area must be for UX 2.0's Autopilot board (its six
/// columns 18 wide), narrower showing the plain list; and how wide for the
/// chosen card's side panel, and how many columns that takes.
pub const PILOT_BOARD_AT: usize = 108;
const PILOT_SIDE_AT: usize = 152;
const PILOT_SIDE_W: usize = 44;
/// Where a diagram of a plan that can't be drawn beside the board is shown
/// properly (`Markdown::parse`).
const PLAN_PLACE: &str = "its plan note in Notes";

/// Whether the Settings view is drawn as its board: with UX 2.0 on and the
/// screen wide enough.
pub fn settings_board_shown(m: &Mission) -> bool {
    m.snap.rules.ux2 && m.size.0 as usize >= super::settings_board::BOARD_AT
}

/// An image atom (src/ui/images.rs) a board places once its lines are
/// drawn: its name, which the next draw replaces in place, where it starts,
/// how wide and tall, and what it draws.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub name: String,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub shape: Shape,
}

/// What an image atom draws: a meter (`images::meter`) how full, in what
/// color, with a tick; a series of bars (`images::series`) scaled to
/// `max`, what is past `over` red, with its `marks` (a chart's outlined
/// part, grid and "now" edge), which one row tall is a sparkline; a
/// setting's switch (`images::switch`), on or off; or the keyboard as a heatmap (`images::keyboard`), how often each key was
/// pressed, with the keys of a group outlined; or someone's avatar
/// (`images::avatar`), their photo from the snapshot's `photos` once it
/// has loaded, else the initial of `label` on `color`.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Meter {
        frac: f32,
        color: Color,
        tick: Option<f32>,
    },
    Series {
        bars: Vec<crate::ui::images::Bar>,
        max: f32,
        over: Option<f32>,
        marks: crate::ui::images::Marks,
    },
    Switch {
        on: bool,
    },
    Keyboard {
        heat: crate::ui::images::Heat,
        outlined: crate::ui::images::Outlined,
    },
    Avatar {
        who: Option<crate::photos::Who>,
        label: String,
        color: Color,
    },
}

impl Image {
    /// A meter one row tall.
    fn meter(name: String, x: usize, y: usize, width: usize, frac: f32, color: Color) -> Image {
        Image {
            name,
            x: x as u16,
            y: y as u16,
            width: width as u16,
            height: 1,
            shape: Shape::Meter {
                frac,
                color,
                tick: None,
            },
        }
    }

    /// How full a meter is; `None` for any other shape.
    pub fn frac(&self) -> Option<f32> {
        match self.shape {
            Shape::Meter { frac, .. } => Some(frac),
            Shape::Series { .. }
            | Shape::Switch { .. }
            | Shape::Keyboard { .. }
            | Shape::Avatar { .. } => None,
        }
    }
}

/// A card's meter's color: its column's tone, red once a stage failed.
fn pilot_tone(item: &Item) -> Tone {
    if item
        .data
        .pointer("/pilot/failed")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        return Tone::Failed;
    }
    match item.group.as_deref() {
        Some("Working") => Tone::Working,
        Some("Waiting on you") => Tone::Needs,
        Some("Landing") => Tone::Landing,
        Some("Landed") => Tone::Done,
        _ => Tone::Faint,
    }
}

/// UX 2.0's Autopilot view as a board (src/autopilot/view.rs): its heading
/// (`AUTO`, the note bold, its facts dim, the button that opens its menu,
/// and today's spend against its limit as a meter at the right), a column per place an item stands, each
/// item a rounded card of its tab's number (or its place in line), a red ✕
/// once a stage failed, its first line, where it is dim, and a meter of
/// its stages done; the chosen card's edge in the accent color. A skipped
/// item's card, in the last column, is its first line dim over two lines
/// and "skipped by you", with no meter. On a screen
/// `PILOT_SIDE_AT` wide the chosen item sits beside it in full, with its
/// stages and its plan under them, and the diagrams the plan draws as
/// pictures come back, for the frame to place.
#[allow(clippy::too_many_arguments)]
fn pilot_board(
    view: &View,
    cols: usize,
    rows: usize,
    m: &Mission,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
    meters: &mut Vec<Image>,
) -> Vec<Picture> {
    const TALL: usize = 6;
    const SPEND_W: usize = 12;
    let theme = &m.snap.theme;
    let items = &view.data.items;
    let chosen = |i: usize| m.cursor >= 0 && i == m.cursor as usize;
    let lit = |i: usize| !chosen(i) && m.hover == Some(Spot::Index(i as isize));
    if let Some((at, head)) = items.iter().enumerate().find(|(_, i)| views::is_summary(i)) {
        let spend = head.data.get("spend").and_then(serde_json::Value::as_array);
        let spend_w = if spend.is_some() { SPEND_W + 2 } else { 0 };
        // Its menu's button, lit under the mouse or while Tab's focus is on
        // the heading, before the spend.
        let button_lit = m.hover == Some(Spot::Menu(at as isize))
            || (m.focus == Some(Focus::Item) && chosen(at));
        let button = kit::keycap("m", "☰ Menu", button_lit, m.snap.caps, theme);
        let button_w = spans_width(&button);
        let button_x = cols.saturating_sub(spend_w + button_w);
        hits.push(HitBox {
            y: lines.len() as u16,
            x0: button_x as u16,
            x1: (button_x + button_w).saturating_sub(1) as u16,
            spot: Spot::Menu(at as isize),
        });
        hits.push(HitBox {
            y: lines.len() as u16,
            x0: 0,
            x1: cols.saturating_sub(1) as u16,
            spot: Spot::Index(at as isize),
        });
        let mut row = Row::default();
        row.put(" AUTO ", bold(ACCENT))
            .put(head.title.clone(), bold(theme.bright))
            .put(format!("   {}", head.detail), fg(theme.dim));
        let fill = if chosen(at) {
            Some(theme.highlight)
        } else {
            lit(at).then_some(theme.panel)
        };
        let mut spans = row.fitted(cols.saturating_sub(spend_w + button_w), Style::new());
        spans.extend(button);
        if let Some(spend) = spend {
            let n = |i: usize| {
                spend
                    .get(i)
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.0)
            };
            let frac = if n(1) > 0.0 {
                (n(0) / n(1)) as f32
            } else {
                0.0
            };
            let tone = if frac >= 1.0 {
                Tone::Failed
            } else if frac >= 0.8 {
                Tone::Needs
            } else {
                Tone::Done
            };
            meters.push(Image::meter(
                "autopilot/spend".into(),
                cols.saturating_sub(SPEND_W + 1),
                lines.len(),
                SPEND_W,
                frac,
                tone.fg(theme),
            ));
            spans.push(Span::raw(" ".repeat(spend_w)));
        }
        lines.push(Line { spans, fill });
        lines.push(Row::default().line(cols, None));
    }
    let body = rows.saturating_sub(1 + lines.len());
    let side_w = beside_w(cols, PILOT_SIDE_AT, PILOT_SIDE_W);
    let about = usize::try_from(m.cursor)
        .ok()
        .and_then(|c| items.get(c))
        .filter(|_| side_w > 0)
        .map(|item| pilot_about(item, side_w, theme, ready_diagrams(m).as_ref()));
    let board_w = cols - side_w;
    if let Some(about) = &about {
        let last = about.last_top(body);
        side_hits(last, board_w + 2, side_w - 2, lines.len(), body, hits);
    }
    let about = about
        .map(|a| {
            let top = m.side_top().min(a.last_top(body));
            a.window(top, body)
        })
        .unwrap_or_default();
    // Each picture where the pane's lines put it on the screen.
    let pictures = about
        .pictures
        .iter()
        .map(|p| Picture {
            x: p.x + (board_w + 2) as u16,
            y: p.y + lines.len() as u16,
            ..p.clone()
        })
        .collect();
    let about = about.lines;
    let columns = crate::autopilot::view::COLUMNS;
    let col_w = board_w / columns.len();
    let w = col_w - 1;
    let inner = w.saturating_sub(4);
    let top = lines.len();
    let mut drawn: Vec<Vec<Vec<Span<'static>>>> = Vec::new();
    for (ci, column) in columns.iter().enumerate() {
        let in_column: Vec<usize> = (0..items.len())
            .filter(|&i| items[i].group.as_deref() == Some(*column))
            .collect();
        let mut out = vec![
            fit(
                vec![
                    Span::styled(format!(" {column}"), bold(theme.text)),
                    Span::styled(format!(" {}", in_column.len()), fg(theme.dim)),
                ],
                col_w,
                Style::new(),
            ),
            vec![Span::raw(" ".repeat(col_w))],
        ];
        let fits = body.saturating_sub(2) / TALL;
        let chosen_at = in_column.iter().position(|&i| chosen(i));
        let first = match chosen_at {
            Some(c) if c >= fits => c + 1 - fits,
            _ => 0,
        };
        for (shown, &index) in in_column.iter().enumerate().skip(first).take(fits) {
            let item = &items[index];
            let edge = kit::card_edge(chosen(index), lit(index).then_some(theme.text), theme);
            let pilot = item.data.get("pilot");
            let skipped = item.data.get("skipped").is_some();
            let (label, where_, frac) = match pilot {
                // A skipped item has no place in line and no stages.
                None if skipped => (String::new(), "skipped by you".to_string(), 0.0),
                Some(p) if p["closed"] == true => (
                    "closed".to_string(),
                    item.detail.clone(),
                    p["done"].as_f64().unwrap_or(0.0) as f32 / 8.0,
                ),
                Some(p) => (
                    format!("tab {}", p["index"]),
                    item.detail
                        .split_once(" · ")
                        .map_or(item.detail.as_str(), |(_, w)| w)
                        .to_string(),
                    p["done"].as_f64().unwrap_or(0.0) as f32 / 8.0,
                ),
                None => {
                    let place = item.data["place"].as_u64().unwrap_or(1);
                    (
                        format!("{place}."),
                        if place == 1 {
                            "opens next".to_string()
                        } else {
                            format!("{} in line", ordinal(place))
                        },
                        0.0,
                    )
                }
            };
            let name = pilot
                .and_then(|p| p["item"].as_str())
                .or_else(|| item.data["next"].as_str())
                .unwrap_or(&item.title);
            let failed = pilot.and_then(|p| p["failed"].as_bool()) == Some(true);
            let name_style = if chosen(index) {
                bold(theme.bright)
            } else if skipped {
                fg(theme.dim)
            } else {
                fg(theme.text)
            };
            // A skipped card has no meter, so its item takes two lines, and
            // where it is the one the meter has.
            let inside = if skipped {
                let wrapped = text::wrap(name, inner);
                let rest = wrapped.get(1..).unwrap_or_default().join(" ");
                vec![
                    vec![Span::styled(
                        wrapped.first().cloned().unwrap_or_default(),
                        name_style,
                    )],
                    vec![Span::styled(text::cut(&rest, inner), name_style)],
                    vec![Span::styled(text::cut(&where_, inner), fg(theme.faint))],
                ]
            } else {
                vec![
                    vec![Span::styled(text::cut(name, inner), name_style)],
                    vec![Span::styled(text::cut(&where_, inner), fg(theme.dim))],
                ]
            };
            let card = kit::card(
                w,
                TALL - 1,
                vec![Span::styled(label, bold(ACCENT))],
                if failed {
                    vec![kit::mark(Mark::Failed, 0, theme)]
                } else {
                    Vec::new()
                },
                &inside,
                edge,
            );
            out.extend(card.into_iter().map(|mut line| {
                line.push(Span::raw(" "));
                line
            }));
            out.push(vec![Span::raw(" ".repeat(col_w))]);
            let y = top + 2 + (shown - first) * TALL;
            if !skipped && y + 3 < top + body {
                meters.push(Image::meter(
                    format!("autopilot/{}", item.id),
                    ci * col_w + 2,
                    y + 3,
                    inner,
                    frac,
                    pilot_tone(item).fg(theme),
                ));
            }
            for dy in 0..TALL - 1 {
                hits.push(HitBox {
                    y: (y + dy) as u16,
                    x0: (ci * col_w) as u16,
                    x1: (ci * col_w + w).saturating_sub(1) as u16,
                    spot: Spot::Index(index as isize),
                });
            }
        }
        if in_column.is_empty() {
            out.push(fit(
                vec![Span::styled(" none", fg(theme.faint))],
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
        let mut spans = fit(spans, board_w, Style::new());
        if side_w > 0 {
            spans.push(Span::styled("│ ", fg(theme.faint)));
            spans.extend(fit(
                about.get(i).cloned().unwrap_or_default(),
                side_w - 2,
                Style::new(),
            ));
        }
        lines.push(Line { spans, fill: None });
    }
    pictures
}

/// The chosen item beside the board: its first line in full, bold and
/// wrapped, where it stands dim, a rule, then a tab's eight stages, each
/// its name and its words in its stage's color; under them, for a card, a
/// rule and its plan, as a ticket's description sits beside the Tickets
/// board: the plan note's name dim, then its Markdown, whole, for the
/// board to show the part that fits. A card whose item links no plan says so, faint, and so does one
/// whose plan note isn't there, and a tab left from a note worked before,
/// whose plans aren't read.
fn pilot_about(item: &Item, side_w: usize, theme: &Theme, images: Option<&Images>) -> About {
    let inner = side_w.saturating_sub(3);
    let name = item
        .data
        .pointer("/pilot/item")
        .and_then(serde_json::Value::as_str)
        .or_else(|| item.data["next"].as_str())
        .unwrap_or(&item.title);
    let mut out: Vec<Vec<Span<'static>>> = text::wrap(name, inner)
        .into_iter()
        .map(|l| vec![Span::styled(l, bold(theme.bright))])
        .collect();
    let detail = match item.data["place"].as_u64() {
        Some(1) => "Opens next".to_string(),
        Some(n) => format!("Opens {} in line", ordinal(n)),
        None => item.detail.clone(),
    };
    for l in text::wrap(&detail, inner) {
        out.push(vec![Span::styled(l, fg(theme.dim))]);
    }
    let stages = item
        .data
        .pointer("/pilot/stages")
        .and_then(serde_json::Value::as_array);
    if stages.is_some() {
        out.push(vec![kit::rule(inner, theme)]);
    }
    for (name, cell) in crate::autopilot::view::STAGES
        .iter()
        .zip(stages.into_iter().flatten())
    {
        let look = cell
            .get(0)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let words = cell
            .get(1)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        out.push(vec![
            Span::styled(text::pad(name, 12), fg(theme.dim)),
            Span::styled(words.to_string(), Stage::named(look).tone().style(theme)),
        ]);
    }
    let is_a_card = item.group.is_some();
    if !is_a_card {
        return About {
            lines: out,
            pictures: Vec::new(),
        };
    }
    out.push(vec![kit::rule(inner, theme)]);
    let plan_name = item.data["planName"].as_str();
    let said = |words: String, tone: Color, out: &mut Vec<Vec<Span<'static>>>| {
        for l in text::wrap(&words, inner) {
            out.push(vec![Span::styled(l, fg(tone))]);
        }
    };
    match (plan_name, item.data["plan"].as_str()) {
        (Some(name), Some(plan)) if !plan.trim().is_empty() => {
            said(name.to_string(), theme.dim, &mut out);
            out.push(Vec::new());
            with_description(out, plan, PLAN_PLACE, inner, theme, images)
        }
        (name, plan) => {
            let words = match (name, plan) {
                (Some(name), None) => format!("Plan not found: {name}"),
                (Some(name), Some(_)) => format!("Nothing in {name} yet"),
                (None, _) if item.data["planElsewhere"] == true => {
                    "Its plan is with another note".to_string()
                }
                (None, _) => "No plan".to_string(),
            };
            said(words, theme.faint, &mut out);
            About {
                lines: out,
                pictures: Vec::new(),
            }
        }
    }
}

// ── The CI view ─────────────────────────────────────────────────────────────

/// The lines the CI pane leaves above the line that says why when it
/// first shows a check (`Mission::side_start`).
pub const CI_ABOVE_WHY: usize = 6;
/// The CI list's width beside the pane, at its narrowest and widest.
const CI_LIST_MIN: usize = 56;
const CI_LIST_MAX: usize = 84;

/// The tab open on PR `number` of `repo`, as `choices::pr_places` finds it,
/// so #42 in one repository never names another's tab; a PR with no
/// repository matches by number.
pub fn tab_of_pr<'a>(tabs: &'a [TabModel], number: u64, repo: &str) -> Option<&'a TabModel> {
    let in_repo = |t: &TabModel| {
        repo.is_empty()
            || t.pr_repo
                .as_deref()
                .or(t.repo.as_deref())
                .is_some_and(|r| r.eq_ignore_ascii_case(repo))
    };
    tabs.iter()
        .find(|t| number > 0 && t.pr == Some(number) && in_repo(t))
}

/// A CI item's PR number and repository.
pub fn ci_pr(item: &Item) -> (u64, &str) {
    let ci = &item.data["ci"];
    (
        ci["number"].as_u64().unwrap_or(0),
        ci["repo"].as_str().unwrap_or(""),
    )
}

/// How long a check ran: "45s", "4m 12s", "1h 3m".
fn ran_for(secs: i64) -> String {
    match secs.max(0) {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m {}s", s / 60, s % 60),
        s => format!("{}h {}m", s / 3600, s % 3600 / 60),
    }
}

/// The pane's row of a check's line that says why: under the pane's head,
/// at its place in the step; `None` when its log wasn't read.
pub fn ci_why_row(item: &Item, check: usize) -> Option<usize> {
    let checks = views::ci_checks(item);
    let at = checks.get(check)?.pointer("/step/whyAt")?.as_u64()?;
    Some(3 + at as usize)
}

/// A failed check's line that says why, numbered as in its whole log.
fn ci_why_marked(check: &serde_json::Value, inner: usize, theme: &Theme) -> Vec<Span<'static>> {
    let step = &check["step"];
    match (step["whyAt"].as_u64(), step["first"].as_u64()) {
        (Some(at), Some(first)) => {
            let n = (first + at) as usize;
            let why = check["why"].as_str().unwrap_or("");
            kit::marked(n, n.to_string().len(), why, inner, theme)
        }
        _ => vec![Span::styled("Its log wasn't read.", fg(theme.faint))],
    }
}

/// The chosen failed check beside the CI list: its name, a dim line of its
/// PR, run, how long it ran and its failing step's name, a rule, then the
/// step's lines numbered as in the whole log, the line that says why
/// marked (`kit::marked`); under them each other failed check of the PR,
/// its name and its own line that says why.
fn ci_about(item: &Item, chosen: usize, side_w: usize, theme: &Theme) -> About {
    let inner = side_w.saturating_sub(3);
    let checks = views::ci_checks(item);
    let Some(check) = checks.get(chosen) else {
        return About::default();
    };
    let ci = &item.data["ci"];
    let step = &check["step"];
    let mut facts: Vec<String> = Vec::new();
    match ci["number"].as_u64().filter(|n| *n > 0) {
        Some(n) => facts.push(format!("#{n}")),
        None => facts.extend(ci["branch"].as_str().map(str::to_string)),
    }
    if let Some(run) = check["run"].as_str() {
        facts.push(format!("run {run}"));
    }
    if let Some(secs) = check["secs"].as_i64() {
        facts.push(ran_for(secs));
    }
    if let Some(name) = step["name"].as_str().filter(|n| !n.is_empty()) {
        facts.push(format!("the failing step, {name}"));
    } else if step.is_object() {
        facts.push("the log's last lines".into());
    }
    let name = check["name"].as_str().unwrap_or("check");
    let mut lines = kit::pane(
        name,
        vec![Span::styled(facts.join(" · "), fg(theme.dim))],
        inner,
        theme,
    );
    match step["lines"].as_array() {
        Some(step_lines) => {
            let first = step["first"].as_u64().unwrap_or(1) as usize;
            let why_at = step["whyAt"].as_u64().map(|n| n as usize);
            let num_w = (first + step_lines.len()).to_string().len();
            for (i, text) in step_lines.iter().enumerate() {
                let text = text.as_str().unwrap_or("");
                if Some(i) == why_at {
                    lines.push(kit::marked(first + i, num_w, text, inner, theme));
                    continue;
                }
                lines.push(vec![
                    Span::styled(format!(" {:>num_w$} ", first + i), fg(theme.faint)),
                    Span::styled(text.to_string(), fg(theme.text)),
                ]);
            }
        }
        None if check["job"].is_string() => lines.push(vec![Span::styled(
            "Its log wasn't read this look.",
            fg(theme.faint),
        )]),
        None => lines.push(vec![Span::styled(
            "Not a GitHub Actions job, with no log to read here: M-r opens it in the browser.",
            fg(theme.faint),
        )]),
    }
    for (_, other) in checks.iter().enumerate().filter(|(i, _)| *i != chosen) {
        lines.push(Vec::new());
        lines.push(vec![kit::head_rule(inner, theme)]);
        lines.push(vec![
            kit::mark(Mark::Failed, 0, theme),
            Span::raw(" "),
            Span::styled(
                other["name"].as_str().unwrap_or("check").to_string(),
                bold(theme.text),
            ),
        ]);
        lines.push(ci_why_marked(other, inner, theme));
    }
    About {
        lines,
        pictures: Vec::new(),
    }
}

/// The spans lit under the mouse, on the panel's surface.
fn lit(spans: Vec<Span<'static>>, theme: &Theme) -> Vec<Span<'static>> {
    spans
        .into_iter()
        .map(|s| {
            let style = s.style.bg(theme.panel);
            Span::styled(s.content, style)
        })
        .collect()
}

/// A CI row's left part and its right (the PR's tab), `cols` wide.
fn with_right(
    left: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    cols: usize,
) -> Vec<Span<'static>> {
    let right_w = spans_width(&right);
    let mut out = fit(left, cols.saturating_sub(right_w + 1), Style::new());
    out.extend(right);
    out.push(Span::raw(" "));
    out
}

/// With UX 2.0, the CI view: its sections' labels with their counts; each
/// failing PR its mark, number, title and tab, a dim line of its
/// repository, branch and checks failed, and a line for each failed check
/// with why it failed; each running or passing PR a line. On a wide
/// screen the chosen check's failing step is a pane beside the list; on a
/// narrow one under it.
fn ci_rows(
    view: &View,
    cols: usize,
    rows: usize,
    m: &Mission,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
) {
    let theme = &m.snap.theme;
    if let Some(e) = &view.data.error {
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
    let body = rows.saturating_sub(1 + top);
    let wide = cols >= SIDE_AT;
    let list_w = if wide {
        (cols * 2 / 5).clamp(CI_LIST_MIN, CI_LIST_MAX)
    } else {
        cols
    };
    let cursor = usize::try_from(m.cursor).ok();
    let check = m.chosen_check();
    let chosen_item = cursor.and_then(|c| view.data.items.get(c));
    let shows_pane = chosen_item.is_some_and(|i| !views::ci_checks(i).is_empty());
    // The list's lines, each with what a click on it chooses.
    let mut entries: Vec<(Vec<Span<'static>>, Option<Spot>)> = Vec::new();
    let mut chosen_line = 0;
    for (index, item) in view.data.items.iter().enumerate() {
        if views::is_header(item) {
            let count = item.data["count"].as_u64().unwrap_or(0) as usize;
            let tone = match item.id.strip_prefix("header:") {
                Some("running") => Tone::Working,
                Some("passing") => Tone::Done,
                _ => Tone::Failed,
            };
            if !entries.is_empty() {
                entries.push((Vec::new(), None));
            }
            entries.push((
                kit::label(&item.title, tone, Some(count), list_w, theme),
                None,
            ));
            continue;
        }
        let at = index as isize;
        let is_chosen = cursor == Some(index);
        let row_chosen = is_chosen && check.is_none();
        if row_chosen {
            chosen_line = entries.len();
        }
        let (number, repo) = ci_pr(item);
        let state = views::ci_state(item);
        let mark = match state {
            "running" => kit::mark(Mark::Working, m.snap.frame, theme),
            "passing" => kit::mark(Mark::Passed, 0, theme),
            _ => kit::mark(Mark::Failed, 0, theme),
        };
        let title = item
            .title
            .split_once(' ')
            .filter(|(n, _)| n.starts_with('#'))
            .map_or(item.title.as_str(), |(_, t)| t);
        let mut left = vec![mark, Span::raw(" ")];
        if number > 0 {
            left.push(Span::styled(
                format!("#{number} "),
                Tone::Accent.style(theme),
            ));
        }
        left.push(Span::styled(title.to_string(), Tone::Text.bold(theme)));
        // A running or passing PR's counts sit at the right, before its
        // tab, so a long title is cut rather than them.
        let mut right = Vec::new();
        if state != "failing" {
            right.push(Span::styled(format!("{}  ", item.detail), fg(theme.dim)));
        }
        match (number, m.pr_tab(number, repo)) {
            (0, _) => {}
            (_, Some(t)) => right.push(Span::styled(format!("tab {}", t.index), fg(theme.dim))),
            (_, None) => right.push(Span::styled("no tab", fg(theme.faint))),
        }
        let spot = Spot::Index(at);
        let hover = m.hover == Some(spot);
        let line = kit::row(
            with_right(left, right, list_w - 1),
            list_w,
            row_chosen,
            theme,
        );
        entries.push((if hover { lit(line, theme) } else { line }, Some(spot)));
        if state != "failing" {
            continue;
        }
        let ci = &item.data["ci"];
        let mut facts: Vec<String> = [ci["repo"].as_str(), ci["branch"].as_str()]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if let (Some(f), Some(t)) = (ci["failed"].as_u64(), ci["total"].as_u64()) {
            facts.push(format!("{f} of {t} failed"));
        }
        let line = kit::row(
            vec![Span::styled(
                format!("  {}", facts.join(" · ")),
                fg(theme.dim),
            )],
            list_w,
            row_chosen,
            theme,
        );
        entries.push((if hover { lit(line, theme) } else { line }, Some(spot)));
        for (n, c) in views::ci_checks(item).into_iter().enumerate() {
            let check_chosen = is_chosen && check == Some(n);
            if check_chosen {
                chosen_line = entries.len();
            }
            let spot = Spot::Check(at, n);
            let line = kit::row(
                vec![
                    Span::raw("  "),
                    kit::mark(Mark::Failed, 0, theme),
                    Span::raw(" "),
                    Span::styled(
                        c["name"].as_str().unwrap_or("check").to_string(),
                        fg(theme.text),
                    ),
                    Span::styled(
                        format!("  {}", c["why"].as_str().unwrap_or("")),
                        fg(theme.dim),
                    ),
                ],
                list_w,
                check_chosen,
                theme,
            );
            let line = if m.hover == Some(spot) {
                lit(line, theme)
            } else {
                line
            };
            entries.push((line, Some(spot)));
        }
    }
    // The pane: beside the list on a wide screen, else under it, taking
    // what the list leaves of the screen's lower half.
    let pane_w = if wide { cols - list_w } else { cols };
    let list_h = if wide || !shows_pane {
        body
    } else {
        entries.len().max(1).min(body / 2)
    };
    let pane_top = if wide { top } else { top + list_h + 1 };
    let pane_h = if wide {
        body
    } else {
        body.saturating_sub(list_h + 1)
    };
    let about = chosen_item
        .filter(|_| shows_pane && pane_h > 0)
        .map(|item| ci_about(item, check.unwrap_or(0), pane_w, theme));
    if let Some(about) = &about {
        let x = if wide { list_w + 2 } else { 2 };
        side_hits(
            about.last_top(pane_h),
            x,
            pane_w - 2,
            pane_top,
            pane_h,
            hits,
        );
    }
    let about = about
        .map(|a| {
            let top = m.side_top().min(a.last_top(pane_h));
            a.window(top, pane_h)
        })
        .unwrap_or_default()
        .lines;
    let first = (chosen_line + 1).saturating_sub(list_h);
    let shown: Vec<_> = entries.into_iter().skip(first).take(list_h).collect();
    for (i, (_, spot)) in shown.iter().enumerate() {
        if let Some(spot) = spot {
            hits.push(HitBox {
                y: (top + i) as u16,
                x0: 0,
                x1: list_w.saturating_sub(1) as u16,
                spot: *spot,
            });
        }
    }
    for i in 0..list_h {
        let mut spans = fit(
            shown.get(i).map(|(l, _)| l.clone()).unwrap_or_default(),
            list_w,
            Style::new(),
        );
        if wide {
            spans.push(kit::rule_v(theme));
            spans.push(Span::raw(" "));
            spans.extend(fit(
                about.get(i).cloned().unwrap_or_default(),
                pane_w - 2,
                Style::new(),
            ));
        }
        lines.push(Line { spans, fill: None });
    }
    if !wide && pane_h > 0 {
        lines.push(Line {
            spans: vec![kit::rule(cols, theme)],
            fill: None,
        });
        for i in 0..pane_h {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(fit(
                about.get(i).cloned().unwrap_or_default(),
                cols - 2,
                Style::new(),
            ));
            lines.push(Line { spans, fill: None });
        }
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

/// The keys the footer names for the view in front, each a key and what it
/// does (a pair with no key is words of its own).
fn hints(m: &Mission, view: Option<&View>, beside: bool) -> Vec<(String, String)> {
    let pairs = |p: &[(&str, &str)]| -> Vec<(String, String)> {
        p.iter()
            .map(|(k, w)| (k.to_string(), w.to_string()))
            .collect()
    };
    if m.jump.is_some() {
        return pairs(&[
            ("", "type a view's name"),
            ("↵", "go there"),
            ("esc", "cancel"),
        ]);
    }
    // While a filter types, letters are the filter's, not keys, and what
    // is typed leads the line: the bar it ends may be too wide to show it.
    if let Some(f) = m.filter.as_ref().filter(|f| f.typing) {
        let mut out = vec![(format!("/{}▏", f.text), String::new())];
        out.extend(pairs(&[("↑↓", "move")]));
        out.extend(if m.picker {
            pairs(&[("↵", "actions"), ("esc", "close")])
        } else {
            pairs(&[("↵", "keep it"), ("esc", "clear")])
        });
        return out;
    }
    // With UX 2.0 the bar names the keys through its groups and views.
    let next: &[(&str, &str)] = if m.snap.tab_focus && m.snap.rules.ux2 {
        &[("tab", "focus")]
    } else if m.snap.tab_focus {
        &[("[ ]", "views"), ("tab", "focus")]
    } else {
        &[("tab", "next view")]
    };
    let close: &[(&str, &str)] = &[("?", "keys"), ("esc", "close")];
    let with = |first: &[(&str, &str)]| {
        let mut out = pairs(first);
        if beside {
            out.extend(pairs(&[("J K", "scroll")]));
        }
        out.extend(pairs(next));
        out.extend(pairs(close));
        out
    };
    let Some(view) = view else {
        return with(&[
            ("hjkl", "move"),
            ("↵", "actions (↵↵ go to tab)"),
            ("/", "filter"),
        ]);
    };
    match view.id {
        ViewId::Settings if settings_board_shown(m) => with(&[
            ("hl", "category"),
            ("jk", "option"),
            ("↵", "change"),
            ("/", "search every category"),
            ("e", "open the file"),
        ]),
        ViewId::Settings => with(&[("jk", "move"), ("↵", "change"), ("/", "search")]),
        ViewId::Keys if keys_board_shown(m, view, m.size.0 as usize) => {
            let mut out = pairs(&[
                ("hl", "group"),
                ("jk", "key"),
                ("↵", "press it here"),
                ("/", "filter keys, words, tags"),
                ("?", "press a key to find it"),
            ]);
            out.extend(pairs(next));
            out.extend(pairs(&[("esc", "close")]));
            out
        }
        ViewId::Keys => {
            let mut out = pairs(&[
                ("jk", "move"),
                ("↵", "press it"),
                ("/", "filter"),
                ("?", "find a key"),
            ]);
            out.extend(pairs(next));
            out.extend(pairs(&[("esc", "close")]));
            out
        }
        ViewId::Calendar => with(&[
            ("jk", "events"),
            ("hl", "day"),
            ("↵", "menu"),
            ("o", "join"),
            ("y n m", "reply"),
        ]),
        ViewId::Usage if m.snap.rules.ux2 && m.size.0 as usize >= super::usage_board::BOARD_AT => {
            with(&[
                ("jk", "tab or service"),
                ("↵", "go to the tab"),
                ("w", if m.usage_week { "today" } else { "this week" }),
                ("/", "filter"),
            ])
        }
        ViewId::Autopilot if m.snap.rules.ux2 && m.size.0 as usize >= PILOT_BOARD_AT => with(&[
            ("hl", "column"),
            ("jk", "move"),
            ("↵", "actions"),
            ("m", "menu"),
        ]),
        ViewId::Autopilot if pilot_menu_shown(view) => with(&[
            ("jk", "move"),
            ("↵", "actions"),
            ("m", "menu"),
            ("/", "filter"),
        ]),
        ViewId::Ci if m.snap.rules.ux2 => with(&[
            ("jk", "move"),
            ("l", "log"),
            ("o", "tab"),
            ("M-r", "browser"),
            ("↵", "actions"),
            ("/", "filter"),
        ]),
        ViewId::Notifications if m.snap.notice_counts.is_some() => with(&[
            ("jk", "move"),
            ("↵", "go to it"),
            ("m", "menu"),
            ("x", "dismiss"),
            ("1-7", "kind"),
            ("/", "filter"),
        ]),
        ViewId::Resources => with(&[
            ("jk", "move"),
            ("↵", "menu: go to the tab, end a process"),
            ("s", "sort by CPU or memory"),
            ("/", "filter"),
        ]),
        ViewId::Prs | ViewId::Tickets => with(&[
            ("hl", "column"),
            ("jk", "move"),
            ("↵", "what to do with it"),
        ]),
        ViewId::Plugin(_) => with(&[("jk", "move"), ("↵", "open"), ("r", "refresh")]),
        // A scope set says so on its key, which clears it.
        ViewId::WorkMap if m.snap.rules.ux2 && !m.picker => with(&[
            ("↵", "go"),
            (
                "o",
                if m.scope == Some(Scope::Tabs) {
                    "show all"
                } else {
                    "only open"
                },
            ),
            (
                "n",
                if m.scope == Some(Scope::Untabbed) {
                    "show all"
                } else {
                    "only no tab"
                },
            ),
            ("z", "fold"),
            ("/", "filter"),
            ("m", "menu"),
        ]),
        ViewId::WorkMap => with(&[("jk", "move"), ("↵", "actions, or fold"), ("/", "filter")]),
        _ => with(&[("jk", "move"), ("↵", "actions"), ("/", "filter")]),
    }
}

/// The Notifications view as a history by day, with UX 2.0 (`history`):
/// a keycap for each kind with its week's count, the one shown lit, and
/// the week's count and the sidebar's at the right; the column heads over
/// a dashed rule; then each day's label and its notifications, a row each:
/// its time, its kind's mark, its title (bright while the sidebar shows
/// it), what it led to, and whether the sidebar still shows it.
#[allow(clippy::too_many_arguments)]
fn notice_board(
    view: &View,
    counts: &super::history::Counts,
    cols: usize,
    rows: usize,
    m: &Mission,
    top_row: usize,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
) {
    use crate::sidebar::history::Kind;
    let theme = &m.snap.theme;
    let caps = m.snap.caps;
    // The keycaps.
    let mut spans = vec![Span::raw(" ")];
    let mut x = 1;
    for key in 1..=7 {
        let name = Kind::of_key(key).map_or("All", Kind::name);
        let lit = m.notice_kind == key;
        let under = m.hover == Some(Spot::Kind(key)) || m.focus == Some(Focus::Kind(key));
        let mut cap = kit::keycap(
            &key.to_string(),
            &format!("{name} {}", counts.keys[key - 1]),
            lit,
            caps,
            theme,
        );
        if under && !lit {
            cap = cap
                .into_iter()
                .map(|s| {
                    let style = s.style.bg(kit::Surface::Raised.color(theme));
                    Span::styled(s.content, style)
                })
                .collect();
        }
        let w = spans_width(&cap);
        hits.push(HitBox {
            y: (top_row + lines.len()) as u16,
            x0: x as u16,
            x1: (x + w).saturating_sub(1) as u16,
            spot: Spot::Kind(key),
        });
        x += w;
        spans.extend(cap);
    }
    let total = format!(
        "{} this week · {} in the sidebar ",
        counts.week, counts.shown
    );
    let gap = cols.saturating_sub(x + width(&total));
    if gap > 0 {
        spans.push(Span::raw(" ".repeat(gap)));
        spans.push(Span::styled(total, Tone::Dim.style(theme)));
    }
    lines.push(Line {
        spans: kit::fit(spans, cols, Style::new()),
        fill: None,
    });
    lines.push(Row::default().line(cols, None));
    // The column heads.
    const WHEN: usize = 6;
    const SIDEBAR: usize = 10;
    let rest = cols.saturating_sub(1 + WHEN + 3 + SIDEBAR);
    let led_w = (rest * 2 / 5).min(36);
    let what_w = rest.saturating_sub(led_w + 2);
    let head = |text: &str| Span::styled(text.to_uppercase(), Tone::Dim.bold(theme));
    let pad = |n: usize| Span::raw(" ".repeat(n));
    lines.push(Line {
        spans: kit::fit(
            vec![
                pad(2),
                head(&format!("{:>4}", "When")),
                pad(WHEN - 3),
                head(&text::pad("What", what_w)),
                pad(2),
                head(&text::pad("What it led to", led_w)),
                pad(1),
                head(&format!("{:>w$}", "Sidebar", w = SIDEBAR - 1)),
            ],
            cols,
            Style::new(),
        ),
        fill: None,
    });
    lines.push(Line {
        spans: vec![
            Span::raw(" "),
            kit::head_rule(cols.saturating_sub(2), theme),
            Span::raw(" "),
        ],
        fill: None,
    });
    let items = &view.data.items;
    if items.is_empty() {
        let empty = view.data.empty.clone().unwrap_or_default();
        lines.push(
            Row::default()
                .put(format!("  {empty}"), fg(theme.dim))
                .take()
                .line(cols, None),
        );
        return;
    }
    let room = rows.saturating_sub(1 + lines.len()).max(1);
    let cursor = m.cursor.max(0) as usize;
    let first = (cursor + 1).saturating_sub(room);
    for (index, item) in items.iter().enumerate().skip(first).take(room) {
        let chosen = index == cursor && m.cursor >= 0;
        let lit = !chosen && m.hover == Some(Spot::Index(index as isize));
        hits.push(HitBox {
            y: (top_row + lines.len()) as u16,
            x0: 0,
            x1: cols.saturating_sub(1) as u16,
            spot: Spot::Index(index as isize),
        });
        let spans: Vec<Span<'static>> = if views::is_header(item) {
            let count = item
                .data
                .get("count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as usize;
            kit::label(
                &item.title,
                Tone::Dim,
                Some(count),
                cols.saturating_sub(1),
                theme,
            )
        } else if super::history::fold_of(item).is_some() {
            vec![
                Span::styled(
                    format!(" ▸ {}", item.title.to_uppercase()),
                    Tone::Dim.bold(theme),
                ),
                Span::styled(format!("   {}", item.detail), Tone::Faint.style(theme)),
            ]
        } else {
            let (sidebar, led_to) = super::history::notice_of(item).unwrap_or_default();
            let shown = sidebar == crate::sidebar::history::Sidebar::Shown.word();
            let at = item
                .time
                .map(|t| crate::util::local_at_ms(t).strftime("%-H:%M").to_string())
                .unwrap_or_default();
            let (source, _) = super::choices::notice_of(item);
            let mark = match crate::sidebar::history::kind_of(&source) {
                _ if item.state == State::Attention => Mark::Failed,
                Some(Kind::Landed) => Mark::Landing,
                Some(Kind::Prs) => Mark::Passed,
                Some(Kind::Slack) => Mark::Mention,
                Some(Kind::Inbox) => Mark::Comment,
                Some(Kind::Calendar) => Mark::Reminder,
                Some(Kind::Notes) => Mark::Note,
                None => Mark::Idle,
            };
            let title = if shown {
                Tone::Text.style(theme).fg(theme.bright)
            } else {
                Tone::Dim.style(theme)
            };
            let state = if shown { Tone::Accent } else { Tone::Faint };
            vec![
                Span::styled(format!("{at:>5} "), Tone::Faint.style(theme)),
                kit::mark(mark, m.snap.frame, theme),
                Span::raw(" "),
                Span::styled(text::pad(&text::cut(&item.title, what_w), what_w), title),
                pad(2),
                Span::styled(
                    text::pad(&text::cut(led_to, led_w), led_w),
                    Tone::Dim.style(theme),
                ),
                pad(1),
                Span::styled(
                    format!("{sidebar:>w$}", w = SIDEBAR - 1),
                    state.style(theme),
                ),
            ]
        };
        let mut line = kit::row(spans, cols, chosen, theme);
        if lit {
            line = line
                .into_iter()
                .map(|s| {
                    let style = s.style.bg(theme.panel);
                    Span::styled(s.content, style)
                })
                .collect();
        }
        lines.push(Line {
            spans: line,
            fill: None,
        });
    }
}

/// Whether the Autopilot view shows the mode, whose menu `m` opens.
pub(super) fn pilot_menu_shown(view: &View) -> bool {
    view.data
        .items
        .iter()
        .any(|i| i.id == crate::autopilot::view::HEADING)
}

/// The key a footer's label names: its arrows and words as the keys
/// they are, a single key as it is written; none for a pair (`jk`, `J K`),
/// whose rows and pane take the click themselves.
fn footer_key(label: &str) -> Option<crate::mux::input::Key> {
    let name = match label {
        "↵" => "Enter",
        "esc" => "Escape",
        "tab" => "Tab",
        l if l.chars().count() == 1 || l.starts_with("M-") => l,
        _ => return None,
    };
    crate::mux::input::Key::parse(name)
}

/// UX 2.0's footer whose keys take a click (the CI, Work map and Resources views'): the kit's
/// hints, each key that names one a spot (`Spot::Key`) at row `y`, lit
/// under the mouse.
fn key_footer(
    pairs: &[(String, String)],
    m: &Mission,
    theme: &Theme,
    y: u16,
    hits: &mut Vec<HitBox>,
) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut x = 1;
    for (key, what) in pairs {
        let p: [(&str, &str); 1] = [(key.as_str(), what.as_str())];
        let mut spans = kit::hints(&p, theme);
        if !out.is_empty() {
            out.push(Span::raw("  "));
            x += 2;
        }
        if let Some(k) = footer_key(key) {
            let w = width(key);
            hits.push(HitBox {
                y,
                x0: x as u16,
                x1: (x + w).saturating_sub(1) as u16,
                spot: Spot::Key(k),
            });
            if m.hover == Some(Spot::Key(k)) {
                spans[0].style = spans[0].style.add_modifier(Modifier::REVERSED);
            }
        }
        x += spans_width(&spans);
        out.extend(spans);
    }
    out
}

/// The footer's keys: with UX 2.0 the kit's hints, each key in the accent;
/// else one dim line, as before.
fn footer(pairs: &[(String, String)], ux2: bool, theme: &Theme) -> Vec<Span<'static>> {
    if ux2 {
        let p: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(k, w)| (k.as_str(), w.as_str()))
            .collect();
        return kit::hints(&p, theme);
    }
    let said: Vec<String> = pairs
        .iter()
        .map(|(k, w)| match (k.is_empty(), w.is_empty()) {
            (true, _) => w.clone(),
            (_, true) => k.clone(),
            _ => format!("{k} {w}"),
        })
        .collect();
    vec![Span::styled(said.join("  "), fg(theme.dim))]
}

/// The bar's first row, with easy mode's × at its right end when it has
/// one.
fn with_close(head: Row, m: &Mission, cols: usize, hits: &mut Vec<HitBox>) -> Line {
    if !(m.snap.close && cols > 8) {
        return head.line(cols, None);
    }
    let mut spans = head.fitted(cols - width(CLOSE) - 1, Style::new());
    let lit = m.focus == Some(Focus::Close) || m.hover == Some(Spot::Close);
    let mut style = bold(m.snap.theme.bright);
    if lit {
        style = style.add_modifier(Modifier::REVERSED);
    }
    spans.push(Span::styled(CLOSE, style));
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
    Line { spans, fill: None }
}

/// What the bar shows besides the views' names: the view a name being
/// typed matches, the tabs' counts, the filter, and whether the filter
/// takes a click.
struct Bar {
    jumped: Option<usize>,
    counts: Vec<Span<'static>>,
    filter: Vec<Span<'static>>,
    filter_hit: bool,
}

/// With UX 2.0, the bar as two rows and a rule (board 3 of the canvas):
/// the groups that have a view, the one in front lit, then the tabs'
/// counts; that group's views, the one in front lit and underlined, then
/// the filter, and the keys that move through them at the right. While a
/// view's name is typed, the group it is in shows, the view lit as it is
/// matched.
fn grouped_bar(
    m: &Mission,
    groups: &[(views::Group, Vec<usize>)],
    bar: Bar,
    cols: usize,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
) {
    let snap = &m.snap;
    let theme = &snap.theme;
    let caps = snap.caps;
    let shown_view = bar.jumped.unwrap_or(match m.focus {
        Some(Focus::View(v)) => v,
        _ => m.view,
    });
    let shown = groups
        .iter()
        .find(|(_, held)| held.contains(&shown_view))
        .or(groups.first());
    let mut head = Row::default();
    head.put(" ", Style::new())
        .put("MISSION CONTROL", bold(ACCENT))
        .put("  ", Style::new());
    for (g, held) in groups {
        let lit = shown.is_some_and(|(s, _)| s == g);
        // Under the mouse, lit while a click would change the view: the
        // group in front's too, away from its first view.
        let ring = m.focus == Some(Focus::Group(*g))
            || (m.hover == Some(Spot::Group(*g)) && held[0] != m.view);
        let x = head.width();
        let name = kit::bar_name(g.name(), kit::BarRow::Groups, lit, ring, caps, theme);
        hits.push(HitBox {
            y: 0,
            x0: x as u16,
            x1: (x + spans_width(&name)).saturating_sub(1) as u16,
            spot: Spot::Group(*g),
        });
        head.spans.extend(name);
        head.put(" ", Style::new());
    }
    head.put(" ", Style::new());
    head.spans.extend(bar.counts);
    lines.push(with_close(head, m, cols, hits));
    let mut row = Row::default();
    row.put(" ", Style::new());
    for &i in shown.map(|(_, held)| held.as_slice()).unwrap_or(&[]) {
        let title = match i.checked_sub(1).and_then(|v| snap.views.get(v)) {
            Some(v) => views::bar_title(v),
            None => "Tabs",
        };
        let x = row.width();
        let name = if bar.jumped == Some(i) {
            vec![Span::styled(
                format!(" {title} "),
                Style::new()
                    .bg(YELLOW)
                    .fg(Color::Indexed(232))
                    .add_modifier(Modifier::BOLD),
            )]
        } else {
            let ring =
                m.focus == Some(Focus::View(i)) || (m.hover == Some(Spot::View(i)) && i != m.view);
            kit::bar_name(title, kit::BarRow::Views, i == m.view, ring, caps, theme)
        };
        hits.push(HitBox {
            y: 1,
            x0: x as u16,
            x1: (x + spans_width(&name)).saturating_sub(1) as u16,
            spot: Spot::View(i),
        });
        row.spans.extend(name);
        row.put(" ", Style::new());
    }
    if !bar.filter.is_empty() && bar.filter_hit {
        let x = row.width() + 2;
        hits.push(HitBox {
            y: 1,
            x0: x as u16,
            x1: (row.width() + spans_width(&bar.filter)).saturating_sub(1) as u16,
            spot: Spot::Filter,
        });
    }
    row.spans.extend(bar.filter);
    let keys = kit::hints(&[("{ }", "groups"), ("[ ]", "views")], theme);
    // At the right, a column in from the edge, two clear of what is before.
    let gap = cols.saturating_sub(row.width() + spans_width(&keys) + 1);
    if gap >= 2 {
        row.put(" ".repeat(gap), Style::new());
        row.spans.extend(keys);
    }
    lines.push(row.line(cols, None));
    lines.push(Line {
        spans: vec![kit::rule(cols, theme)],
        fill: None,
    });
}

/// Mission control's lines for a screen `cols` by `rows`, and where a click
/// on each part lands.
pub fn screen(m: &Mission, cols: usize, rows: usize) -> (Vec<Line>, Vec<HitBox>) {
    let (lines, hits, _, _) = drawn(m, cols, rows);
    (lines, hits)
}

/// The screen as `screen` draws it, with what is placed over it as images:
/// the meters, charts and sparklines (the Autopilot, Resources and Usage
/// boards') and the pictures (the diagrams of the
/// description beside the PR and ticket boards, and of the plan beside the
/// Autopilot board).
pub fn drawn(
    m: &Mission,
    cols: usize,
    rows: usize,
) -> (Vec<Line>, Vec<HitBox>, Vec<Image>, Vec<Picture>) {
    let snap = &m.snap;
    let theme = &snap.theme;
    let mut hits = Vec::new();
    let mut meters = Vec::new();
    let mut pictures = Vec::new();
    let jumped = m.jump.as_ref().and_then(|j| j.matched);
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
    let mut lines = Vec::new();
    if let Some(groups) = m.groups() {
        let bar = Bar {
            jumped,
            counts,
            filter,
            filter_hit: filter_stop || (m.filter.is_some() && snap.tab_focus),
        };
        grouped_bar(m, &groups, bar, cols, &mut lines, &mut hits);
    } else {
        let mut names: Vec<(String, usize)> = vec![("Tabs".into(), snap.tabs.len())];
        names.extend(
            snap.views
                .iter()
                .map(|v| (v.title.clone(), v.data.items.len())),
        );
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
        lines.push(with_close(head, m, cols, &mut hits));
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
            let pilot_board_shown = snap.rules.ux2
                && v.id == ViewId::Autopilot
                && cols >= PILOT_BOARD_AT
                && v.loaded
                && v.data.setup.is_none()
                && !v.data.items.is_empty();
            let ci_shown = snap.rules.ux2
                && v.id == ViewId::Ci
                && v.loaded
                && v.data.setup.is_none()
                && !v.data.items.is_empty();
            let settings_shown = snap.rules.ux2
                && v.id == ViewId::Settings
                && cols >= super::settings_board::BOARD_AT
                && v.loaded;
            let resources_shown = snap.rules.ux2
                && v.id == ViewId::Resources
                && cols >= super::resources_board::BOARD_AT
                && v.loaded
                && !v.data.items.is_empty();
            if pilot_board_shown {
                pictures = pilot_board(v, cols, rows, m, &mut lines, &mut hits, &mut meters);
            } else if settings_shown {
                super::settings_board::board(v, m, cols, rows, &mut lines, &mut hits, &mut meters);
            } else if resources_shown {
                super::resources_board::board(v, cols, rows, m, &mut lines, &mut hits, &mut meters);
            } else if ci_shown {
                ci_rows(v, cols, rows, m, &mut lines, &mut hits);
            } else if let Some(columns) = v.columns() {
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
                    if snap.rules.ux2 && matches!(v.id, ViewId::Prs | ViewId::Tickets) {
                        pictures = board_cards(
                            v, columns, cols, room, m, theme, top, &mut lines, &mut hits,
                        );
                    } else {
                        board(v, columns, cols, room, m, theme, top, &mut lines, &mut hits);
                    }
                }
            } else if snap.rules.ux2
                && v.id == ViewId::Usage
                && cols >= super::usage_board::BOARD_AT
                && super::usage_board::board(v, m, cols, rows, &mut lines, &mut hits, &mut meters)
            {
            } else if keys_board_shown(m, v, cols) {
                super::keys_board::board(v, cols, rows, m, &mut lines, &mut hits, &mut meters);
            } else if v.id == ViewId::Calendar && super::calendar::is_day(v) {
                let lit = match (m.hover, m.focus) {
                    (Some(Spot::Key(k)), _) | (_, Some(Focus::Key(k))) => Some(k),
                    _ => None,
                };
                let look = super::calendar::Look {
                    cols,
                    rows: rows.saturating_sub(1 + lines.len()),
                    cursor: usize::try_from(m.cursor).ok(),
                    tz: &snap.tz,
                    now_ms: snap.now_secs * 1000,
                    theme,
                    top_row: lines.len(),
                    ux2: crate::settings::is_on("ux2"),
                    caps: snap.caps,
                    date: m.calendar_day.as_deref().and_then(|d| d.parse().ok()),
                    next: snap.calendar_next.as_ref(),
                    work: &snap.views,
                    notes: &snap.calendar_notes,
                    lit,
                    side_top: m.side_top(),
                };
                let (day, at, avatars) = super::calendar::day(v, &look);
                lines.extend(day);
                hits.extend(at);
                meters.extend(avatars);
            } else {
                list(v, cols, rows, m, 0, &mut lines, &mut hits);
            }
        }
    }
    lines.truncate(rows.saturating_sub(1));
    while lines.len() < rows.saturating_sub(1) {
        lines.push(Row::default().line(cols, None));
    }
    let mut keys_hit = Vec::new();
    let foot = match &m.note {
        Some(note) => Row::default()
            .put(format!(" {note}"), fg(theme.bright))
            .take(),
        None => {
            let mut row = Row::default();
            row.put(" ", Style::new());
            let beside = hits.iter().any(|h| matches!(h.spot, Spot::Side(_)));
            let pairs = hints(m, view, beside);
            if snap.rules.ux2
                && view.is_some_and(|v| {
                    matches!(v.id, ViewId::Ci | ViewId::WorkMap | ViewId::Resources)
                        || (v.id == ViewId::Usage && cols >= super::usage_board::BOARD_AT)
                        || (v.id == ViewId::Settings && cols >= super::settings_board::BOARD_AT)
                        || keys_board_shown(m, v, cols)
                })
            {
                let y = rows.saturating_sub(1) as u16;
                row.spans
                    .extend(key_footer(&pairs, m, theme, y, &mut keys_hit));
            } else {
                row.spans.extend(footer(&pairs, snap.rules.ux2, theme));
            }
            row
        }
    };
    lines.push(foot.line(cols, None));
    hits.retain(|h| (h.y as usize) < rows.saturating_sub(1));
    hits.extend(keys_hit);
    meters.retain(|image| (image.y + image.height) as usize <= rows.saturating_sub(1));
    (lines, hits, meters, pictures)
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

    #[test]
    fn the_check_after_a_clean_rebase_shows_in_the_tests_cell() {
        let at = |phase: &str| {
            landing_stages(&serde_json::json!({ "holding": true, "phase": phase }))[0]
                .1
                .clone()
        };
        assert_eq!(at("checking after the rebase"), "checking…");
        assert_eq!(at("check failed"), "✕ check failed");
        assert_eq!(at("rebasing and testing again"), "testing…");
    }

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
            screens: Default::default(),
            pilot: None,
            all_tabs: Vec::new(),
            caps: false,
            cell: None,
            diagrams: Default::default(),
            notice_counts: None,
            photos: Default::default(),
            calendar_next: None,
            calendar_notes: Vec::new(),
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
    fn with_ux2_the_work_map_is_a_table_per_repository_with_the_chosen_row_s_pane_under_it() {
        let tree = |path: &str, branch: &str, main| views::Worktree {
            path: path.into(),
            branch: Some(branch.into()),
            main,
            repo: "/r".into(),
        };
        let trees = vec![
            tree("/r", "main", true),
            tree("/r/wt/ab-1", "AB-1-fix", false),
        ];
        let revived = vec![views::Revived {
            tree: "/r/wt/ab-1".into(),
            repo: "/r".into(),
            branch: "AB-1-fix".into(),
            prompt: "fix the login".into(),
            ..views::Revived::default()
        }];
        let facts = views::MapFacts {
            landings: std::collections::HashMap::from([(
                "/r".into(),
                "lands straight on main".into(),
            )]),
            counts: std::collections::HashMap::from([(
                "/r/wt/ab-1".into(),
                views::Counts {
                    head: "a b".into(),
                    default: "main".into(),
                    ahead: Some(3),
                    to_push: Some(0),
                    ..views::Counts::default()
                },
            )]),
            now_secs: 0,
        };
        let items = views::work_map(&[], &trees, &revived, &|_| None, &facts);
        let map = views::shown(
            &list_view(ViewId::WorkMap, "Work map", items),
            "",
            None,
            &views::Folds::default(),
        );
        let mut m = mission(Vec::new(), vec![map]);
        m.snap.rules.ux2 = true;
        m.view = 1;
        m.cursor = 1;
        let (lines, hits) = screen(&m, 140, 36);
        let t = texts(&lines);
        let heads: Vec<&String> = t
            .iter()
            .filter(|l| l.contains("worktree") && l.contains("where it stands"))
            .collect();
        assert_eq!(heads.len(), 1, "the heads once: {t:#?}");
        let heading = t.iter().find(|l| l.contains("▾ r")).unwrap();
        assert!(
            heading.contains("/r · lands straight on main"),
            "{heading:?}"
        );
        assert!(heading.ends_with("1 worktree · 0 open"), "{heading:?}");
        let row = t.iter().find(|l| l.contains("AB-1-fix ")).unwrap();
        assert!(row.contains("AB-1") && row.contains("no tab") && row.contains("no PR"));
        for said in [
            "path         /r/wt/ab-1",
            "last prompt  fix the login",
            "branch       AB-1-fix · 3 commits ahead of main · nothing to push",
        ] {
            assert!(t.iter().any(|l| l.contains(said)), "{said}: {t:#?}");
        }
        let z = hits
            .iter()
            .find(|h| h.spot == Spot::Key(crate::mux::input::Key::parse("z").unwrap()))
            .expect("z on the footer is a click");
        assert!(
            hits.iter().any(|h| matches!(h.spot, Spot::Side(_))),
            "the pane scrolls with J and K"
        );
        assert_eq!(
            &t[z.y as usize]
                .chars()
                .skip(z.x0 as usize)
                .take(1)
                .collect::<String>(),
            "z"
        );
    }

    #[test]
    fn a_filter_being_typed_leads_the_keys_line() {
        let mut m = mission(vec![tab(1, "one"), tab(2, "two")], Vec::new());
        m.filter = Some(crate::sidebar::render::Filter {
            text: "tw".into(),
            typing: true,
        });
        let (lines, _) = screen(&m, 60, 20);
        let keys = texts(&lines).last().cloned().unwrap();
        assert!(keys.starts_with(" /tw▏  ↑↓ move  ↵ keep it"), "{keys:?}");
        m.picker = true;
        let (lines, _) = screen(&m, 60, 20);
        let keys = texts(&lines).last().cloned().unwrap();
        assert!(keys.contains("↵ actions  esc close"), "{keys:?}");
        assert!(!keys.contains("jk"), "j and k are typed: {keys:?}");
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

    #[test]
    fn with_ux2_a_card_shows_its_screen_its_state_on_the_edge_and_its_branch() {
        let mut working = tab(3, "M-d uncommitted");
        working.state = TabState::Working;
        let mut m = mission(vec![working], Vec::new());
        m.snap.rules.ux2 = true;
        m.snap.screens.insert(
            TabId(3),
            crate::mission::Screen {
                lines: (1..=40)
                    .map(|n| vec![Span::raw(format!("line {n}"))])
                    .collect::<Vec<_>>()
                    .into(),
                branch: Some("md-uncommitted".into()),
            },
        );
        let (lines, _) = screen(&m, 3 * 56, 30);
        let text = texts(&lines);
        let top = text
            .iter()
            .position(|l| l.contains("3 M-d uncommitted"))
            .unwrap();
        assert!(text[top].contains(" working"), "{text:#?}");
        assert!(text[top].trim_end().ends_with("─╮│"), "{text:#?}");
        let bottom = top + text[top..].iter().position(|l| l.contains("╰")).unwrap();
        // The screen's last line just above the footer with the branch.
        assert!(text[bottom - 1].contains("md-uncommitted"), "{text:#?}");
        assert!(text[bottom - 2].contains("line 40"), "{text:#?}");
        assert!(!text.iter().any(|l| l.contains("line 1 ")));
        // The stage's box wears the kit's tone for it, as the sidebar does.
        let heading = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.contains("WORKING"))
            .unwrap();
        assert_eq!(heading.style.fg, Some(Tone::Working.fg(&Theme::DARK)));
    }

    #[test]
    fn with_ux2_a_card_keeps_its_screens_colors_and_shows_its_checks_after_the_branch() {
        let mut working = tab(3, "colors");
        working.state = TabState::Working;
        working.checks = Some(crate::facts::Checks {
            pass: 12,
            fail: 1,
            pending: 0,
        });
        let mut m = mission(vec![working], Vec::new());
        m.snap.rules.ux2 = true;
        let lit = Style::new()
            .fg(Color::Indexed(2))
            .add_modifier(Modifier::BOLD);
        m.snap.screens.insert(
            TabId(3),
            crate::mission::Screen {
                lines: vec![vec![Span::raw("$ "), Span::styled("passed", lit)]].into(),
                branch: Some("colors-branch".into()),
            },
        );
        let (lines, _) = screen(&m, 3 * 56, 30);
        let text = texts(&lines);
        let footer = text
            .iter()
            .position(|l| l.contains("colors-branch"))
            .unwrap();
        assert!(text[footer].contains("colors-branch  ✕1  ✓12"), "{text:#?}");
        assert!(!text[footer].contains('◌'), "{text:#?}");
        let screen_line = &lines[footer - 1];
        let passed = screen_line
            .spans
            .iter()
            .find(|s| s.content.contains("passed"))
            .unwrap();
        assert_eq!(passed.style.fg, Some(Color::Indexed(2)));
        assert!(passed.style.add_modifier.contains(Modifier::BOLD));
        let fail = screen_line_span(&lines[footer], "✕1");
        assert_eq!(fail.style.fg, Some(Tone::Failed.fg(&Theme::DARK)));
    }

    fn screen_line_span<'a>(line: &'a Line, has: &str) -> &'a Span<'static> {
        line.spans.iter().find(|s| s.content.contains(has)).unwrap()
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
            spent: 0.0,
            lands_alone: false,
            hands_free: false,
            target: String::new(),
            drained: false,
            failing: String::new(),
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

    /// The CI view as the source and `views::ci_sections` give it: #27
    /// failing two checks, each with its failing step read, #19
    /// running and #13 passing.
    fn ci_view() -> View {
        let step = |first: u64, lines: &[&str], why_at: u64| {
            serde_json::json!({ "name": "cargo test", "first": first, "lines": lines,
                "whyAt": why_at, "why": lines[why_at as usize] })
        };
        let failing = Item::new("f", "#27 Cache the images").data(serde_json::json!({ "ci": {
            "number": 27, "repo": "acme/app", "branch": "cache", "state": "failing",
            "failed": 2, "total": 9,
            "checks": [
                { "name": "test (ubuntu)", "job": "11", "run": "8812", "secs": 252,
                  "why": "test result: FAILED", "url": "https://github.com/acme/app/actions/runs/8812/job/11",
                  "step": step(40, &["Run cargo test", "running 3 tests", "test result: FAILED", "Error: exit 101"], 2) },
                { "name": "clippy", "job": "12", "run": "8812", "secs": 30,
                  "why": "error: unused variable", "url": "https://github.com/acme/app/actions/runs/8812/job/12",
                  "step": step(7, &["Run cargo clippy", "error: unused variable"], 1) },
            ],
        } }));
        let progress = |id: &str, title: &str, state: &str, detail: &str, n: u64| {
            Item::new(id, title)
                .detail(detail)
                .data(serde_json::json!({ "ci": {
                "number": n, "repo": "acme/app", "state": state } }))
        };
        let items = views::ci_sections(&[
            failing,
            progress(
                "r",
                "#19 Image cache",
                "running",
                "4 of 5 passed, 1 running",
                19,
            ),
            progress("p", "#13 Docs", "passing", "9/9", 13),
        ]);
        list_view(ViewId::Ci, "CI", items)
    }

    #[test]
    fn a_footer_label_clicks_the_key_it_names() {
        let key = |k: &str| crate::mux::input::Key::parse(k);
        assert_eq!(footer_key("↵"), key("Enter"));
        assert_eq!(footer_key("esc"), key("Escape"));
        assert_eq!(footer_key("M-r"), key("M-r"));
        assert_eq!(footer_key("l"), key("l"));
        assert_eq!(footer_key("jk"), None);
        assert_eq!(footer_key("J K"), None);
    }

    #[test]
    fn with_ux2_the_ci_view_heads_its_sections_and_shows_the_chosen_check_s_step_beside() {
        let mut working = tab(6, "cache");
        working.pr = Some(27);
        working.pr_repo = Some("acme/app".into());
        let mut m = mission(vec![working], vec![ci_view()]);
        m.snap.rules.ux2 = true;
        m.view = 1;
        m.cursor = 1;
        let mut lines = Vec::new();
        let mut hits = Vec::new();
        ci_rows(&m.snap.views[0], 160, 21, &m, &mut lines, &mut hits);
        let text = texts(&lines);
        assert!(
            text[0].starts_with(" FAILING") && text[0].contains("1 │"),
            "{}",
            text[0]
        );
        assert!(
            text[1].contains("✕ #27 Cache the images") && text[1].contains("tab 6 │"),
            "{}",
            text[1]
        );
        assert!(
            text[2].contains("acme/app · cache · 2 of 9 failed"),
            "{}",
            text[2]
        );
        assert!(
            text[3].contains("✕ test (ubuntu)  test result: FAILED"),
            "{}",
            text[3]
        );
        assert!(
            text[4].contains("✕ clippy  error: unused variable"),
            "{}",
            text[4]
        );
        assert!(text[6].starts_with(" RUNNING"), "{}", text[6]);
        assert!(
            text[7].contains("#19 Image cache")
                && text[7].contains("4 of 5 passed, 1 running  no tab"),
            "{}",
            text[7]
        );
        assert!(text[7].contains("no tab"), "{}", text[7]);
        assert!(
            text[9].starts_with(" PASSING")
                && text[10].contains("✓ #13 Docs")
                && text[10].contains("9/9  no tab")
        );
        // The pane: the first check's step, numbered as in its log, the
        // line that says why marked, then the other check's line.
        assert!(text[0].contains("│ test (ubuntu)"), "{}", text[0]);
        assert!(
            text[1].contains("#27 · run 8812 · 4m 12s · the failing step, cargo test"),
            "{}",
            text[1]
        );
        assert!(text[3].contains("│  40 Run cargo test"), "{}", text[3]);
        assert!(text[5].contains("│ ▶42 test result: FAILED"), "{}", text[5]);
        let marked = lines[5].spans.iter().find(|s| s.content == "▶").unwrap();
        assert_eq!(marked.style.bg, Some(Tone::Failed.tint(&Theme::DARK)));
        assert!(text[8].contains("┄┄┄"), "{}", text[8]);
        assert!(text[9].contains("✕ clippy"), "{}", text[9]);
        assert!(
            text[10].contains("▶8 error: unused variable"),
            "{}",
            text[10]
        );
        assert!(
            hits.iter().any(|h| h.y == 4 && h.spot == Spot::Check(1, 1)),
            "a failed check's line is a click"
        );
    }

    #[test]
    fn with_ux2_the_ci_view_draws_on_a_screen_with_room_for_almost_nothing() {
        let mut m = mission(Vec::new(), vec![ci_view()]);
        m.snap.rules.ux2 = true;
        m.view = 1;
        m.cursor = 1;
        for rows in 0..4 {
            let mut lines = Vec::new();
            let mut hits = Vec::new();
            ci_rows(&m.snap.views[0], 100, rows, &m, &mut lines, &mut hits);
        }
    }

    #[test]
    fn with_ux2_the_chosen_check_is_the_one_beside_and_a_narrow_screen_puts_it_under() {
        let mut m = mission(Vec::new(), vec![ci_view()]);
        m.snap.rules.ux2 = true;
        m.view = 1;
        m.cursor = 1;
        m.ci_check = Some((
            "f".into(),
            "clippy\thttps://github.com/acme/app/actions/runs/8812/job/12".into(),
        ));
        let mut lines = Vec::new();
        let mut hits = Vec::new();
        ci_rows(&m.snap.views[0], 100, 30, &m, &mut lines, &mut hits);
        let text = texts(&lines);
        assert!(text[1].contains("no tab"), "{}", text[1]);
        let under = text
            .iter()
            .position(|l| l.contains("  clippy"))
            .expect("its pane");
        assert!(under > 10, "under the list: {text:?}");
        assert!(text[under + 1].contains("#27 · run 8812 · 30s"), "{text:?}");
        assert!(
            text.iter().any(|l| l.contains("▶8 error: unused variable")),
            "{text:?}"
        );
        assert!(
            text.iter().any(|l| l.contains("▶42 test result: FAILED")),
            "the other check's line under it: {text:?}"
        );
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
    fn with_ux2_the_bar_is_the_groups_over_the_views_of_the_one_in_front() {
        let mut m = mission(
            vec![tab(1, "api")],
            vec![
                list_view(ViewId::WorkMap, "Work map", vec![]),
                list_view(ViewId::Prs, "PRs", vec![]),
                list_view(ViewId::Settings, "Settings", vec![]),
            ],
        );
        m.snap.rules.ux2 = true;
        m.view = 2;
        let (lines, hits) = screen(&m, 100, 12);
        let t = texts(&lines);
        assert!(
            t[0].starts_with(" MISSION CONTROL   Work   Code   System"),
            "{:?}",
            t[0]
        );
        assert!(t[1].starts_with("  PRs  "), "{:?}", t[1]);
        assert!(
            t[1].trim_end().ends_with("{ } groups  [ ] views"),
            "{:?}",
            t[1]
        );
        assert_eq!(t[2], "─".repeat(100));
        let lit = lines[1].spans.iter().find(|s| s.content == "PRs").unwrap();
        assert!(lit.style.add_modifier.contains(Modifier::UNDERLINED));
        let code = hits
            .iter()
            .find(|h| h.spot == Spot::Group(views::Group::Code))
            .unwrap();
        assert_eq!(
            (code.y, &t[0][code.x0 as usize..=code.x1 as usize]),
            (0, " Code ")
        );
        let prs = hits.iter().find(|h| h.spot == Spot::View(2)).unwrap();
        assert_eq!(
            (prs.y, &t[1][prs.x0 as usize..=prs.x1 as usize]),
            (1, " PRs ")
        );
        m.view = 1;
        let t = texts(&screen(&m, 100, 12).0);
        assert!(t[1].starts_with("  Tabs   Map  "), "{:?}", t[1]);
    }

    #[test]
    fn settings_rows_line_up_as_name_value_and_what_it_does() {
        let items = views::settings_items(&[], false);
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
    fn with_ux2_settings_is_a_board_of_categories_and_the_options_of_the_one_in_front() {
        let mut all = crate::settings::schema();
        let changed = all
            .iter_mut()
            .find(|o| o.category == "Sidebar" && o.choices.is_none())
            .unwrap();
        changed.value = serde_json::Value::Bool(!crate::util::truthy(&changed.default));
        let (key, label) = (changed.key.clone(), changed.label.clone());
        let items = views::settings_items_of(&all, &[], true);
        let at = items
            .iter()
            .position(|i| i.id == format!("setting:{key}"))
            .unwrap();
        let mut m = mission(
            Vec::new(),
            vec![list_view(ViewId::Settings, "Settings", items.clone())],
        );
        m.snap.rules.ux2 = true;
        m.view = 1;
        m.size = (160, 40);
        m.cursor = at as isize;
        let (lines, hits, images, _) = drawn(&m, 160, 40);
        let text = texts(&lines);
        let all_text = text.join("\n");
        // The panel: the Sidebar's chip of 1 before its count.
        let sidebar = text
            .iter()
            .find(|l| l.starts_with("▌ Sidebar"))
            .unwrap_or_else(|| panic!("{all_text}"));
        let count = items
            .iter()
            .find(|i| i.id == "header:Sidebar")
            .unwrap()
            .data["count"]
            .to_string();
        assert!(
            sidebar[..sidebar.find('│').unwrap()]
                .trim_end()
                .ends_with(&format!(" 1  {count}")),
            "{sidebar}"
        );
        assert!(all_text.contains("Tabs and header"), "{all_text}");
        assert!(all_text.contains("YOUR FILES"), "{all_text}");
        // The pane: the category's heading, the changed option with its
        // chip and key, and what Enter does.
        assert!(
            all_text.contains("1 changed from its default"),
            "{all_text}"
        );
        let row = text
            .iter()
            .find(|l| l.contains(&format!(" {label} ")) && l.contains(&key))
            .unwrap_or_else(|| panic!("{all_text}"));
        assert!(row.contains(" changed "), "{row}");
        assert!(all_text.contains("· Enter turns it back"), "{all_text}");
        // A switch an on/off option in front, flipped by a click on it.
        let switches: Vec<&Image> = images
            .iter()
            .filter(|i| i.name.starts_with("settings/switch/"))
            .collect();
        let options = items
            .iter()
            .filter(|i| i.id.starts_with("setting:") && i.data["choice"] == false)
            .filter(|i| views::search_text(i).contains("sidebar"))
            .count();
        assert!(!switches.is_empty() && switches.len() <= options);
        assert!(hits.iter().any(|h| h.spot == Spot::Menu(at as isize)));
        // A click on a category chooses its first row without changing it.
        let sidebar_first = items.iter().position(|i| i.id == "header:Sidebar").unwrap() + 1;
        assert!(
            hits.iter()
                .any(|h| h.x0 == 0 && h.spot == Spot::Category(sidebar_first as isize))
        );
        assert!(
            !hits
                .iter()
                .any(|h| h.x0 == 0 && matches!(h.spot, Spot::Index(_)))
        );
        assert!(
            text.last().unwrap().contains("hl category"),
            "{:?}",
            text.last()
        );

        // A search lists the matches of every category.
        m.filter = Some(crate::sidebar::render::Filter {
            text: "notif".into(),
            typing: false,
        });
        let shown = views::shown(&m.snap.views[0], "notif", None, &views::Folds::default());
        m.snap.views[0] = shown;
        m.cursor = 1;
        let text = texts(&drawn(&m, 160, 40).0).join("\n");
        assert!(text.contains("Matching “notif”"), "{text}");
        let labels = m.snap.views[0]
            .data
            .items
            .iter()
            .filter(|i| views::is_header(i))
            .count();
        assert!(labels >= 2, "{text}");

        // Narrower, the plain list.
        m.size = (120, 40);
        let text = texts(&drawn(&m, 120, 40).0).join("\n");
        assert!(!text.contains("CATEGORIES"), "{text}");
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
    fn with_ux2_the_pr_board_draws_each_pr_as_a_card() {
        let item = |n: u32, column: &str, checks: &str| {
            Item::new(
                n.to_string(),
                format!("#{n} PR {n} lays the settings view out"),
            )
            .detail("w")
            .group(column)
            .data(serde_json::json!({
                "pr": { "number": n }, "checks": checks, "meta": "w · queue 1",
            }))
        };
        let items = vec![
            item(1, "In review", "success"),
            item(2, "In review", "failure"),
            item(3, "Approved", "pending"),
        ];
        let mut m = mission(Vec::new(), vec![list_view(ViewId::Prs, "PRs", items)]);
        m.snap.rules.ux2 = true;
        m.view = 1;
        m.cursor = 1;
        let (lines, hits) = screen(&m, 150, 30);
        let text = texts(&lines);
        assert!(
            text[3].contains("IN REVIEW") && text[3].contains("APPROVED"),
            "{}",
            text[3]
        );
        assert!(
            text[5].contains("╭ #1 ─") && text[5].contains(" ✓ ─╮") && text[5].contains(" ◌ ─╮"),
            "{}",
            text[5]
        );
        assert!(
            text[6].contains("│ PR 1 lays the") && !text[6].contains("#1"),
            "the title's first line: {}",
            text[6]
        );
        assert!(
            text[7].contains("│ settings view"),
            "its second: {}",
            text[7]
        );
        assert!(text[8].contains("│ w · queue 1"), "{}", text[8]);
        assert!(text[9].contains("╰─"), "{}", text[9]);
        assert!(
            text[11].contains("#2") && text[11].contains(" ✕ "),
            "{}",
            text[11]
        );
        let edge = lines[11].spans.iter().find(|s| s.content == "╭").unwrap();
        assert_eq!(edge.style.fg, Some(ACCENT), "the chosen card's edge");
        assert!(hits.iter().any(|h| h.spot == Spot::Index(1) && h.y == 12));
    }

    #[test]
    fn the_description_beside_a_board_draws_a_ready_diagram_as_a_picture_cut_at_its_bottom() {
        let body = "Why\n\n```mermaid\nflowchart LR\n  A[Login] --> B[Token]\n```\n\nAfter it.";
        let item = Item::new("1", "#1 PR 1")
            .group("In review")
            .data(serde_json::json!({ "pr": { "number": 1 }, "author": "ana", "body": body }));
        let mut m = mission(vec![], vec![list_view(ViewId::Prs, "PRs", vec![item])]);
        m.snap.rules.ux2 = true;
        m.view = 1;
        m.cursor = 0;
        m.size = (160, 20);
        let columns = crate::sources::prboard::columns(true);
        let draw = |m: &Mission| {
            let (mut lines, mut hits) = (Vec::new(), Vec::new());
            let view = &m.snap.views[0];
            let pictures = board_cards(
                view,
                columns,
                160,
                20,
                m,
                &Theme::DARK,
                2,
                &mut lines,
                &mut hits,
            );
            (texts(&lines), pictures)
        };
        // Nothing ready, or a client that draws no images: its text drawing.
        let (text, pictures) = draw(&m);
        assert!(pictures.is_empty());
        assert!(text.iter().any(|l| l.contains("Login")), "{text:?}");

        let (found, place) = description_beside(&m).expect("the pane is shown");
        let source = super::super::diagrams::sources(found, place).remove(0);
        let key = crate::views::markdown::Wanted::Diagram(source).key();
        let image = std::sync::Arc::new(image::RgbaImage::new(400, 800));
        let ready = super::super::diagrams::Ready {
            content: crate::ui::images::picture_key(&image),
            image,
        };
        m.snap.diagrams.insert(key.clone(), ready);
        let (text, pictures) = draw(&m);
        assert!(pictures.is_empty(), "ready, but the client draws no images");
        assert!(text.iter().any(|l| l.contains("Login")), "{text:?}");

        // Ready on a client that draws them: the rows its picture takes at
        // the pane's width, blank, and no text drawing.
        m.snap.cell = Some((8, 16));
        let (text, pictures) = draw(&m);
        assert!(!text.iter().any(|l| l.contains("Login")), "{text:?}");
        let [picture] = &pictures[..] else {
            panic!("one picture: {pictures:?}");
        };
        assert_eq!(picture.key, key);
        assert!(picture.id >= crate::ui::images::SERVER_IDS);
        assert_eq!((picture.cols, picture.rows), (47, 47), "the pane's width");
        // Under the head and "Why", inside the pane right of the rule, on
        // the screen's rows, and cut where the pane ends.
        let why = text.iter().position(|l| l.contains("│ Why")).unwrap();
        assert_eq!(picture.y as usize, 2 + why + 2);
        assert_eq!(picture.x as usize, 160 - SIDE_W + 2);
        assert_eq!(picture.shown as usize, 20 - (why + 2));
        assert!(
            !text.iter().any(|l| l.contains("After it.")),
            "cut: {text:?}"
        );

        // On a screen too narrow for the pane there is nothing to ask for.
        m.size = (120, 20);
        assert!(description_beside(&m).is_none());
    }

    #[test]
    fn with_ux2_the_pr_board_names_reviewers_or_the_tab_and_shows_the_chosen_pr_beside_it() {
        let item = |n: u32, column: &str, reviewers: &str| {
            Item::new(n.to_string(), format!("#{n} PR {n}"))
                .group(column)
                .data(serde_json::json!({
                    "pr": { "number": n }, "checks": "success", "meta": "w",
                    "reviewers": reviewers, "author": "ana", "commits": 3,
                    "additions": 214, "deletions": 88,
                    "body": "## Summary\nLay the **Settings** view out in two columns.",
                }))
        };
        let items = vec![
            item(1, "In review", "sam, ben"),
            item(2, "Changes asked", "ben"),
        ];
        let mut working = tab(3, "settings");
        working.pr = Some(1);
        let mut m = mission(vec![working], vec![list_view(ViewId::Prs, "PRs", items)]);
        m.snap.rules.ux2 = true;
        m.view = 1;
        m.cursor = 1;
        let columns = crate::sources::prboard::columns(true);
        let mut lines = Vec::new();
        let mut hits = Vec::new();
        let view = &m.snap.views[0];
        board_cards(
            view,
            columns,
            160,
            20,
            &m,
            &Theme::DARK,
            0,
            &mut lines,
            &mut hits,
        );
        let text = texts(&lines);
        assert!(text[0].contains("CHANGES ASKED"), "{}", text[0]);
        assert!(text[5].contains("│ tab 3 · "), "{}", text[5]);
        assert!(text[5].contains("│ ben"), "the reviewers: {}", text[5]);
        assert!(text[0].contains("│ #2 PR 2"), "{}", text[0]);
        assert!(
            text[1].contains("ana · 3 commits · +214 −88 · ✓ checks"),
            "{}",
            text[1]
        );
        assert!(
            text[3].contains("│ Summary") && text[5].contains("Lay the Settings"),
            "the body through Markdown: {text:?}"
        );
        let bold_word = lines[5]
            .spans
            .iter()
            .find(|s| s.content.contains("Settings"))
            .unwrap();
        assert!(bold_word.style.add_modifier.contains(Modifier::BOLD));
        let light = pr_about(&m.snap.views[0].data.items[1], 50, &Theme::LIGHT, None);
        let plain = light
            .lines
            .iter()
            .flatten()
            .find(|s| s.content.contains("Lay the"))
            .unwrap();
        assert_eq!(plain.style.fg, Some(Theme::LIGHT.text), "the theme's text");
        let (narrow, _) = {
            let mut l = Vec::new();
            let mut h = Vec::new();
            board_cards(view, columns, 120, 20, &m, &Theme::DARK, 0, &mut l, &mut h);
            (texts(&l), h)
        };
        assert!(!narrow[0].contains("#2 PR 2"), "{}", narrow[0]);
        // On a wide screen the pane keeps its share of it: 50 of 160 is 75
        // of 240, its rule at the board's right.
        let mut l = Vec::new();
        let mut h = Vec::new();
        board_cards(view, columns, 240, 20, &m, &Theme::DARK, 0, &mut l, &mut h);
        let wide = texts(&l);
        assert_eq!(
            wide[0].chars().position(|c| c == '│'),
            Some(240 - 75),
            "{}",
            wide[0]
        );
    }

    #[test]
    fn a_pane_beside_a_board_keeps_its_share_of_any_screen() {
        assert_eq!(beside_w(120, SIDE_AT, SIDE_W), 0);
        assert_eq!(beside_w(160, SIDE_AT, SIDE_W), 50);
        assert_eq!(beside_w(240, SIDE_AT, SIDE_W), 75);
        assert_eq!(beside_w(160, PILOT_SIDE_AT, PILOT_SIDE_W), 44);
        assert_eq!(beside_w(240, PILOT_SIDE_AT, PILOT_SIDE_W), 66);
    }

    #[test]
    fn a_pr_card_names_the_tab_on_its_number_in_its_own_repository() {
        let card = |repo: &str| {
            Item::new(repo.to_string(), "#42 Fix it".to_string())
                .group("In review")
                .data(serde_json::json!({
                    "pr": { "number": 42, "repo": repo }, "reviewers": "sam",
                }))
        };
        let meta = |tabs: Vec<TabModel>, repo: &str| {
            let m = mission(tabs, vec![]);
            let face = pr_face(&card(repo), &m, &Theme::DARK);
            face.meta
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        };
        let mut app = tab(1, "app");
        app.pr = Some(42);
        app.pr_repo = Some("acme/app".to_string());
        assert!(meta(vec![app.clone()], "acme/app").starts_with("tab 1"));
        assert_eq!(meta(vec![app.clone()], "acme/web"), "sam");
        let mut web = tab(2, "web");
        web.pr = Some(42);
        web.repo = Some("Acme/Web".to_string());
        assert!(meta(vec![app, web.clone()], "acme/web").starts_with("tab 2"));
        web.repo = None;
        assert_eq!(
            meta(vec![web.clone()], "acme/web"),
            "sam",
            "a tab with no repository takes no repository's card"
        );
        assert!(
            meta(vec![web], "").starts_with("tab 2"),
            "a card with no repository matches by number"
        );
    }

    #[test]
    fn with_ux2_the_ticket_board_draws_each_ticket_as_a_card_and_the_chosen_one_beside_it() {
        let ticket = |id: &str, column: &str, priority: u64, extra: serde_json::Value| {
            let mut data = serde_json::json!({
                "ticket": id, "priority": priority, "priorityLabel": "Urgent",
                "title": format!("Fix the RSVP menu of {id}"), "state": column,
                "project": "Web app", "estimate": 2.0,
                "description": "The menu closes on a refresh.\n\n## Steps\n1. Open the menu",
            });
            for (k, v) in extra.as_object().unwrap() {
                data[k] = v.clone();
            }
            let mut i = Item::new(id, format!("{id} Fix the RSVP menu of {id}"))
                .group(column)
                .data(data);
            i.time = Some((rules().now_secs() - 3 * 86_400) * 1000);
            i
        };
        let items = vec![
            ticket("AB-1", "Todo", 1, serde_json::json!({})),
            ticket("AB-2", "In progress", 2, serde_json::json!({ "tab": 4 })),
            ticket("AB-3", "Done", 5, serde_json::json!({ "tab": 4 })),
            ticket(
                "AB-4",
                "Closed",
                5,
                serde_json::json!({ "state": "Duplicate" }),
            ),
        ];
        let mut m = mission(
            vec![tab(4, "AB-2")],
            vec![list_view(ViewId::Tickets, "Tickets", items)],
        );
        m.snap.rules.ux2 = true;
        m.view = 1;
        m.cursor = 0;
        let columns = crate::sources::tickets::columns(true);
        let (mut lines, mut hits) = (Vec::new(), Vec::new());
        board_cards(
            &m.snap.views[0],
            columns,
            160,
            24,
            &m,
            &Theme::DARK,
            0,
            &mut lines,
            &mut hits,
        );
        let text = texts(&lines);
        assert!(
            text[0].contains("TODO") && text[0].contains("DONE") && !text[0].contains("CLOSED"),
            "{}",
            text[0]
        );
        assert!(
            text[2].contains("╭ AB-1 ─") && text[2].contains(" !!! ─╮"),
            "{}",
            text[2]
        );
        assert!(text[2].contains(" !! ─╮"), "high: {}", text[2]);
        assert!(text[3].contains("│ Fix the RSVP menu"), "{}", text[3]);
        assert!(text[5].contains("│ Web app · 2 pts"), "{}", text[5]);
        assert!(text[5].contains("│ tab 4 · "), "{}", text[5]);
        assert!(
            text[5].contains("│ 3d ago"),
            "a done one says when: {}",
            text[5]
        );
        assert!(
            lines[3].spans.iter().any(|s| s.content.contains("Fix")
                && s.style.add_modifier.contains(Modifier::CROSSED_OUT)),
            "a done one struck through"
        );
        // The chosen ticket beside the board, its description through
        // Markdown.
        assert!(text[0].contains("│ AB-1 Fix the RSVP menu"), "{}", text[0]);
        assert!(
            text[1].contains("Todo · !!! Urgent · Web app · 2 pts · you"),
            "{}",
            text[1]
        );
        assert!(
            text[3].contains("│ The menu closes on a refresh."),
            "{text:?}"
        );
        assert!(text.iter().any(|l| l.contains("│ Steps")), "{text:?}");
        let low = ticket("AB-5", "Todo", 4, serde_json::json!({}));
        let pane: Vec<String> = ticket_about(&low, 50, &Theme::DARK, None)
            .lines
            .iter()
            .map(|l| l.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(pane[1].contains("Todo · Low · Web app"), "{}", pane[1]);
        // The ticket canceled this week, faint in the row under the columns.
        let label = text
            .iter()
            .position(|l| l.contains("CLOSED THIS WEEK"))
            .unwrap();
        assert!(text[label - 1].contains("┄"), "{}", text[label - 1]);
        assert!(text[label + 2].contains("╭ AB-4"), "{}", text[label + 2]);
        assert!(
            text[label + 5].contains("│ 3d ago · duplicate"),
            "{}",
            text[label + 5]
        );
        assert!(
            hits.iter()
                .any(|h| h.spot == Spot::Index(3) && usize::from(h.y) == label + 2)
        );
    }

    #[test]
    fn with_ux2_the_prs_merged_this_week_are_a_row_under_the_columns() {
        let item = |n: u32, column: &str| {
            let mut i = Item::new(n.to_string(), format!("#{n} PR {n}"))
                .group(column)
                .data(serde_json::json!({ "pr": { "number": n }, "mergedBy": "ana" }));
            i.time = Some((rules().now_secs() - 86_400) * 1000);
            i
        };
        let items = vec![item(1, "In review"), item(2, "Merged"), item(3, "Merged")];
        let mut m = mission(Vec::new(), vec![list_view(ViewId::Prs, "PRs", items)]);
        m.snap.rules.ux2 = true;
        m.cursor = 2;
        let columns = crate::sources::prboard::columns(true);
        let (mut lines, mut hits) = (Vec::new(), Vec::new());
        board_cards(
            &m.snap.views[0],
            columns,
            100,
            24,
            &m,
            &Theme::DARK,
            0,
            &mut lines,
            &mut hits,
        );
        let text = texts(&lines);
        assert!(!text[0].contains("MERGED"), "not a column: {}", text[0]);
        let label = text
            .iter()
            .position(|l| l.contains("MERGED THIS WEEK"))
            .unwrap();
        assert!(text[label].trim_end().ends_with('2'), "{}", text[label]);
        assert!(
            text[label + 2].contains("╭ #2") && text[label + 2].contains("╭ #3"),
            "{}",
            text[label + 2]
        );
        assert!(
            text[label + 5].contains("│ 1d ago · ana"),
            "{}",
            text[label + 5]
        );
        let edge = lines[label + 2]
            .spans
            .iter()
            .filter(|s| s.content == "╭")
            .nth(1)
            .unwrap();
        assert_eq!(edge.style.fg, Some(ACCENT), "the chosen one in the row");
    }

    #[test]
    fn with_ux2_a_narrow_pr_board_shows_the_chosen_pr_s_column() {
        let item = |n: u32, column: &str| {
            Item::new(n.to_string(), format!("#{n} PR {n}"))
                .group(column)
                .data(serde_json::json!({ "pr": { "number": n } }))
        };
        let columns = crate::sources::prboard::columns(true);
        let last = columns[columns.len() - 2];
        let items = vec![item(1, columns[0]), item(2, last)];
        let mut m = mission(Vec::new(), vec![list_view(ViewId::Prs, "PRs", items)]);
        m.snap.rules.ux2 = true;
        let draw = |m: &Mission| {
            let (mut l, mut h) = (Vec::new(), Vec::new());
            board_cards(
                &m.snap.views[0],
                columns,
                55,
                20,
                m,
                &Theme::DARK,
                0,
                &mut l,
                &mut h,
            );
            (texts(&l), h)
        };
        // The first PR chosen: the columns from the first, more to the right.
        m.cursor = 0;
        let (text, _) = draw(&m);
        assert!(
            text[0].contains(&columns[0].to_uppercase()) && text[0].ends_with("›"),
            "{}",
            text[0]
        );
        assert!(!text[0].contains("MERGE"), "{}", text[0]);
        // The last column's PR chosen: its column in sight, its card drawn
        // and clickable inside the board, more to the left.
        m.cursor = 1;
        let (text, hits) = draw(&m);
        assert!(
            text[0].contains("MERGE") && text[0].contains("‹"),
            "{}",
            text[0]
        );
        assert!(text.iter().any(|l| l.contains("#2")), "{text:?}");
        assert!(
            hits.iter()
                .any(|h| h.spot == Spot::Index(1) && usize::from(h.x1) < 55),
            "{hits:?}"
        );
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

    #[test]
    fn with_ux2_resources_is_a_board_of_the_computer_and_each_tab() {
        use crate::usage::resources::{self, History, Proc, TabPanes, Top};
        let proc = |pid, ppid, name: &str, cpu, memory| Proc {
            pid,
            ppid,
            name: name.into(),
            cpu,
            memory,
        };
        let list = vec![
            proc(100, 1, "zsh", 0.0, 2 << 20),
            proc(101, 100, "node", 120.0, 900 << 20),
            proc(102, 101, "sh", 0.0, 1 << 20),
            proc(103, 101, "esbuild", 4.0, 300 << 20),
            proc(104, 100, "git", 0.0, 10 << 20),
            proc(105, 100, "cat", 0.0, 1 << 20),
            proc(200, 1, "zsh", 0.5, 3 << 20),
        ];
        let tabs = [
            TabPanes {
                tab: TabId(1),
                index: 1,
                name: "calm".into(),
                pids: vec![200],
            },
            TabPanes {
                tab: TabId(3),
                index: 3,
                name: "build".into(),
                pids: vec![100],
            },
        ];
        let mut history = History::default();
        let now = std::time::Instant::now();
        history.add(now, &resources::tab_usage(&tabs, &list), Some(38.0));
        let top = Top {
            cpu: Some(38.0),
            cores: 10,
            memory_used: 21 << 30,
            memory_total: 32 << 30,
            own_cpu: 2.0,
            own_memory: 84 << 20,
        };
        let data = resources::view(&tabs, Some(&list), Some(1), &history, Some(&top));
        let view = View {
            id: ViewId::Resources,
            title: "Resources".into(),
            data,
            loaded: true,
        };
        let mut m = mission(vec![tab(1, "calm"), tab(3, "build")], vec![view]);
        m.snap.rules.ux2 = true;
        m.view = 1;
        // The snapshot's CPU order, as the server sorts it for the client.
        super::super::resources_board::Sort::Cpu.sort(&mut m.snap.views[0].data.items);
        m.cursor = 0;
        let (lines, hits, images, _) = drawn(&m, 160, 30);
        let s = crate::mux::input::Key::parse("s").unwrap();
        assert!(
            hits.iter().any(|h| h.spot == Spot::Key(s)),
            "the footer's s takes a click"
        );
        let text = texts(&lines);
        let all = text.join("\n");
        assert!(all.contains("CPU 38% of 10 cores"), "{all}");
        assert!(all.contains("Memory 21.0 GB of 32.0 GB"), "{all}");
        assert!(all.contains("Switchboard itself: 2% CPU, 84 MB"), "{all}");
        assert!(all.contains("CPU, LAST 2 MINUTES"), "{all}");
        let build = text
            .iter()
            .position(|l| l.contains("3 build"))
            .expect("build's row");
        assert!(text[build].contains("124%"), "{}", text[build]);
        assert!(text[build + 1].contains("node 120%"), "{}", text[build + 1]);
        assert!(
            text[build + 1].contains("✕ a core pegged"),
            "{}",
            text[build + 1]
        );
        // The chosen row's tree: its four heaviest, then the rest.
        assert!(text[build + 2].contains("├ zsh"), "{all}");
        assert!(text[build + 3].contains("  ├ node"), "{all}");
        assert!(text[build + 6].contains("2 more: sh, cat"), "{all}");
        let calm = text
            .iter()
            .position(|l| l.contains("1 calm"))
            .expect("calm's row");
        assert!(calm > build + 6);
        assert!(all.contains("Memory is resident"), "{all}");
        // One CPU and one memory sparkline a row, and the top's two.
        let named = |n: &str| images.iter().filter(|i| i.name == n).count();
        assert_eq!(named("resources/3/cpu"), 1);
        assert_eq!(named("resources/3/memory"), 1);
        assert_eq!(named("resources/1/cpu"), 1);
        assert_eq!(named("resources/1/memory"), 1);
        assert_eq!(named("resources/cpu"), 1);
        assert_eq!(
            images
                .iter()
                .find(|i| i.name == "resources/memory")
                .and_then(|i| i.frac()),
            Some(21.0 / 32.0)
        );
        // At any width the computer's memory meter ends at the right edge,
        // its CPU sparkline stretching too.
        for cols in [136, 160, 240] {
            m.size = (cols as u16, 30);
            let (_, _, images, _) = drawn(&m, cols, 30);
            let image = |n: &str| images.iter().find(|i| i.name == n).unwrap();
            let meter = image("resources/memory");
            assert_eq!((meter.x + meter.width) as usize, cols - 1, "at {cols}");
            assert!(image("resources/cpu").width >= 40, "at {cols}");
        }
        m.size = (160, 30);
        // Narrower, the table.
        let (lines, _) = screen(&m, 120, 30);
        assert!(texts(&lines).iter().any(|l| l.contains("% CPU")));
    }

    fn pilot_summary() -> crate::autopilot::Summary {
        crate::autopilot::Summary {
            target: String::new(),
            drained: false,
            failing: String::new(),
            note: "Switchboard".into(),
            busy: 1,
            open: 2,
            slots: 2,
            cap: 5,
            next: "Third".into(),
            paused: false,
            away: false,
            ready_only: false,
            skipped: 1,
            spend_limit: 20.0,
            spent: 5.0,
            lands_alone: true,
            hands_free: false,
        }
    }

    fn pilot_row(tab: u32, item: &str, state: TabState) -> crate::autopilot::view::Row {
        crate::autopilot::view::Row {
            tab: TabId(tab),
            index: tab as usize,
            item: item.into(),
            state,
            was_in: true,
            loop_skill: None,
            phase: None,
            landing: None,
            left: false,
            autoland: false,
            closed: None,
            in_note: true,
        }
    }

    #[test]
    fn with_ux2_autopilot_keeps_the_chosen_card_in_sight() {
        let rows: Vec<_> = (1..=10)
            .map(|n| pilot_row(n, &format!("item-{n}"), TabState::Working))
            .collect();
        let list = crate::autopilot::view::items(
            &pilot_summary(),
            "",
            &rows,
            &[],
            &[],
            &std::collections::HashMap::new(),
        );
        let last = list
            .iter()
            .position(|i| i.title.contains("item-10"))
            .unwrap();
        let mut m = mission(
            Vec::new(),
            vec![list_view(ViewId::Autopilot, "Autopilot", list)],
        );
        m.snap.rules.ux2 = true;
        m.view = 1;
        m.cursor = last as isize;
        let (lines, hits, meters, _) = drawn(&m, 125, 24);
        let text = texts(&lines);
        assert!(
            text[..23].iter().any(|l| l.contains("item-10")),
            "the chosen card in sight: {text:#?}"
        );
        assert!(hits.iter().any(|h| h.spot == Spot::Index(last as isize)));
        assert!(meters.iter().all(|meter| (meter.y as usize) < 23));
    }

    #[test]
    fn with_ux2_autopilot_is_a_board_of_its_items_with_the_skipped_last() {
        let mut landing = pilot_row(3, "Give notes stable IDs", TabState::Working);
        landing.loop_skill = Some("queue".into());
        landing.phase = Some("pushing".into());
        landing.landing = Some(serde_json::json!({ "holding": true, "phase": "pushing" }));
        let rows = vec![
            landing,
            pilot_row(4, "Port the PR board", TabState::Waiting),
        ];
        let list = crate::autopilot::view::items(
            &pilot_summary(),
            "",
            &rows,
            &["Third".into(), "Fourth".into()],
            &["Old idea".into()],
            &std::collections::HashMap::from([
                (
                    "Give notes stable IDs".to_string(),
                    crate::cmd::worktree_from_note::PlanNote {
                        name: "Plan - Stable IDs".into(),
                        text: Some(
                            "## Today\n\nNotes are found by name.\n\n## Steps\n\n1. Give each a line of its own."
                                .into(),
                        ),
                    },
                ),
                (
                    "Third".to_string(),
                    crate::cmd::worktree_from_note::PlanNote {
                        name: "Plan - Third".into(),
                        text: None,
                    },
                ),
            ]),
        );
        let at_title = |title: &str| list.iter().position(|i| i.title == title).unwrap();
        let chosen = at_title("Give notes stable IDs");
        let (no_plan, plan_missing) = (at_title("Port the PR board"), at_title("1. Third"));
        let mut m = mission(
            Vec::new(),
            vec![list_view(ViewId::Autopilot, "Autopilot", list)],
        );
        m.snap.rules.ux2 = true;
        m.view = 1;
        m.size = (160, 30);
        // A card whose item links no plan says so, and so does one whose
        // plan note isn't there; the heading says nothing of a plan.
        let beside = |m: &mut Mission, cursor: usize| {
            m.cursor = cursor as isize;
            texts(&drawn(m, 160, 30).0).join("\n")
        };
        let pane = beside(&mut m, no_plan);
        assert!(pane.contains("No plan"), "{pane}");
        assert!(description_beside(&m).is_none());
        let pane = beside(&mut m, plan_missing);
        assert!(pane.contains("Plan not found: Plan - Third"), "{pane}");
        let pane = beside(&mut m, 0);
        assert!(!pane.contains("No plan"), "{pane}");
        m.cursor = chosen as isize;
        let (lines, _, meters, _) = drawn(&m, 160, 30);
        let text = texts(&lines);
        // The chosen card's plan under its stages: the plan note's name,
        // then its Markdown, which is the text its diagrams are asked for.
        let line_of = |s: &str| {
            text.iter()
                .position(|l| l.contains(s))
                .unwrap_or_else(|| panic!("{s} in {text:#?}"))
        };
        assert!(line_of("closed") < line_of("Plan - Stable IDs"));
        assert!(line_of("Plan - Stable IDs") < line_of("Today"));
        assert!(line_of("Today") < line_of("Notes are found by name."));
        assert!(line_of("Notes are found by name.") < line_of("Give each a line of its own."));
        let (plan, _) = description_beside(&m).expect("the plan beside the board");
        assert!(plan.starts_with("## Today"), "{plan}");
        m.size = (140, 30);
        assert!(description_beside(&m).is_none(), "too narrow for the pane");
        m.size = (160, 30);
        let at = text
            .iter()
            .position(|l| l.contains("AUTO Switchboard"))
            .unwrap();
        assert!(
            text[at].contains("1 of 2 working · 2 of 5 open · lands without asking")
                && text[at].contains("$5.00 of $20 today"),
            "{}",
            text[at]
        );
        let heads = &text[at + 2];
        for column in crate::autopilot::view::COLUMNS {
            assert!(heads.contains(column), "{heads}");
        }
        assert!(
            heads.contains("Next up 2") && heads.contains("Landed 0"),
            "{heads}"
        );
        let board = text.join("\n");
        assert!(
            board.contains("tab 3") && board.contains("tab 4"),
            "{board}"
        );
        assert!(
            board.contains("opens next") && board.contains("2nd in line"),
            "{board}"
        );
        // The chosen card's stages beside the board.
        assert!(
            board.contains("✓ held") && board.contains("pushing…"),
            "{board}"
        );
        // The skipped item a card in the last column, with no band under
        // the board.
        assert!(heads.contains("Skipped 1"), "{heads}");
        let col = |l: &str, s: &str| l[..l.find(s).unwrap()].chars().count();
        let skipped_at = col(heads, "Skipped 1");
        let card = text.iter().position(|l| l.contains("Old idea")).unwrap();
        assert!(col(&text[card], "Old idea") >= skipped_at, "{}", text[card]);
        assert!(
            text[card..card + 3]
                .iter()
                .any(|l| l.contains("skipped by you")),
            "{board}"
        );
        assert_eq!(
            text.iter().filter(|l| l.contains("Skipped")).count(),
            1,
            "{board}"
        );
        // A meter for each card, the landing one 6 of 8 stages done, and
        // one for the spend: a quarter of the limit.
        let held = meters
            .iter()
            .find(|meter| meter.name == "autopilot/pilot:@3")
            .unwrap();
        assert_eq!(held.frac(), Some(6.0 / 8.0));
        let spend = meters
            .iter()
            .find(|meter| meter.name == "autopilot/spend")
            .unwrap();
        assert_eq!((spend.frac(), spend.y as usize), (Some(0.25), at));
        assert_eq!(meters.len(), 5);
    }

    #[test]
    fn with_ux2_notifications_are_a_history_by_day_with_their_kinds() {
        use crate::sidebar::history::{Entry, Kind, Sidebar};
        const DAY: i64 = 86_400_000;
        let now = rules().now_ms;
        let entry = |source: &str, id: &str, title: &str, at: i64, sidebar: Sidebar| Entry {
            source: source.into(),
            id: id.into(),
            at,
            item: Item::new(id, title),
            sidebar,
            led_to: None,
            replaced: false,
        };
        let a = entry("slack", "a", "DM Ada", now, Sidebar::Shown);
        let mut b = entry(
            "prs",
            "b",
            "PR 12345 merged, widgets",
            now - DAY,
            Sidebar::Dismissed,
        );
        b.led_to = Some(("you went to tab 5".into(), now));
        let c = entry(
            "inbox",
            "c",
            "A ticket assigned to you",
            now - 4 * DAY,
            Sidebar::Older,
        );
        let all = [&a, &b, &c];
        let tz = jiff::tz::TimeZone::UTC;
        let today = crate::util::local_at_ms(now)
            .with_time_zone(tz.clone())
            .date();
        let rows = |kind: Option<Kind>| {
            crate::mission::history::rows(&all, kind, &Default::default(), today, &tz, 3)
        };
        let mut m = mission(
            Vec::new(),
            vec![list_view(
                ViewId::Notifications,
                "Notifications",
                rows(None),
            )],
        );
        m.snap.rules.ux2 = true;
        m.snap.notice_counts = Some(crate::mission::history::counts(&all, today, &tz));
        m.view = 1;
        let (lines, hits) = screen(&m, 140, 30);
        let text = texts(&lines).join("\n");
        for want in [
            "1 All 3",
            "PRs 1",
            "Slack 1",
            "Inbox 1",
            "Landed 0",
            "3 this week · 1 in the sidebar",
            "WHEN",
            "WHAT IT LED TO",
            "SIDEBAR",
            "TODAY",
            "YESTERDAY",
            "you went to tab 5",
            "dismissed",
            "shown",
            "more, folded · Enter shows them",
        ] {
            assert!(text.contains(want), "{want}: {text}");
        }
        assert!(
            !text.contains("A ticket assigned to you"),
            "its day is folded: {text}"
        );
        assert!(hits.iter().any(|h| h.spot == Spot::Kind(3)));
        // 3 leaves only the PRs.
        m.snap.views[0].data.items = rows(Some(Kind::Prs));
        m.notice_kind = 3;
        let text = texts(&screen(&m, 140, 30).0).join("\n");
        assert!(text.contains("PR 12345 merged"), "{text}");
        assert!(!text.contains("DM Ada"), "{text}");
    }

    #[test]
    fn with_ux2_the_chosen_landing_stays_in_sight_under_two_headings() {
        let landing = |n: usize| {
            Item::new(format!("land:/w/{n}"), format!("1. landing-{n}"))
                .detail("waiting")
                .data(serde_json::json!({ "landing": { "holding": false, "place": n } }))
        };
        let mut items = vec![views::header("/a:direct", "app · straight to main", 4)];
        items.extend((1..=4).map(landing));
        items.push(views::header("/b:direct", "api · straight to main", 5));
        items.extend((5..=9).map(landing));
        let last = items.len() - 1;
        let mut m = mission(
            Vec::new(),
            vec![list_view(ViewId::Landing, "Landing", items)],
        );
        m.snap.rules.ux2 = true;
        m.view = 1;
        m.cursor = last as isize;
        let (lines, _) = screen(&m, 140, 24);
        let text = texts(&lines);
        assert!(
            text[..23].iter().any(|l| l.contains("landing-9")),
            "{text:#?}"
        );
    }

    #[test]
    fn with_ux2_landing_fills_the_screen_whatever_the_headings_out_of_sight() {
        let landing = |n: usize| {
            Item::new(format!("land:/w/{n}"), format!("1. landing-{n}"))
                .detail("waiting")
                .data(serde_json::json!({ "landing": { "holding": false, "place": 1 } }))
        };
        let mut items = Vec::new();
        for n in 1..=12 {
            items.push(views::header(
                &format!("/r{n}:direct"),
                &format!("repo-{n}"),
                1,
            ));
            items.push(landing(n));
        }
        let last = items.len() - 1;
        let mut m = mission(
            Vec::new(),
            vec![list_view(ViewId::Landing, "Landing", items)],
        );
        m.snap.rules.ux2 = true;
        m.view = 1;
        m.cursor = last as isize;
        let (lines, _) = screen(&m, 140, 26);
        let text = texts(&lines);
        let shown = text.iter().filter(|l| l.contains("landing-")).count();
        // 22 rows under the bar: four repositories of five, and the landing before them.
        assert_eq!(shown, 5, "{text:#?}");
        assert!(text[..25].iter().any(|l| l.contains("landing-12")));
    }

    #[test]
    fn with_ux2_a_landing_is_a_row_of_stage_cells_with_its_note_under_its_name() {
        let landing = |n: &str, holding: bool, place: Option<u64>, phase: Option<&str>| {
            Item::new(format!("land:/w/{n}"), format!("1. {n}"))
                .detail(if holding {
                    "landing · pushing · tab 3"
                } else {
                    "waiting · tab 4"
                })
                .data(serde_json::json!({ "landing": {
                    "holding": holding, "place": place, "phase": phase,
                } }))
        };
        let items = vec![
            views::header("/r:direct", "switchboard · straight to main", 2),
            landing("note-stable-ids", true, None, Some("pushing")),
            landing("note-benchmark", false, Some(2), None),
        ];
        let mut m = mission(
            Vec::new(),
            vec![list_view(ViewId::Landing, "Landing", items)],
        );
        m.snap.rules.ux2 = true;
        m.view = 1;
        let (lines, _) = screen(&m, 140, 20);
        let text = texts(&lines);
        let at = text
            .iter()
            .position(|l| l.contains("switchboard · straight to main"))
            .unwrap();
        let heads = &text[at + 1];
        assert!(
            text[at + 2].starts_with("┄┄┄"),
            "the dashed rule under the heads"
        );
        assert!(
            heads.contains("worktree") && heads.contains("leak check") && heads.contains("applied"),
            "{heads}"
        );
        let holder = &text[at + 3];
        assert!(
            holder.contains("note-stable-ids") && !holder.contains("1. "),
            "{holder}"
        );
        assert!(
            holder.contains("✓ held") && holder.contains("pushing…") && holder.contains("waiting"),
            "{holder}"
        );
        assert!(
            text[at + 4].contains("landing · pushing · tab 3"),
            "{}",
            text[at + 4]
        );
        assert!(text[at + 5].contains("2nd"), "{}", text[at + 5]);
        // Each cell is a run of its stage's background: pushing is blue.
        let pushing = lines[at + 3]
            .spans
            .iter()
            .find(|s| s.content.contains("pushing…"))
            .unwrap();
        assert_eq!(pushing.style.bg, Some(Tone::Working.tint(&Theme::DARK)));
        // At any width the cells stretch so the last one ends at the right
        // edge, its head centered over it.
        for cols in [140, 160, 240] {
            let (lines, _) = screen(&m, cols, 20);
            let holder = &lines[at + 3];
            let tinted = holder
                .spans
                .iter()
                .fold((0, 0), |(x, end), s| {
                    let next = x + width(&s.content);
                    (next, if s.style.bg.is_some() { next } else { end })
                })
                .1;
            assert_eq!(tinted, cols - 1, "at {cols}: {}", holder.text());
        }
        // A failed stage is a red cell with ✕ and the phase's words.
        let failing = Item::new("land:/w/x", "1. note-full-titles").data(serde_json::json!({
            "landing": { "holding": true, "phase": "tests failed 2 of 401" },
        }));
        let queued = |state: &str| {
            Item::new("https://pr/2031", "1. #2031 settings page").data(serde_json::json!({
                "queue": { "state": state, "position": 3 },
            }))
        };
        let more = vec![
            views::header("/r:direct", "switchboard · straight to main", 1),
            failing,
            views::header("/r:queue", "web-app · merge queue", 2),
            queued("QUEUED"),
            queued("UNMERGEABLE"),
        ];
        let mut f = mission(
            Vec::new(),
            vec![list_view(ViewId::Landing, "Landing", more)],
        );
        f.snap.rules.ux2 = true;
        f.view = 1;
        let (flines, _) = screen(&f, 140, 30);
        let ftext = texts(&flines);
        let row = ftext
            .iter()
            .position(|l| l.contains("note-full-titles"))
            .unwrap();
        let cell = flines[row]
            .spans
            .iter()
            .find(|s| s.content.contains("✕ 2 of 401"))
            .unwrap();
        assert_eq!(cell.style.bg, Some(Color::Indexed(52)));
        // A merge queue's PR is a row of its own stages.
        let heads = ftext.iter().find(|l| l.contains("pull request")).unwrap();
        assert!(
            heads.contains("CI") && heads.contains("deploy test"),
            "{heads}"
        );
        let pr = ftext
            .iter()
            .position(|l| l.contains("#2031 settings page"))
            .unwrap();
        assert!(
            ftext[pr].contains("✓ CI")
                && ftext[pr].contains("✓ approved")
                && ftext[pr].contains("3rd in queue"),
            "{}",
            ftext[pr]
        );
        assert!(ftext[pr + 2].contains("✕ can't merge"), "{}", ftext[pr + 2]);
        // Off, the rows are as before.
        m.snap.rules.ux2 = false;
        let (lines, _) = screen(&m, 140, 20);
        assert!(
            texts(&lines)
                .iter()
                .any(|l| l.contains("1. note-stable-ids"))
        );
    }

    /// A description of `n` paragraphs, "Line 1" to "Line <n>".
    fn long_description(n: usize) -> String {
        (1..=n)
            .map(|i| format!("Line {i}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    #[test]
    fn the_pane_beside_a_board_scrolls_to_its_later_lines_and_stops_at_its_end() {
        let item = Item::new("menu", "Fix the menu")
            .group("Todo")
            .data(serde_json::json!({
                "ticket": "menu", "state": "Todo", "description": long_description(30),
            }));
        let mut m = mission(
            Vec::new(),
            vec![list_view(ViewId::Tickets, "Tickets", vec![item])],
        );
        m.snap.rules.ux2 = true;
        m.view = 1;
        m.size = (160, 20);
        let pane = |m: &Mission| texts(&drawn(m, 160, 20).0).join("\n");
        let (_, hits, _, _) = drawn(&m, 160, 20);
        m.hits = hits;
        let last = m.side_last().expect("the pane is shown");
        assert!(last > 0, "longer than the pane");
        let first = pane(&m);
        assert!(first.contains("Line 1\n"), "{first}");
        assert!(!first.contains("Line 30"), "{first}");
        assert!(first.contains("J K scroll"), "the footer: {first}");

        assert!(m.scroll_side(4, last));
        assert_eq!(m.side_top(), 4);
        let scrolled = pane(&m);
        assert!(!scrolled.contains("Line 1\n"), "{scrolled}");
        assert!(scrolled.contains("Line 3\n"), "{scrolled}");

        // Past its end it stops with its last line at the bottom.
        m.scroll_side(1000, last);
        assert_eq!(m.side_top(), last);
        let end = pane(&m);
        assert!(end.contains("Line 30"), "{end}");
        assert!(!m.scroll_side(1, last), "nothing further");
        m.scroll_side(-1000, last);
        assert_eq!(m.side_top(), 0);
    }

    #[test]
    fn the_autopilot_plan_beside_the_board_scrolls_too() {
        let rows = vec![pilot_row(3, "Give notes stable IDs", TabState::Working)];
        let list = crate::autopilot::view::items(
            &pilot_summary(),
            "",
            &rows,
            &[],
            &[],
            &std::collections::HashMap::from([(
                "Give notes stable IDs".to_string(),
                crate::cmd::worktree_from_note::PlanNote {
                    name: "Plan - Stable IDs".into(),
                    text: Some(long_description(40)),
                },
            )]),
        );
        let chosen = list
            .iter()
            .position(|i| i.title == "Give notes stable IDs")
            .unwrap();
        let mut m = mission(
            Vec::new(),
            vec![list_view(ViewId::Autopilot, "Autopilot", list)],
        );
        m.snap.rules.ux2 = true;
        m.view = 1;
        m.size = (160, 30);
        m.cursor = chosen as isize;
        let (lines, hits, _, _) = drawn(&m, 160, 30);
        assert!(!texts(&lines).join("\n").contains("Line 40"));
        m.hits = hits;
        let last = m.side_last().expect("the pane is shown");
        m.scroll_side(1000, last);
        let end = texts(&drawn(&m, 160, 30).0).join("\n");
        assert!(end.contains("Line 40"), "{end}");
        assert!(!end.contains("Plan - Stable IDs"), "{end}");

        // Another card's pane shows from its top, and so does this one
        // once chosen again after it was drawn.
        m.cursor = 0;
        assert_eq!(m.side_top(), 0);
        m.forget_side_elsewhere();
        m.cursor = chosen as isize;
        assert_eq!(m.side_top(), 0);
    }

    #[test]
    fn a_window_of_the_pane_cuts_a_picture_at_its_top_and_bottom() {
        let about = About {
            lines: vec![Vec::new(); 30],
            pictures: vec![Picture {
                key: "k".into(),
                id: 1,
                x: 0,
                y: 5,
                cols: 10,
                rows: 10,
                from: 0,
                shown: 10,
            }],
        };
        let window = about.clone().window(8, 4);
        assert_eq!(window.lines.len(), 4);
        let [p] = &window.pictures[..] else {
            panic!("{:?}", window.pictures);
        };
        assert_eq!((p.y, p.from, p.shown), (0, 3, 4));
        let window = about.clone().window(0, 8);
        assert_eq!(
            (
                window.pictures[0].y,
                window.pictures[0].from,
                window.pictures[0].shown
            ),
            (5, 0, 3)
        );
        assert!(about.window(15, 5).pictures.is_empty(), "scrolled past it");
    }

    #[test]
    fn with_ux2_the_usage_view_is_a_board_of_charts_tabs_limits_and_requests() {
        use crate::requests::{ServiceCount, Tally};
        use crate::usage::charts::{self, Limit, Model, SpanSpend, TabSpend};
        let now = rules().now_secs();
        let start = now * 1000 - 2 * charts::QUARTER_MS;
        let tab = |key: &str, index: usize, name: &str, spent: f64, share: f64| TabSpend {
            key: key.into(),
            tab: Some(index as u32),
            index,
            name: name.into(),
            color: Some("23ef59".into()),
            spent,
            share,
            first_ms: Some(start),
            rate: Some(1.21),
        };
        let span = SpanSpend {
            total: 4.0,
            start_ms: start,
            bin_ms: charts::QUARTER_MS,
            bins: vec![vec![1.0, 0.0], vec![0.0, 1.0], vec![2.0, 0.0]],
            tabs: vec![
                tab("tab:7", 7, "api", 3.0, 0.75),
                tab("tab:3", 3, "docs", 1.0, 0.25),
            ],
        };
        let github = ServiceCount {
            service: "GitHub".into(),
            total: 30,
            protocols: [("GraphQL".to_string(), 30)].into_iter().collect(),
            by_time: vec![1; 24],
            sources: [("PR board".to_string(), 20), ("CI".to_string(), 10)]
                .into_iter()
                .collect(),
        };
        let linear = ServiceCount {
            service: "Linear".into(),
            total: 10,
            protocols: [("GraphQL".to_string(), 10)].into_iter().collect(),
            by_time: vec![0; 24],
            sources: [("ticket view".to_string(), 10)].into_iter().collect(),
        };
        let day = Tally {
            total: 40,
            services: vec![github, linear],
            protocols: [("GraphQL".to_string(), 40)].into_iter().collect(),
        };
        let model = Model {
            today: span.clone(),
            week: {
                // A tab that spent yesterday, nothing today.
                let mut week = span;
                week.tabs.push(tab("tab:9", 9, "notes", 2.0, 0.33));
                week.bins = week
                    .bins
                    .iter()
                    .map(|b| [b.clone(), vec![0.0]].concat())
                    .collect();
                week
            },
            limits: vec![
                Limit {
                    key: "five_hour".into(),
                    name: "5-hour".into(),
                    used: 29.0,
                    resets_at: now + 3 * 3600,
                    pace: Some(58.0),
                    detail: None,
                },
                Limit {
                    key: "seven_day".into(),
                    name: "Week".into(),
                    used: 40.0,
                    resets_at: now + 86400,
                    pace: None,
                    detail: None,
                },
            ],
            limits_at: Some(now * 1000 - 12_000),
            requests: Some((day.clone(), {
                // Slack had requests this week, none today.
                let mut week = day.clone();
                week.services.push(ServiceCount {
                    service: "Slack".into(),
                    total: 4,
                    protocols: [("REST".to_string(), 4)].into_iter().collect(),
                    by_time: vec![0, 0, 0, 0, 4, 0, 0],
                    sources: [("relay".to_string(), 4)].into_iter().collect(),
                });
                week
            })),
            requests_file: "requests/today.tsv".into(),
        };
        let mut base = vec![Item::new("spent", "Spent today $4.00")];
        base.extend(crate::usage::sessions::request_items(&day));
        let items = charts::items(base, &model);
        let mut m = mission(
            Vec::new(),
            vec![list_view(ViewId::Usage, "Usage", items.clone())],
        );
        m.snap.rules.ux2 = true;
        m.view = 1;
        m.size = (160, 40);
        // The cursor on the first tab's row.
        let first_tab = items.iter().position(|i| i.id == "tab:tab:7").unwrap();
        m.cursor = first_tab as isize;
        let (lines, hits, images, _) = drawn(&m, 160, 40);
        let text = texts(&lines).join("\n");
        assert!(text.contains("Today $4.00"), "{text}");
        assert!(text.contains(" 7 api"), "{text}");
        assert!(text.contains("75%"), "{text}");
        assert!(text.contains("tab 7 since"), "{text}");
        assert!(text.contains("$1.21 an hour"), "{text}");
        assert!(text.contains("at this pace, 58% by then"), "{text}");
        assert!(text.contains("12s ago"), "{text}");
        assert!(text.contains("40 today to 2 services"), "{text}");
        assert!(text.contains("jk tab or service"), "{text}");
        // One chart, with the chosen tab outlined; a meter a limit, the
        // 5-hour one with its pace as a tick; a sparkline a service.
        let charts: Vec<&Image> = images.iter().filter(|i| i.name == "usage/chart").collect();
        assert_eq!(charts.len(), 1);
        match &charts[0].shape {
            Shape::Series { marks, bars, .. } => {
                assert_eq!(marks.outline, Some(0));
                assert_eq!(bars.len(), 3);
            }
            other => panic!("{other:?}"),
        }
        let ticks = images
            .iter()
            .filter(|i| matches!(i.shape, Shape::Meter { tick: Some(_), .. }))
            .count();
        assert_eq!(ticks, 1);
        let named = |prefix: &str| images.iter().filter(|i| i.name.starts_with(prefix)).count();
        assert_eq!(named("usage/limit/"), 2);
        // Slack, with requests this week and none today, has no row today.
        assert_eq!(named("usage/spark/"), 2);
        assert!(!text.contains("Slack"), "{text}");
        // An unchosen service's meter is filled in the dim text color.
        let meter = images
            .iter()
            .find(|i| i.name == "usage/share/Linear")
            .unwrap();
        assert!(
            matches!(meter.shape, Shape::Meter { color, .. } if color == m.snap.theme.dim),
            "{:?}",
            meter.shape
        );
        assert!(
            hits.iter()
                .any(|h| h.spot == Spot::Index(first_tab as isize))
        );
        // A tab's row lights under the mouse.
        let docs = items.iter().position(|i| i.id == "tab:tab:3").unwrap();
        m.hover = Some(Spot::Index(docs as isize));
        let (lines, _, _, _) = drawn(&m, 160, 40);
        let row = lines.iter().find(|l| l.text().contains(" 3 docs")).unwrap();
        assert!(
            row.spans
                .iter()
                .any(|s| s.content.contains("docs") && s.style.bg == Some(m.snap.theme.panel))
        );
        m.hover = None;
        // Each service shows what made its requests on its second line,
        // under headings over its sparkline and meter.
        let service = items
            .iter()
            .position(|i| i.id == "requests:GitHub")
            .unwrap();
        m.cursor = service as isize;
        let text = texts(&drawn(&m, 160, 40).0).join("\n");
        assert!(text.contains("made by PR board 20 · CI 10"), "{text}");
        assert!(text.contains("made by ticket view 10"), "{text}");
        assert!(text.contains("by hour"), "{text}");
        assert!(text.contains("share of today"), "{text}");
        // At any width the chart reaches By tab, whose share ends at the
        // right edge, and a service's sparkline and meter fill its row up
        // to the count at the right edge.
        for cols in [136, 160, 240] {
            m.size = (cols as u16, 40);
            let (lines, _, images, _) = drawn(&m, cols, 40);
            let image = |name: &str| images.iter().find(|i| i.name == name).unwrap();
            let chart = image("usage/chart");
            let heading = lines
                .iter()
                .map(|l| l.text())
                .find(|t| t.contains("BY TAB"))
                .unwrap();
            let by_tab = heading.find("BY TAB").unwrap();
            let chart_end = (chart.x + chart.width) as usize;
            assert!(
                by_tab - chart_end <= 6,
                "at {cols}: chart ends {chart_end}, By tab at {by_tab}"
            );
            assert_eq!(width(heading.trim_end()), cols - 2, "at {cols}: {heading}");
            let spark = image("usage/spark/GitHub");
            let meter = image("usage/share/GitHub");
            assert_eq!(spark.height, 2);
            let meter_end = (meter.x + meter.width) as usize;
            assert!(cols - meter_end <= 11, "at {cols}: meter ends {meter_end}");
            assert!(spark.width >= 24 && meter.width >= 20, "at {cols}");
        }
        m.size = (160, 40);
        // A tab of the week alone is drawn today at none, and can be chosen.
        let notes = items.iter().position(|i| i.id == "tab:tab:9").unwrap();
        m.cursor = notes as isize;
        let (lines, hits, _, _) = drawn(&m, 160, 40);
        let text = texts(&lines).join("\n");
        assert!(text.contains(" 9 notes"), "{text}");
        assert!(hits.iter().any(|h| h.spot == Spot::Index(notes as isize)));
        // A service of the week alone is passed over today, and in the
        // week's board can be chosen; on a short screen the board scrolls
        // to it.
        let slack = items.iter().position(|i| i.id == "requests:Slack").unwrap();
        assert!(m.week_only_hidden());
        // The plain list shows it, so a narrow screen lets the cursor reach it.
        m.size = (120, 40);
        assert!(!m.week_only_hidden());
        m.size = (160, 40);
        assert!(crate::mission::views::passed_over(&items[slack], true));
        assert!(!crate::mission::views::passed_over(&items[slack], false));
        m.cursor = slack as isize;
        m.size = (160, 24);
        m.usage_week = true;
        let (lines, hits, images, _) = drawn(&m, 160, 24);
        let text = texts(&lines).join("\n");
        assert!(text.contains("made by relay 4"), "{text}");
        assert!(hits.iter().any(|h| h.spot == Spot::Index(slack as isize)));
        assert!(images.iter().all(|i| ((i.y + i.height) as usize) < 24));
        assert!(
            !text.contains("This week $"),
            "scrolled past the heading: {text}"
        );
        m.usage_week = false;
        m.size = (160, 40);
        // Too narrow for the board: the list.
        let text = texts(&drawn(&m, 120, 40).0).join("\n");
        assert!(!text.contains("SPEND BY QUARTER HOUR"), "{text}");
    }

    #[test]
    fn with_ux2_the_keys_view_is_a_board_of_groups_a_heatmap_the_chosen_key_and_its_table() {
        use crate::mission::keymap::{self, Shown};
        let mut bindings = crate::mux::keys::Bindings::default();
        bindings.apply(&serde_json::json!({ "root": { "M-g": "new-tab" } }));
        let counts: std::collections::HashMap<String, u64> =
            [("M-j".to_string(), 212), ("M-t".to_string(), 3)]
                .into_iter()
                .collect();
        let shown = Shown {
            github: true,
            linear: true,
            ask: true,
            easy: false,
        };
        let items = keymap::items(&bindings, shown, &counts);
        let mut view = list_view(ViewId::Keys, "Keys", items.clone());
        view.data.head = keymap::head(&counts, true);
        let mut m = mission(Vec::new(), vec![view]);
        m.snap.rules.ux2 = true;
        m.view = 1;
        m.size = (160, 44);
        let find = items
            .iter()
            .position(|i| keymap::press_of(i) == Some(crate::mux::keys::Action::FindTab))
            .unwrap();
        m.cursor = find as isize;
        let (lines, hits, images, _) = drawn(&m, 160, 44);
        let text = texts(&lines).join("\n");
        assert!(text.contains("WORKSPACE"), "{text}");
        assert!(text.contains("#tabs"), "{text}");
        assert!(text.contains("EACH APP'S OWN"), "{text}");
        assert!(text.contains("THROUGH M-P ONLY"), "{text}");
        assert!(text.contains("212 this week · your most used"), "{text}");
        assert!(text.contains("the outlined ones are #tabs"), "{text}");
        assert!(text.contains("WHAT IT DOES"), "{text}");
        assert!(text.contains("hl group"), "{text}");
        // The keyboard, with #tabs' Alt keys outlined and M-j's presses.
        let [keyboard] = &images[..] else {
            panic!("{images:?}");
        };
        match &keyboard.shape {
            Shape::Keyboard { heat, outlined } => {
                assert_eq!(heat.get(&'J'), Some(&212));
                assert!(outlined.contains(&'J'));
                assert!(outlined.contains(&'1'), "M-1 goes to a tab");
            }
            other => panic!("{other:?}"),
        }
        // A click on a table row chooses it (and a second presses it); one
        // on a group or a key of the keyboard only chooses, never pressing.
        assert!(hits.iter().any(|h| h.spot == Spot::Index(find as isize)));
        let panel_hits: Vec<_> = hits.iter().filter(|h| h.x0 == 0 && h.y > 2).collect();
        assert!(!panel_hits.is_empty());
        assert!(panel_hits.iter().all(|h| matches!(h.spot, Spot::Choose(_))));
        assert!(
            hits.iter()
                .any(|h| h.spot == Spot::Choose(find as isize) && h.x0 > 0)
        );
        // M-g, bound in keys.json, is yours in its group's table.
        let new_tab = items
            .iter()
            .position(|i| keymap::press_of(i) == Some(crate::mux::keys::Action::NewTab))
            .unwrap();
        m.cursor = new_tab as isize;
        let text = texts(&drawn(&m, 160, 44).0).join("\n");
        let row = text.lines().find(|l| l.contains("▌M-g")).unwrap_or("");
        assert!(row.contains("yours"), "{text}");
        // With habits off, what turns the counts on in the keyboard's place.
        m.snap.views[0].data.head = keymap::head(&Default::default(), false);
        let (lines, _, images, _) = drawn(&m, 160, 44);
        assert!(images.is_empty());
        assert!(texts(&lines).join("\n").contains("Turn on Suggestions"));
        // Too narrow for the board: the list.
        let text = texts(&drawn(&m, 90, 44).0).join("\n");
        assert!(!text.contains("WHAT IT DOES"), "{text}");
    }

    #[test]
    fn the_keys_board_lays_out_from_the_screens_width() {
        use crate::mission::keymap::{self, Shown};
        let bindings = crate::mux::keys::Bindings::default();
        let counts: std::collections::HashMap<String, u64> =
            [("M-j".to_string(), 212)].into_iter().collect();
        let shown = Shown {
            github: true,
            linear: true,
            ask: true,
            easy: false,
        };
        let items = keymap::items(&bindings, shown, &counts);
        let mut view = list_view(ViewId::Keys, "Keys", items.clone());
        view.data.head = keymap::head(&counts, true);
        let mut m = mission(Vec::new(), vec![view]);
        m.snap.rules.ux2 = true;
        m.view = 1;
        m.cursor = items
            .iter()
            .position(|i| keymap::press_of(i) == Some(crate::mux::keys::Action::FindTab))
            .unwrap() as isize;
        let lines_at = |m: &mut Mission, cols: usize| {
            m.size = (cols as u16, 45);
            let (lines, _, images, _) = drawn(m, cols, 45);
            (texts(&lines), images)
        };
        let mut right_edges = Vec::new();
        for cols in [120, 160, 240] {
            let (text, images) = lines_at(&mut m, cols);
            let all = text.join("\n");
            assert!(text.iter().all(|l| width(l) <= cols), "{cols}: {all}");
            assert_eq!(images.len(), 1, "{cols}: the keyboard");
            assert!(all.contains("Search and jump to a tab"), "{cols}: {all}");
            // The table's From column ends at the right edge.
            let row = text.iter().find(|l| l.contains("▌M-j")).unwrap();
            assert!(row.trim_end().ends_with("default"), "{cols}: {row}");
            right_edges.push(width(row.trim_end()));
            // The chosen key beside the keyboard at every width here; at
            // 120 the tags column, less important, has given way.
            assert!(all.contains("212 this week"), "{cols}: {all}");
            assert_eq!(all.contains("TAGS"), cols > 120, "{cols}: {all}");
        }
        assert!(
            right_edges.windows(2).all(|w| w[0] < w[1]),
            "{right_edges:?}"
        );
        // Too short for the panel of groups: the list, which reaches them all.
        m.size = (160, 24);
        let (lines, _, _, _) = drawn(&m, 160, 24);
        assert!(!texts(&lines).join("\n").contains("WHAT IT DOES"));
        // Narrower, the chosen key beside gives way too.
        let (text, _) = lines_at(&mut m, 104);
        let all = text.join("\n");
        assert!(!all.contains("TAGS"), "{all}");
        assert!(!all.contains("212 this week"), "{all}");
        assert!(all.contains("WHAT IT DOES"), "{all}");
    }
}
