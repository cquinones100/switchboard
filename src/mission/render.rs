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

use super::views::{self, View, ViewId};
use super::{Focus, HitBox, Mission, Spot};
use crate::items::{Item, State};
use crate::mux::input::{Key, KeyCode, Mods};
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

// ── UX 2.0's Tabs: cards grouped as the sidebar, then the shelf (board 3) ──

/// A card's least width with UX 2.0, and its height.
pub const CARD_W: usize = 40;
const CARD_H: usize = 17;
/// The columns between two cards side by side.
const CARD_GAP: usize = 2;
/// The most cards side by side, however wide the screen.
const MOST_ACROSS: usize = 4;
/// The longest name a card's top edge shows, and a shelf line.
const CARD_NAME: usize = 20;
const SHELF_NAME: usize = 26;

/// Whether a stage's tabs are lines on the shelf under the cards rather
/// than cards.
fn on_shelf(stage: model::Stage) -> bool {
    matches!(stage, model::Stage::Landing | model::Stage::Idle)
}

/// How many cards go side by side on a screen `cols` wide: as many as fit
/// at their least width, at most `MOST_ACROSS`.
pub fn across(cols: usize) -> usize {
    ((cols + CARD_GAP) / (CARD_W + CARD_GAP)).clamp(1, MOST_ACROSS)
}

/// The card columns' x and width, sharing the screen's width equally.
fn card_columns(cols: usize, across: usize) -> Vec<(usize, usize)> {
    let each = Col::Stretch {
        weight: 1,
        min: CARD_W.min(cols),
        max: None,
    };
    kit::columns(&vec![each; across], cols, CARD_GAP, &[])
        .into_iter()
        .flatten()
        .collect()
}

/// The tabs' groups as UX 2.0 draws them: the stages of cards in the
/// sidebar's order, then the shelf's (Landing, then Idle), each with its
/// tabs as indices into `tabs`.
fn tab_groups(tabs: &[TabModel], r: &Rules) -> Vec<(model::Stage, Vec<usize>)> {
    let staged: Vec<(model::Stage, Vec<usize>)> = model::grouped(tabs, None, r)
        .into_iter()
        .map(|s| {
            let at = s
                .tabs
                .iter()
                .map(|t| tabs.iter().position(|x| x.id == t.id).unwrap_or(0))
                .collect();
            (s.stage, at)
        })
        .collect();
    let (shelf, cards): (Vec<_>, Vec<_>) = staged.into_iter().partition(|(s, _)| on_shelf(*s));
    cards.into_iter().chain(shelf).collect()
}

/// The Tabs view's grid for whichever look it has: UX 2.0's groups and
/// shelf, or the stage boxes.
pub fn tabs_grid(m: &Mission, cols: usize) -> Grid {
    if m.snap.rules.ux2 {
        grid_ux2(&m.snap.tabs, &m.snap.rules, cols, m.shelf_open)
    } else {
        grid(
            &m.snap.tabs,
            &m.snap.rules,
            cols,
            card_width(cols, m.snap.card),
        )
    }
}

/// Whether the shelf shows only its labels: with room for one column of
/// cards, until Enter on a label opens it.
fn shelf_folded(cols: usize, open: bool) -> bool {
    across(cols) == 1 && !open
}

/// UX 2.0's Tabs laid out: each group of cards in rows of `across`, then
/// each shelf line a row of its own; a folded shelf's group is one row, its
/// first tab standing for its label.
pub fn grid_ux2(tabs: &[TabModel], r: &Rules, cols: usize, open: bool) -> Grid {
    let n = across(cols);
    let folded = shelf_folded(cols, open);
    let mut order = Vec::new();
    let mut rows = Vec::new();
    for (stage, at) in tab_groups(tabs, r) {
        let shown: &[usize] = if on_shelf(stage) && folded {
            &at[..1]
        } else {
            &at
        };
        let first = order.len();
        order.extend_from_slice(shown);
        let places: Vec<usize> = (first..order.len()).collect();
        let per_row = if on_shelf(stage) { 1 } else { n };
        rows.extend(places.chunks(per_row).map(<[usize]>::to_vec));
    }
    Grid {
        order,
        shelves: Vec::new(),
        rows,
    }
}

/// A tab's state in a word or two and how long it has been in it, for
/// its card's top edge: "waiting 4m", "working 2m".
fn state_for(tab: &TabModel, r: &Rules) -> String {
    let words = match tab.state {
        TabState::Waiting => "waiting",
        TabState::Working => "working",
        TabState::Background => "in the background",
        TabState::Done => "done",
        TabState::Idle => "idle",
    };
    let ago = text::ago(tab.since, r.now_secs());
    if ago.is_empty() {
        words.to_string()
    } else {
        format!("{words} {ago}")
    }
}

/// A tab's PR's checks as the board's chip: all passed, how many of them
/// have run, or how many failed; "no PR" dim without one.
fn checks_chip(tab: &TabModel, caps: bool, theme: &Theme) -> Vec<Span<'static>> {
    let Some(c) = tab.checks else {
        let words = match tab.pr {
            Some(n) => format!("#{n}"),
            None => "no PR".into(),
        };
        return vec![Span::styled(words, Tone::Dim.style(theme))];
    };
    let total = c.pass + c.fail + c.pending;
    let (mark, words) = if c.fail > 0 {
        (Mark::Failed, format!("{} of {total}", c.fail))
    } else if c.pending > 0 {
        (Mark::Pending, format!("{} of {total}", c.pass))
    } else {
        (Mark::Passed, format!("{} passed", c.pass))
    };
    let (glyph, tone) = mark.look();
    let mut chip = kit::chip(&format!("{glyph} {words}"), tone, caps, theme);
    chip.pop();
    chip
}

/// The cells of a card's context meter.
const CONTEXT_CELLS: usize = 10;

/// A card's context meter, `ctx` and its cells, in the needs-you tone once
/// it is as full as the Sessions view calls full.
fn context_meter(tab: &TabModel, theme: &Theme) -> Vec<Span<'static>> {
    let Some(used) = tab.context else {
        return Vec::new();
    };
    let tone = if f64::from(used) >= crate::usage::sessions::FULL {
        Tone::Needs
    } else {
        Tone::Dim
    };
    let mut out = vec![Span::styled("ctx ", Tone::Dim.style(theme))];
    out.extend(kit::meter(used, CONTEXT_CELLS, tone, theme));
    out
}

/// A card's foot: its branch dim, its context meter, its checks' chip at
/// the right edge; the meter goes first where the card is too narrow.
fn card_foot(
    tab: &TabModel,
    branch: Option<&str>,
    inner: usize,
    caps: bool,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let chip = checks_chip(tab, caps, theme);
    let chip_w = spans_width(&chip);
    let meter = context_meter(tab, theme);
    let meter_w = spans_width(&meter);
    let at = kit::columns(
        &[
            Col::Stretch {
                weight: 1,
                min: 1,
                max: None,
            },
            Col::Fixed {
                min: meter_w,
                natural: meter_w,
                cap: meter_w,
            },
            Col::Right {
                natural: chip_w,
                cap: chip_w,
            },
        ],
        inner,
        1,
        &[1],
    );
    let mut line = Vec::new();
    if let Some(Some(place)) = at.first() {
        kit::place(
            &mut line,
            vec![Span::styled(
                branch.unwrap_or_default().to_string(),
                Tone::Dim.style(theme),
            )],
            *place,
        );
    }
    if let Some(Some(place)) = at.get(1)
        && !meter.is_empty()
    {
        kit::place(&mut line, meter, *place);
    }
    if let Some(Some(place)) = at.get(2) {
        kit::place(&mut line, chip, *place);
    }
    line
}

/// A section's name inside a card, in capitals and bold in its tone.
fn card_label(text: &str, tone: Tone, theme: &Theme) -> Vec<Span<'static>> {
    // The kit's section label, less the space it starts with at a
    // screen's edge, since the card's border is there.
    kit::label(text, tone, None, width(&text.to_uppercase()) + 1, theme)
        .into_iter()
        .skip(1)
        .collect()
}

/// Text wrapped across a card, at most `most` lines, the last cut with …
/// when there was more.
fn wrapped(text: &str, inner: usize, most: usize, style: Style) -> Vec<Vec<Span<'static>>> {
    let all = text::wrap(text, inner);
    let more = all.len() > most;
    all.into_iter()
        .take(most)
        .enumerate()
        .map(|(i, line)| {
            let line = if more && i + 1 == most {
                format!("{}…", text::cut(&line, inner.saturating_sub(1)))
            } else {
                line
            };
            vec![Span::styled(line, style)]
        })
        .collect()
}

/// A turn's words for a card: Haiku's sentence once it has one, else the
/// first line of its reply.
fn turn_words(tab: &TabModel) -> Option<String> {
    tab.summary.clone().or_else(|| {
        let reply = tab.turn.as_ref()?.reply.as_deref()?;
        let plain = unlinked(&text::plain(reply));
        plain
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(str::to_string)
    })
}

/// What a card holds under its top edge, by what its tab is doing, and
/// which of its lines are a numbered option (its line, its number).
struct Inside {
    lines: Vec<Vec<Span<'static>>>,
}

/// Where a card is and how it is drawn.
struct CardLook<'a> {
    w: usize,
    h: usize,
    chosen: bool,
    hovered: bool,
    /// Whether the chips under Now have room.
    chips: bool,
    screen: Option<&'a super::Screen>,
    /// Whether it shows its screen (`s`).
    shows_screen: bool,
}

/// A card waiting on you: what it asks, word for word, under its kind; its
/// options numbered; and what Claude said before asking.
/// The most lines of what a tab asks a card shows: a request longer than
/// that at a card's least width isn't answered from the card, a number
/// going to the tab, whose prompt shows all of it (`ask_fits`).
const ASK_LINES: usize = 6;

/// What a waiting tab asks: its kind, its words and its options. A number
/// answers what the screen shows, so the screen's options win over the
/// hook's; past a call's first question, its words do too.
/// An option: its label, its description, and whether it is known whole
/// (paired with the hook's, or one of Claude Code's own).
type AskOption = (String, String, bool);

fn ask_words(tab: &TabModel, screen: Option<&super::Screen>) -> (String, String, Vec<AskOption>) {
    let screen_options = screen.map(|s| s.options.clone()).unwrap_or_default();
    let bare = |options: Vec<String>, whole: bool| -> Vec<AskOption> {
        options
            .into_iter()
            .map(|o| (o, String::new(), whole))
            .collect()
    };
    let screen_question = screen.map(|s| s.question.clone()).unwrap_or_default();
    let (kind, words, options) = match &tab.ask {
        Some(a) if a.kind == crate::agent::WaitKind::Question && a.questions > 1 => (
            "Question".to_string(),
            if screen_question.is_empty() {
                a.words.clone()
            } else {
                screen_question
            },
            bare(screen_options, false),
        ),
        Some(a) if a.kind == crate::agent::WaitKind::Question => (
            if a.header.is_empty() {
                "Question".to_string()
            } else {
                format!("Question · {}", a.header)
            },
            a.words.clone(),
            {
                // The screen's options in its order, each as the hook gave
                // it whole, with its description; one only the screen has
                // (an answer of your own) as it is.
                // Paired by place, as Claude Code lists them in the hook's
                // order, never by words two options may share.
                let given = |i: usize, label: &str| {
                    a.options
                        .get(i)
                        .is_some_and(|o| o == label || (!label.is_empty() && o.starts_with(label)))
                        .then_some(i)
                };
                let listed: Vec<String> = if screen_options.is_empty() {
                    a.options.clone()
                } else {
                    screen_options
                };
                listed
                    .into_iter()
                    .enumerate()
                    .map(|(at, label)| match given(at, &label) {
                        Some(i) => (
                            a.options[i].clone(),
                            a.details.get(i).cloned().unwrap_or_default(),
                            true,
                        ),
                        None => (label, String::new(), false),
                    })
                    .collect()
            },
        ),
        Some(a) => (
            if a.tool == "Bash" {
                "Bash command".to_string()
            } else {
                a.tool.clone()
            },
            a.words.clone(),
            {
                // Claude Code's own options, whole unless the screen shows
                // more of one than its label.
                let more = screen.map(|s| s.more.clone()).unwrap_or_default();
                screen_options
                    .into_iter()
                    .enumerate()
                    .map(|(at, o)| {
                        let whole = !more.contains(&(at as u32 + 1));
                        (o, String::new(), whole)
                    })
                    .collect()
            },
        ),
        None => {
            let words = tab
                .turn
                .as_ref()
                .and_then(|t| t.pending.clone())
                .or_else(|| tab.wait.clone())
                .unwrap_or_default();
            (
                "Waiting on you".to_string(),
                words,
                bare(screen_options, false),
            )
        }
    };
    (kind, words, options)
}

/// The most lines of an option's description a card shows.
const DETAIL_LINES: usize = 2;

/// Whether a waiting card shows all of what its tab asks, at a card's
/// least width, else it says the tab has all of it: a question asking one thing,
/// its options' descriptions short enough to show, or a Bash command,
/// which its words are whole. Any other permission (an edit's change, a
/// connector's input), or a call of several questions (whose later
/// options' descriptions the hook doesn't give), is more than the card
/// keeps.
pub fn ask_fits(tab: &TabModel, screen: Option<&super::Screen>) -> bool {
    let whole = tab.ask.as_ref().is_some_and(|a| match a.kind {
        crate::agent::WaitKind::Question => a.questions <= 1,
        crate::agent::WaitKind::Permission => a.tool == "Bash" && !a.words.is_empty(),
    });
    let (_, words, options) = ask_words(tab, screen);
    let details_fit = options
        .iter()
        .all(|(_, d, _)| text::wrap(d, CARD_W - 4 - 3).len() <= DETAIL_LINES);
    whole && details_fit && ask_lines(tab, &words, CARD_W - 4).is_some_and(|l| l.len() <= ASK_LINES)
}

/// What a tab asks laid out in `cols`: a command character for character,
/// every space kept (two spaces in a quoted name are another name), `None`
/// when it holds a character a card can't draw (a tab, an escape); a
/// question wrapped on its words.
fn ask_lines(tab: &TabModel, words: &str, cols: usize) -> Option<Vec<String>> {
    let command = tab
        .ask
        .as_ref()
        .is_some_and(|a| a.kind == crate::agent::WaitKind::Permission);
    if !command {
        return Some(text::wrap(words, cols));
    }
    if words.chars().any(|c| c.is_control() && c != '\n') {
        return None;
    }
    let cols = cols.max(1);
    let mut out = Vec::new();
    for line in words.split('\n') {
        let mut now = String::new();
        let mut used = 0;
        for c in line.chars() {
            let w = width(c.encode_utf8(&mut [0; 4]));
            if used + w > cols {
                out.push(std::mem::take(&mut now));
                used = 0;
            }
            now.push(c);
            used += w;
        }
        out.push(now);
    }
    Some(out)
}

fn asks_inside(tab: &TabModel, look: &CardLook, rows: usize, theme: &Theme) -> Inside {
    let inner = look.w.saturating_sub(4);
    let (kind, words, options) = ask_words(tab, look.screen);
    let options: Vec<AskOption> = options.into_iter().take(9).collect();
    let before = tab
        .turn
        .as_ref()
        .and_then(|t| t.before.as_deref())
        .map(|b| unlinked(&text::plain(b)))
        .unwrap_or_default();
    let mut lines = vec![card_label(&kind, Tone::Needs, theme)];
    // The words first, whole up to `ASK_LINES`; one longer says where
    // the rest is. Before it asked takes what is left.
    let laid = ask_lines(tab, &words, inner).unwrap_or_else(|| text::wrap(&words, inner));
    let style = Tone::Text.style(theme);
    if ask_fits(tab, look.screen) {
        lines.extend(laid.into_iter().map(|l| vec![Span::styled(l, style)]));
    } else {
        lines.extend(
            laid.into_iter()
                .take(ASK_LINES - 1)
                .map(|l| vec![Span::styled(l, style)]),
        );
        lines.push(vec![Span::styled(
            "the tab shows all of it",
            Tone::Dim.style(theme),
        )]);
    }
    // The options as the question lists them, with no numbers: Enter's
    // menu answers with one.
    if !options.is_empty() {
        lines.push(Vec::new());
        for (option, detail, _) in &options {
            let text = text::cut(option, inner.saturating_sub(5));
            lines.push(kit::option(
                &text,
                None,
                inner.saturating_sub(1),
                false,
                theme,
            ));
            // Its description under it, dim, as the prompt shows it.
            for line in wrapped(
                detail,
                inner.saturating_sub(3),
                DETAIL_LINES,
                Tone::Dim.style(theme),
            )
            .into_iter()
            .filter(|l| !l.iter().all(|s| s.content.trim().is_empty()))
            {
                let mut indented = vec![Span::raw("   ")];
                indented.extend(line);
                lines.push(indented);
            }
        }
    }
    if !before.is_empty() {
        lines.push(Vec::new());
        lines.push(card_label("Before it asked", Tone::Dim, theme));
        let room = rows.saturating_sub(lines.len()).clamp(1, 5);
        lines.extend(wrapped(&before, inner, room, Tone::Dim.style(theme)));
    }
    Inside { lines }
}

/// A working card: this turn so far, then the call it is on now and for
/// how long, then the chips for its phase, review round and diff.
fn doing_inside(tab: &TabModel, look: &CardLook, r: &Rules, theme: &Theme) -> Inside {
    let inner = look.w.saturating_sub(4);
    let mut lines = Vec::new();
    if tab.summary.is_some() || tab.turn.is_some() {
        lines.push(card_label("This turn", Tone::Dim, theme));
    }
    match (&tab.summary, tab.turn.as_ref()) {
        (Some(summary), _) => lines.extend(wrapped(summary, inner, 3, Tone::Text.style(theme))),
        (None, Some(turn)) => {
            let prompt = unlinked(&text::plain(&turn.prompt));
            lines.extend(wrapped(
                &format!("› {prompt}"),
                inner,
                2,
                Tone::Dim.style(theme),
            ));
            if let Some(words) = turn_words(tab) {
                lines.extend(wrapped(&words, inner, 2, Tone::Text.style(theme)));
            }
        }
        (None, None) => {}
    }
    if !lines.is_empty() {
        lines.push(Vec::new());
    }
    lines.push(card_label("Now", Tone::Working, theme));
    let (tool, arg, since) = match &tab.doing {
        Some(d) => (
            d.tool.clone(),
            d.arg.split('\n').next().unwrap_or("").to_string(),
            Some(d.since),
        ),
        None => (
            // Between calls it is thinking; compacting says so itself.
            tab.activity
                .clone()
                .filter(|a| a == "compacting context")
                .unwrap_or_else(|| "thinking".into()),
            String::new(),
            None,
        ),
    };
    let how_long = since.map_or_else(String::new, |s| text::ago(Some(s), r.now_secs()));
    let at = kit::columns(
        &[
            Col::Fixed {
                min: 1,
                natural: 1,
                cap: 1,
            },
            Col::Fixed {
                min: 4,
                natural: width(&tool),
                cap: 16,
            },
            Col::Stretch {
                weight: 1,
                min: 0,
                max: None,
            },
            Col::Right {
                natural: width(&how_long),
                cap: 6,
            },
        ],
        inner,
        1,
        &[],
    );
    let mut now = Vec::new();
    let parts = [
        vec![kit::mark(Mark::Working, 0, theme)],
        vec![Span::styled(tool, Tone::Text.bold(theme))],
        vec![Span::styled(arg, Tone::Dim.style(theme))],
        vec![Span::styled(how_long, Tone::Dim.style(theme))],
    ];
    for (spans, place) in parts.into_iter().zip(at) {
        if let Some(place) = place {
            kit::place(&mut now, spans, place);
        }
    }
    lines.push(now);
    if look.chips {
        let mut chips: Vec<Span<'static>> = Vec::new();
        if let Some(phase) = tab.phase.as_deref().filter(|p| !p.is_empty()) {
            chips.extend(kit::chip(phase, Tone::Working, false, theme));
        }
        if tab.round > 0 {
            chips.extend(kit::chip(
                &format!("round {}", tab.round),
                Tone::Landing,
                false,
                theme,
            ));
        }
        if let Some((add, del)) = tab.size {
            chips.extend(kit::chip(
                &format!("+{add} −{del}"),
                Tone::Dim,
                false,
                theme,
            ));
        }
        if !chips.is_empty() {
            lines.push(Vec::new());
            lines.push(chips);
        }
    }
    Inside { lines }
}

/// A card neither waiting nor working: what its last turn did, and its
/// state in words.
fn last_inside(tab: &TabModel, look: &CardLook, r: &Rules, theme: &Theme) -> Inside {
    let inner = look.w.saturating_sub(4);
    let mut lines = vec![card_label("Last turn", Tone::Dim, theme)];
    match turn_words(tab) {
        Some(words) => lines.extend(wrapped(&words, inner, 5, Tone::Text.style(theme))),
        None => lines.push(vec![Span::styled(
            tab.path
                .as_ref()
                .map(crate::paths::tilde)
                .unwrap_or_default(),
            Tone::Dim.style(theme),
        )]),
    }
    lines.push(Vec::new());
    lines.extend(wrapped(
        &model::state_words(tab, r),
        inner,
        2,
        Tone::Dim.style(theme),
    ));
    Inside { lines }
}

/// A tab's card with UX 2.0, drawn open: its mark, number and name on the
/// top rule, its state and how long at the rule's right end, the bar down
/// its left in its group's tone (the accent when chosen), what it is about inside by what it is
/// doing (or, with `s` or no Claude in it, its screen's last lines), and
/// its branch and checks at its foot.
fn card_2(
    tab: &TabModel,
    tone: Tone,
    look: &CardLook,
    frame: usize,
    m: &Mission,
) -> Vec<Vec<Span<'static>>> {
    let snap = &m.snap;
    let (r, theme) = (&snap.rules, &snap.theme);
    let inner = look.w.saturating_sub(4);
    // Its rows under the top rule, less the foot.
    let rows = look.h.saturating_sub(2);
    let mark = Mark::of_badge(model::badge_state(tab, r));
    let hover =
        (look.hovered && !look.chosen).then(|| tab.color.map(Color::from).unwrap_or(theme.bright));
    let words = state_for(tab, r);
    let right = vec![Span::styled(words, mark.look().1.style(theme))];
    let title = vec![
        kit::mark(mark, frame, theme),
        Span::raw(" "),
        Span::styled(tab.index.to_string(), Tone::Dim.style(theme)),
        Span::raw(" "),
        Span::styled(text::cut(&tab.name, CARD_NAME), bold(theme.bright)),
    ];
    let inside = if look.shows_screen {
        let lines: &[Vec<Span<'static>>] = look.screen.map_or(&[], |s| &s.lines[..]);
        let shown = &lines[lines.len().saturating_sub(rows)..];
        let mut out = vec![Vec::new(); rows - shown.len()];
        out.extend(shown.iter().cloned());
        Inside { lines: out }
    } else {
        match tab.state {
            TabState::Waiting => asks_inside(tab, look, rows, theme),
            TabState::Working => doing_inside(tab, look, r, theme),
            // Between turns with only its background running: its last
            // turn, and what runs behind it in the state's words.
            _ => last_inside(tab, look, r, theme),
        }
    };
    let mut lines: Vec<Vec<Span<'static>>> = inside.lines.into_iter().take(rows).collect();
    lines.resize(rows, Vec::new());
    let branch = look.screen.and_then(|s| s.branch.as_deref());
    lines.push(card_foot(tab, branch, inner, snap.caps, theme));
    kit::open_card(
        look.w,
        look.h,
        title,
        right,
        &lines,
        tone,
        look.chosen,
        hover,
        theme,
    )
}

/// A shelf line: its mark, its number in its color, its name, how long (or
/// its place in line to land) at a fixed column, and what its last turn
/// did, which is the first to go on a narrow screen.
fn shelf_line(
    tab: &TabModel,
    widths: (usize, usize),
    cols: usize,
    chosen: bool,
    m: &Mission,
) -> Line {
    let (r, theme) = (&m.snap.rules, &m.snap.theme);
    let (name_w, when_w) = widths;
    let when = if tab.landing.is_some() {
        model::landing_words(tab)
    } else {
        text::ago(tab.since, r.now_secs())
    };
    // A landing tab's state is its place in line, said already.
    let what = turn_words(tab)
        .or_else(|| tab.landing.is_none().then(|| model::state_words(tab, r)))
        .unwrap_or_default();
    let number = tab.index.to_string();
    let at = kit::columns(
        &[
            Col::Fixed {
                min: 1,
                natural: 1,
                cap: 1,
            },
            Col::Fixed {
                min: 1,
                natural: width(&number).max(2),
                cap: 4,
            },
            Col::Fixed {
                min: 6,
                natural: name_w,
                cap: SHELF_NAME,
            },
            Col::Fixed {
                min: 0,
                natural: when_w,
                cap: 22,
            },
            Col::Stretch {
                weight: 1,
                min: 16,
                max: None,
            },
        ],
        cols.saturating_sub(2),
        2,
        &[4],
    );
    let color = tab.color.map(Color::from).unwrap_or(theme.bright);
    let name_style = if chosen {
        bold(theme.bright)
    } else {
        Tone::Text.style(theme)
    };
    let right_aligned =
        |text: String, w: usize| format!("{}{text}", " ".repeat(w.saturating_sub(width(&text))));
    let parts = [
        vec![kit::mark(
            Mark::of_badge(model::badge_state(tab, r)),
            m.snap.frame,
            theme,
        )],
        vec![Span::styled(number, fg(color))],
        vec![Span::styled(tab.name.clone(), name_style)],
        vec![Span::styled(
            right_aligned(when, at[3].map_or(0, |(_, w)| w)),
            Tone::Dim.style(theme),
        )],
        vec![Span::styled(what, Tone::Dim.style(theme))],
    ];
    let mut spans = vec![Span::raw(" ")];
    for (part, place) in parts.into_iter().zip(at) {
        if let Some((x, w)) = place {
            kit::place(&mut spans, part, (x + 1, w));
        }
    }
    Line {
        spans: fit(spans, cols, Style::new()),
        fill: chosen.then(|| kit::Surface::Chosen.color(theme)),
    }
}

/// UX 2.0's Tabs (board 3, laid out as board 45 says): each group of cards
/// under its label with its count, as many cards side by side as fit at
/// their least width sharing the width equally; then the shelf, Landing and
/// Idle a line a tab, folded to their labels when only one card fits;
/// scrolled to keep the chosen one in view.
fn cards_ux2(
    m: &Mission,
    cols: usize,
    body: usize,
    top_row: usize,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
) {
    let snap = &m.snap;
    let (r, theme) = (&snap.rules, &snap.theme);
    let groups = tab_groups(&snap.tabs, r);
    if groups.is_empty() {
        lines.push(
            Row::default()
                .put("  No tabs.", fg(theme.dim))
                .take()
                .line(cols, None),
        );
        return;
    }
    let n = across(cols);
    let xs = card_columns(cols, n);
    let folded = shelf_folded(cols, m.shelf_open);
    let card_h = if body > CARD_H + 1 {
        CARD_H
    } else {
        body.saturating_sub(1).max(8)
    };
    let shelf: Vec<&TabModel> = groups
        .iter()
        .filter(|(s, _)| on_shelf(*s))
        .flat_map(|(_, at)| at.iter().map(|&i| &snap.tabs[i]))
        .collect();
    let name_w = kit::natural(shelf.iter().map(|t| t.name.as_str()));
    let when_w = shelf
        .iter()
        .map(|t| {
            width(&if t.landing.is_some() {
                model::landing_words(t)
            } else {
                text::ago(t.since, r.now_secs())
            })
        })
        .max()
        .unwrap_or(0);
    let mut drawn: Vec<Line> = Vec::new();
    // Each card's or line's place: its first and last line, its columns,
    // its place in the order.
    let mut at: Vec<(usize, usize, usize, usize, usize)> = Vec::new();
    let mut place = 0usize;
    let (mut cursor_top, mut cursor_bottom) = (0, 0);
    for (stage, tabs_in) in &groups {
        if !drawn.is_empty() {
            drawn.push(Line {
                spans: vec![Span::raw(" ".repeat(cols))],
                fill: None,
            });
        }
        let tone = Mark::of_badge(stage.color()).look().1;
        let label_at = drawn.len();
        let mut label = kit::label(stage.title(), tone, Some(tabs_in.len()), cols, theme);
        if on_shelf(*stage) && folded {
            let chosen = m.cursor == place as isize;
            if chosen {
                label = label
                    .into_iter()
                    .map(|s| {
                        let style = s.style.add_modifier(Modifier::REVERSED);
                        Span::styled(s.content, style)
                    })
                    .collect();
                cursor_top = label_at;
                cursor_bottom = label_at + 1;
            }
            drawn.push(Line {
                spans: label,
                fill: None,
            });
            at.push((label_at, label_at, 0, cols, place));
            place += 1;
            continue;
        }
        drawn.push(Line {
            spans: label,
            fill: None,
        });
        if on_shelf(*stage) {
            for &i in tabs_in {
                let tab = &snap.tabs[i];
                let chosen = m.cursor == place as isize;
                let y = drawn.len();
                if chosen {
                    // Its label kept in sight with it while both fit.
                    cursor_top = if y + 1 - label_at <= body {
                        label_at
                    } else {
                        y
                    };
                    cursor_bottom = y + 1;
                }
                drawn.push(shelf_line(tab, (name_w, when_w), cols, chosen, m));
                at.push((y, y, 0, cols, place));
                place += 1;
            }
            continue;
        }
        for row in tabs_in.chunks(n) {
            let top = drawn.len();
            let mut row_lines: Vec<Vec<Span<'static>>> = vec![Vec::new(); card_h];
            for (c, &i) in row.iter().enumerate() {
                let tab = &snap.tabs[i];
                let (x, w) = xs[c];
                let chosen = m.cursor == place as isize;
                let look = CardLook {
                    w,
                    h: card_h,
                    chosen,
                    hovered: m.hover == Some(Spot::Index(place as isize)),
                    chips: n >= 3,
                    screen: snap.screens.get(&tab.id),
                    // A tab with no Claude is about what its screen shows.
                    shows_screen: tab.claude.is_none()
                        || (m.card_screen == Some((tab.id, m.cursor)) && chosen),
                };
                let card = card_2(tab, tone, &look, snap.frame, m);
                for (line, spans) in row_lines.iter_mut().zip(card) {
                    kit::place(line, spans, (x, w));
                }
                at.push((top, top + card_h - 1, x, x + w, place));
                if chosen {
                    cursor_top = if row.as_ptr() == tabs_in.as_ptr() {
                        label_at
                    } else {
                        top
                    };
                    cursor_bottom = top + card_h;
                }
                place += 1;
            }
            for spans in row_lines {
                drawn.push(Line {
                    spans: fit(spans, cols, Style::new()),
                    fill: None,
                });
            }
        }
    }
    let start = cursor_top.min(cursor_bottom.saturating_sub(body));
    let end = start + body;
    lines.extend(drawn.iter().skip(start).take(body).cloned());
    let shown = |y: usize| y >= start && y < end;
    for (top, bottom, x0, x1, index) in at {
        for y in (top..=bottom).filter(|y| shown(*y)) {
            hits.push(HitBox {
                y: (y - start + top_row) as u16,
                x0: x0 as u16,
                x1: x1.saturating_sub(1) as u16,
                spot: Spot::Index(index as isize),
            });
        }
    }
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
    if r.ux2 {
        cards_ux2(m, cols, body, top_row, lines, hits);
        return;
    }
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
                        let tab = &snap.tabs[g.order[i]];
                        let chosen = i as isize == m.cursor;
                        let hovered = m.hover == Some(Spot::Index(i as isize));
                        card(tab, cw, h, chosen, hovered, snap.frame, r, theme)
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

/// A UX 2.0 board card's five lines and the blank one under it.
const BOARD_CARD_TALL: usize = 6;
/// The PR and ticket boards' week row: its rule, label and blank line, and
/// a card.
const WEEK_ROWS: usize = 3 + BOARD_CARD_TALL - 1;

/// Whether the PR and ticket boards (UX 2.0's) show their last column, the
/// week's merged or closed row, under a body `body` rows tall: only with
/// room for a card above it.
pub fn week_row_fits(body: usize) -> bool {
    cards_fit(body.saturating_sub(WEEK_ROWS))
}

/// Whether the PR and ticket boards (UX 2.0's) have room for a row of cards
/// under their column headings in a body `body` rows tall.
pub fn cards_fit(body: usize) -> bool {
    body >= 2 + BOARD_CARD_TALL
}

/// UX 2.0's PR and ticket boards, drawn with the kit's board (board 0c) as
/// boards 10 and 19 draw them: a column per place an item stands, its head
/// in the state's tone with its count, each item a card (its number or ID
/// in the accent on the top edge, its checks' mark or its priority at the
/// edge's right end, its title on up to two lines, then a dim line about
/// it); the chosen card's edge in the accent. The views' last group (the
/// PRs merged and the tickets canceled in the last week) is a row of the
/// same cards, faint, under the columns, each saying when. The chosen
/// item's description, through Markdown, sits beside them on a screen
/// `SIDE_AT` wide and under them on a narrower one with the room, and the
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
    let board_w = cols - side_w;
    let face = |item: &Item| {
        if tickets {
            ticket_face(item, m, theme)
        } else {
            pr_face(item, m, theme)
        }
    };
    let card_of = |index: usize, item: &Item, past: bool| {
        let chosen = index as isize == m.cursor;
        let hovered = !chosen && m.hover == Some(Spot::Index(index as isize));
        let mut face = face(item);
        if past {
            face.meta = vec![Span::styled(
                week_words(item, tickets, m.snap.now_secs),
                fg(theme.faint),
            )];
        }
        face_card(face, chosen, hovered, past, theme)
    };
    // The week's row when it has anything and the columns keep room for a
    // card.
    let week_h = if !week_items.is_empty() && week_row_fits(body) {
        WEEK_ROWS
    } else {
        0
    };
    let held: Vec<Vec<(usize, &Item)>> = columns.iter().map(|c| in_group(c)).collect();
    let board_columns: Vec<kit::BoardColumn> = columns
        .iter()
        .zip(&held)
        .map(|(column, items)| kit::BoardColumn {
            name: column.to_string(),
            tone: column_tone(column),
            cards: items
                .iter()
                .map(|(i, item)| card_of(*i, item, false))
                .collect(),
        })
        .collect();
    let drawn = kit::board(&board_columns, board_w, body - week_h, more_lit(m), theme);
    board_hits(&drawn, top_row, hits, |c, i| held[c][i].0);
    let mut rows = drawn.lines;
    let under = side_w == 0 && chosen_item.is_some();
    if !under {
        rows.resize(body - week_h, Vec::new());
    }
    if week_h > 0 {
        let title = if tickets {
            "Closed this week"
        } else {
            "Merged this week"
        };
        rows.push(vec![kit::head_rule(board_w, theme)]);
        rows.push(kit::label(
            title,
            Tone::Dim,
            Some(week_items.len()),
            board_w.saturating_sub(1),
            theme,
        ));
        rows.push(Vec::new());
        // As many cards as fit, as wide as the columns', from the one that
        // keeps the chosen in sight.
        let k = ((board_w + 1) / (kit::BOARD_COLUMN_MIN + 1)).clamp(1, columns.len().max(1));
        let w = (board_w.saturating_sub(k - 1) / k).max(1);
        let across = ((board_w + 1) / (w + 1)).max(1);
        let chosen_at = week_items.iter().position(|(i, _)| *i as isize == m.cursor);
        let first = chosen_at.map_or(0, |c| (c + 1).saturating_sub(across));
        let shown: Vec<&(usize, &Item)> = week_items.iter().skip(first).take(across).collect();
        let cards: Vec<Vec<Vec<Span<'static>>>> = shown
            .iter()
            .map(|(index, item)| kit::board_card(&card_of(*index, item, true), w, theme))
            .collect();
        let tall = cards.iter().map(Vec::len).max().unwrap_or(0);
        let y = top_row + rows.len();
        for (c, ((index, _), card)) in shown.iter().zip(&cards).enumerate() {
            for dy in 0..card.len() {
                hits.push(HitBox {
                    y: (y + dy) as u16,
                    x0: (c * (w + 1)) as u16,
                    x1: (c * (w + 1) + w).saturating_sub(1) as u16,
                    spot: Spot::Index(*index as isize),
                });
            }
        }
        for line in 0..tall {
            let mut spans = Vec::new();
            for card in &cards {
                spans.extend(fit(
                    card.get(line).cloned().unwrap_or_default(),
                    w,
                    Style::new(),
                ));
                spans.push(Span::raw(" "));
            }
            rows.push(fit(spans, board_w, Style::new()));
        }
    }
    let images = ready_diagrams(m);
    let about_w = if under { board_w } else { side_w };
    let about = chosen_item.filter(|_| about_w > 0).map(|item| {
        if tickets {
            ticket_about(item, about_w, theme, images.as_ref())
        } else {
            pr_about(item, about_w, theme, images.as_ref())
        }
    });
    let pictures = if under {
        pane_under(about, &mut rows, body, board_w, m, top_row, hits, theme)
    } else {
        pane_beside(
            about, &mut rows, body, board_w, side_w, m, top_row, hits, theme,
        )
    };
    lines.extend(rows.into_iter().map(|spans| Line { spans, fill: None }));
    pictures
}

/// How many rows the pane under a board needs at the least to be shown.
const UNDER_MIN: usize = 6;

/// The chosen item's pane beside a board's `rows`, `side_w` wide after a
/// rule, on each of the `body` rows: the part of it its height has room
/// for, from where it was scrolled to; the diagrams it places come back.
#[allow(clippy::too_many_arguments)]
fn pane_beside(
    about: Option<About>,
    rows: &mut Vec<Vec<Span<'static>>>,
    body: usize,
    board_w: usize,
    side_w: usize,
    m: &Mission,
    top_row: usize,
    hits: &mut Vec<HitBox>,
    theme: &Theme,
) -> Vec<Picture> {
    rows.resize(body, Vec::new());
    let Some(about) = about.filter(|_| side_w > 0) else {
        return Vec::new();
    };
    side_hits(
        about.last_top(body),
        board_w + 2,
        side_w - 2,
        top_row,
        body,
        hits,
    );
    let top = m.side_top().min(about.last_top(body));
    let about = about.window(top, body);
    for (i, row) in rows.iter_mut().enumerate() {
        let mut spans = fit(std::mem::take(row), board_w, Style::new());
        spans.push(Span::styled("│ ", fg(theme.faint)));
        spans.extend(fit(
            about.lines.get(i).cloned().unwrap_or_default(),
            side_w - 2,
            Style::new(),
        ));
        *row = spans;
    }
    about
        .pictures
        .iter()
        .map(|p| Picture {
            x: p.x + (board_w + 2) as u16,
            y: p.y + top_row as u16,
            ..p.clone()
        })
        .collect()
}

/// The chosen item's pane under a board's `rows` (board 45, below
/// `SIDE_AT`): a rule, then as much of it as the rest of the `body` rows
/// hold, when that is `UNDER_MIN` or more; the diagrams it places come
/// back.
#[allow(clippy::too_many_arguments)]
fn pane_under(
    about: Option<About>,
    rows: &mut Vec<Vec<Span<'static>>>,
    body: usize,
    cols: usize,
    m: &Mission,
    top_row: usize,
    hits: &mut Vec<HitBox>,
    theme: &Theme,
) -> Vec<Picture> {
    rows.truncate(body);
    let room = body.saturating_sub(rows.len() + 1);
    let Some(about) = about.filter(|_| room >= UNDER_MIN) else {
        rows.resize(body, Vec::new());
        return Vec::new();
    };
    rows.push(vec![kit::rule(cols, theme)]);
    let at = top_row + rows.len();
    side_hits(about.last_top(room), 0, cols, at, room, hits);
    let top = m.side_top().min(about.last_top(room));
    let about = about.window(top, room);
    for i in 0..room {
        rows.push(fit(
            about.lines.get(i).cloned().unwrap_or_default(),
            cols,
            Style::new(),
        ));
    }
    about
        .pictures
        .iter()
        .map(|p| Picture {
            y: p.y + at as u16,
            ..p.clone()
        })
        .collect()
}

/// Whether a board's "‹ more" and "more ›" markers are lit: under the
/// mouse or Tab's focus.
fn more_lit(m: &Mission) -> [bool; 2] {
    [KeyCode::Left, KeyCode::Right].map(|code| {
        let k = Key::new(code, Mods::NONE);
        m.hover == Some(Spot::Key(k)) || m.focus == Some(Focus::Key(k))
    })
}

/// A drawn board's hit boxes from `top_row`: each card it shows, by the
/// item `index` gives for its column and place, and its "more" markers,
/// which press ← and →, moving a column.
fn board_hits(
    drawn: &kit::Board,
    top_row: usize,
    hits: &mut Vec<HitBox>,
    index: impl Fn(usize, usize) -> usize,
) {
    for p in &drawn.placed {
        for dy in 0..p.h {
            hits.push(HitBox {
                y: (top_row + p.y + dy) as u16,
                x0: p.x as u16,
                x1: (p.x + p.w).saturating_sub(1) as u16,
                spot: Spot::Index(index(p.column, p.card) as isize),
            });
        }
    }
    for (more, code) in [
        (drawn.more_left, KeyCode::Left),
        (drawn.more_right, KeyCode::Right),
    ] {
        if let Some((x, w)) = more {
            hits.push(HitBox {
                y: top_row as u16,
                x0: x as u16,
                x1: (x + w).saturating_sub(1) as u16,
                spot: Spot::Key(Key::new(code, Mods::NONE)),
            });
        }
    }
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

/// A board's card of an item's face: its label in the accent (faint for a
/// done ticket or the week's row), its title, its meta as its fact line.
fn face_card(face: Face, chosen: bool, hovered: bool, past: bool, theme: &Theme) -> kit::BoardCard {
    let name = if past || face.done {
        fg(theme.faint)
    } else {
        bold(ACCENT)
    };
    kit::BoardCard {
        mark: None,
        name: Span::styled(face.label, name),
        state: face.right,
        title: face.title,
        title_look: if past {
            kit::Title::Past
        } else if face.done {
            kit::Title::Struck
        } else {
            kit::Title::Live
        },
        facts: [face.meta]
            .into_iter()
            .filter(|line| spans_width(line) > 0)
            .collect(),
        meter: None,
        chosen,
        hovered,
    }
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
pub(super) struct About {
    pub(super) lines: Vec<Vec<Span<'static>>>,
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
/// ticket's, or the plan of the Autopilot board's chosen item, whose pane
/// sits beside the board or under it.
pub fn description_beside(m: &Mission) -> Option<(&str, &'static str)> {
    let view = m.current()?;
    let (field, place, board_at) = match view.id {
        ViewId::Prs => ("body", crate::views::pr::PLACE, 0),
        ViewId::Tickets => ("description", crate::views::ticket::PLACE, 0),
        ViewId::Autopilot => ("plan", PLAN_PLACE, PILOT_BOARD_AT),
        _ => return None,
    };
    let is_a_board = view.columns().is_some() || view.id == ViewId::Autopilot;
    let board_with_the_pane = m.snap.rules.ux2
        && is_a_board
        && view.data.setup.is_none()
        && view.loaded
        && m.laid().0 as usize >= board_at;
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
pub(super) fn markdown_lines(
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

/// When a notification came, for its row's end: the clock time alone
/// under a day's heading (the history), else the date and the time; a
/// meeting still to come says when it starts.
fn notice_when(item: &Item, now: &jiff::Zoned) -> String {
    let Some(ms) = item.time else {
        return String::new();
    };
    let to_come = ms.div_euclid(1000) > now.timestamp().as_second();
    if super::history::notice_of(item).is_some() && !to_come {
        crate::ui::when::clock(ms, now)
    } else {
        crate::ui::when::notice(ms, now)
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
    // `i`'s output over the list, whether the board or the list is drawn.
    if snap.rules.ux2
        && view.id == ViewId::Landing
        && m.landing_output
        && cols < LANDING_OUTPUT_AT
        && let Some(landing) = m
            .chosen_item()
            .and_then(|i| i.data.get("landing"))
            .filter(|l| l.is_object())
    {
        let budget = rows.saturating_sub(1 + lines.len());
        lines.extend(landing_output_over(landing, cols, budget, m));
        return;
    }
    if snap.rules.ux2
        && view.id == ViewId::Landing
        && stage_board_fits(view, cols, snap.now_secs * 1000)
    {
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
    // The Notifications view says when each came, its column as wide as
    // the longest it shows; every other view, how long ago, in five.
    let now = jiff::Timestamp::from_second(snap.now_secs)
        .unwrap_or_default()
        .to_zoned(snap.tz.clone());
    let when_w = if view.id == ViewId::Notifications {
        shown
            .iter()
            .map(|(_, i)| width(&notice_when(i, &now)))
            .max()
            .unwrap_or(0)
            .max(5)
    } else {
        5
    };
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
        let at = if view.id == ViewId::Notifications {
            notice_when(item, &now)
        } else {
            when(item, snap.now_secs)
        };
        let mut spans = row.fitted(cols.saturating_sub(when_w + 1), Style::new());
        spans.push(Span::styled(format!("{at:>when_w$} "), fg(theme.faint)));
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
            vec![Span::styled(format!("   {}", item.title), fg(theme.dim))]
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

/// UX 2.0's Landing view, as board 5 draws it and board 45 lays it out:
/// the stages a worktree goes through landing straight on its branch, and
/// a merge queue's PR's.
const STAGES: [&str; 6] = ["tests", "review", "leak check", "lock", "push", "applied"];
const QUEUE_STAGES: [&str; 5] = ["CI", "approved", "merge queue", "merge", "deploy test"];
/// A cell's widest, its two ends in it: 13 columns of words ("3rd in queue").
const CELL_MAX: usize = 15;
/// From this wide every cell is `CELL_MAX`; narrower, each column is as
/// wide as its widest words.
const CELLS_FULL_AT: usize = 160;
/// The worktree column's width on the board, what it keeps before a
/// waiting cell gives up its words, and the least it narrows to.
const WORKTREE_W: usize = 37;
const WORKTREE_MIN: usize = 20;
/// A waiting cell with no room for its words: the mark alone.
const WAITING_MARK: &str = "◌";
/// Under this wide the chosen worktree's last output is behind `i`.
pub(crate) const LANDING_OUTPUT_AT: usize = 140;
/// The detail's labels' column.
const DETAIL_LABEL: usize = 12;

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

/// A stage's cell: how far it got, its words, and the dim note under it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct StageCell {
    stage: Stage,
    label: String,
    note: String,
}

fn stage_cell(stage: Stage, label: impl Into<String>) -> StageCell {
    StageCell {
        stage,
        label: label.into(),
        note: String::new(),
    }
}

/// A landing's six stages from what the view holds of it (`data.landing`):
/// a worktree is in the landing queue only once its tests, review and leak
/// check are done (queue-and-clean takes the lock after them), so those are
/// done, with the numbers its record (`land-lock record`) holds: the tests
/// passed ("✓ 412", or "✕ 2 of 401" with the failing file under it), the
/// review's clean rounds ("✓ 2 clean", or "round 1" with its reviewer and
/// how long ago under it while it goes on) and the leak check's ("✓ none",
/// "✕ found"); without a record, "passed", "clean" and "none". A waiter is
/// queued for the lock at its place; the one holding it is at the step its
/// phase names, a failure included (`tests failed …`, `review blocked …`,
/// the words after it shown in the cell), the check after a clean rebase
/// (`checking …`, `check failed`) in the tests' cell.
fn landing_stages(landing: &serde_json::Value, now_ms: i64) -> [StageCell; 6] {
    let done = |s: &str| stage_cell(Stage::Done, format!("✓ {s}"));
    let running = |s: &str| stage_cell(Stage::Running, s);
    let waiting = || stage_cell(Stage::Waiting, "waiting");
    let record = &landing["record"];
    let number = |v: &serde_json::Value, k: &str| v.get(k).and_then(serde_json::Value::as_u64);
    let words = |v: &serde_json::Value, k: &str| {
        printable(v.get(k).and_then(serde_json::Value::as_str).unwrap_or(""))
    };
    let failed_tests = |t: &serde_json::Value| {
        number(t, "failed").unwrap_or(0) > 0
            || t.get("ok").and_then(serde_json::Value::as_bool) == Some(false)
    };
    let tests = match &record["tests"] {
        t if t.is_object() && failed_tests(t) && number(t, "total").is_none() => StageCell {
            stage: Stage::Failed,
            label: "✕ failed".into(),
            note: words(t, "file"),
        },
        t if t.is_object() && failed_tests(t) => StageCell {
            stage: Stage::Failed,
            label: format!(
                "✕ {} of {}",
                number(t, "failed").unwrap_or(0),
                number(t, "total").unwrap_or(0)
            ),
            note: words(t, "file"),
        },
        t if t.is_object() => match number(t, "passed").or(number(t, "total")) {
            Some(n) => done(&n.to_string()),
            None => done("passed"),
        },
        _ => done("passed"),
    };
    let review = match &record["review"] {
        r if r.is_object() && number(r, "clean_rounds").unwrap_or(0) >= 2 => {
            done(&format!("{} clean", number(r, "clean_rounds").unwrap_or(2)))
        }
        r if r.is_object() => {
            let ago = text::ago(
                r.get("at")
                    .and_then(serde_json::Value::as_i64)
                    .map(|ms| ms / 1000),
                now_ms / 1000,
            );
            StageCell {
                stage: Stage::Running,
                label: format!("round {}", number(r, "round").unwrap_or(1)),
                note: [words(r, "reviewer"), ago]
                    .into_iter()
                    .filter(|w| !w.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · "),
            }
        }
        _ => done("clean"),
    };
    let leak = match number(&record["leak"], "found") {
        Some(n) if n > 0 => stage_cell(Stage::Failed, "✕ found"),
        _ => done("none"),
    };
    let mut out = [tests, review, leak, waiting(), waiting(), waiting()];
    if landing.get("holding").and_then(serde_json::Value::as_bool) != Some(true) {
        let place = landing
            .get("place")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1);
        out[3] = stage_cell(Stage::Queued, ordinal(place));
        return out;
    }
    out[3] = done("held");
    let phase = landing
        .get("phase")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let failed = |rest: &str, or: &str, note: String| {
        let rest = rest.trim_start_matches([':', ' ']).trim();
        StageCell {
            stage: Stage::Failed,
            label: format!("✕ {}", if rest.is_empty() { or } else { rest }),
            note,
        }
    };
    let tests_note = out[0].note.clone();
    if let Some(rest) = phase.strip_prefix("tests failed") {
        out[0] = failed(rest, "failed", tests_note);
    } else if phase.starts_with("check failed") {
        out[0] = failed("", "check failed", tests_note);
    } else if let Some(rest) = phase.strip_prefix("review blocked") {
        out[1] = failed(rest, "blocked", out[1].note.clone());
    } else if phase.contains("testing") {
        out[0] = running("testing…");
    } else if phase.starts_with("checking") {
        out[0] = running("checking…");
    } else if phase.starts_with("reviewing") {
        // A round under way keeps its number; with none recorded, or the
        // last loop's clean rounds, it says only that it reviews.
        out[1] = if out[1].stage == Stage::Running {
            out[1].clone()
        } else {
            running("reviewing…")
        };
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
fn queue_stages(queue: &serde_json::Value) -> [StageCell; 5] {
    let state = queue
        .get("state")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let position = queue
        .get("position")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let waiting = || stage_cell(Stage::Waiting, "waiting");
    let mut out = [
        stage_cell(Stage::Done, "✓ CI"),
        stage_cell(Stage::Done, "✓ approved"),
        stage_cell(
            Stage::Queued,
            format!("{} in queue", ordinal(position.max(1))),
        ),
        waiting(),
        waiting(),
    ];
    match state {
        "AWAITING_CHECKS" => out[2] = stage_cell(Stage::Running, "checks…"),
        "UNMERGEABLE" => out[2] = stage_cell(Stage::Failed, "✕ can't merge"),
        "MERGEABLE" => out[2] = stage_cell(Stage::Done, "✓ ready"),
        "LOCKED" => {
            out[2] = stage_cell(Stage::Done, "✓ ready");
            out[3] = stage_cell(Stage::Running, "merging…");
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

/// An item's stage cells, none for a row that isn't a landing or a PR in
/// a merge queue.
fn stages_of(item: &Item, now_ms: i64) -> Vec<StageCell> {
    if let Some(landing) = item.data.get("landing").filter(|l| l.is_object()) {
        landing_stages(landing, now_ms).to_vec()
    } else if let Some(queue) = item.data.get("queue").filter(|q| q.is_object()) {
        queue_stages(queue).to_vec()
    } else {
        Vec::new()
    }
}

/// The stage names a section's heading puts over its cells.
fn heads_of(item: &Item) -> &'static [&'static str] {
    // A direct landing's heading names its target after `:direct:`, which
    // may be a branch called queue, so it is told apart first.
    if item.id.contains(":direct") {
        &STAGES
    } else if item.id.ends_with(":queue") {
        &QUEUE_STAGES
    } else {
        &[]
    }
}

/// A section of the Landing view laid out across `cols`: the worktree
/// column and each cell's, from x 0 of the line, and whether a waiting
/// cell shows its mark alone.
#[derive(Clone, Debug, PartialEq, Eq)]
struct StageLayout {
    worktree: (usize, usize),
    cells: Vec<(usize, usize)>,
    marks: bool,
}

/// How a section whose cells are `rows` lays out across `cols`, as board
/// 45 says: the cells at the right edge one column apart, each `CELL_MAX`
/// from 160 columns, the worktree column stretching over what is left.
/// Narrower, the cells narrow to their widest words; then, when the
/// worktree column would be under its width on the board, a waiting cell
/// shows ◌ alone, so a column where every row waits is three wide; then
/// the worktree column narrows to 20. `None` when even that doesn't fit.
fn stage_layout(cols: usize, heads: &[&str], rows: &[Vec<StageCell>]) -> Option<StageLayout> {
    // A row starts with the pointer and a space, and leaves the last
    // column empty.
    let room = cols.saturating_sub(3);
    let widest = |c: usize, marks: bool| {
        let words = rows
            .iter()
            .filter_map(|r| r.get(c))
            .map(|cell| {
                let label = if marks && cell.stage == Stage::Waiting {
                    WAITING_MARK
                } else {
                    cell.label.as_str()
                };
                width(label).max(width(&cell.note).saturating_sub(2))
            })
            .max()
            .unwrap_or(1);
        let all_wait = rows
            .iter()
            .all(|r| r.get(c).is_none_or(|cell| cell.stage == Stage::Waiting));
        let head = if marks && all_wait {
            0
        } else {
            heads.get(c).map_or(0, |h| width(h))
        };
        (words.max(head) + 2).min(CELL_MAX)
    };
    let widths = |full: bool, marks: bool| -> Vec<usize> {
        (0..heads.len())
            .map(|c| if full { CELL_MAX } else { widest(c, marks) })
            .collect()
    };
    let cells_w = |w: &[usize]| w.iter().sum::<usize>() + w.len();
    let tries = [
        (cols >= CELLS_FULL_AT, false, WORKTREE_W),
        (false, false, WORKTREE_W),
        (false, true, WORKTREE_MIN),
    ];
    let (widths, marks) = tries
        .into_iter()
        .map(|(full, marks, least)| (widths(full, marks), marks, least))
        .find(|(w, _, least)| room >= cells_w(w) + least)
        .map(|(w, marks, _)| (w, marks))?;
    let mut wanted = vec![Col::Stretch {
        weight: 1,
        min: WORKTREE_MIN,
        max: None,
    }];
    wanted.extend(widths.iter().map(|&w| Col::Right { natural: w, cap: w }));
    let at = kit::columns(&wanted, room, 1, &[]);
    let place = |c: Option<(usize, usize)>| c.map_or((2, 0), |(x, w)| (x + 2, w));
    Some(StageLayout {
        worktree: place(at[0]),
        cells: at[1..].iter().map(|&c| place(c)).collect(),
        marks,
    })
}

/// Whether every section of the Landing view lays out across `cols`.
fn stage_board_fits(view: &View, cols: usize, now_ms: i64) -> bool {
    stage_sections(view, now_ms)
        .iter()
        .all(|(heads, rows)| heads.is_empty() || stage_layout(cols, heads, rows).is_some())
}

/// Each section's stage names and its rows' cells, a section to each
/// heading (and one before the first, with no names).
fn stage_sections(view: &View, now_ms: i64) -> Vec<(&'static [&'static str], Vec<Vec<StageCell>>)> {
    let mut out: Vec<(&'static [&'static str], Vec<Vec<StageCell>>)> = vec![(&[], Vec::new())];
    for item in &view.data.items {
        if views::is_header(item) {
            out.push((heads_of(item), Vec::new()));
            continue;
        }
        let cells = stages_of(item, now_ms);
        if let Some(last) = out.last_mut()
            && !cells.is_empty()
        {
            last.1.push(cells);
        }
    }
    out
}

/// What the chosen worktree waits on, from what the view holds of it.
fn waits_on(landing: &serde_json::Value, now_ms: i64) -> String {
    if landing.get("holding").and_then(serde_json::Value::as_bool) == Some(true) {
        return "nothing: it holds the lock".into();
    }
    let holder = &landing["holder"];
    let name = holder["name"].as_str().unwrap_or("");
    if name.is_empty() {
        return "its turn for the lock".into();
    }
    let phase = holder["phase"].as_str().unwrap_or("landing");
    let since = text::ago(holder["since"].as_i64().map(|ms| ms / 1000), now_ms / 1000);
    let mut out = format!("{name}, which holds the lock · {phase}");
    if !since.is_empty() {
        out.push_str(&format!(" · {since}"));
    }
    out
}

/// What happens next for the chosen worktree.
fn then_words(landing: &serde_json::Value) -> String {
    let onto = landing["target"]
        .as_str()
        .map_or("the default branch".to_string(), |t| t.to_string());
    if landing.get("holding").and_then(serde_json::Value::as_bool) != Some(true) {
        return format!("takes the lock at its turn, catches up with {onto}, pushes, cleans up");
    }
    let phase = landing["phase"].as_str().unwrap_or("");
    if phase.starts_with("tests failed")
        || phase.starts_with("check failed")
        || phase.starts_with("review blocked")
    {
        "lets go of the lock, fixes it, and lands again".into()
    } else if phase.contains("testing") || phase.starts_with("checking") {
        format!("pushes onto {onto}, then cleans up")
    } else if phase.starts_with("reviewing") {
        format!("pushes onto {onto} once the review is clean")
    } else if phase.starts_with("pushing") {
        "applies what landed, then cleans up".into()
    } else if phase.starts_with("applying") {
        "cleans up".into()
    } else if phase.starts_with("cleaning") {
        "lets go of the lock".into()
    } else {
        format!("catches up with {onto}, pushes, cleans up")
    }
}

/// The chosen worktree's last review round, in a line: its round, its
/// reviewer, how long ago, and codex-review's summary.
fn review_line(review: &serde_json::Value, now_ms: i64) -> String {
    let mut parts = vec![format!("round {}", review["round"].as_u64().unwrap_or(1))];
    if let Some(r) = review["reviewer"].as_str().filter(|r| !r.is_empty()) {
        parts.push(r.to_string());
    }
    let ago = text::ago(review["at"].as_i64().map(|ms| ms / 1000), now_ms / 1000);
    if !ago.is_empty() {
        parts.push(format!("{ago} ago"));
    }
    if let Some(n) = review["clean_rounds"].as_u64().filter(|n| *n > 0) {
        parts.push(format!("{n} clean"));
    } else if let Some(n) = review["findings"].as_u64() {
        parts.push(format!("{n} found"));
    }
    if let Some(s) = review["summary"].as_str().filter(|s| !s.is_empty()) {
        parts.push(s.to_string());
    }
    printable(&parts.join(" · "))
}

/// The keys the chosen worktree's detail offers: Enter its menu, and on a
/// narrow screen `i` its last output over the list (`serve::landing_key`).
fn landing_keys(landing: &serde_json::Value, narrow: bool) -> Vec<(&'static str, &'static str)> {
    let mut keys = vec![(
        "Enter",
        "go to its tab, move it in the queue, read its output",
    )];
    if narrow && landing["record"]["output"].is_object() {
        keys.push(("i", "its last output here"));
    }
    keys
}

/// The chosen worktree's detail under the rows, as board 5 draws it: what
/// it waits on, what happens then and where it is, its keys, then its last
/// review round (or its last tests) with the tail of that output, faint,
/// and what earlier rounds found with the commit that fixed each. `output`
/// is how many of the output's lines it shows; under `LANDING_OUTPUT_AT` columns
/// it shows none, `i` showing them over the list.
fn landing_detail(
    landing: &serde_json::Value,
    cols: usize,
    output: usize,
    m: &Mission,
    top_row: usize,
    hits: &mut Vec<HitBox>,
) -> Vec<Line> {
    let theme = &m.snap.theme;
    let now_ms = m.snap.now_secs * 1000;
    let narrow = cols < LANDING_OUTPUT_AT;
    let mut lines = vec![Line {
        spans: vec![kit::rule(cols, theme)],
        fill: None,
    }];
    let value_w = cols.saturating_sub(DETAIL_LABEL + 2).min(kit::WRAP_MAX);
    let fact = |label: &str, value: &str| {
        let mut row = Row::default();
        row.put(
            format!("  {label:<w$}", w = DETAIL_LABEL - 2),
            fg(theme.dim),
        )
        .put(text::cut(&printable(value), value_w), fg(theme.text));
        row.line(cols, None)
    };
    lines.push(fact("waits on", &waits_on(landing, now_ms)));
    lines.push(fact("then", &then_words(landing)));
    lines.push(fact(
        "worktree",
        landing["root"].as_str().unwrap_or_default(),
    ));
    let keys = landing_keys(landing, narrow);
    if !keys.is_empty() {
        let y = (top_row + lines.len()) as u16;
        let mut spans = vec![Span::raw(" ".repeat(DETAIL_LABEL))];
        for (key, what) in keys {
            let x0 = spans_width(&spans);
            let parsed = footer_key(key);
            let pointed = parsed
                .is_some_and(|k| m.hover == Some(Spot::Key(k)) || m.focus == Some(Focus::Key(k)));
            let lit = pointed || (key == "i" && m.landing_output);
            spans.extend(kit::keycap(key, what, lit, m.snap.caps, theme));
            if let Some(k) = parsed {
                hits.push(HitBox {
                    y,
                    x0: x0 as u16,
                    x1: spans_width(&spans).saturating_sub(1) as u16,
                    spot: Spot::Key(k),
                });
            }
        }
        lines.push(Line {
            spans: fit(spans, cols, Style::new()),
            fill: None,
        });
    }
    let record = &landing["record"];
    let review = &record["review"];
    let out = &record["output"];
    // The section named for what the kept output came from: the tests run
    // again after the review are the last tests, not the last review.
    let tests_last = out["of"].as_str() == Some("tests") && record["tests"].is_object();
    let last = if review.is_object() && !tests_last {
        Some(("Last review", review_line(review, now_ms)))
    } else if record["tests"].is_object() {
        let t = &record["tests"];
        let failed = t["failed"].as_u64().unwrap_or(0);
        let file = t["file"]
            .as_str()
            .filter(|f| !f.is_empty())
            .map(|f| format!(" · {}", printable(f)))
            .unwrap_or_default();
        let words = if failed > 0 {
            format!(
                "{failed} of {} failed{file}",
                t["total"].as_u64().unwrap_or(0)
            )
        } else if t["ok"].as_bool() == Some(false) {
            format!("failed{file}")
        } else {
            match t["passed"].as_u64().or(t["total"].as_u64()) {
                Some(n) => format!("{n} passed"),
                None => "passed".into(),
            }
        };
        Some(("Last tests", words))
    } else {
        None
    };
    if let Some((label, words)) = last {
        lines.push(Line {
            spans: kit::label(label, Tone::Dim, None, cols, theme),
            fill: None,
        });
        let mut row = Row::default();
        row.put(
            format!("  {}", text::cut(&words, cols.saturating_sub(3))),
            fg(theme.text),
        );
        lines.push(row.line(cols, None));
    }
    for found in record["found"].as_array().into_iter().flatten() {
        let mut row = Row::default();
        row.put(
            format!("  round {} found ", found["round"].as_u64().unwrap_or(0)),
            fg(theme.dim),
        )
        .put(
            printable(found["title"].as_str().unwrap_or_default()),
            fg(theme.text),
        );
        match found["fixed_by"].as_str().filter(|c| !c.is_empty()) {
            Some(commit) => row
                .put(" · fixed by ", fg(theme.dim))
                .put(printable(commit), fg(ACCENT)),
            None => row.put(" · not fixed", fg(theme.dim)),
        };
        lines.push(row.line(cols, None));
    }
    if !narrow {
        let tail: Vec<&str> = out["tail"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .collect();
        for line in &tail[tail.len().saturating_sub(output)..] {
            let mut row = Row::default();
            row.put(
                format!("  {}", printable(line)),
                Style::new().fg(theme.dim).add_modifier(Modifier::DIM),
            );
            lines.push(row.line(cols, None));
        }
    }
    lines
}

/// A line of a recorded output as text alone: a tab a space and every
/// other control character dropped, so an escape sequence in a log (a
/// clipboard write, a graphics command) is shown inert, never sent to the
/// terminal.
fn printable(line: &str) -> String {
    line.chars()
        .filter_map(|c| match c {
            '\t' => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect()
}

/// The chosen worktree's last output over the list (`i`, on a screen too
/// narrow to show it under the rows), as many of its last lines as fit.
fn landing_output_over(
    landing: &serde_json::Value,
    cols: usize,
    rows: usize,
    m: &Mission,
) -> Vec<Line> {
    let theme = &m.snap.theme;
    let out = &landing["record"]["output"];
    let of = if out["of"].as_str() == Some("review") {
        "Last review"
    } else {
        "Last tests"
    };
    let mut lines = vec![Line {
        spans: kit::label(of, Tone::Dim, None, cols, theme),
        fill: None,
    }];
    let tail: Vec<&str> = out["tail"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .collect();
    let room = rows.saturating_sub(lines.len());
    for line in &tail[tail.len().saturating_sub(room)..] {
        let mut row = Row::default();
        row.put(format!("  {}", printable(line)), fg(theme.text));
        lines.push(row.line(cols, None));
    }
    lines
}

/// UX 2.0's Landing view: each repository's queue under its heading (how
/// it lands, its counts at the right edge) and the stages' names, each
/// worktree two lines, its name bold with a cell per stage, then a dim
/// note of where it is with each cell's note under it; then the chosen
/// worktree's detail.
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
    let now_ms = m.snap.now_secs * 1000;
    let items = &view.data.items;
    let budget = rows.saturating_sub(1 + lines.len());
    let cursor = (m.cursor.max(0) as usize).min(items.len().saturating_sub(1));
    let chosen_landing = items
        .get(cursor)
        .filter(|_| m.cursor >= 0)
        .and_then(|i| i.data.get("landing"))
        .filter(|l| l.is_object());
    // The rows each item takes: a heading its line, then its stage names
    // and the dashed rule under them; a landing or a queued PR its row of
    // cells and its note; anything else a line.
    let height = |item: &Item| {
        if views::is_header(item) {
            if heads_of(item).is_empty() { 1 } else { 3 }
        } else if stages_of(item, now_ms).is_empty() {
            1
        } else {
            2
        }
    };
    let all: usize = items.iter().map(height).sum();
    // The detail without its output's lines, kept at least half the room
    // from the rows; what the rows and the whole detail leave goes to
    // those, so its newest lines are the ones shown.
    let mut scratch = Vec::new();
    let detail = chosen_landing
        .map(|l| landing_detail(l, cols, 0, m, 0, &mut scratch).len())
        .unwrap_or(0);
    let row_budget = budget.saturating_sub(detail.min(budget / 2));
    let output_lines = budget.saturating_sub(detail + all.min(row_budget));
    // As many as fit, from as far back of the chosen one as they can, so
    // it is always in sight.
    let mut first = cursor;
    let mut used = items.get(cursor).map_or(0, height);
    while first > 0 && used + height(&items[first - 1]) <= row_budget {
        first -= 1;
        used += height(&items[first]);
    }
    let mut shown = 0;
    let room = items[first.min(items.len())..]
        .iter()
        .take_while(|i| {
            shown += height(i);
            shown <= row_budget
        })
        .count()
        .max(1);
    // Each section's layout, by the heading it is under.
    let layouts: Vec<Option<StageLayout>> = stage_sections(view, now_ms)
        .iter()
        .map(|(heads, rows)| stage_layout(cols, heads, rows))
        .collect();
    let mut section = items[..first.min(items.len())]
        .iter()
        .filter(|i| views::is_header(i))
        .count();
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
            section += 1;
        }
        let layout = layouts.get(section).cloned().flatten();
        if views::is_header(item) {
            hit(lines);
            let right = item
                .data
                .get("right")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| {
                    item.data
                        .get("count")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0)
                        .to_string()
                });
            let room = cols.saturating_sub(1);
            let at = kit::columns(
                &[
                    Col::Stretch {
                        weight: 1,
                        min: 1,
                        max: None,
                    },
                    Col::Right {
                        natural: width(&right),
                        cap: room / 2,
                    },
                ],
                room,
                2,
                &[1],
            );
            let mut heading = Vec::new();
            if let Some(col) = at[0] {
                kit::place(
                    &mut heading,
                    vec![Span::styled(format!(" {}", item.title), bold(theme.bright))],
                    col,
                );
            }
            if let Some(col) = at[1] {
                kit::place(&mut heading, vec![Span::styled(right, fg(theme.dim))], col);
            }
            lines.push(Line {
                spans: fit(heading, cols, Style::new()),
                fill,
            });
            let names = heads_of(item);
            if let Some(layout) = layout.filter(|_| !names.is_empty()) {
                let first = if names.len() == STAGES.len() {
                    "worktree"
                } else {
                    "pull request"
                };
                let mut heads = Vec::new();
                kit::place(
                    &mut heads,
                    vec![Span::styled(first, fg(theme.dim))],
                    layout.worktree,
                );
                for (name, &col) in names.iter().zip(&layout.cells) {
                    kit::place(
                        &mut heads,
                        vec![Span::styled(kit::centered(name, col.1), fg(theme.dim))],
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
        let stages = stages_of(item, now_ms);
        let Some(layout) = layout.filter(|_| !stages.is_empty()) else {
            hit(lines);
            let mut row = Row::default();
            row.put(pointer, fg(ACCENT))
                .put(" ", Style::new())
                .put(item.title.clone(), fg(theme.text))
                .put("  ", Style::new())
                .put(item.detail.clone(), fg(theme.dim));
            lines.push(row.line(cols, fill));
            continue;
        };
        let name = item
            .title
            .split_once(". ")
            .map_or(item.title.as_str(), |(_, n)| n);
        hit(lines);
        let mut top = vec![Span::styled(pointer, fg(ACCENT)), Span::raw(" ")];
        kit::place(
            &mut top,
            vec![Span::styled(name.to_string(), bold(theme.bright))],
            layout.worktree,
        );
        let mut under = vec![Span::styled(pointer, fg(ACCENT)), Span::raw(" ")];
        kit::place(
            &mut under,
            vec![Span::styled(item.detail.clone(), fg(theme.dim))],
            layout.worktree,
        );
        for (cell, &col) in stages.iter().zip(&layout.cells) {
            let label = if layout.marks && cell.stage == Stage::Waiting {
                WAITING_MARK
            } else {
                cell.label.as_str()
            };
            kit::place(
                &mut top,
                kit::cell(label, col.1, cell.stage.tone(), m.snap.caps, theme),
                col,
            );
            if !cell.note.is_empty() {
                kit::place(
                    &mut under,
                    vec![kit::cell_note(&cell.note, col.1, theme)],
                    col,
                );
            }
        }
        lines.push(Line {
            spans: fit(top, cols, Style::new()),
            fill,
        });
        hit(lines);
        lines.push(Line {
            spans: fit(under, cols, Style::new()),
            fill,
        });
    }
    if let Some(landing) = chosen_landing {
        let detail = landing_detail(landing, cols, output_lines, m, top_row + lines.len(), hits);
        let room = rows.saturating_sub(1 + lines.len());
        lines.extend(detail.into_iter().take(room));
    }
}

/// Whether the Keys view is drawn as UX 2.0's board (`keys_board`): with
/// UX 2.0, room for it and a key to show.
pub fn keys_board_shown(m: &Mission, v: &View, cols: usize) -> bool {
    m.snap.rules.ux2
        && v.id == ViewId::Keys
        && cols >= super::keys_board::BOARD_AT
        && v.data.items.iter().any(|i| !views::is_header(i))
        && super::keys_board::fits(&v.data.items, m.laid().1 as usize)
}

/// How wide the main area must be for UX 2.0's Autopilot board (its six
/// columns 17 wide), narrower showing the plain list; and how wide for the
/// chosen card's pane beside it, under it on a narrower screen, and how
/// many columns that takes.
pub const PILOT_BOARD_AT: usize = 108;
const PILOT_SIDE_AT: usize = 200;
const PILOT_SIDE_W: usize = 44;
/// Where a diagram of a plan that can't be drawn beside the board is shown
/// properly (`Markdown::parse`).
const PLAN_PLACE: &str = "its plan note in Notes";

/// Whether the Slack view is drawn as its board: with UX 2.0 on, once
/// something is saved.
pub fn slack_board_shown(m: &Mission, v: &View) -> bool {
    m.snap.rules.ux2 && v.id == ViewId::Slack && v.loaded && v.data.at.is_some()
}

/// Whether the Settings view is drawn as its board: with UX 2.0 on and the
/// screen wide enough.
pub fn settings_board_shown(m: &Mission) -> bool {
    m.snap.rules.ux2 && m.laid().0 as usize >= super::settings_board::BOARD_AT
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
    /// A Slack file's picture (`images::picture_shared`), from the
    /// snapshot's `slack_pictures` by its file ID.
    Picture {
        key: String,
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
            | Shape::Avatar { .. }
            | Shape::Picture { .. } => None,
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

/// How long a card's item has worked, while it works ("worked 42m"), or
/// how long it took once landed ("took 1h12m"); none before it opened.
fn pilot_timer(pilot: &serde_json::Value, now_secs: i64) -> Option<String> {
    let started = pilot["started"].as_i64()?;
    Some(match pilot["ended"].as_i64() {
        Some(ended) => format!("took {}", text::span(ended - started)),
        None => format!("worked {}", text::span(now_secs - started)),
    })
}

/// UX 2.0's Autopilot view as a board (src/autopilot/view.rs): its heading
/// (`AUTO`, the note bold, its facts dim, the button that opens its menu,
/// and today's spend against its limit as a meter at the right), then the
/// kit's board (board 0c), a column per place an item stands, each item a
/// card (`pilot_card`), its meter of stages done placed as an image. On a
/// screen `PILOT_SIDE_AT` wide the chosen item sits beside it in full, with
/// its stages and its plan under them, and under the board on a narrower
/// one with the room, its five columns taking the width; the diagrams the
/// plan draws as pictures come back, for the frame to place.
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
    let top = lines.len();
    let side_w = beside_w(cols, PILOT_SIDE_AT, PILOT_SIDE_W);
    let board_w = cols - side_w;
    let held: Vec<Vec<usize>> = crate::autopilot::view::COLUMNS
        .iter()
        .map(|column| {
            (0..items.len())
                .filter(|&i| items[i].group.as_deref() == Some(*column))
                .collect()
        })
        .collect();
    let board_columns: Vec<kit::BoardColumn> = crate::autopilot::view::COLUMNS
        .iter()
        .zip(&held)
        .map(|(column, indices)| kit::BoardColumn {
            name: column.to_string(),
            tone: pilot_column_tone(column),
            cards: indices
                .iter()
                .map(|&i| pilot_card(&items[i], chosen(i), lit(i), m))
                .collect(),
        })
        .collect();
    let drawn = kit::board(&board_columns, board_w, body, more_lit(m), theme);
    board_hits(&drawn, top, hits, |c, i| held[c][i]);
    for p in &drawn.placed {
        let card = &board_columns[p.column].cards[p.card];
        let item = &items[held[p.column][p.card]];
        if let (Some(row), Some((frac, tone))) = (card.meter_row(p.w), card.meter) {
            meters.push(Image::meter(
                format!("autopilot/{}", item.id),
                p.x + 2,
                top + p.y + row,
                p.w.saturating_sub(4),
                frac,
                tone.fg(theme),
            ));
        }
    }
    let mut rows = drawn.lines;
    let about = usize::try_from(m.cursor).ok().and_then(|c| items.get(c));
    let under = side_w == 0 && about.is_some_and(|item| !views::is_summary(item));
    let about_w = if under { board_w } else { side_w };
    let about = about
        .filter(|_| about_w > 0)
        .map(|item| pilot_about(item, about_w, theme, ready_diagrams(m).as_ref()));
    let pictures = if under {
        pane_under(about, &mut rows, body, board_w, m, top, hits, theme)
    } else {
        pane_beside(about, &mut rows, body, board_w, side_w, m, top, hits, theme)
    };
    lines.extend(rows.into_iter().map(|spans| Line { spans, fill: None }));
    pictures
}

/// An Autopilot column's tone, as the kit marks where an item stands.
fn pilot_column_tone(column: &str) -> Tone {
    match column {
        "Working" => Tone::Working,
        "Waiting on you" => Tone::Needs,
        "Landing" => Tone::Landing,
        "Landed" => Tone::Done,
        "Skipped" => Tone::Faint,
        _ => Tone::Dim,
    }
}

/// An Autopilot item's card: on its top edge its state's mark and its
/// tab's number (or its place in line, or "closed"), a red ✕ at the right
/// once a stage failed; its first line, where it is and how long it has
/// worked as its facts, and a meter of its stages done. A skipped item's
/// card says "skipped by you", and one held by the open item it waits on
/// says so, both dim with no meter.
fn pilot_card(item: &Item, chosen: bool, hovered: bool, m: &Mission) -> kit::BoardCard {
    let theme = &m.snap.theme;
    let pilot = item.data.get("pilot");
    let skipped = item.data.get("skipped").is_some();
    let held = item.data.get("waitsOn").is_some();
    let failed = pilot.and_then(|p| p["failed"].as_bool()) == Some(true);
    let stages = |p: &serde_json::Value| p["done"].as_f64().unwrap_or(0.0) as f32 / 8.0;
    let (name, where_, meter) = match pilot {
        // A skipped item has no place in line and no stages.
        None if skipped => (String::new(), "skipped by you".to_string(), None),
        // Nor has one held by the open item it waits on.
        None if held => (String::new(), item.detail.clone(), None),
        Some(p) if p["closed"] == true => {
            ("closed".to_string(), item.detail.clone(), Some(stages(p)))
        }
        Some(p) => (
            format!("tab {}", p["index"]),
            item.detail
                .split_once(" · ")
                .map_or(item.detail.as_str(), |(_, w)| w)
                .to_string(),
            Some(stages(p)),
        ),
        None => {
            let place = item.data["place"].as_u64().unwrap_or(1);
            (
                ordinal(place),
                if place == 1 {
                    "opens next".to_string()
                } else {
                    format!("{} in line", ordinal(place))
                },
                None,
            )
        }
    };
    let mark = if failed {
        None
    } else {
        match item.group.as_deref() {
            Some("Next up") => Some(Mark::Pending),
            Some("Working") => Some(Mark::Working),
            Some("Waiting on you") => Some(Mark::Needs),
            Some("Landing") => Some(Mark::Landing),
            Some("Landed") => Some(Mark::Passed),
            _ => None,
        }
    };
    let title = pilot
        .and_then(|p| p["item"].as_str())
        .or_else(|| item.data["next"].as_str())
        .unwrap_or(&item.title)
        .to_string();
    let mut facts = vec![vec![Span::styled(
        where_,
        fg(if skipped { theme.faint } else { theme.dim }),
    )]];
    if let Some(words) = pilot.and_then(|p| pilot_timer(p, m.snap.now_secs)) {
        facts.push(vec![Span::styled(words, fg(theme.dim))]);
    }
    kit::BoardCard {
        mark: mark.map(|k| kit::mark(k, m.snap.frame, theme)),
        name: Span::styled(name, Tone::Text.bold(theme)),
        state: if failed {
            vec![kit::mark(Mark::Failed, 0, theme)]
        } else {
            Vec::new()
        },
        title,
        title_look: if skipped || held {
            kit::Title::Past
        } else {
            kit::Title::Live
        },
        facts,
        meter: meter
            .filter(|_| !skipped)
            .map(|frac| (frac, pilot_tone(item))),
        chosen,
        hovered,
    }
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
        None if item.data.get("waitsOn").is_some() => format!(
            "Waits on {}, still open: opens once it is ticked",
            item.data["waitsOn"].as_str().unwrap_or_default()
        ),
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
pub(super) fn lit(spans: Vec<Span<'static>>, theme: &Theme) -> Vec<Span<'static>> {
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
    // Several views at once: Tab goes between them, Esc back to one.
    if m.grid.is_some() && !m.filter.as_ref().is_some_and(|f| f.typing) && m.jump.is_none() {
        return pairs(&[
            ("tab", "next view"),
            ("jk", "move"),
            ("Enter", "actions"),
            ("m", "view actions"),
            ("esc", "one view"),
        ]);
    }
    if m.jump.is_some() {
        return pairs(&[
            ("", "type a view's name"),
            ("Enter", "go there"),
            ("esc", "cancel"),
        ]);
    }
    // While a filter types, letters are the filter's, not keys, and what
    // is typed leads the line: the bar it ends may be too wide to show it.
    if let Some(f) = m.filter.as_ref().filter(|f| f.typing) {
        let mut out = vec![(format!("/{}▏", f.text), String::new())];
        out.extend(pairs(&[("↑↓", "move")]));
        out.extend(if m.picker {
            pairs(&[("Enter", "actions"), ("esc", "close")])
        } else {
            pairs(&[("Enter", "keep it"), ("esc", "clear")])
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
    // Every view: Enter the chosen item's actions, `m` the view's, after
    // the keys that move, in the canvas's words where it words them for
    // the view.
    let with_acts = |moves: &[(&str, &str)], enter: &str, view: &str, rest: &[(&str, &str)]| {
        let mut first = moves.to_vec();
        first.extend([("Enter", enter), ("m", view)]);
        first.extend_from_slice(rest);
        with(&first)
    };
    const ACTS: &str = "actions";
    const VIEW: &str = "view actions";
    let filter: &[(&str, &str)] = &[("/", "filter")];
    let Some(view) = view else {
        return with_acts(&[("hjkl", "move")], ACTS, VIEW, filter);
    };
    match view.id {
        ViewId::Reminders if super::reminders::is_month(m) => {
            with_acts(super::reminders::hints(), ACTS, VIEW, &[])
        }
        ViewId::Routines if crate::routines::view::is_board(m) => with_acts(
            &[("jk", "move")],
            ACTS,
            VIEW,
            crate::routines::view::hints((m.size.0 as usize) < crate::routines::view::BESIDE_AT),
        ),
        ViewId::Settings if settings_board_shown(m) => with_acts(
            &[("hl", "category"), ("jk", "option")],
            "its choices",
            VIEW,
            &[("/", "search every category")],
        ),
        ViewId::Settings => with_acts(&[("jk", "move")], "its choices", VIEW, &[("/", "search")]),
        ViewId::Keys if keys_board_shown(m, view, m.laid().0 as usize) => with_acts(
            &[("hl", "group"), ("jk", "key")],
            ACTS,
            VIEW,
            &[("/", "filter keys, words, tags")],
        ),
        ViewId::Slack if slack_board_shown(m, view) => {
            let mut keys = Vec::new();
            let threads = crate::settings::is_on("slack_threads");
            if threads && !super::slack_board::beside(m.laid().0 as usize) {
                keys.push(("i", "thread"));
            }
            keys.push(("/", "filter"));
            with_acts(&[("jk", "move")], ACTS, VIEW, &keys)
        }
        ViewId::Landing if m.snap.rules.ux2 => {
            let narrow = (m.laid().0 as usize) < LANDING_OUTPUT_AT;
            let output = m
                .chosen_item()
                .is_some_and(|i| i.data["landing"]["record"]["output"].is_object());
            let rest: &[(&str, &str)] = match (narrow && output, m.landing_output) {
                (true, true) => &[("i", "back to the list")],
                (true, false) => &[("i", "its last output")],
                _ => &[],
            };
            with_acts(&[("jk", "move")], ACTS, VIEW, rest)
        }
        ViewId::Calendar => with_acts(&[("jk", "events"), ("hl", "day")], ACTS, VIEW, &[]),
        ViewId::Autopilot if m.snap.rules.ux2 && m.laid().0 as usize >= PILOT_BOARD_AT => {
            with_acts(
                &[("hl", "column"), ("jk", "move")],
                ACTS,
                "view actions: its settings, pause",
                &[],
            )
        }
        ViewId::Autopilot => with_acts(
            &[("jk", "move")],
            ACTS,
            "view actions: its settings, pause",
            filter,
        ),
        ViewId::Inbox if super::inbox_board::shown(m, view) => {
            let ticket: &[(&str, &str)] = if super::inbox_board::beside(m.laid().0 as usize) {
                &[]
            } else if m.inbox_pane {
                &[("i", "back to the list")]
            } else {
                &[("i", "its ticket")]
            };
            let mut rest = ticket.to_vec();
            rest.extend_from_slice(filter);
            with_acts(&[("jk", "move")], "actions, marks read", VIEW, &rest)
        }
        ViewId::Resources => with_acts(
            &[("jk", "move")],
            "actions: go to the tab, end a process",
            VIEW,
            filter,
        ),
        ViewId::Prs | ViewId::Tickets => {
            with_acts(&[("hl", "column"), ("jk", "move")], ACTS, VIEW, &[])
        }
        ViewId::Usage
            if m.snap.rules.ux2 && m.laid().0 as usize >= super::usage_board::BOARD_AT =>
        {
            with_acts(&[("jk", "tab or service")], ACTS, VIEW, filter)
        }
        _ => with_acts(&[("jk", "move")], ACTS, VIEW, filter),
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
    // A chip for each kind, which a click shows alone (as `m`'s menu does).
    let mut spans = vec![Span::raw(" ")];
    let mut x = 1;
    for key in 1..=7 {
        let name = Kind::of_key(key).map_or("All", Kind::name);
        let lit = m.notice_kind == key;
        let under = m.hover == Some(Spot::Kind(key)) || m.focus == Some(Focus::Kind(key));
        let tone = if lit { Tone::Accent } else { Tone::Dim };
        let mut cap = kit::chip(
            &format!("{name} {}", counts.keys[key - 1]),
            tone,
            caps,
            theme,
        );
        cap.push(Span::raw(" "));
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
    const WHEN: usize = 9;
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
                head(&format!("{:>w$}", "When", w = WHEN - 2)),
                pad(3),
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
            let now = jiff::Timestamp::from_second(m.snap.now_secs)
                .unwrap_or_default()
                .to_zoned(m.snap.tz.clone());
            // The clock time alone, a meeting to come's start too, so it
            // fits the column.
            let at = item
                .time
                .map(|t| crate::ui::when::clock(t, &now))
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
                Span::styled(format!("{at:>w$} ", w = WHEN - 1), Tone::Faint.style(theme)),
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

/// The key a footer's label names: its arrows and words as the keys
/// they are, a single key as it is written; none for a pair (`jk`, `J K`),
/// whose rows and pane take the click themselves.
fn footer_key(label: &str) -> Option<crate::mux::input::Key> {
    let name = match label {
        "↵" | "Enter" => "Enter",
        "esc" => "Escape",
        "tab" => "Tab",
        l if l.chars().count() == 1 || l.starts_with("M-") => l,
        _ => return None,
    };
    crate::mux::input::Key::parse(name)
}

/// The footer, whose keys take a click and a Tab stop: the kit's hints
/// with UX 2.0, dim words without, each key that names one a spot (`Spot::Key`) at row `y`, lit
/// under the mouse.
fn key_footer(
    pairs: &[(String, String)],
    m: &Mission,
    theme: &Theme,
    (y, cols): (u16, usize),
    hits: &mut Vec<HitBox>,
) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut x = 1;
    for (key, what) in pairs {
        let p: [(&str, &str); 1] = [(key.as_str(), what.as_str())];
        // With UX 2.0 the kit's hints, each key in the accent; else dim
        // words, as before, the key its own span so it lights alone.
        let mut spans = if m.snap.rules.ux2 {
            kit::hints(&p, theme)
        } else {
            let dim = fg(theme.dim);
            match (key.is_empty(), what.is_empty()) {
                (true, _) => vec![Span::styled(what.clone(), dim)],
                (_, true) => vec![Span::styled(key.clone(), dim)],
                _ => vec![
                    Span::styled(key.clone(), dim),
                    Span::styled(format!(" {what}"), dim),
                ],
            }
        };
        if !out.is_empty() {
            out.push(Span::raw("  "));
            x += 2;
        }
        // Only a key the line shows whole takes a click and a Tab stop: one
        // past the screen's edge, cut off, is no control to reach, though
        // what it does may run past it.
        let shown = |end: usize| x + end < cols;
        // The keys it names, each with the byte it starts at and its length
        // in bytes: the label's one key, or each key of a group (`jk`,
        // `H L`, `[ ]`, `↑↓`), as long as every one of them is a key. Words
        // with nothing said of them (a filter being typed) are no keys.
        let keys: Vec<(crate::mux::input::Key, usize, usize)> = match footer_key(key) {
            _ if what.is_empty() => Vec::new(),
            Some(k) => vec![(k, 0, key.len())],
            None => {
                let named = |c: char| {
                    let name = match c {
                        '↑' => "Up".to_string(),
                        '↓' => "Down".to_string(),
                        c => c.to_string(),
                    };
                    crate::mux::input::Key::parse(&name)
                };
                let each: Vec<_> = key
                    .char_indices()
                    .filter(|(_, c)| *c != ' ')
                    .map(|(at, c)| named(c).map(|k| (k, at, c.len_utf8())))
                    .collect();
                if each.iter().all(Option::is_some) {
                    each.into_iter().flatten().collect()
                } else {
                    Vec::new()
                }
            }
        };
        let keys: Vec<_> = keys
            .into_iter()
            .filter(|&(_, at, w)| shown(width(&key[..at + w])))
            .collect();
        if !keys.is_empty() {
            let style = spans[0].style;
            let lit = |k| m.hover == Some(Spot::Key(k)) || m.focus == Some(Focus::Key(k));
            // A group's letters are spans of their own, so the one a click
            // or Tab's focus is on lights alone.
            let mut parts: Vec<Span<'static>> = Vec::new();
            let mut done = 0;
            for &(k, at, w) in &keys {
                if at > done {
                    parts.push(Span::styled(key[done..at].to_string(), style));
                }
                let mut part_style = style;
                // Lit under the mouse, and with Tab's focus on it, so Enter
                // and Space show what they press.
                if lit(k) {
                    part_style = part_style.add_modifier(Modifier::REVERSED);
                }
                parts.push(Span::styled(key[at..at + w].to_string(), part_style));
                // Its columns: what is before it, then its own width.
                let from = x + width(&key[..at]);
                hits.push(HitBox {
                    y,
                    x0: from as u16,
                    x1: (from + width(&key[at..at + w])).saturating_sub(1) as u16,
                    spot: Spot::Key(k),
                });
                done = at + w;
            }
            if done < key.len() {
                parts.push(Span::styled(key[done..].to_string(), style));
            }
            spans.splice(0..1, parts);
        }
        x += spans_width(&spans);
        out.extend(spans);
    }
    out
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

/// How many of a bar's names, `widths` wide with a space between, to skip
/// so the one at `target` and the `tail` after it end inside `cols`, after
/// the `before` columns ahead of the names and a `‹ ` once any are skipped.
fn bar_skip(
    widths: &[usize],
    target: Option<usize>,
    before: usize,
    tail: usize,
    cols: usize,
) -> usize {
    let Some(t) = target.filter(|&t| t < widths.len()) else {
        return 0;
    };
    let end_of = |skip: usize| {
        let names: usize = widths[skip..=t].iter().map(|w| w + 1).sum();
        before + if skip > 0 { 2 } else { 0 } + names - 1 + tail
    };
    let mut skip = 0;
    while skip < t && end_of(skip) > cols {
        skip += 1;
    }
    skip
}

/// With UX 2.0, the bar as two rows and a rule (board 3 of the canvas):
/// the groups that have a view, the one in front lit, then the tabs'
/// counts; that group's views, the one in front lit and underlined, then
/// the filter, and the keys that move through them at the right. While a
/// view's name is typed, the group it is in shows, the view lit as it is
/// matched. With several views at once (`grid`), the first row lights
/// every group a shown view is in and the second names the shown views,
/// the one with the focus lit and underlined, with how many at its right.
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
    let in_grid: Vec<usize> = m.grid_shown().into_iter().map(|(_, at)| at).collect();
    let gridded = !in_grid.is_empty();
    let mut head = Row::default();
    head.put(" ", Style::new())
        .put("MISSION CONTROL", bold(ACCENT))
        .put("  ", Style::new());
    for (g, held) in groups {
        let lit = if gridded {
            held.iter().any(|v| in_grid.contains(v))
        } else {
            shown.is_some_and(|(s, _)| s == g)
        };
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
    let names: &[usize] = if gridded {
        &in_grid
    } else {
        shown.map(|(_, held)| held.as_slice()).unwrap_or(&[])
    };
    let title_of = |i: usize| match i.checked_sub(1).and_then(|v| snap.views.get(v)) {
        Some(v) => views::bar_title(v),
        None => "Tabs",
    };
    // With Tab's focus on a view's name or the filter past the screen's
    // edge, the row scrolls: the names before it give way to a ‹.
    let (target, tail) = match m.focus {
        Some(Focus::View(v)) => (names.iter().position(|&i| i == v), 0),
        Some(Focus::Filter) => (names.len().checked_sub(1), 1 + spans_width(&bar.filter)),
        _ => (None, 0),
    };
    let widths: Vec<usize> = names.iter().map(|&i| width(title_of(i)) + 2).collect();
    let skip = bar_skip(&widths, target, 1, tail, cols);
    let mut row = Row::default();
    row.put(" ", Style::new());
    if skip > 0 {
        row.put("‹ ", fg(theme.dim));
    }
    for &i in &names[skip..] {
        let title = title_of(i);
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
    let keys = if gridded {
        vec![Span::styled(
            format!("{} views at once", in_grid.len()),
            Tone::Dim.style(theme),
        )]
    } else {
        kit::hints(&[("M-[ ]", "groups"), ("[ ]", "views")], theme)
    };
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
    let mut hits = Vec::new();
    let mut meters = Vec::new();
    let mut lines = bar(m, cols, &mut hits);
    let view = m.view.checked_sub(1).and_then(|i| m.snap.views.get(i));
    let pictures = body(m, view, cols, rows, &mut lines, &mut hits, &mut meters);
    let (lines, hits, meters) = finish(m, view, cols, rows, lines, hits, meters);
    (lines, hits, meters, pictures)
}

/// The bar: with UX 2.0 its two rows and a rule (`grouped_bar`), else
/// one row naming every view.
pub(super) fn bar(m: &Mission, cols: usize, hits: &mut Vec<HitBox>) -> Vec<Line> {
    let snap = &m.snap;
    let theme = &snap.theme;
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
    // With the Reminders view in front, the bar's right counts them.
    // And with Routines, them and when the next runs.
    let counts = match view
        .filter(|v| matches!(v.id, ViewId::Reminders | ViewId::Routines) && snap.rules.ux2)
    {
        Some(v) if v.id == ViewId::Routines => {
            let now = jiff::Timestamp::from_second(snap.now_secs)
                .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
                .to_zoned(snap.tz.clone());
            vec![Span::styled(
                crate::routines::view::bar_words(v, &now),
                fg(theme.dim),
            )]
        }
        Some(v) => vec![Span::styled(super::reminders::bar_words(v), fg(theme.dim))],
        None => tally(&snap.tabs, &snap.rules, theme),
    };
    let mut lines = Vec::new();
    if let Some(groups) = m.groups() {
        let bar = Bar {
            jumped,
            counts,
            filter,
            filter_hit: filter_stop || (m.filter.is_some() && snap.tab_focus),
        };
        grouped_bar(m, &groups, bar, cols, &mut lines, hits);
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
        let tail = if m.focus == Some(Focus::Filter) {
            2 + spans_width(&counts) + spans_width(&filter)
        } else {
            0
        };
        let widths: Vec<usize> = labels.iter().map(|l| width(l)).collect();
        let skip = bar_skip(&widths, target, width(prefix), tail, cols);
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
        lines.push(with_close(head, m, cols, hits));
    }
    lines
}

/// What is under the bar for view `view` (`None` the tabs) on a screen
/// `cols` by `rows`, after the `lines` already there, and the pictures
/// placed over it.
pub(super) fn body(
    m: &Mission,
    view: Option<&View>,
    cols: usize,
    rows: usize,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
    meters: &mut Vec<Image>,
) -> Vec<Picture> {
    let snap = &m.snap;
    let theme = &snap.theme;
    let mut pictures = Vec::new();
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
            cards(m, cols, body, top, lines, hits);
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
                pictures = pilot_board(v, cols, rows, m, lines, hits, meters);
            } else if settings_shown {
                super::settings_board::board(v, m, cols, rows, lines, hits, meters);
            } else if resources_shown {
                super::resources_board::board(v, cols, rows, m, lines, hits, meters);
            } else if ci_shown {
                ci_rows(v, cols, rows, m, lines, hits);
            } else if let Some(columns) = v.columns() {
                if v.data.setup.is_some() || !v.loaded {
                    list(v, cols, rows, m, 0, lines, hits);
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
                        pictures = board_cards(v, columns, cols, room, m, theme, top, lines, hits);
                    } else {
                        board(v, columns, cols, room, m, theme, top, lines, hits);
                    }
                }
            } else if snap.rules.ux2
                && v.id == ViewId::Usage
                && cols >= super::usage_board::BOARD_AT
                && super::usage_board::board(v, m, cols, rows, lines, hits, meters)
            {
            } else if super::inbox_board::shown(m, v) {
                super::inbox_board::board(v, cols, rows, m, lines, hits, meters);
            } else if keys_board_shown(m, v, cols) {
                super::keys_board::board(v, cols, rows, m, lines, hits, meters);
            } else if slack_board_shown(m, v) {
                let look = super::slack_board::Look {
                    cols,
                    rows: rows.saturating_sub(1 + lines.len()),
                    top_row: lines.len(),
                    cursor: usize::try_from(m.cursor).ok(),
                    theme,
                    caps: snap.caps,
                    now_ms: snap.now_secs * 1000,
                    tz: &snap.tz,
                    thread: snap.slack_thread.as_ref(),
                    threads_on: crate::settings::is_on("slack_threads"),
                    pictures: &snap.slack_pictures,
                    graphics: snap.cell.is_some(),
                    work: &snap.slack_work,
                    tabs: if snap.all_tabs.is_empty() {
                        &snap.tabs
                    } else {
                        &snap.all_tabs
                    },
                    side_top: m.side_top(),
                    thread_open: m.slack_thread_open,
                    failed: snap.slack_thread_failed.as_deref(),
                    files_failed: &snap.slack_files_failed,
                    files_kept: &snap.slack_files_kept,
                    hover: match m.hover {
                        Some(Spot::Index(i)) => usize::try_from(i).ok(),
                        _ => None,
                    },
                };
                let (board, at, images) = super::slack_board::board(v, &look);
                lines.extend(board);
                hits.extend(at);
                meters.extend(images);
            } else if v.id == ViewId::Reminders && super::reminders::is_month(m) {
                super::reminders::month(v, m, cols, rows, lines, hits);
            } else if v.id == ViewId::Routines && crate::routines::view::is_board(m) {
                crate::routines::view::board(v, m, cols, rows, lines, hits);
            } else if v.id == ViewId::Calendar && super::calendar::is_day(v) {
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
                    side_top: m.side_top(),
                };
                let (day, at, avatars) = super::calendar::day(v, &look);
                lines.extend(day);
                hits.extend(at);
                meters.extend(avatars);
            } else {
                list(v, cols, rows, m, 0, lines, hits);
            }
        }
    }
    pictures
}

/// The lines cut or padded to the screen's rows, and the footer: the keys,
/// or what the last action did.
pub(super) fn finish(
    m: &Mission,
    view: Option<&View>,
    cols: usize,
    rows: usize,
    mut lines: Vec<Line>,
    mut hits: Vec<HitBox>,
    mut meters: Vec<Image>,
) -> (Vec<Line>, Vec<HitBox>, Vec<Image>) {
    let snap = &m.snap;
    let theme = &snap.theme;
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
            // Every footer's keys take a click and a Tab stop, with UX 2.0
            // or without.
            let y = rows.saturating_sub(1) as u16;
            row.spans
                .extend(key_footer(&pairs, m, theme, (y, cols), &mut keys_hit));
            row
        }
    };
    lines.push(foot.line(cols, None));
    hits.retain(|h| (h.y as usize) < rows.saturating_sub(1));
    hits.extend(keys_hit);
    meters.retain(|image| (image.y + image.height) as usize <= rows.saturating_sub(1));
    (lines, hits, meters)
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
            landing_stages(&serde_json::json!({ "holding": true, "phase": phase }), 0)[0]
                .label
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
            inbox_ticket: None,
            inbox_links: Default::default(),
            slack_thread: None,
            slack_pictures: Default::default(),
            slack_work: Vec::new(),
            slack_thread_failed: None,
            slack_files_failed: Default::default(),
            slack_files_kept: Default::default(),
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
            .find(|h| h.spot == Spot::Key(crate::mux::input::Key::parse("m").unwrap()))
            .expect("m on the footer is a click");
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
            "m"
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
        assert!(
            keys.starts_with(" /tw▏  ↑↓ move  Enter keep it"),
            "{keys:?}"
        );
        m.picker = true;
        let (lines, _) = screen(&m, 60, 20);
        let keys = texts(&lines).last().cloned().unwrap();
        assert!(keys.contains("Enter actions  esc close"), "{keys:?}");
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

    /// The tabs board 3 draws: one waiting on a question, two working,
    /// one idle and one landing.
    fn board_3() -> Mission {
        use crate::agent::{Doing, Wait, WaitKind};
        let now = rules().now_secs();
        let mut asks = tab(1, "relay images");
        asks.state = TabState::Waiting;
        asks.since = Some(now - 240);
        asks.ask = Some(Wait {
            kind: WaitKind::Question,
            tool: "AskUserQuestion".into(),
            header: "Layout".into(),
            words: "Which layout should the cards use?".into(),
            options: vec!["Cards".into(), "Rows".into()],
            details: vec![String::new(), "one line a tab, as the sidebar".into()],
            questions: 1,
        });
        asks.turn = Some(crate::sidebar::claude::Turn {
            prompt: "lay out the cards".into(),
            before: Some("Two layouts fit the board.".into()),
            ..Default::default()
        });
        asks.checks = Some(crate::facts::Checks {
            pass: 12,
            fail: 1,
            pending: 0,
        });
        asks.pr = Some(7);
        let mut working = tab(2, "summaries");
        working.state = TabState::Working;
        working.since = Some(now - 120);
        working.summary = Some("Wrote the summary module.".into());
        working.doing = Some(Doing {
            tool: "Bash".into(),
            arg: "cargo test --lib mission".into(),
            since: now - 60,
            call: "c".into(),
        });
        working.size = Some((10, 2));
        working.round = 2;
        let mut other = tab(3, "docs");
        other.state = TabState::Working;
        let mut idle = tab(4, "quiet one");
        idle.since = Some(now - 7200);
        idle.summary = Some("Fixed the flaky test.".into());
        let mut landing = tab(5, "landing one");
        landing.landing = Some(crate::facts::Landing {
            holding: false,
            line: 2,
            method: crate::facts::Method::Direct,
        });
        let mut m = mission(vec![asks, working, other, idle, landing], Vec::new());
        m.snap.rules.ux2 = true;
        m.snap.screens.insert(
            TabId(2),
            crate::mission::Screen {
                lines: (1..=40)
                    .map(|n| vec![Span::raw(format!("line {n}"))])
                    .collect::<Vec<_>>()
                    .into(),
                branch: Some("note/summaries".into()),
                options: Vec::new(),
                question: String::new(),
                more: Vec::new(),
            },
        );
        m
    }

    #[test]
    fn with_ux2_the_tabs_are_cards_under_their_groups_labels_and_a_shelf_under_them() {
        let mut m = board_3();
        m.cursor = 1;
        let (lines, hits) = screen(&m, 160, 60);
        let text = texts(&lines);
        let row = |has: &str| {
            text.iter()
                .position(|l| l.contains(has))
                .unwrap_or_else(|| panic!("{has}: {text:#?}"))
        };
        // Each group a label with its count at the right edge, no frame
        // round it, over open cards whose bars wear its tone.
        let needs = row("NEEDS YOU");
        assert!(
            text[needs].ends_with('1') && width(&text[needs]) == 159,
            "{text:#?}"
        );
        let working = row("WORKING");
        assert!(text[working].ends_with('2'), "{text:#?}");
        // A working card shows its review loop's round among its chips.
        row("round 2");
        let bar = lines[needs + 2]
            .spans
            .iter()
            .find(|s| s.content.contains('▎'))
            .unwrap();
        assert_eq!(bar.style.fg, Some(Tone::Needs.fg(&Theme::DARK)));
        assert!(text[needs + 1].starts_with("─ "), "{text:#?}");
        assert!(
            !text.iter().any(|l| l.contains(['╭', '╮', '╰', '╯', '│'])),
            "{text:#?}"
        );
        // The chosen card's bar in the accent.
        let chosen_bar = lines[working + 2]
            .spans
            .iter()
            .find(|s| s.content.contains('▎'))
            .unwrap();
        assert_eq!(chosen_bar.style.fg, Some(Tone::Accent.fg(&Theme::DARK)));
        // The waiting card: how long on its edge, its question word for
        // word under its kind, its options with no numbers, and why it asked.
        assert!(text[needs + 1].contains("waiting 4m"), "{text:#?}");
        assert!(text[row("QUESTION · LAYOUT")].starts_with("▎ "));
        row("Which layout should the cards use?");
        let cards = row("Cards");
        assert!(
            text[cards].contains("Cards") && !text[cards].trim_end().ends_with(" 1"),
            "{text:#?}"
        );
        assert!(!text[row("Rows")].trim_end().ends_with(" 2"), "{text:#?}");
        // An option's description under it, as the prompt shows it.
        assert_eq!(row("one line a tab, as the sidebar"), row("Rows") + 1);
        row("BEFORE IT ASKED");
        row("Two layouts fit the board.");
        assert!(text[row("✕ 1 of 13")].contains('▎'));
        // The working card: this turn, then the call it is on and for how
        // long, then its chips, and its branch with no PR at its foot.
        assert!(text[working + 1].contains("working 2m"), "{text:#?}");
        row("THIS TURN");
        row("Wrote the summary module.");
        let now_line = row("cargo test --lib mission");
        assert!(text[now_line].contains("Bash") && text[now_line].contains("1m"));
        row("+10 −2");
        let foot = row("note/summaries");
        assert!(text[foot].contains("no PR"), "{text:#?}");
        // Its screen shows only once s asks for it.
        assert!(!text.iter().any(|l| l.contains("line 40")));
        m.card_screen = Some((TabId(2), 1));
        let shown = texts(&screen(&m, 160, 60).0);
        assert!(shown.iter().any(|l| l.contains("line 40")), "{shown:#?}");
        // Landing and Idle are the shelf, a line a tab under the cards.
        let landing = row("LANDING");
        assert!(landing > foot);
        assert!(text[landing + 1].contains("landing one"));
        assert!(
            text[landing + 1].contains("2nd in line to land"),
            "{text:#?}"
        );
        let idle = row("IDLE");
        assert!(text[idle + 1].contains("quiet one") && text[idle + 1].contains("2h"));
        assert!(
            text[idle + 1].contains("Fixed the flaky test."),
            "{text:#?}"
        );
        // Each card and line is a click away, the shelf's too.
        for i in 0..5 {
            assert!(hits.iter().any(|h| h.spot == Spot::Index(i)), "{i}");
        }
    }

    #[test]
    fn with_ux2_a_command_too_long_for_its_card_is_never_answered_from_it() {
        let mut m = board_3();
        let long: Vec<String> = (1..=12).map(|n| format!("echo step {n}")).collect();
        let ask = crate::agent::Wait {
            kind: crate::agent::WaitKind::Permission,
            tool: "Bash".into(),
            header: String::new(),
            words: long.join("\n"),
            options: Vec::new(),
            details: Vec::new(),
            questions: 0,
        };
        assert!(ask_fits(&m.snap.tabs[0], None));
        // An edit's path alone is less than what it would change.
        let edit = crate::agent::Wait {
            tool: "Edit".into(),
            words: "/work/src/lib.rs".into(),
            ..ask.clone()
        };
        let mut editing = m.snap.tabs[0].clone();
        editing.ask = Some(edit);
        assert!(!ask_fits(&editing, None));
        // A command keeps every space; one with a character a card can't
        // draw is never answered from it.
        let mut spaced = m.snap.tabs[0].clone();
        spaced.ask = Some(crate::agent::Wait {
            tool: "Bash".into(),
            words: "rm -rf 'old  backup'".into(),
            ..ask.clone()
        });
        assert!(ask_fits(&spaced, None));
        let mut shown = mission(vec![spaced.clone()], Vec::new());
        shown.snap.rules.ux2 = true;
        let text = texts(&screen(&shown, 160, 40).0);
        assert!(
            text.iter().any(|l| l.contains("rm -rf 'old  backup'")),
            "{text:#?}"
        );
        spaced.ask.as_mut().unwrap().words = "echo\tx".into();
        assert!(!ask_fits(&spaced, None));
        m.snap.tabs[0].ask = Some(ask);
        assert!(!ask_fits(&m.snap.tabs[0], None));
        let text = texts(&screen(&m, 160, 60).0);
        assert!(
            text.iter().any(|l| l.contains("the tab shows all of it")),
            "{text:#?}"
        );
        assert!(!text.iter().any(|l| l.contains("step 12")));
    }

    #[test]
    fn with_ux2_a_long_shelf_scrolls_to_its_chosen_line() {
        let tabs: Vec<TabModel> = (1..=40).map(|n| tab(n, &format!("idle {n}"))).collect();
        let mut m = mission(tabs, Vec::new());
        m.snap.rules.ux2 = true;
        m.cursor = 39;
        let (lines, hits) = screen(&m, 160, 20);
        let text = texts(&lines);
        assert!(text.iter().any(|l| l.contains("idle 40")), "{text:#?}");
        assert!(hits.iter().any(|h| h.spot == Spot::Index(39)));
        m.cursor = 0;
        let text = texts(&screen(&m, 160, 20).0);
        assert!(text.iter().any(|l| l.contains("IDLE")), "{text:#?}");
    }

    #[test]
    fn with_ux2_a_card_follows_the_question_its_screen_shows_and_its_options() {
        let mut m = board_3();
        m.cursor = 1;
        if let Some(a) = m.snap.tabs[0].ask.as_mut() {
            a.questions = 2;
        }
        m.snap.screens.insert(
            TabId(1),
            crate::mission::Screen {
                options: vec!["Soft".into(), "Vivid".into()],
                question: "Which colors?".into(),
                ..Default::default()
            },
        );
        let text = texts(&screen(&m, 160, 60).0);
        // The second question, as the screen shows it, with the options a
        // number answers there, never the first's.
        assert!(
            text.iter().any(|l| l.contains("Which colors?")),
            "{text:#?}"
        );
        assert!(text.iter().any(|l| l.contains("Vivid")));
        assert!(
            !text
                .iter()
                .any(|l| l.contains("Which layout") || l.contains("Rows"))
        );
    }

    #[test]
    fn with_ux2_a_waiting_card_s_options_have_no_numbers_or_clicks_of_their_own() {
        let m = board_3();
        let (lines, hits) = screen(&m, 160, 60);
        let text = texts(&lines);
        let rows = text.iter().find(|l| l.contains("Rows")).unwrap();
        assert!(!rows.contains(" 2 "), "{rows}");
        assert!(
            !hits.iter().any(|h| matches!(h.spot, Spot::Key(k)
                if matches!(k.code, crate::mux::input::KeyCode::Char(d) if d.is_ascii_digit()))),
            "Enter's menu answers it"
        );
    }

    #[test]
    fn with_ux2_a_tab_with_only_its_background_running_shows_its_last_turn() {
        let mut watching = tab(1, "watching");
        watching.state = TabState::Background;
        watching.background = 1;
        watching.summary = Some("Started the deploy watch.".into());
        let mut m = mission(vec![watching], Vec::new());
        m.snap.rules.ux2 = true;
        let text = texts(&screen(&m, 160, 40).0);
        assert!(text.iter().any(|l| l.contains("LAST TURN")), "{text:#?}");
        assert!(!text.iter().any(|l| l.contains("thinking")), "{text:#?}");
    }

    #[test]
    fn with_ux2_a_card_s_foot_meters_its_context_beside_its_branch() {
        let mut t = tab(1, "work 1");
        t.state = TabState::Working;
        t.context = Some(62);
        let mut m = mission(vec![t], Vec::new());
        m.snap.rules.ux2 = true;
        let text = texts(&screen(&m, 120, 40).0);
        assert!(
            text.iter().any(|l| l.contains("ctx ▰▰▰▰▰▰▱▱▱▱")),
            "{text:#?}"
        );
        m.snap.tabs[0].context = None;
        let text = texts(&screen(&m, 120, 40).0);
        assert!(!text.iter().any(|l| l.contains("ctx ")), "{text:#?}");
    }

    #[test]
    fn with_ux2_the_cards_fill_any_width_and_give_way_in_board_45_s_order() {
        let mut tabs: Vec<TabModel> = (1..=5)
            .map(|n| {
                let mut t = tab(n, &format!("work {n}"));
                t.state = TabState::Working;
                t.size = Some((3, 1));
                t.phase = Some("tests".into());
                t
            })
            .collect();
        let mut idle = tab(9, "quiet");
        idle.summary = Some("Answered the question about the layout.".into());
        tabs.push(idle);
        let mut m = mission(tabs, Vec::new());
        m.snap.rules.ux2 = true;
        m.cursor = 5;
        for (cols, across) in [(120, 2), (160, 3), (240, 4)] {
            let (lines, _) = screen(&m, cols, 80);
            let text = texts(&lines);
            let first = text.iter().position(|l| l.contains("work 1")).unwrap();
            // Each card open: its bar under its top rule, no corner of a box.
            let bars: Vec<usize> = text[first + 1]
                .char_indices()
                .filter(|(_, c)| *c == '▎')
                .map(|(i, _)| width(&text[first + 1][..i]))
                .collect();
            assert_eq!(bars.len(), across, "{cols}: {text:#?}");
            assert!(
                !text.iter().any(|l| l.contains(['╭', '╮', '╰', '╯'])),
                "{cols}: {text:#?}"
            );
            // Equal shares of the width: every card as wide, give or take
            // what rounding leaves.
            let edges: Vec<usize> = bars.windows(2).map(|w| w[1] - w[0]).collect();
            if let (Some(least), Some(most)) = (edges.iter().min(), edges.iter().max()) {
                assert!(most - least <= 1, "{cols}: {edges:?}");
            }
            let label = text.iter().position(|l| l.contains("WORKING")).unwrap();
            assert_eq!(width(&text[label]), cols - 1, "{cols}");
            assert!(text[label].ends_with('5'));
            // The chips under Now go below 160 columns; the shelf keeps its
            // summaries.
            let chips = text.iter().any(|l| l.contains("+3 −1"));
            assert_eq!(chips, cols >= 160, "{cols}: {text:#?}");
            assert!(
                text.iter()
                    .any(|l| l.contains("quiet") && l.contains("Answered")),
                "{cols}: {text:#?}"
            );
        }
        // Each row of cards a row the keys move through, each shelf line one.
        let g = grid_ux2(&m.snap.tabs, &m.snap.rules, 160, false);
        assert_eq!(g.rows, vec![vec![0, 1, 2], vec![3, 4], vec![5]]);
        // With room for one card, the shelf folds to its label until Enter
        // on it opens it.
        let g = grid_ux2(&m.snap.tabs, &m.snap.rules, 60, false);
        assert_eq!(g.rows.len(), 6);
        let (lines, _) = screen(&m, 60, 120);
        let text = texts(&lines);
        assert!(text.iter().any(|l| l.contains("IDLE")));
        assert!(!text.iter().any(|l| l.contains("quiet")), "{text:#?}");
        m.shelf_open = true;
        let text = texts(&screen(&m, 60, 120).0);
        assert!(text.iter().any(|l| l.contains("quiet")), "{text:#?}");
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

    #[test]
    fn the_reminders_view_draws_a_month_with_its_pane_and_the_months_ahead() {
        let now = rules().now_secs();
        let mut api = tab(1, "api");
        api.remind_at = Some(now + 3 * 86_400);
        api.remind_note = Some("pick the handoff back up".into());
        let mut web = tab(2, "web");
        web.remind_at = Some(now - 600);
        let mut far = tab(3, "far");
        far.remind_at = Some(now + 60 * 86_400);
        let tabs = vec![api, web, far];
        let items = crate::mission::reminders::items(&tabs, &rules());
        let view = list_view(ViewId::Reminders, "Reminders", items);
        let draw_at = |cols: usize, rows: usize| {
            let mut m = mission(tabs.clone(), vec![view.clone()]);
            m.snap.rules.ux2 = true;
            m.snap.all_tabs = tabs.clone();
            m.view = 1;
            m.size = (cols as u16, rows as u16);
            // Monday the 18th, api's day; today is Friday the 15th.
            m.reminders_day = Some(jiff::civil::date(2027, 1, 18));
            let (lines, hits, _, _) = drawn(&m, cols, rows);
            (lines, hits)
        };
        let draw = |cols: usize| draw_at(cols, 44);
        let (lines, hits) = draw(240);
        let text = texts(&lines).join("\n");
        assert!(text.contains("JANUARY 2027"), "{text}");
        assert!(text.contains("3 reminders · 1 came due"), "{text}");
        assert!(text.contains("1 api"), "{text}");
        assert!(text.contains("Monday, January 18"), "{text}");
        assert!(text.contains("in 3 days"), "{text}");
        assert!(text.contains("pick the handoff back up"), "{text}");
        assert!(text.contains("Go there, change or clear it"), "{text}");
        assert!(
            text.contains("THIS MONTH") && text.contains("LATER"),
            "{text}"
        );
        assert!(text.contains("due"), "{text}");
        assert!(text.contains("THE MONTHS AHEAD"), "{text}");
        assert!(text.contains("Jan 2") && text.contains("Mar 1"), "{text}");
        assert!(
            text.contains("› │"),
            "the › at the month's right edge: {text}"
        );
        let today_red = lines
            .iter()
            .flat_map(|l| &l.spans)
            .any(|s| s.content == "15" && s.style.fg == Some(Color::Indexed(196)));
        assert!(today_red, "today's number is red");
        assert!(hits.iter().any(|h| matches!(h.spot, Spot::Day(_))));
        assert!(hits.iter().any(|h| matches!(h.spot, Spot::Index(0))));

        let (lines, _) = draw(120);
        let text = texts(&lines).join("\n");
        assert!(text.contains("this month and later"), "{text}");
        assert!(!text.contains("LATER"), "the lists wait behind i: {text}");
        assert!(
            text.contains("▌1") && !text.contains("▌1 api"),
            "a narrow day leaves the name out: {text}"
        );
        assert!(text.contains("Monday, January 18"), "{text}");

        // A short screen keeps the chosen reminder's keys, showing its week.
        let (lines, _) = draw_at(120, 24);
        let text = texts(&lines).join("\n");
        assert!(
            text.contains("Go there") && text.contains("clear"),
            "{text}"
        );
        assert!(text.contains("▌1"), "the chosen day's week: {text}");
    }

    #[test]
    fn a_footer_key_with_tabs_focus_is_lit() {
        let mut m = mission(vec![tab(1, "one")], Vec::new());
        m.snap.rules.ux2 = true;
        let menu = crate::mux::input::Key::parse("m").unwrap();
        let lit = |m: &Mission| {
            let (lines, _) = screen(m, 120, 20);
            lines.last().unwrap().spans.iter().any(|s| {
                s.content.trim() == "m" && s.style.add_modifier.contains(Modifier::REVERSED)
            })
        };
        assert!(!lit(&m));
        m.focus = Some(Focus::Key(menu));
        assert!(lit(&m));
        // A click, with UX 2.0 off as on.
        for ux2 in [false, true] {
            m.snap.rules.ux2 = ux2;
            let (_, hits) = screen(&m, 120, 20);
            assert!(hits.iter().any(|h| h.spot == Spot::Key(menu)), "ux2 {ux2}");
        }
        // Each key of a group is a key of its own, symbols too.
        m.snap.rules.ux2 = false;
        m.snap.tab_focus = true;
        let (_, hits) = screen(&m, 120, 20);
        for c in ["[", "]"] {
            let k = crate::mux::input::Key::parse(c).unwrap();
            assert!(hits.iter().any(|h| h.spot == Spot::Key(k)), "{c}");
        }
        m.snap.tab_focus = false;
        // A filter being typed is no key, whatever its letters.
        m.filter = Some(crate::sidebar::render::Filter {
            text: "queue".into(),
            typing: true,
        });
        let (_, typed) = screen(&m, 120, 20);
        let q = crate::mux::input::Key::parse("q").unwrap();
        assert!(!typed.iter().any(|h| h.spot == Spot::Key(q)));
        m.filter = None;
        let (_, hits) = screen(&m, 120, 20);
        for c in ["h", "j", "k", "l"] {
            let k = crate::mux::input::Key::parse(c).unwrap();
            assert_eq!(
                hits.iter().filter(|h| h.spot == Spot::Key(k)).count(),
                1,
                "{c}"
            );
        }
        // A key cut off at the screen's edge is no click or Tab stop.
        m.snap.rules.ux2 = false;
        let esc = crate::mux::input::Key::parse("Escape").unwrap();
        let (_, hits) = screen(&m, 60, 20);
        assert!(!hits.iter().any(|h| h.spot == Spot::Key(esc)));
        assert!(hits.iter().any(|h| h.spot == Spot::Key(menu)));
    }

    #[test]
    fn every_views_footer_names_its_actions_and_the_views() {
        let has = |pairs: &[(String, String)], key: &str, what: &str| {
            pairs.iter().any(|(k, w)| k == key && w == what)
        };
        for ux2 in [false, true] {
            let mut m = mission(vec![tab(1, "one")], Vec::new());
            m.snap.rules.ux2 = ux2;
            let pairs = hints(&m, None, false);
            assert!(has(&pairs, "Enter", "actions") && has(&pairs, "m", "view actions"));
            for id in [
                ViewId::Closed,
                ViewId::Calendar,
                ViewId::Settings,
                ViewId::Keys,
                ViewId::Notifications,
                ViewId::Inbox,
                ViewId::Landing,
                ViewId::Resources,
                ViewId::Usage,
                ViewId::WorkMap,
                ViewId::Ci,
                ViewId::Prs,
                ViewId::Slack,
                ViewId::Reminders,
            ] {
                let mut m = mission(
                    vec![tab(1, "one")],
                    vec![list_view(id.clone(), "V", Vec::new())],
                );
                m.snap.rules.ux2 = ux2;
                m.view = 1;
                let pairs = hints(&m, m.current(), false);
                let enter = pairs
                    .iter()
                    .any(|(k, w)| k == "Enter" && w.starts_with("actions"))
                    || (id == ViewId::Settings && has(&pairs, "Enter", "its choices"));
                assert!(
                    enter
                        && pairs
                            .iter()
                            .any(|(k, w)| k == "m" && w.starts_with("view actions")),
                    "{id:?}: {pairs:?}"
                );
                for gone in [
                    "=", "o", "x", "e", "s", "w", "z", "t", "M-r", "y n m", "1-7", "g", "↵",
                ] {
                    assert!(
                        !pairs.iter().any(|(k, _)| k == gone),
                        "{id:?} names {gone}: {pairs:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_routines_view_draws_its_rows_week_and_pane_at_any_width() {
        use crate::routines::routine::{Outcome, Routine, Run};
        use crate::routines::schedule::{Days, Schedule};
        use std::collections::HashMap;
        let now_secs = rules().now_secs();
        let now = jiff::Timestamp::from_second(now_secs)
            .unwrap()
            .to_zoned(jiff::tz::TimeZone::UTC);
        let routine = |id: &str, name: &str, schedule| Routine {
            id: id.into(),
            name: name.into(),
            dir: "/work/api".into(),
            when: String::new(),
            schedule,
            tab_policy: crate::routines::TabPolicy::SameTab,
            when_busy: crate::routines::WhenBusy::Wait,
            when_missed: crate::routines::WhenMissed::OnWaking,
            prompt: "Read the new issues and sort them by area.".into(),
            model: String::new(),
            paused: false,
            session: "s1".into(),
            session_dir: "/work/api".into(),
            handled: now_secs - 60,
        };
        let triage = routine(
            "r1",
            "Morning triage",
            Schedule::Days {
                days: Days::ALL,
                at: 8 * 60 + 45,
            },
        );
        let once = routine(
            "r2",
            "Ship notes",
            Schedule::Once {
                at: now_secs + 7200,
            },
        );
        let runs = HashMap::from([(
            "r1".to_string(),
            vec![
                Run {
                    due: now_secs - 600,
                    ran: now_secs - 600,
                    tab: "9".into(),
                    secs: 95,
                    cost: 0.12,
                    outcome: Outcome::Ran,
                    words: "Sorted four new issues".into(),
                },
                Run {
                    due: now_secs - 300,
                    ran: 0,
                    tab: String::new(),
                    secs: 0,
                    cost: 0.0,
                    outcome: Outcome::Missed,
                    words: "asleep past 8:45 AM, so skipped".into(),
                },
            ],
        )]);
        let live = HashMap::new();
        let facts = crate::routines::view::Facts {
            now,
            tabs: vec![("r1".into(), Some(TabId(9)), "9".into())],
            live: &live,
            runs: &runs,
        };
        let items = crate::routines::view::items(&[triage, once], &facts);
        let view = list_view(ViewId::Routines, "Routines", items);
        let mut nine = tab(9, "Morning triage");
        nine.index = 9;
        let draw = |cols: usize, pane: bool| {
            let mut m = mission(vec![nine.clone()], vec![view.clone()]);
            m.snap.rules.ux2 = true;
            m.snap.all_tabs = vec![nine.clone()];
            m.view = 1;
            m.size = (cols as u16, 44);
            m.routines_pane = pane;
            let (lines, hits, _, _) = drawn(&m, cols, 44);
            (texts(&lines).join("\n"), hits)
        };
        let (text, hits) = draw(240, false);
        for want in [
            "RECURRING",
            "ONCE",
            "Morning triage",
            "Every day at 8:45 AM",
            "tab 9 · open",
            "THIS WEEK",
            "PROMPT",
            "added to Claude's system prompt",
            "Run now",
            "WHERE IT RUNS",
            "RUNS TODAY",
            "Sorted four new issues",
            "tab 9  1m  $0.12",
            "2 routines · next at",
            "– missed or busy, so skipped",
        ] {
            assert!(text.contains(want), "{want}: {text}");
        }
        assert!(
            text.lines()
                .any(|l| l.contains("AM  │") || l.contains("PM  │")),
            "when it runs next at the list's right edge: {text}"
        );
        assert!(hits.iter().any(|h| matches!(h.spot, Spot::Index(0))));
        assert!(hits.iter().any(|h| matches!(h.spot, Spot::Key(_))));

        assert!(
            hits.iter().any(|h| matches!(h.spot, Spot::Side(_))),
            "the pane scrolls"
        );

        // Many routines: the chosen one's rows stay in sight.
        let many: Vec<Routine> = (0..20)
            .map(|i| {
                routine(
                    &format!("m{i:02}"),
                    &format!("Routine {i:02}"),
                    Schedule::Days {
                        days: Days::ALL,
                        at: 9 * 60 + i,
                    },
                )
            })
            .collect();
        let items = crate::routines::view::items(&many, &facts);
        let mut m = mission(
            Vec::new(),
            vec![list_view(ViewId::Routines, "Routines", items)],
        );
        m.snap.rules.ux2 = true;
        m.view = 1;
        m.size = (240, 30);
        m.cursor = 19;
        let (lines, _, _, _) = drawn(&m, 240, 30);
        let text = texts(&lines).join("\n");
        assert!(text.contains("▌↻  Routine 19"), "{text}");
        assert!(
            text.contains("THIS WEEK"),
            "the last chosen shows the week: {text}"
        );

        // Under 160: each run's tab, then the rows' schedule go.
        let (text, _) = draw(150, false);
        assert!(
            text.contains("1m  $0.12") && !text.contains("tab 9  1m"),
            "{text}"
        );
        assert!(!text.contains("Every day at 8:45 AM  "), "{text}");
        assert!(text.contains("PROMPT"), "{text}");

        // Under 140 the pane is behind i.
        let (text, _) = draw(120, false);
        assert!(!text.contains("PROMPT"), "{text}");
        assert!(text.contains("i the chosen routine"), "{text}");
        let (text, _) = draw(120, true);
        assert!(
            text.contains("PROMPT") && text.contains("Run now"),
            "{text}"
        );
        assert!(!text.contains("THIS WEEK"), "{text}");
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
            t[1].trim_end().ends_with("M-[ ] groups  [ ] views"),
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

    fn work_views() -> Vec<View> {
        vec![
            list_view(ViewId::Landing, "Landing", vec![]),
            list_view(ViewId::Autopilot, "Autopilot", vec![]),
            list_view(ViewId::WorkMap, "Work map", vec![]),
            list_view(ViewId::Closed, "Closed", vec![]),
            list_view(ViewId::Sessions, "Sessions", vec![]),
        ]
    }

    #[test]
    fn with_ux2_a_narrow_bar_scrolls_its_views_row_to_tabs_focus() {
        let mut m = mission(vec![tab(1, "api")], work_views());
        m.snap.rules.ux2 = true;
        m.snap.tab_focus = true;
        let t = texts(&screen(&m, 48, 12).0);
        assert!(t[1].starts_with("  Tabs   Landing "), "{:?}", t[1]);
        assert!(!t[1].contains('‹'), "{:?}", t[1]);
        m.focus = Some(Focus::View(5));
        let (lines, hits) = screen(&m, 48, 12);
        let t = texts(&lines);
        assert!(t[1].starts_with(" ‹  Autopilot "), "{:?}", t[1]);
        assert!(t[1].contains(" Sessions "), "{:?}", t[1]);
        let last = hits.iter().find(|h| h.spot == Spot::View(5)).unwrap();
        assert!(last.x1 < 48, "{last:?}");
        assert_eq!(
            t[1].chars()
                .skip(last.x0 as usize)
                .take((last.x1 - last.x0 + 1) as usize)
                .collect::<String>(),
            " Sessions "
        );
        assert!(hits.iter().all(|h| h.spot != Spot::View(0)));
        m.focus = Some(Focus::Filter);
        let (lines, hits) = screen(&m, 48, 12);
        let t = texts(&lines);
        assert!(t[1].ends_with(" / filter"), "{:?}", t[1]);
        let filter = hits.iter().find(|h| h.spot == Spot::Filter).unwrap();
        assert!(filter.x1 < 48, "{filter:?}");
    }

    #[test]
    fn a_narrow_one_row_bar_scrolls_to_tabs_focus() {
        let mut m = mission(vec![tab(1, "api")], work_views());
        m.snap.tab_focus = true;
        let t = texts(&screen(&m, 60, 12).0);
        assert!(!t[0].contains('‹'), "{:?}", t[0]);
        m.focus = Some(Focus::View(5));
        let (lines, hits) = screen(&m, 60, 12);
        let t = texts(&lines);
        assert!(t[0].contains("‹ "), "{:?}", t[0]);
        let last = hits.iter().find(|h| h.spot == Spot::View(5)).unwrap();
        assert!(last.x1 < 60, "{last:?}");
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
            text[3].contains("IN REVIEW 2") && text[3].contains("APPROVED 1"),
            "each count right after its name: {}",
            text[3]
        );
        assert!(
            text[3].contains("DRAFT 0") && text[6].contains("nothing here"),
            "an empty column keeps its head and a dashed slot: {}",
            text[6]
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
            text[7].contains("│ settings view out"),
            "its second: {}",
            text[7]
        );
        assert!(text[8].contains("│ w · queue 1"), "{}", text[8]);
        assert!(text[9].contains("╰──"), "no footer: {}", text[9]);
        assert!(
            text[11].contains("╭ #2 ─") && text[11].contains(" ✕ ─╮"),
            "a row between cards: {}",
            text[11]
        );
        let edge = lines[11].spans.iter().find(|s| s.content == "╭").unwrap();
        assert_eq!(
            edge.style.fg,
            Some(Tone::Accent.fg(&Theme::DARK)),
            "the chosen card's edge"
        );
        let title = lines[12]
            .spans
            .iter()
            .find(|s| s.content.contains("PR 2"))
            .unwrap();
        assert!(title.style.add_modifier.contains(Modifier::BOLD));
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

        // On a narrower screen the pane goes under the board, so its
        // diagrams are still asked for.
        m.size = (120, 20);
        assert!(description_beside(&m).is_some());
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
        assert!(text[4].contains("│ tab 3 · "), "{}", text[4]);
        assert!(text[4].contains("│ ben"), "the reviewers: {}", text[4]);
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
        // Narrower, the pane is under the board, after a rule.
        assert!(!narrow[0].contains("#2 PR 2"), "{}", narrow[0]);
        let under = narrow
            .iter()
            .position(|l| l.starts_with("#2 PR 2"))
            .unwrap_or_else(|| panic!("{narrow:#?}"));
        assert!(narrow[under - 1].starts_with("───"), "{narrow:#?}");
        assert!(narrow[..under].iter().any(|l| l.contains("╭ #2")));
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
        assert_eq!(beside_w(160, PILOT_SIDE_AT, PILOT_SIDE_W), 0, "under it");
        assert_eq!(beside_w(200, PILOT_SIDE_AT, PILOT_SIDE_W), 55);
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
    fn with_ux2_the_pr_and_ticket_boards_draw_the_kit_s_cards_at_any_width() {
        let pr = Item::new("1", "#1 Fix the menu")
            .group("In review")
            .data(serde_json::json!({ "pr": { "number": 1 }, "checks": "success", "meta": "w" }));
        let ticket = Item::new("AB-1", "AB-1 Fix the menu")
            .group("Todo")
            .data(serde_json::json!({
                "ticket": "AB-1", "priority": 1, "title": "Fix the menu", "state": "Todo",
                "project": "Web app",
            }));
        for (id, title, item, columns, head, edge) in [
            (
                ViewId::Prs,
                "PRs",
                pr,
                crate::sources::prboard::columns(true),
                "IN REVIEW 1",
                "╭ #1 ─",
            ),
            (
                ViewId::Tickets,
                "Tickets",
                ticket,
                crate::sources::tickets::columns(true),
                "TODO 1",
                "╭ AB-1 ─",
            ),
        ] {
            let mut m = mission(Vec::new(), vec![list_view(id, title, vec![item])]);
            m.snap.rules.ux2 = true;
            m.view = 1;
            m.cursor = 0;
            for cols in [120, 160, 240] {
                let (mut lines, mut hits) = (Vec::new(), Vec::new());
                board_cards(
                    &m.snap.views[0],
                    columns,
                    cols,
                    20,
                    &m,
                    &Theme::DARK,
                    0,
                    &mut lines,
                    &mut hits,
                );
                let text = texts(&lines);
                // The same anatomy at every width: the head, a blank row,
                // the card's top edge, its title on one line, its fact
                // line, its bottom edge, and no open card's bar.
                assert!(text[0].contains(head), "{title} at {cols}: {text:#?}");
                assert!(
                    text[2].starts_with(edge) || text[2].contains(&format!(" {edge}")),
                    "{title} at {cols}: {text:#?}"
                );
                let x = text[2].find(edge).unwrap();
                let at = text[2][..x].chars().count();
                let rest = |row: usize| text[row].chars().skip(at).collect::<String>();
                assert!(
                    rest(3).starts_with("│ Fix the menu"),
                    "{title} at {cols}: {text:#?}"
                );
                assert!(rest(4).starts_with("│ "), "{title} at {cols}: {text:#?}");
                assert!(!rest(4).starts_with("│  "), "{title} at {cols}: {text:#?}");
                assert!(rest(5).starts_with("╰─"), "{title} at {cols}: {text:#?}");
                assert!(!text.iter().any(|l| l.contains('▎')), "{title} at {cols}");
                let edge = lines[2]
                    .spans
                    .iter()
                    .find(|s| s.content == "╭")
                    .unwrap_or_else(|| panic!("{title} at {cols}: {text:#?}"));
                assert_eq!(
                    edge.style.fg,
                    Some(Tone::Accent.fg(&Theme::DARK)),
                    "{title} at {cols}: the chosen card's edge"
                );
                // Under 140 columns the chosen card's pane is under the
                // board, beside it from there.
                let beside = text[0].contains(" │ ");
                assert_eq!(beside, cols >= SIDE_AT, "{title} at {cols}: {text:#?}");
            }
        }
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
        assert!(text[3].contains("nothing here"), "{}", text[3]);
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
            text[label + 4].contains("│ 1d ago · ana"),
            "{}",
            text[label + 4]
        );
        let edge = lines[label + 2]
            .spans
            .iter()
            .filter(|s| s.content == "╭")
            .nth(1)
            .unwrap();
        assert_eq!(
            edge.style.fg,
            Some(Tone::Accent.fg(&Theme::DARK)),
            "the chosen one in the row"
        );
        // On a screen under 140 columns the chosen one's pane is under the
        // row, after a rule.
        assert!(text[label + 6].starts_with("───"), "{text:#?}");
        assert!(text[label + 7].starts_with("#3 PR 3"), "{text:#?}");
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
        // 55 columns hold three of the five 16 wide. The first PR chosen:
        // the columns from the first, the two others counted at the right.
        m.cursor = 0;
        let (text, hits) = draw(&m);
        assert!(text[0].ends_with("2 more ›"), "{}", text[0]);
        assert!(!text[0].contains("‹"), "{}", text[0]);
        assert!(
            text[1].starts_with(&columns[0].to_uppercase()) && !text[1].contains("MERGE"),
            "{}",
            text[1]
        );
        let right = Key::new(KeyCode::Right, Mods::NONE);
        assert!(
            hits.iter()
                .any(|h| h.spot == Spot::Key(right) && h.y == 0 && usize::from(h.x1) == 54),
            "the marker is a click: {hits:?}"
        );
        // The last column's PR chosen: its column in sight with its
        // neighbors, its card drawn and clickable inside the board, the
        // rest counted at the left.
        m.cursor = 1;
        let (text, hits) = draw(&m);
        assert!(text[0].starts_with("‹ 2 more"), "{}", text[0]);
        assert!(!text[0].contains("›"), "{}", text[0]);
        assert!(text[1].contains("IN MERGE QUEUE 1"), "{}", text[1]);
        assert!(text.iter().any(|l| l.contains("╭ #2")), "{text:?}");
        assert!(
            hits.iter()
                .any(|h| h.spot == Spot::Index(1) && usize::from(h.x1) < 55),
            "{hits:?}"
        );
        let left = Key::new(KeyCode::Left, Mods::NONE);
        assert!(hits.iter().any(|h| h.spot == Spot::Key(left) && h.y == 0));
        // Lit under the mouse.
        m.hover = Some(Spot::Key(left));
        let (mut l, mut h) = (Vec::new(), Vec::new());
        board_cards(
            &m.snap.views[0],
            columns,
            55,
            20,
            &m,
            &Theme::DARK,
            0,
            &mut l,
            &mut h,
        );
        assert!(
            l[0].spans[0]
                .style
                .add_modifier
                .contains(Modifier::REVERSED)
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
        let menu = crate::mux::input::Key::parse("m").unwrap();
        assert!(
            hits.iter().any(|h| h.spot == Spot::Key(menu)),
            "the footer's m takes a click"
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
            started: None,
            ended: None,
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
    fn with_ux2_autopilot_times_each_card_until_it_lands() {
        let now = 1_700_100_000;
        let mut working = pilot_row(3, "Give notes stable IDs", TabState::Working);
        working.started = Some(now - 42 * 60);
        let mut landed = pilot_row(4, "Port the PR board", TabState::Idle);
        landed.left = true;
        landed.started = Some(now - 86_000);
        landed.ended = Some(now - 86_000 + 72 * 60);
        let list = crate::autopilot::view::items(
            &pilot_summary(),
            "",
            &[working, landed],
            &[],
            &[],
            &[],
            &std::collections::HashMap::new(),
        );
        let mut m = mission(
            Vec::new(),
            vec![list_view(ViewId::Autopilot, "Autopilot", list)],
        );
        m.snap.rules.ux2 = true;
        m.snap.now_secs = now;
        m.view = 1;
        let (lines, _, _, _) = drawn(&m, 300, 24);
        let text = texts(&lines).join("\n");
        assert!(
            text.contains("tab 3 ─") && text.contains("worked 42m"),
            "{text}"
        );
        assert!(
            text.contains("tab 4 ─") && text.contains("took 1h12m"),
            "{text}"
        );
        // A narrow card keeps its tab's number and shows the time alone.
        let (lines, _, _, _) = drawn(&m, 160, 24);
        let text = texts(&lines).join("\n");
        assert!(text.contains("tab 3 ─") && text.contains(" 42m "), "{text}");
        assert!(
            text.contains("tab 4 ─") && text.contains(" 1h12m "),
            "{text}"
        );
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
            &[],
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
        m.size = (200, 30);
        // A card whose item links no plan says so, and so does one whose
        // plan note isn't there; the heading says nothing of a plan.
        let beside = |m: &mut Mission, cursor: usize| {
            m.cursor = cursor as isize;
            texts(&drawn(m, 200, 30).0).join("\n")
        };
        let pane = beside(&mut m, no_plan);
        assert!(pane.contains("No plan"), "{pane}");
        assert!(description_beside(&m).is_none());
        let pane = beside(&mut m, plan_missing);
        assert!(pane.contains("Plan not found: Plan - Third"), "{pane}");
        let pane = beside(&mut m, 0);
        assert!(!pane.contains("No plan"), "{pane}");
        m.cursor = chosen as isize;
        let (lines, _, meters, _) = drawn(&m, 200, 30);
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
        m.size = (100, 30);
        assert!(description_beside(&m).is_none(), "too narrow for the board");
        m.size = (200, 30);
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
            assert!(heads.contains(&column.to_uppercase()), "{heads}");
        }
        assert!(
            heads.contains("NEXT UP 2") && heads.contains("LANDED 0"),
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
        assert!(heads.contains("SKIPPED 1"), "{heads}");
        let col = |l: &str, s: &str| l[..l.find(s).unwrap()].chars().count();
        let skipped_at = col(heads, "SKIPPED 1");
        let card = text.iter().position(|l| l.contains("Old idea")).unwrap();
        assert!(col(&text[card], "Old idea") >= skipped_at, "{}", text[card]);
        assert!(
            text[card..card + 3]
                .iter()
                .any(|l| l.contains("skipped by you")),
            "{board}"
        );
        assert_eq!(
            text.iter().filter(|l| l.contains("SKIPPED")).count(),
            1,
            "{board}"
        );
        // A meter for each card with stages, the landing one 6 of 8 done,
        // as its last line, and one for the spend: a quarter of the limit.
        let held = meters
            .iter()
            .find(|meter| meter.name == "autopilot/pilot:@3")
            .unwrap();
        assert_eq!(held.frac(), Some(6.0 / 8.0));
        let card = text.iter().position(|l| l.contains("▲ tab 3")).unwrap();
        let bottom = (card..text.len())
            .find(|&y| text[y].chars().nth(held.x as usize - 2) == Some('╰'))
            .unwrap();
        assert_eq!(held.y as usize, bottom - 1, "{text:#?}");
        let spend = meters
            .iter()
            .find(|meter| meter.name == "autopilot/spend")
            .unwrap();
        assert_eq!((spend.frac(), spend.y as usize), (Some(0.25), at));
        assert_eq!(meters.len(), 3);
        // At 160 columns the chosen card's pane is under the board, the
        // six columns taking the width.
        let (lines, _, _, _) = drawn(&m, 160, 40);
        let text = texts(&lines);
        let heads = text.iter().position(|l| l.contains("NEXT UP 2")).unwrap();
        assert!(!text[heads].contains('│'), "{}", text[heads]);
        let rule = (heads..text.len())
            .find(|&y| text[y].starts_with("────"))
            .unwrap_or_else(|| panic!("{text:#?}"));
        assert!(
            text[rule + 1].contains("Give notes stable IDs"),
            "{text:#?}"
        );
    }

    #[test]
    fn notifications_say_when_they_came_at_their_rows_end() {
        // 2027-01-15 08:00 UTC, a Friday.
        let now = rules().now_ms;
        let at = |ms: i64, id: &str, title: &str| {
            let mut i = Item::new(format!("prs\t{id}"), title);
            i.time = Some(ms);
            i
        };
        let items = vec![
            at(now - 18 * 60_000, "a", "Checks passed"),
            at(now - 4 * 86_400_000, "b", "Review asked"),
            at(now + 7 * 3_600_000, "c", "Standup"),
        ];
        let mut m = mission(
            Vec::new(),
            vec![list_view(ViewId::Notifications, "Notifications", items)],
        );
        m.view = 1;
        let text = texts(&screen(&m, 80, 12).0);
        let row = |title: &str| {
            text.iter()
                .find(|l| l.contains(title))
                .cloned()
                .unwrap_or_default()
        };
        assert!(row("Checks passed").ends_with(" 7:42 AM"), "{text:?}");
        assert!(row("Review asked").ends_with(" Mon 8:00 AM"), "{text:?}");
        assert!(row("Standup").ends_with(" Starts 3:00 PM"), "{text:?}");
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
            "All 3",
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
            "more, folded",
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
        // 22 rows under the bar: the chosen one's detail, then three
        // repositories of five and the landing before them.
        assert_eq!(shown, 4, "{text:#?}");
        assert!(text[..25].iter().any(|l| l.contains("landing-12")));
    }

    #[test]
    fn a_recorded_output_s_escape_sequences_are_shown_inert() {
        assert_eq!(
            printable("ok\u{1b}]52;c;aGk=\u{7}\tdone\u{9b}2J"),
            "ok]52;c;aGk= done2J"
        );
    }

    #[test]
    fn a_landing_s_record_puts_its_numbers_in_its_cells() {
        let now = 1_000_000_000;
        let cells = |record: serde_json::Value| {
            landing_stages(
                &serde_json::json!({ "holding": false, "place": 1, "record": record }),
                now,
            )
        };
        let none = cells(serde_json::Value::Null);
        assert_eq!(
            [&none[0].label, &none[1].label, &none[2].label],
            ["✓ passed", "✓ clean", "✓ none"],
            "without a record, the words of before"
        );
        let passed = cells(serde_json::json!({
            "tests": { "passed": 412, "failed": 0, "total": 412 },
            "review": { "round": 3, "reviewer": "codex", "clean_rounds": 2 },
            "leak": { "found": 0 },
        }));
        assert_eq!(
            [&passed[0].label, &passed[1].label, &passed[2].label],
            ["✓ 412", "✓ 2 clean", "✓ none"]
        );
        let failing = cells(serde_json::json!({
            "tests": { "passed": 399, "failed": 2, "total": 401, "file": "tests/it/mission.rs" },
            "review": { "round": 1, "reviewer": "codex", "at": now - 180_000 },
            "leak": { "found": 1 },
        }));
        assert_eq!(
            (
                failing[0].stage,
                failing[0].label.as_str(),
                failing[0].note.as_str()
            ),
            (Stage::Failed, "✕ 2 of 401", "tests/it/mission.rs")
        );
        assert_eq!(
            (
                failing[1].stage,
                failing[1].label.as_str(),
                failing[1].note.as_str()
            ),
            (Stage::Running, "round 1", "codex · 3m")
        );
        assert_eq!(
            (failing[2].stage, failing[2].label.as_str()),
            (Stage::Failed, "✕ found")
        );
        let again = landing_stages(
            &serde_json::json!({ "holding": true, "phase": "reviewing again", "record": {
                "review": { "round": 2, "reviewer": "codex", "clean_rounds": 0, "at": now },
            } }),
            now,
        );
        assert_eq!(
            (
                again[1].stage,
                again[1].label.as_str(),
                again[1].note.as_str()
            ),
            (Stage::Running, "round 2", "codex · <1m"),
            "a review under way keeps its round"
        );
        let unrecorded = landing_stages(
            &serde_json::json!({ "holding": true, "phase": "reviewing" }),
            now,
        );
        assert_eq!(unrecorded[1].label, "reviewing…");
        let uncounted = cells(serde_json::json!({
            "tests": { "ok": false, "file": "Tests (macOS arm64)" },
        }));
        assert_eq!(
            (
                uncounted[0].stage,
                uncounted[0].label.as_str(),
                uncounted[0].note.as_str()
            ),
            (Stage::Failed, "✕ failed", "Tests (macOS arm64)"),
            "a CI failure the log gave no counts for"
        );
    }

    /// A Landing view of one repository: a holder testing with a record,
    /// two waiters, and the holder chosen.
    fn landing_board() -> Mission {
        let tail: Vec<String> = (1..=30).map(|n| format!("test line {n} ... ok")).collect();
        let holder = Item::new("land:/w/a", "1. note-stable-ids")
            .detail("landing · pushing · tab 3")
            .data(serde_json::json!({ "landing": {
                "root": "/w/note-stable-ids", "tab": "@3", "holding": true, "phase": "pushing",
                "output": "/w/.git/landing-output.log",
                "record": {
                    "tests": { "passed": 412, "failed": 0, "total": 412 },
                    "review": { "round": 2, "reviewer": "codex", "clean_rounds": 2,
                                "summary": "No issues left." },
                    "found": [{ "round": 1, "title": "The lock leaks on Esc", "fixed_by": "abc1234" }],
                    "output": { "of": "review", "tail": tail },
                },
            } }));
        let waiter = |n: &str, place: u64, first: bool, last: bool| {
            Item::new(format!("land:/w/{n}"), format!("{}. {n}", place + 1))
                .detail("waiting · tab 4")
                .data(serde_json::json!({ "landing": {
                    "root": format!("/w/{n}"), "holding": false, "place": place,
                    "first": first, "last": last,
                    "holder": { "name": "note-stable-ids", "phase": "pushing" },
                } }))
        };
        let mut heading = views::header("/r:direct", "switchboard · straight on main", 3);
        heading.data["right"] = "3 landing · 1 holds the lock".into();
        let items = vec![
            heading,
            holder,
            waiter("note-benchmark", 1, true, false),
            waiter("note-tips", 2, false, true),
        ];
        let mut m = mission(
            Vec::new(),
            vec![list_view(ViewId::Landing, "Landing", items)],
        );
        m.snap.rules.ux2 = true;
        m.view = 1;
        m.cursor = 1;
        m
    }

    #[test]
    fn with_ux2_landing_s_columns_fill_any_width_and_narrow_in_board_45_s_order() {
        let m = landing_board();
        // The last tinted column of a row: the cells end at the right edge.
        let tinted_end = |line: &Line| {
            line.spans
                .iter()
                .fold((0, 0), |(x, end), s| {
                    let next = x + width(&s.content);
                    (
                        next,
                        if s.style.bg.is_some() && s.style.bg != Some(Theme::DARK.highlight) {
                            next
                        } else {
                            end
                        },
                    )
                })
                .1
        };
        let row_of =
            |text: &[String], what: &str| text.iter().position(|l| l.contains(what)).unwrap();
        for cols in [120, 160, 240] {
            let (lines, _) = screen(&m, cols, 40);
            let text = texts(&lines);
            let heading = &text[row_of(&text, "straight on main")];
            assert!(
                heading.trim_end().ends_with("3 landing · 1 holds the lock")
                    && width(heading.trim_end()) == cols - 1,
                "at {cols}, the counts at the right edge: {heading}"
            );
            let waiter = row_of(&text, "note-tips");
            assert_eq!(
                tinted_end(&lines[waiter]),
                cols - 1,
                "at {cols}: {}",
                text[waiter]
            );
        }
        // From 160 the cells are the board's 13 wide and the worktree
        // stretches; at 120 they narrow to their words first.
        let cell_w = |cols: usize| {
            let (lines, _) = screen(&m, cols, 40);
            let text = texts(&lines);
            let row = &lines[row_of(&text, "note-tips")];
            row.spans
                .iter()
                .filter(|s| s.content.contains("waiting"))
                .map(|s| width(&s.content))
                .next()
                .unwrap()
        };
        assert_eq!(cell_w(240), CELL_MAX - 2);
        assert_eq!(cell_w(160), CELL_MAX - 2);
        assert!(cell_w(120) < CELL_MAX - 2, "{}", cell_w(120));
        // Narrower still, a waiting cell is ◌ alone, then the worktree
        // column is 20 wide; narrower than that is the list.
        let (lines, _) = screen(&m, 84, 40);
        let text = texts(&lines);
        assert!(text[row_of(&text, "note-tips")].contains("◌"), "{text:#?}");
        let (lines, _) = screen(&m, 60, 40);
        assert!(!texts(&lines).iter().any(|l| l.contains("leak check")));
    }

    #[test]
    fn with_ux2_the_chosen_landing_s_detail_says_what_it_waits_on_and_its_last_review() {
        let mut m = landing_board();
        let (lines, hits) = screen(&m, 160, 50);
        let text = texts(&lines).join("\n");
        for words in [
            "waits on",
            "nothing: it holds the lock",
            "applies what landed, then cleans up",
            "/w/note-stable-ids",
            "LAST REVIEW",
            "round 2 · codex",
            "No issues left.",
            "round 1 found The lock leaks on Esc · fixed by abc1234",
            "test line 30 ... ok",
            "go to its tab, move it in the queue",
            "✓ 412",
            "✓ 2 clean",
        ] {
            assert!(text.contains(words), "{words:?} in {text}");
        }
        let o = crate::mux::input::Key::parse("Enter").unwrap();
        assert!(hits.iter().any(|h| h.spot == Spot::Key(o)));
        // Under the mouse, or with Tab's focus on it, a keycap is lit.
        let lit = |m: &Mission| {
            screen(m, 160, 50)
                .0
                .iter()
                .flat_map(|l| l.spans.clone())
                .any(|s| {
                    s.content.contains(" go to its tab")
                        && s.style.add_modifier.contains(Modifier::BOLD)
                })
        };
        assert!(!lit(&m));
        m.hover = Some(Spot::Key(o));
        assert!(lit(&m));
        m.hover = None;
        m.focus = Some(Focus::Key(o));
        assert!(lit(&m));
        m.focus = None;
        // A waiter waits on the one holding the lock.
        m.cursor = 2;
        let text = texts(&screen(&m, 160, 50).0).join("\n");
        assert!(
            text.contains("note-stable-ids, which holds the lock · pushing"),
            "{text}"
        );
        assert!(
            !text.contains("down the queue"),
            "J and K are no keys: {text}"
        );
        // Under 140 columns the output is behind i, shown over the list.
        m.cursor = 1;
        let text = texts(&screen(&m, 130, 50).0).join("\n");
        assert!(
            !text.contains("test line 30") && text.contains("i  its last output here"),
            "{text}"
        );
        m.landing_output = true;
        let text = texts(&screen(&m, 130, 50).0).join("\n");
        assert!(
            text.contains("test line 30 ... ok") && !text.contains("leak check"),
            "{text}"
        );
        // Output from tests run after the review is named for them.
        let mut t = landing_board();
        let landing = t
            .snap
            .views
            .iter_mut()
            .find(|v| v.id == ViewId::Landing)
            .unwrap();
        landing.data.items[1].data["landing"]["record"]["output"]["of"] = "tests".into();
        let text = texts(&screen(&t, 160, 50).0).join("\n");
        assert!(
            text.contains("LAST TESTS")
                && text.contains("412 passed")
                && !text.contains("LAST REVIEW"),
            "{text}"
        );
        // Narrow enough for the plain list, it still shows over it.
        let text = texts(&screen(&m, 70, 50).0).join("\n");
        assert!(text.contains("test line 30 ... ok"), "{text}");
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
