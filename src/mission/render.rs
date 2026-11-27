//! Mission control's screen (lib/sidebar/render.js's `missionLines`,
//! `missionGrid`, `card` and `boardLines`): a plain function from what a
//! client's mission control holds to the lines to draw, each exactly the
//! screen's width, and where a click on each part lands, so the tests can
//! read it as text.
//!
//! At the top a bar naming the views (the tabs, then each view, with how
//! many items each has), the counts of tabs waiting, finished and working,
//! and the filter; two rows and a rule (`grouped_bar`), the
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
//! - the Autopilot view as a board of cards (`pilot_board`),
//!   each with a meter placed as an image over the lines (`Image`);
//! - a table (Resources): its columns' names, its rows, and a totals row;
//!   with room, the board `resources_board` draws instead;
//! - any other view, one row per item, a heading over each group, Settings'
//!   rows as name, value and what the option does; with room,
//!   Settings as the board `settings_board` draws.

use std::sync::LazyLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use regex::Regex;

use super::views::{self, View, ViewId};
use super::{Focus, HitBox, Mission, Spot};
use crate::items::{Item, State};
use crate::mux::input::{Key, KeyCode, Mods};
use crate::repos::Way;
use crate::sidebar::model::{self, Rules, TabModel, TabState};
use crate::sidebar::render::Line;
use crate::sidebar::text::{self, Badge, width};
use crate::ui::Theme;
use crate::ui::kit::{self, Col, Mark, Tone};
use crate::ui::when::{self, Said};
use crate::views::markdown::{Images, PictureRow};

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
        text::spans_width(&self.spans)
    }

    /// The spans cut to `cols` (an ellipsis in place of what didn't fit) and
    /// padded with spaces in `pad` to fill them.
    fn fitted(self, cols: usize, pad: Style) -> Vec<Span<'static>> {
        text::fit_spans(self.spans, cols, pad)
    }

    fn line(self, cols: usize, fill: Option<Color>) -> Line {
        Line {
            spans: self.fitted(cols, Style::new()),
            fill,
        }
    }
}

/// An item's mark: needing you yellow, under way blue, the rest faint.
fn item_mark(item: &Item, theme: &Theme) -> Span<'static> {
    match item.state {
        State::Attention => Span::styled("●", bold(YELLOW)),
        State::Active => Span::styled("●", bold(Color::Indexed(67))),
        State::Idle => Span::styled("○", fg(theme.faint)),
    }
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

// ── UX 2.0's Tabs: cards grouped as the sidebar, then the shelf (board 3) ──

/// A card's least width, and the most rows one takes: each is
/// as tall as what it holds, up to this. A request counts as shown whole
/// (`ask_fits`) only when its kind, its words, a row clear and its options
/// with their descriptions fit the rows this leaves inside (`ASK_ROWS`):
/// less the card's edges, the row clear under the top one, the foot and
/// the row over it.
pub const CARD_W: usize = 40;
const CARD_H: usize = 20;
/// The columns between two cards side by side.
const CARD_GAP: usize = 2;
/// The most cards side by side, however wide the screen.
const MOST_ACROSS: usize = 4;
/// The longest name a shelf line shows.
const SHELF_NAME: usize = 26;

/// The group of cards a stage's tabs are drawn in (board 3): Needs you
/// (a finished tab's too), and Working, which the stages of work under way
/// without you (a monitor, a review loop, a CI loop, queue-and-clean) are
/// part of here; `None` for a stage whose tabs are lines on the shelf
/// under the cards.
fn card_group(stage: model::Stage) -> Option<model::Stage> {
    use model::Stage;
    match stage {
        Stage::Needs => Some(Stage::Needs),
        Stage::Working | Stage::Monitoring | Stage::Review | Stage::Ci | Stage::Queue => {
            Some(Stage::Working)
        }
        _ => None,
    }
}

/// Whether a stage's tabs are lines on the shelf under the cards rather
/// than cards.
fn on_shelf(stage: model::Stage) -> bool {
    card_group(stage).is_none()
}

/// The group of cards a tab in `stage` is drawn in: its stage's, and
/// Working for one whose Claude is at work while its build fails, which
/// the sidebar keeps under Build failing; `None` for a line on the shelf.
/// A tab landing, queued, testing a deploy or merged is a line there
/// whatever its Claude is doing, as board 3 draws a landing one.
fn card_group_of(tab: &TabModel, stage: model::Stage) -> Option<model::Stage> {
    card_group(stage).or_else(|| {
        (stage == model::Stage::Failing && tab.state.busy()).then_some(model::Stage::Working)
    })
}

/// Whether a tab is a line on the shelf rather than a card.
pub fn on_shelf_tab(tab: &TabModel, r: &Rules) -> bool {
    card_group_of(tab, model::stage_of(tab, r)).is_none()
}

/// The shelf's stages in the board's order: where a tab's PR or landing
/// stands, then the loops and reminders, then the idle tabs and those with
/// no Claude, then other repositories'. A stage not listed here follows
/// them (`tab_groups`), so no tab is left out of the view.
const SHELF: [model::Stage; 11] = [
    model::Stage::Failing,
    model::Stage::Landing,
    model::Stage::Queued,
    model::Stage::Deploy,
    model::Stage::Merged,
    model::Stage::Scheduled,
    model::Stage::Later,
    model::Stage::Idle,
    model::Stage::Other,
    model::Stage::RemindersElsewhere,
    model::Stage::OtherRepos,
];

/// The column the cards start at, under their group's label.
const CARD_X: usize = 1;

/// How many cards go side by side on a screen `cols` wide: as many as fit
/// at their least width after the column they start at, at most
/// `MOST_ACROSS`.
pub fn across(cols: usize) -> usize {
    ((cols.saturating_sub(CARD_X) + CARD_GAP) / (CARD_W + CARD_GAP)).clamp(1, MOST_ACROSS)
}

/// The card columns' x and width, sharing the screen's width equally from
/// `x0`: the column under their label's first letter (`CARD_X`) to the
/// screen's edge, or, from 0, to the column before the last, which a
/// scroll bar takes.
fn card_columns(cols: usize, across: usize, x0: usize) -> Vec<(usize, usize)> {
    let room = cols.saturating_sub(CARD_X);
    let each = Col::Stretch {
        weight: 1,
        min: CARD_W.min(room),
        max: None,
    };
    kit::columns(&vec![each; across], room, CARD_GAP, &[])
        .into_iter()
        .flatten()
        .map(|(x, w)| (x + x0, w))
        .collect()
}

/// The tabs' groups as UX 2.0 draws them: the groups of cards (Needs you,
/// then Working, each stage in it in the sidebar's order), then the
/// shelf's stages, each with its tabs as indices into `tabs`.
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
    let mut groups: Vec<(model::Stage, Vec<usize>)> = Vec::new();
    for group in [model::Stage::Needs, model::Stage::Working] {
        let mut at: Vec<usize> = staged
            .iter()
            .flat_map(|(s, at)| {
                at.iter()
                    .copied()
                    .filter(move |&i| card_group_of(&tabs[i], *s) == Some(group))
            })
            .collect();
        // Working's stages as one group, by the tabs' numbers, as each
        // of them orders its own.
        if group == model::Stage::Working {
            at.sort_by_key(|&i| (tabs[i].index, tabs[i].place));
        }
        if !at.is_empty() {
            groups.push((group, at));
        }
    }
    // The shelf: each stage's tabs that are no card, the stages the list
    // names first.
    let shelved = |stage: model::Stage, at: &[usize]| -> Option<(model::Stage, Vec<usize>)> {
        let left: Vec<usize> = at
            .iter()
            .copied()
            .filter(|&i| card_group_of(&tabs[i], stage).is_none())
            .collect();
        (!left.is_empty()).then_some((stage, left))
    };
    for stage in SHELF {
        groups.extend(
            staged
                .iter()
                .filter(|(s, _)| *s == stage)
                .filter_map(|(s, at)| shelved(*s, at)),
        );
    }
    groups.extend(
        staged
            .iter()
            .filter(|(s, _)| !SHELF.contains(s))
            .filter_map(|(s, at)| shelved(*s, at)),
    );
    groups
}

/// The Tabs view's grid: its groups and shelf.
pub fn tabs_grid(m: &Mission, cols: usize) -> Grid {
    grid(
        &m.snap.tabs,
        &m.snap.rules,
        cols,
        m.shelf_open,
        repos_folded(m),
    )
}

/// Whether the shelf shows only its labels: with room for one column of
/// cards, until Enter on a label opens it.
fn shelf_folded(cols: usize, open: bool) -> bool {
    across(cols) == 1 && !open
}

/// Whether Other repos, the tabs the sidebar's repository filter leaves
/// out, shows only its label: until Enter or a click opens it, or a /
/// search, whose matches are all reachable.
pub fn repos_folded(m: &Mission) -> bool {
    !m.other_repos_open
        && m.filter
            .as_ref()
            .is_none_or(|f| f.typed().trim().is_empty())
}

/// Whether a stage shows only its label, its first tab standing for it:
/// the shelf's on a narrow screen (`shelf_folded`), and Other repos while
/// `repos_folded`.
fn stage_folded(stage: model::Stage, shelf: bool, repos: bool) -> bool {
    on_shelf(stage) && shelf || stage == model::Stage::OtherRepos && repos
}

/// UX 2.0's Tabs laid out: each group of cards in rows of `across`, then
/// each shelf line a row of its own; a folded group is one row, its first
/// tab standing for its label.
pub fn grid(tabs: &[TabModel], r: &Rules, cols: usize, open: bool, repos_folded: bool) -> Grid {
    let n = across(cols);
    let folded = shelf_folded(cols, open);
    let mut order = Vec::new();
    let mut rows = Vec::new();
    for (stage, at) in tab_groups(tabs, r) {
        let shown: &[usize] = if stage_folded(stage, folded, repos_folded) {
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
    let ago = when::ago(tab.since, r.now_secs(), Said::Short);
    if ago.is_empty() {
        words.to_string()
    } else {
        format!("{words} {ago}")
    }
}

/// A tab's PR's checks as the board writes them, in their mark's tone: all
/// passed, how many of them have run, or how many failed; "no PR" dim
/// without one. Merged, it says so with Merged's mark, since a merged tab
/// at work again wears its work's mark in its title.
fn checks_words(tab: &TabModel, theme: &Theme) -> Vec<Span<'static>> {
    if tab.merge == Some(model::MergeState::Merged) {
        let words = match (&tab.landed_on, tab.pr) {
            (Some((hash, _)), _) => format!("landed {hash}"),
            (None, Some(n)) => format!("#{n} merged"),
            (None, None) => "merged".into(),
        };
        let (glyph, tone) = Mark::of_badge(Badge::Merged).look();
        return vec![Span::styled(format!("{glyph} {words}"), tone.style(theme))];
    }
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
    vec![Span::styled(format!("{glyph} {words}"), tone.style(theme))]
}

/// A card's foot, as board 3 draws it: its branch dim, whole where it
/// fits, then its checks two columns after it, and how long the tab has
/// been open dim at the right edge; the age goes first where the card is
/// too narrow, then the branch is cut, never the checks.
fn card_foot(
    tab: &TabModel,
    branch: Option<&str>,
    inner: usize,
    now_secs: i64,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let checks = checks_words(tab, theme);
    let checks_w = text::spans_width(&checks);
    let branch = branch.unwrap_or_default();
    let lead = if branch.is_empty() { 0 } else { 2 };
    let room = inner.saturating_sub(checks_w + lead).max(1);
    let mut line = Vec::new();
    if !branch.is_empty() {
        line.push(Span::styled(
            text::cut(branch, room),
            Tone::Dim.style(theme),
        ));
        line.push(Span::raw("  "));
    }
    line.extend(checks);
    let opened = tab
        .opened
        .map(|at| format!("opened {}", text::span(now_secs - at)))
        .unwrap_or_default();
    let used = text::spans_width(&line);
    if !opened.is_empty() && used + 2 + width(&opened) <= inner {
        line.push(Span::raw(" ".repeat(inner - used - width(&opened))));
        line.push(Span::styled(opened, Tone::Dim.style(theme)));
    }
    line
}

/// A tab's name on its card: with several PRs, the current one's number
/// before it (board 128).
fn card_name(tab: &TabModel) -> String {
    match tab.prs.first().filter(|p| p.current) {
        Some(current) => format!("#{} {}", current.number, tab.name),
        None => tab.name.clone(),
    }
}

/// The PRs a card lists (board 128): a blank line, their label with their
/// count, then a row each, the current one first and marked `›`, its
/// repository when not the tab's, where it stands at the right in its
/// mark's tone. None with one PR or none.
fn pr_lines(tab: &TabModel, inner: usize, theme: &Theme) -> Vec<Vec<Span<'static>>> {
    if tab.prs.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![Vec::new()];
    let count = tab.prs.len().to_string();
    let mut label = card_label("PRs", Tone::Dim, theme);
    let used = text::spans_width(&label);
    label.push(Span::raw(
        " ".repeat(inner.saturating_sub(used + width(&count))),
    ));
    label.push(Span::styled(count, Tone::Dim.style(theme)));
    lines.push(label);
    for pr in &tab.prs {
        let (glyph, tone) = pr.mark.look();
        let right = if pr.standing.is_empty() {
            String::new()
        } else {
            format!("{glyph} {}", pr.standing)
        };
        let mut row = vec![
            Span::styled(if pr.current { "›" } else { " " }, Tone::Needs.bold(theme)),
            Span::styled(
                format!(" #{}", pr.number),
                if pr.current {
                    Tone::Accent.bold(theme)
                } else {
                    Tone::Accent.style(theme)
                },
            ),
        ];
        if pr.elsewhere {
            let name = pr.repo.rsplit('/').next().unwrap_or(&pr.repo).to_string();
            row.push(Span::styled(format!(" {name}"), Tone::Dim.style(theme)));
        }
        let room = inner.saturating_sub(width(&right) + 1);
        let mut row = text::fit_spans(row, room, Style::new());
        row.push(Span::raw(" "));
        row.push(Span::styled(right, tone.style(theme)));
        lines.push(row);
    }
    lines
}

/// A card's foot for a tab following several PRs: its branch, and how many
/// PRs in place of its one PR's checks (board 128).
fn card_foot_prs(
    tab: &TabModel,
    branch: Option<&str>,
    inner: usize,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let count = format!("{} PRs", tab.prs.len());
    let branch = branch.unwrap_or_default();
    let mut line = Vec::new();
    if !branch.is_empty() {
        let room = inner.saturating_sub(width(&count) + 2).max(1);
        line.push(Span::styled(
            text::cut(branch, room),
            Tone::Dim.style(theme),
        ));
        line.push(Span::raw("  "));
    }
    line.push(Span::styled(count, Tone::Dim.style(theme)));
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
    /// Whether its last line is a row of chips, which the foot follows
    /// with no row clear between them.
    tight: bool,
}

/// Where a card is and how it is drawn.
struct CardLook<'a> {
    w: usize,
    /// The most rows it may take.
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

/// The rows inside a card at its tallest, which a request shown whole
/// fits in.
const ASK_ROWS: usize = CARD_H - 5;

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
/// least width, else it says the tab has all of it: a request known whole
/// (`ask_whole`) that takes no more than the rows a card has (`ASK_ROWS`).
pub fn ask_fits(tab: &TabModel, screen: Option<&super::Screen>) -> bool {
    ask_sized(tab, screen, true)
}

/// Whether what a waiting tab asks is known whole, whatever a card has
/// room for: what a menu opened away from a card answers from (the
/// sidebar's, a reminder's), which shows the
/// request itself.
pub fn ask_whole(tab: &TabModel, screen: Option<&super::Screen>) -> bool {
    ask_sized(tab, screen, false)
}

/// `ask_fits` with `on_card`, else `ask_whole`: a question asking one
/// thing, its options' descriptions short enough to show, or a Bash
/// command, which its words are whole; on a card, all of it within the
/// card's rows too. Any other permission (an edit's change, a connector's
/// input), or a call of several questions (whose later options'
/// descriptions the hook doesn't give), is more than is kept.
fn ask_sized(tab: &TabModel, screen: Option<&super::Screen>, on_card: bool) -> bool {
    let whole = tab.ask.as_ref().is_some_and(|a| match a.kind {
        crate::agent::WaitKind::Question => a.questions <= 1,
        crate::agent::WaitKind::Permission => a.tool == "Bash" && !a.words.is_empty(),
    });
    let (_, words, options) = ask_words(tab, screen);
    // A description's rows as the card lays them out, a blank one between
    // its paragraphs counted, since the card takes its first
    // `DETAIL_LINES` whatever they hold; none for an option without one.
    let detail_rows = |d: &str| {
        if d.trim().is_empty() {
            0
        } else {
            text::wrap(d, CARD_W - 4 - 3).len()
        }
    };
    let details_fit = options
        .iter()
        .all(|(_, d, _)| detail_rows(d) <= DETAIL_LINES);
    // Its options under a row clear, each with its description's rows.
    let option_rows = if options.is_empty() {
        0
    } else {
        1 + options
            .iter()
            .map(|(_, d, _)| 1 + detail_rows(d))
            .sum::<usize>()
    };
    whole
        && details_fit
        && ask_lines(tab, &words, CARD_W - 4).is_some_and(|l| {
            l.len() <= ASK_LINES && (!on_card || 1 + l.len() + option_rows <= ASK_ROWS)
        })
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
    // Only with room for its label and a line of it under what it asks.
    if !before.is_empty() && lines.len() + 3 <= rows {
        lines.push(Vec::new());
        lines.push(card_label("Before it asked", Tone::Dim, theme));
        let room = rows.saturating_sub(lines.len()).clamp(1, 5);
        lines.extend(wrapped(&before, inner, room, Tone::Dim.style(theme)));
    }
    Inside {
        lines,
        tight: false,
    }
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
    // Waiting for a slot of the queue for heavy work, its mark is the
    // slot's and its time how long it has waited (board 79).
    let waiting = tab.slot.as_ref().filter(|s| s.waiting());
    let since = waiting.map(|s| s.since).or(since);
    let how_long = since.map_or_else(String::new, |s| {
        when::ago(Some(s), r.now_secs(), Said::Short)
    });
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
        vec![kit::mark(
            if waiting.is_some() {
                Mark::Slot
            } else {
                Mark::Working
            },
            0,
            theme,
        )],
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
    let mut tight = false;
    if look.chips {
        let mut chips: Vec<Span<'static>> = Vec::new();
        // The slot it waits for, with its place, or the one it holds, dim.
        if let Some(slot) = &tab.slot {
            let tone = if slot.waiting() {
                Tone::Working
            } else {
                Tone::Dim
            };
            chips.extend(kit::chip(&slot.chip(), tone, false, theme));
        }
        if let Some(way) = tab.way {
            chips.extend(kit::way_chip(way.chip(), false, theme));
        }
        if let Some(phase) = tab.phase.as_deref().filter(|p| !p.is_empty()) {
            chips.extend(kit::chip(phase, Tone::Working, false, theme));
        }
        if tab.round > 0 {
            chips.extend(kit::chip(
                &crate::sidebar::model::round_words(tab, tab.round),
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
            tight = true;
        }
    }
    Inside { lines, tight }
}

/// A card neither waiting nor working: what its last turn did, and its
/// state in words.
fn last_inside(tab: &TabModel, look: &CardLook, r: &Rules, theme: &Theme) -> Inside {
    let inner = look.w.saturating_sub(4);
    // A finished tab's in the tone of its mark, until you look at it.
    let tone = if tab.state == TabState::Done {
        Tone::Done
    } else {
        Tone::Dim
    };
    let mut lines = vec![card_label("Last turn", tone, theme)];
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
    Inside {
        lines,
        tight: false,
    }
}

/// What a tab's card holds, before it is drawn at its row's
/// height: its top edge's words, the lines inside it, and its foot.
struct Card {
    title: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    lines: Vec<Vec<Span<'static>>>,
    /// Whether the foot follows the lines with no row clear between.
    tight: bool,
    foot: Vec<Span<'static>>,
    /// Its edge's color, or none for the chosen card's accent bar.
    edge: Option<Color>,
    w: usize,
    theme: Theme,
}

impl Card {
    /// The rows it takes: its edges, a row clear under the top one, its
    /// lines, a row clear over the foot unless they end in chips, and the
    /// foot.
    fn height(&self) -> usize {
        4 + self.lines.len() + usize::from(!self.tight)
    }

    /// The card `rows` tall, as the kit's card: the foot on its last row
    /// inside, whatever its lines leave clear over it.
    fn drawn(self, rows: usize) -> Vec<Vec<Span<'static>>> {
        let inside_rows = rows.saturating_sub(2);
        let mut inside = vec![Vec::new()];
        inside.extend(self.lines);
        inside.truncate(inside_rows.saturating_sub(1));
        inside.resize(inside_rows.saturating_sub(1), Vec::new());
        inside.push(self.foot);
        match self.edge {
            Some(edge) => kit::card(self.w, rows, self.title, self.right, &inside, edge),
            None => kit::chosen_card(self.w, rows, self.title, self.right, &inside, &self.theme),
        }
    }
}

/// A tab's card (board 3), the kit's card: its mark, its number in its
/// color and its name on the top edge, cut only where the edge has no room,
/// its state and how long at the edge's right end, the edge the plain rule
/// (the tab's color under the mouse, and when chosen the kit's accent bar,
/// board 143), what it is about inside by what it is doing (or, with `s`,
/// its screen's last lines), and its branch and checks at its foot.
fn card_2(tab: &TabModel, look: &CardLook, frame: usize, m: &Mission) -> Card {
    let snap = &m.snap;
    let (r, theme) = (&snap.rules, &snap.theme);
    let inner = look.w.saturating_sub(4);
    // The most rows inside it: less its edges, the row under the top one,
    // the foot and the row over it.
    let rows = look.h.saturating_sub(5);
    let mark = Mark::of_badge(model::badge_state(tab, r));
    let color = tab.color.map(Color::from).unwrap_or(theme.bright);
    let edge = if look.chosen {
        None
    } else if look.hovered {
        Some(color)
    } else {
        Some(kit::border(theme))
    };
    let words = state_for(tab, r);
    let number = tab.index.to_string();
    // The edge's corner and spaces, the mark and the number before the
    // name, and the state after it.
    // A status you set: the kit's chip after its name (board 158).
    let by_you = model::by_hand(tab, r)
        .is_some()
        .then(|| kit::chip(model::BY_YOU, Tone::Dim, false, theme));
    let chip_w = by_you.as_ref().map_or(0, |c| 1 + kit::spans_width(c));
    let name_room = look
        .w
        .saturating_sub(5 + width(&words) + 2 + 2 + width(&number) + 1 + chip_w);
    let right = vec![Span::styled(words, mark.look().1.style(theme))];
    let mut title = vec![
        kit::mark(mark, frame, theme),
        Span::raw(" "),
        Span::styled(number, bold(color)),
        Span::raw(" "),
        Span::styled(
            text::cut(&card_name(tab), name_room.max(1)),
            bold(theme.bright),
        ),
    ];
    if let Some(chip) = by_you {
        title.push(Span::raw(" "));
        title.extend(chip);
    }
    let inside = if look.shows_screen {
        let lines: &[Vec<Span<'static>>] = look.screen.map_or(&[], |s| &s.lines[..]);
        let shown = &lines[lines.len().saturating_sub(rows)..];
        let mut out = vec![Vec::new(); rows - shown.len()];
        out.extend(shown.iter().cloned());
        Inside {
            lines: out,
            tight: false,
        }
    } else {
        match tab.state {
            TabState::Waiting => asks_inside(tab, look, rows, theme),
            TabState::Working => doing_inside(tab, look, r, theme),
            // Between turns with only its background running: its last
            // turn, and what runs behind it in the state's words.
            _ => last_inside(tab, look, r, theme),
        }
    };
    let most = rows + usize::from(inside.tight);
    let branch = look.screen.and_then(|s| s.branch.as_deref());
    // A tab following several PRs lists them under what it is doing, as
    // much of that kept as leaves them room (board 128).
    let prs = if look.shows_screen {
        Vec::new()
    } else {
        pr_lines(tab, inner, theme)
    };
    let mut lines: Vec<Vec<Span<'static>>> = inside
        .lines
        .into_iter()
        .take(most.saturating_sub(prs.len()))
        .collect();
    lines.extend(prs);
    let mut foot = card_foot(tab, branch, inner, r.now_secs(), theme);
    if !tab.prs.is_empty() {
        foot = card_foot_prs(tab, branch, inner, theme);
    }
    Card {
        title,
        right,
        lines: lines.into_iter().take(most).collect(),
        tight: inside.tight,
        foot,
        edge,
        w: look.w,
        theme: *theme,
    }
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
        when::ago(tab.since, r.now_secs(), Said::Short)
    };
    // A landing tab's state is its place in line, said already.
    // A tab with no Claude is where it is.
    let what = turn_words(tab)
        .or_else(|| {
            tab.claude
                .is_none()
                .then(|| tab.path.as_ref().map(crate::paths::tilde))
                .flatten()
        })
        .or_else(|| tab.landing.is_none().then(|| model::state_words(tab, r)))
        .unwrap_or_default();
    // A merged tab at work again is on another group's shelf: its words
    // say it merged before its turn's (state_words says it itself).
    let what = if tab.merge == Some(model::MergeState::Merged)
        && model::badge_state(tab, r) != Badge::Merged
        && (turn_words(tab).is_some() || tab.claude.is_none())
    {
        format!("{} · {what}", model::merge_words(tab))
    } else {
        what
    };
    // A reminder come due in another repository than the filter's names it.
    let what = match tab.from_repo.as_ref().filter(|_| tab.outside_filter) {
        Some(repo) => format!("{repo} · {what}"),
        None => what,
    };
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
        vec![Span::styled(
            right_aligned(number, at[1].map_or(0, |(_, w)| w)),
            bold(color),
        )],
        vec![Span::styled(tab.name.clone(), name_style)],
        vec![Span::styled(
            right_aligned(when, at[3].map_or(0, |(_, w)| w)),
            Tone::Dim.style(theme),
        )],
        // A landing tab's line leads with the way it lands by (board 193).
        tab.way
            .map(|w| kit::way_chip(w.chip(), m.snap.caps, theme))
            .unwrap_or_default()
            .into_iter()
            .chain([Span::styled(what, Tone::Dim.style(theme))])
            .collect(),
    ];
    let mut spans = vec![Span::raw(" ")];
    for (part, place) in parts.into_iter().zip(at) {
        if let Some((x, w)) = place {
            kit::place(&mut spans, part, (x + 1, w));
        }
    }
    Line {
        spans: text::fit_spans(spans, cols, Style::new()),
        fill: chosen.then(|| kit::Surface::Chosen.color(theme)),
    }
}

/// UX 2.0's Tabs (board 3, laid out as board 45 says): each group of cards
/// (Needs you, Working) under its label with its count, as many cards side
/// by side as fit at their least width sharing the width equally, a row of
/// them as tall as its tallest holds; then the shelf, every other stage a
/// line a tab, folded to their labels when only one card fits; scrolled to
/// keep the chosen one in view, the cards a column to the left while a
/// scroll bar has the last one.
fn cards(
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
    let folded = shelf_folded(cols, m.shelf_open);
    let repos = repos_folded(m);
    // A card showing its screen is as tall as the view has room for under
    // its label; any other keeps the rows a request shown whole counts on
    // (`ask_fits`), whatever the view's height, and scrolls with the rest.
    let screen_h = if body > CARD_H + 1 {
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
                when::ago(t.since, r.now_secs(), Said::Short)
            })
        })
        .max()
        .unwrap_or(0);
    // Each card's or line's place: its first and last line, its columns,
    // its place in the order.
    type Places = Vec<(usize, usize, usize, usize, usize)>;
    // The view's lines with the cards from column `x0`, each card's and
    // line's place, and the chosen one's first line and the one after its
    // last.
    let lay = |x0: usize| -> (Vec<Line>, Places, usize, usize) {
        let xs = card_columns(cols, n, x0);
        let mut drawn: Vec<Line> = Vec::new();
        let mut at: Places = Vec::new();
        let mut place = 0usize;
        let (mut cursor_top, mut cursor_bottom) = (0, 0);
        for (stage, tabs_in) in &groups {
            if !drawn.is_empty() {
                drawn.push(Line {
                    spans: vec![Span::raw(" ".repeat(cols))],
                    fill: None,
                });
            }
            let label_at = drawn.len();
            let count = Some(tabs_in.len());
            let mut label = match stage.mark() {
                Some(mark) => {
                    // Turning while one of its tabs works, still otherwise.
                    let turning = tabs_in
                        .iter()
                        .any(|&i| kit::spins(model::badge_state(&snap.tabs[i], r)));
                    let frame = if turning { snap.frame } else { 0 };
                    kit::marked_label(mark, frame, stage.title(), count, cols, theme)
                }
                None => {
                    let tone = Mark::of_badge(stage.color()).look().1;
                    kit::label(stage.title(), tone, count, cols, theme)
                }
            };
            if stage_folded(*stage, folded, repos) {
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
                let held: Vec<Card> = row
                    .iter()
                    .enumerate()
                    .map(|(c, &i)| {
                        let tab = &snap.tabs[i];
                        let at = place + c;
                        let chosen = m.cursor == at as isize;
                        // A tab with no Claude is about what its screen shows.
                        let shows_screen = tab.claude.is_none()
                            || (m.card_screen == Some((tab.id, m.cursor)) && chosen);
                        let look = CardLook {
                            w: xs[c].1,
                            h: if shows_screen { screen_h } else { CARD_H },
                            chosen,
                            hovered: m.hover == Some(Spot::Index(at as isize)),
                            chips: n >= 3,
                            screen: snap.screens.get(&tab.id),
                            shows_screen,
                        };
                        card_2(tab, &look, snap.frame, m)
                    })
                    .collect();
                // The row as tall as its tallest card holds.
                let card_h = held
                    .iter()
                    .map(Card::height)
                    .max()
                    .unwrap_or(CARD_H)
                    .min(CARD_H);
                let mut row_lines: Vec<Vec<Span<'static>>> = vec![Vec::new(); card_h];
                for (c, card) in held.into_iter().enumerate() {
                    let (x, w) = xs[c];
                    let chosen = m.cursor == place as isize;
                    for (line, spans) in row_lines.iter_mut().zip(card.drawn(card_h)) {
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
                        spans: text::fit_spans(spans, cols, Style::new()),
                        fill: None,
                    });
                }
            }
        }
        (drawn, at, cursor_top, cursor_bottom)
    };
    let mut laid = lay(CARD_X);
    // With more than the screen holds, a scroll bar has the last column,
    // so the cards end before it.
    if r.scroll_bars && laid.0.len() > body {
        laid = lay(0);
    }
    let (drawn, at, cursor_top, cursor_bottom) = laid;
    let start = cursor_top.min(cursor_bottom.saturating_sub(body));
    let end = start + body;
    let from = lines.len();
    lines.extend(drawn.iter().skip(start).take(body).cloned());
    rows_bar(&mut lines[from..], (drawn.len(), start, body), cols, m);
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

/// A scroll bar in the last column of a view's rows, `cols` wide, while
/// they run past the screen (Settings' `scroll_bars`): the Tabs view's
/// cards, which end a column before it then, and a list's rows, laid out a
/// column narrower for it. They
/// scroll to keep the chosen card or row in view, so the bar shows where
/// that is and takes no click.
fn rows_bar(
    lines: &mut [Line],
    (total, top, body): (usize, usize, usize),
    cols: usize,
    m: &Mission,
) {
    if !m.snap.rules.scroll_bars || cols < 2 {
        return;
    }
    let bar = kit::scrollbar(total, top, body, body, &m.snap.theme);
    for (line, mark) in lines.iter_mut().zip(bar) {
        let mut spans = text::clip_spans(std::mem::take(&mut line.spans), cols - 1);
        spans.push(mark);
        line.spans = spans;
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
            "Landing" => Badge::Queued.color(),
            "Approved" | "Done" => Badge::Done.color(),
            "Draft" => Some(theme.dim),
            _ => Badge::Working.color(),
        }
        .unwrap_or(theme.text);
        let mut out = vec![
            text::fit_spans(
                vec![Span::styled(
                    format!(" {} {}", column.to_uppercase(), items.len()),
                    bold(color),
                )],
                col_w,
                Style::new(),
            ),
            text::fit_spans(
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
            let first_line = text::fit_spans(
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
            let second = text::fit_spans(
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
            out.push(text::fit_spans(
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
            spans: text::fit_spans(spans, cols, Style::new()),
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
/// it); the chosen card the kit's accent bar. The views' last group (the
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
    let side_w = side_width(m, &view.key(), cols);
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
    let drawn = kit::board(
        &board_columns,
        board_w,
        body - week_h,
        more_lit(m),
        m.snap.rules.scroll_bars,
        theme,
    );
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
                spans.extend(text::fit_spans(
                    card.get(line).cloned().unwrap_or_default(),
                    w,
                    Style::new(),
                ));
                spans.push(Span::raw(" "));
            }
            rows.push(text::fit_spans(spans, board_w, Style::new()));
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
    // Its edge, which a drag moves, lit under the mouse and while dragged.
    let resizable = crate::settings::is_on("resize_side_panes");
    hits.extend((0..body).filter(|_| resizable).map(|i| HitBox {
        y: (top_row + i) as u16,
        x0: board_w as u16,
        x1: board_w as u16,
        spot: Spot::SideEdge(side_w),
    }));
    let edge_lit = m.side_drag.is_some() || matches!(m.hover, Some(Spot::SideEdge(_)));
    let edge = if edge_lit {
        fg(ACCENT)
    } else {
        fg(theme.faint)
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
    let total = about.lines.len();
    let about = about.window(top, body);
    let bar_x = board_w + side_w - 1;
    let bar = pane_bar((total, top, body), (bar_x, top_row), hits, m);
    let inner = side_w - 2 - usize::from(!bar.is_empty());
    for (i, row) in rows.iter_mut().enumerate() {
        let mut spans = text::fit_spans(std::mem::take(row), board_w, Style::new());
        spans.push(Span::styled("│ ", edge));
        spans.extend(text::fit_spans(
            about.lines.get(i).cloned().unwrap_or_default(),
            inner,
            Style::new(),
        ));
        spans.extend(bar.get(i).cloned());
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
    let total = about.lines.len();
    let about = about.window(top, room);
    let bar = pane_bar((total, top, room), (cols.saturating_sub(1), at), hits, m);
    let inner = cols - usize::from(!bar.is_empty());
    for i in 0..room {
        let mut spans = text::fit_spans(
            about.lines.get(i).cloned().unwrap_or_default(),
            inner,
            Style::new(),
        );
        spans.extend(bar.get(i).cloned());
        rows.push(spans);
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

/// The scroll bar of a pane beside or under a board, for `total` lines of
/// which `body` show from `top`, down column `x` from screen row `y`:
/// a span a row, none while it fits or Settings' `scroll_bars` is off
/// (the snapshot's rules);
/// its rows' hits go before the pane's own, so a press there pages the
/// pane and a drag moves it (`Spot::PaneBar`).
fn pane_bar(
    (total, top, body): (usize, usize, usize),
    (x, y): (usize, usize),
    hits: &mut Vec<HitBox>,
    m: &Mission,
) -> Vec<Span<'static>> {
    let theme = &m.snap.theme;
    if !m.snap.rules.scroll_bars {
        return Vec::new();
    }
    let bar = kit::scrollbar(total, top, body, body, theme);
    if !bar.is_empty() {
        let rows = (0..body).map(|row| HitBox {
            y: (y + row) as u16,
            x0: x as u16,
            x1: x as u16,
            spot: Spot::PaneBar { row, body, total },
        });
        hits.splice(0..0, rows);
    }
    bar
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
        "Landing" => Tone::Landing,
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
            .filter(|line| text::spans_width(line) > 0)
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
    let queued = item.group.as_deref() == Some(crate::sources::prboard::LANDING);
    // Landing, its place in its repository's way (board 195): its train,
    // or its place in GitHub's queue.
    let landing = item.data["landing"]
        .as_str()
        .filter(|w| queued && !w.is_empty());
    let meta = match (tab, item.data["reviewers"].as_str()) {
        // Its place in its way, then the tab it is open in.
        _ if landing.is_some() => {
            let mut words = vec![Span::styled(
                landing.unwrap_or_default().to_string(),
                Tone::Landing.style(theme),
            )];
            if let Some(t) = tab {
                words.push(Span::styled(" · ", fg(theme.dim)));
                words.extend(tab_words(t, m, theme));
            }
            words
        }
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
        .map(|ms| when::ago(Some(ms.div_euclid(1000)), now_secs, Said::Ago))
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
pub const SIDE_AT: usize = 140;
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

/// The fewest columns a pane beside a board is resized to.
pub const SIDE_MIN: usize = 30;
/// The fewest columns resizing the pane leaves the PR and ticket boards.
const BOARD_MIN: usize = 90;

/// The columns the pane beside the board of the view `key` takes on a
/// screen `cols` wide (its `View::key`): none where the board has it
/// under its columns (or has none), else the width it was resized to
/// (`Mission::side_widths`, by the pane resize keys or a drag of its
/// edge), kept between `SIDE_MIN` and what leaves the board its narrowest,
/// else, and with Settings' `resize_side_panes` off, its share of the
/// screen.
pub fn side_width(m: &Mission, key: &str, cols: usize) -> usize {
    let (at, share, board_min) = match key {
        "prs" | "tickets" => (SIDE_AT, SIDE_W, BOARD_MIN),
        "autopilot" => (PILOT_SIDE_AT, PILOT_SIDE_W, PILOT_BOARD_AT),
        _ => return 0,
    };
    if cols < at {
        return 0;
    }
    match m.side_widths.get(key) {
        Some(&w) if crate::settings::is_on("resize_side_panes") => {
            w.min(cols.saturating_sub(board_min)).max(SIDE_MIN)
        }
        _ => beside_w(cols, at, share),
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

/// The matching lines under a row of M-j's picker found by what its
/// tab's panes show: each cut around its match to the row's width, dim,
/// the match in the accent, the last saying how many more the tab holds.
fn found_lines(item: &Item, needle: &str, cols: usize, theme: &Theme) -> Vec<Vec<Span<'static>>> {
    let (found, more) = super::pane_search::shown_lines(item);
    let indent = "      ";
    let last = found.len().saturating_sub(1);
    found
        .iter()
        .enumerate()
        .map(|(at, line)| {
            let more = if at == last && more > 0 {
                format!("  {more} more in this tab")
            } else {
                String::new()
            };
            let room = cols.saturating_sub(width(indent) + width(&more) + 1);
            let (before, hit, after) = super::pane_search::cut(line, needle, room);
            vec![
                Span::raw(indent),
                Span::styled(before, Tone::Dim.style(theme)),
                Span::styled(hit, Tone::Accent.bold(theme)),
                Span::styled(after, Tone::Dim.style(theme)),
                Span::styled(more, Tone::Faint.style(theme)),
            ]
        })
        .collect()
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
    let (item, field, place) = item_beside(m)?;
    Some((item.data[field].as_str()?, place))
}

/// The item the board in front shows in its pane, beside the board or
/// under it, whether or not it has a description, with the field its
/// description is in and the place its Markdown is read for.
pub fn item_beside(m: &Mission) -> Option<(&Item, &'static str, &'static str)> {
    let view = m.current()?;
    let (field, place, board_at) = match view.id {
        ViewId::Prs => ("body", crate::views::pr::PLACE, 0),
        ViewId::Tickets => ("description", crate::views::ticket::PLACE, 0),
        ViewId::Autopilot => ("plan", PLAN_PLACE, PILOT_BOARD_AT),
        _ => return None,
    };
    let is_a_board = view.columns().is_some() || view.id == ViewId::Autopilot;
    let board_with_the_pane =
        is_a_board && view.data.setup.is_none() && view.loaded && m.laid().0 as usize >= board_at;
    if !board_with_the_pane {
        return None;
    }
    Some((m.chosen_item()?, field, place))
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
    // Landing, the way it lands by and when it is done there (board 195).
    if let Some(way) = d["way"].as_str().and_then(Way::parse) {
        let done = way.facts()[1].1;
        let mut named = vec![Span::styled(
            way.label().to_string(),
            Tone::Landing.style(theme),
        )];
        named.push(Span::styled(format!(" · done when {done}"), fg(theme.dim)));
        if !fact_line.iter().all(|s| s.content.is_empty()) {
            named.push(Span::styled(" · ", fg(theme.dim)));
        }
        named.extend(fact_line);
        fact_line = named;
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
            when::ago(Some(ms.div_euclid(1000)), now_secs, Said::Short)
        }
        _ => String::new(),
    }
}

/// When a notification came, for its row's end: the clock time alone
/// under a day's heading (the history), with its day under a source's
/// heading (`history::grouped`), else the date and the time; a
/// meeting still to come says when it starts.
fn notice_when(item: &Item, now: &jiff::Zoned, dated: bool) -> String {
    let Some(ms) = item.time else {
        return String::new();
    };
    let to_come = ms.div_euclid(1000) > now.timestamp().as_second();
    // A kept row, or a live one under a source's heading.
    if (super::history::notice_of(item).is_some() || item.group.is_some()) && !to_come {
        // Under its source's heading, which names no day, its day too:
        // with its time where the list has room, else alone.
        if item.group.is_some() && dated {
            crate::ui::when::short(ms, now)
        } else if item.group.is_some() {
            crate::ui::when::compact(ms, now)
        } else {
            crate::ui::when::clock(ms, now)
        }
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
    // The Slack view's line on what your own sign-in would add.
    if let Some(note) = data
        .head
        .get("note")
        .and_then(serde_json::Value::as_str)
        .filter(|_| view.id == ViewId::Slack)
    {
        for w in text::wrap(note, cols.saturating_sub(4)) {
            lines.push(
                Row::default()
                    .put(format!("  {w}"), fg(theme.dim))
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
    if let Some(counts) = snap.notice_counts.as_ref()
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
    // `i`'s output over the list, whether the board or the list is drawn.
    if view.id == ViewId::Landing
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
    if view.id == ViewId::Landing && stage_board_fits(view, cols, snap.now_secs * 1000) {
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
    // M-j's picker's rows found by what their panes show take a row for
    // each matching line under them.
    let heights: Vec<usize> = data.items.iter().map(super::pane_search::height).collect();
    let (first, count) = if heights.iter().any(|h| *h > 1) {
        super::pane_search::window(&heights, cursor, room)
    } else {
        ((cursor + 1).saturating_sub(room), room)
    };
    let shown: Vec<(usize, &Item)> = data
        .items
        .iter()
        .enumerate()
        .skip(first)
        .take(count)
        .collect();
    // The scroll bar counts rows, so it shows what the matching lines push
    // out of sight.
    let total_rows: usize = heights.iter().sum();
    let rows_above: usize = heights.iter().take(first).sum();
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
    // A row with a value beside its label (a setting, a file of yours, the
    // Slack relay view's rows): the labels as wide as the widest, and the
    // values too (up to 40), never under 13 so Settings' stay where they were.
    fn valued(i: &Item) -> Option<&str> {
        i.data.get("value").and_then(serde_json::Value::as_str)
    }
    let label_width = shown
        .iter()
        .filter(|(_, i)| {
            i.data.get("setting").is_some() || i.data.get("own").is_some() || valued(i).is_some()
        })
        .map(|(_, i)| width(&i.title))
        .max()
        .unwrap_or(0)
        // Whole up to 40 (the relay view's longest), never past half the row.
        .min(40)
        .min(cols / 2);
    let value_width = shown
        .iter()
        .filter_map(|(_, i)| valued(i))
        .map(width)
        .max()
        .unwrap_or(0)
        .clamp(13, 40);
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
            .map(|(_, i)| width(&notice_when(i, &now, cols >= DATED_FROM)))
            .max()
            .unwrap_or(0)
            .max(5)
    } else {
        5
    };
    // The rows a column narrower while the scroll bar is beside them.
    let full = cols;
    let cols = if snap.rules.scroll_bars && total_rows > room && cols > 1 {
        cols - 1
    } else {
        cols
    };
    let rows_from = lines.len();
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
        // A source's heading in the Notifications view, open or folded,
        // and a sub-heading under one.
        if view.id == ViewId::Notifications
            && (super::history::group_heading(item).is_some() || super::history::is_part(item))
        {
            let count = item
                .data
                .get("count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            if super::history::is_part(item) {
                row.put(format!("   {}", item.title), fg(theme.dim));
            } else {
                let folded = super::history::folded(item);
                row.put(if folded { " ▸ " } else { " ▾ " }, fg(theme.dim))
                    .put(item.title.to_uppercase(), bold(theme.dim));
            }
            // Under a filter its rows are the matches, not its count.
            if !super::history::under_filter(item) {
                row.put(format!(" {count}"), fg(theme.faint));
            }
            if super::history::folded(item) {
                row.put("   folded", fg(theme.faint));
            }
            lines.push(row.line(cols, fill));
            continue;
        }
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
                .put(text::pad(value, value_width), value_style)
                .put(" ", Style::new());
            if item.data["computer"] == true {
                row.spans.extend(kit::chip(
                    super::settings_board::COMPUTER_CHIP,
                    Tone::Dim,
                    snap.caps,
                    theme,
                ));
                row.put(" ", Style::new());
            }
            if item.data["changed"] == true {
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
            let needle = m
                .filter
                .as_ref()
                .map(|f| f.typed().trim().to_lowercase())
                .unwrap_or_default();
            for found in found_lines(item, &needle, cols, theme) {
                hits.push(HitBox {
                    y: (top_row + lines.len()) as u16,
                    x0: 0,
                    x1: cols.saturating_sub(1) as u16,
                    spot: Spot::Index(index as isize),
                });
                lines.push(Line {
                    spans: kit::fit(found, cols, Style::new()),
                    fill,
                });
            }
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
            notice_when(item, &now, cols >= DATED_FROM)
        } else {
            when(item, snap.now_secs)
        };
        let mut spans = row.fitted(cols.saturating_sub(when_w + 1), Style::new());
        spans.push(Span::styled(format!("{at:>when_w$} "), fg(theme.faint)));
        lines.push(Line { spans, fill });
    }
    rows_bar(
        &mut lines[rows_from..],
        (total_rows, rows_above, room),
        full,
        m,
    );
    let cols = full;
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

/// A tone by the name src/mission/views.rs gives it.
pub(super) fn tone_named(name: &str) -> Tone {
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

/// UX 2.0's Landing view, as board 193 draws it and board 45 lays it out:
/// each section's columns are the steps of the way its repository lands
/// by (`repos::Way::columns`), every way ending in landed and cleaned up.
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
pub const LANDING_OUTPUT_AT: usize = 140;
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
/// "✕ found", or a dim "not run" outside Switchboard's clone, where none
/// runs); without a record, "passed", "clean" and "none". A waiter is
/// queued for the lock at its place; the one holding it is at the step its
/// phase names, a failure included (`tests failed …`, `review blocked …`,
/// the words after it shown in the cell, and `merge conflict <files>`,
/// update-target's merge left for you, in the lock's cell), the check
/// after a clean rebase
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
    let review = review_cell(&record["review"], now_ms);
    let leak = match number(&record["leak"], "found") {
        Some(n) if n > 0 => stage_cell(Stage::Failed, "✕ found"),
        _ if record["leak"]["skipped"] == true => stage_cell(Stage::Waiting, "not run"),
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
    } else if let Some(files) = phase.strip_prefix("merge conflict") {
        // update-target's merge of the default branch, left for you.
        out[3] = failed("", "conflict", printable(files.trim()));
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
        out[4] = running("applying…");
    } else if phase.starts_with("cleaning") {
        out[4] = done("pushed");
        out[5] = running("cleaning…");
    }
    out
}

/// A review loop's cell from what its landing recorded (`record.review`):
/// its clean rounds once there are two, else its round, its reviewer and
/// how long ago, waiting on you at its cap; "✓ clean" with no record, since
/// a tab lands only once its review is clean.
fn review_cell(r: &serde_json::Value, now_ms: i64) -> StageCell {
    let number = |k: &str| r.get(k).and_then(serde_json::Value::as_u64);
    if !r.is_object() {
        return stage_cell(Stage::Done, "✓ clean");
    }
    if number("clean_rounds").unwrap_or(0) >= 2 {
        return stage_cell(
            Stage::Done,
            format!("✓ {} clean", number("clean_rounds").unwrap_or(2)),
        );
    }
    let ago = when::ago(
        r.get("at")
            .and_then(serde_json::Value::as_i64)
            .map(|ms| ms / 1000),
        now_ms / 1000,
        Said::Short,
    );
    let at_cap = r["at_cap"].as_bool() == Some(true);
    StageCell {
        stage: if at_cap { Stage::Asks } else { Stage::Running },
        label: round_of_cap(r),
        note: [
            if at_cap {
                "waiting on you".to_string()
            } else {
                printable(r["reviewer"].as_str().unwrap_or(""))
            },
            ago,
        ]
        .into_iter()
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" · "),
    }
}

/// A PR in Switchboard's merge queue's cells (`data.prqueue`), as board 193
/// draws them: its review from its landing's record, its checks passed
/// (the queue takes a PR only once they have), its place among those
/// queued or in its train, the train testing, rerunning or next, and
/// landed and cleaned up waiting, since a PR leaves the queue as it merges.
fn prqueue_stages(q: &serde_json::Value, now_ms: i64) -> [StageCell; 6] {
    let waiting = || stage_cell(Stage::Waiting, "waiting");
    let place = q["place"].as_u64().unwrap_or(1);
    let train = q["train"].as_str().map(crate::sources::landing::train_name);
    // A PR in the train ahead is next, though the queue counts it as
    // testing too.
    let next = q["next"].as_bool() == Some(true);
    let testing = !next && q["testing"].as_bool() == Some(true);
    let (queued, train_cell) = if testing {
        let ago = when::ago(
            q["since"].as_i64().map(|ms| ms / 1000),
            now_ms / 1000,
            Said::Short,
        );
        let label = if q["rerun"].as_bool() == Some(true) {
            "rerunning"
        } else {
            "testing"
        };
        (
            stage_cell(Stage::Done, format!("✓ {}", ordinal(place))),
            StageCell {
                stage: Stage::Running,
                label: label.into(),
                note: if ago.is_empty() {
                    String::new()
                } else {
                    format!("CI · {ago}")
                },
            },
        )
    } else if next {
        (
            stage_cell(Stage::Done, "✓ next"),
            StageCell {
                stage: Stage::Queued,
                label: "next".into(),
                note: train.unwrap_or_default(),
            },
        )
    } else {
        (stage_cell(Stage::Queued, ordinal(place)), waiting())
    };
    // Its train's CI looked passed once: the queue merges it on the next
    // look that agrees (a job that waits on another shows only once that
    // one ends), or failed to merge it and tries again.
    let (train_cell, landed) = match (
        testing && q["passed"].as_bool() == Some(true),
        q["mergeFailures"].as_u64().unwrap_or(0),
    ) {
        (true, 0) => (stage_cell(Stage::Running, "confirming"), waiting()),
        (true, n) => (
            stage_cell(Stage::Done, "✓ passed"),
            StageCell {
                stage: Stage::Failed,
                label: "✕ not merged".into(),
                note: format!("{n} tries"),
            },
        ),
        _ => (train_cell, waiting()),
    };
    [
        review_cell(&q["record"]["review"], now_ms),
        stage_cell(Stage::Done, "✓ passed"),
        queued,
        train_cell,
        landed,
        waiting(),
    ]
}

/// A merged PR's cells while its worktree is still there (`data.landed`):
/// every step done, landed merged, and cleaned up under way while
/// queue-and-clean cleans it up, else waiting on its worktree.
fn landed_stages(landed: &serde_json::Value) -> Vec<StageCell> {
    let way = landed["way"]
        .as_str()
        .and_then(Way::parse)
        .unwrap_or(Way::GithubQueue);
    let columns = way.columns();
    let mut out: Vec<StageCell> = columns[..columns.len() - 2]
        .iter()
        .map(|_| stage_cell(Stage::Done, "✓"))
        .collect();
    out.push(stage_cell(Stage::Done, "✓ merged"));
    out.push(if landed["cleaning"].as_bool() == Some(true) {
        stage_cell(Stage::Running, "cleaning…")
    } else {
        stage_cell(Stage::Waiting, "worktree left")
    });
    out
}

/// A GitHub merge queue row's stages from what the view holds of it
/// (`data.queue.state` and `position`), as board 193 draws them: GitHub
/// lets a PR into the queue only once its checks pass and it is approved,
/// so those are done, its review unknown to the workspace (a dash); the
/// queued cell is its place, its checks running, or a failure when it
/// can't merge; merging is the landed stage running. Cleaned up waits,
/// since the row leaves the queue once it merges.
fn queue_stages(queue: &serde_json::Value) -> [StageCell; 6] {
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
        stage_cell(Stage::Waiting, "—"),
        stage_cell(Stage::Done, "✓ passed"),
        stage_cell(Stage::Done, "✓ approved"),
        stage_cell(Stage::Queued, ordinal(position.max(1))),
        waiting(),
        waiting(),
    ];
    match state {
        "AWAITING_CHECKS" => out[3] = stage_cell(Stage::Running, "checks…"),
        "UNMERGEABLE" => out[3] = stage_cell(Stage::Failed, "✕ can't merge"),
        "MERGEABLE" => out[3] = stage_cell(Stage::Done, "✓ ready"),
        "LOCKED" => {
            out[3] = stage_cell(Stage::Done, "✓ ready");
            out[4] = stage_cell(Stage::Running, "merging…");
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

/// An item's stage cells, none for a row that isn't a landing, a PR in a
/// merge queue or a merged PR kept until it is cleaned up.
fn stages_of(item: &Item, now_ms: i64) -> Vec<StageCell> {
    let of = |k: &str| item.data.get(k).filter(|v| v.is_object());
    if let Some(landing) = of("landing") {
        landing_stages(landing, now_ms).to_vec()
    } else if let Some(queue) = of("queue") {
        queue_stages(queue).to_vec()
    } else if let Some(q) = of("prqueue") {
        prqueue_stages(q, now_ms).to_vec()
    } else if let Some(landed) = of("landed") {
        landed_stages(landed)
    } else {
        Vec::new()
    }
}

/// The way a section's heading names (`data.way`).
fn way_of(item: &Item) -> Option<Way> {
    item.data
        .get("way")
        .and_then(serde_json::Value::as_str)
        .and_then(Way::parse)
}

/// The stage names a section's heading puts over its cells: its way's.
fn heads_of(item: &Item) -> &'static [&'static str] {
    way_of(item).map_or(&[], Way::columns)
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
    let since = when::ago(
        holder["since"].as_i64().map(|ms| ms / 1000),
        now_ms / 1000,
        Said::Short,
    );
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
    if phase.starts_with("merge conflict") {
        "waits for you to resolve the merge in its worktree and run update-target again".into()
    } else if phase.starts_with("tests failed")
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

/// A review round with its cap, `round 21 of 20`, as codex-review recorded
/// it (`round 3` from a build that recorded no cap).
fn round_of_cap(review: &serde_json::Value) -> String {
    let round = review["round"].as_u64().unwrap_or(1);
    match review["cap"].as_u64() {
        Some(cap) => format!("round {round} of {cap}"),
        None => format!("round {round}"),
    }
}

/// The chosen worktree's last review round, in a line: its round, its
/// reviewer, how long ago, and codex-review's summary.
fn review_line(review: &serde_json::Value, now_ms: i64) -> String {
    let mut parts = vec![round_of_cap(review)];
    if review["at_cap"].as_bool() == Some(true) {
        parts.push("waiting on you".to_string());
    }
    if let Some(r) = review["reviewer"].as_str().filter(|r| !r.is_empty()) {
        parts.push(r.to_string());
    }
    let ago = when::ago(
        review["at"].as_i64().map(|ms| ms / 1000),
        now_ms / 1000,
        Said::Ago,
    );
    if !ago.is_empty() {
        parts.push(ago);
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
        &crate::paths::tilde(landing["root"].as_str().unwrap_or_default()),
    ));
    let keys = landing_keys(landing, narrow);
    if !keys.is_empty() {
        let y = (top_row + lines.len()) as u16;
        let mut spans = vec![Span::raw(" ".repeat(DETAIL_LABEL))];
        for (key, what) in keys {
            let x0 = text::spans_width(&spans);
            let parsed = footer_key(key);
            let pointed = parsed
                .is_some_and(|k| m.hover == Some(Spot::Key(k)) || m.focus == Some(Focus::Key(k)));
            let lit = pointed || (key == "i" && m.landing_output);
            spans.extend(kit::keycap(key, what, lit, m.snap.caps, theme));
            if let Some(k) = parsed {
                hits.push(HitBox {
                    y,
                    x0: x0 as u16,
                    x1: text::spans_width(&spans).saturating_sub(1) as u16,
                    spot: Spot::Key(k),
                });
            }
        }
        lines.push(Line {
            spans: text::fit_spans(spans, cols, Style::new()),
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
                // The repository's name, then the way it lands by and when
                // a tab there is done, dim (board 193).
                // The repository's name alone where its words follow it.
                let shown = item
                    .data
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .filter(|_| item.data.get("words").is_some())
                    .unwrap_or(&item.title);
                let mut name = vec![Span::styled(format!(" {shown}"), bold(theme.bright))];
                if let Some(words) = item
                    .data
                    .get("words")
                    .and_then(serde_json::Value::as_str)
                    .filter(|w| !w.is_empty())
                {
                    name.push(Span::styled(format!("   {words}"), fg(theme.dim)));
                }
                kit::place(&mut heading, name, col);
            }
            if let Some(col) = at[1] {
                kit::place(&mut heading, vec![Span::styled(right, fg(theme.dim))], col);
            }
            lines.push(Line {
                spans: text::fit_spans(heading, cols, Style::new()),
                fill,
            });
            let names = heads_of(item);
            if let Some(layout) = layout.filter(|_| !names.is_empty()) {
                let first = if way_of(item) == Some(Way::Direct) {
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
                    spans: text::fit_spans(heads, cols, Style::new()),
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
            spans: text::fit_spans(top, cols, Style::new()),
            fill,
        });
        hit(lines);
        lines.push(Line {
            spans: text::fit_spans(under, cols, Style::new()),
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
    v.id == ViewId::Keys
        && cols >= super::keys_board::BOARD_AT
        && v.data.items.iter().any(|i| !views::is_header(i))
        && super::keys_board::fits(&v.data.items, m.laid().1 as usize)
}

/// How wide the main area must be for UX 2.0's Autopilot board (its six
/// columns 17 wide), narrower showing the plain list; and how wide for the
/// chosen card's pane beside it, under it on a narrower screen, and how
/// many columns that takes.
pub const PILOT_BOARD_AT: usize = 108;
pub const PILOT_SIDE_AT: usize = 200;
const PILOT_SIDE_W: usize = 44;
/// Where a diagram of a plan that can't be drawn beside the board is shown
/// properly (`Markdown::parse`).
const PLAN_PLACE: &str = "its plan note in Notes";

/// Whether the Autopilot view is drawn as its board on a main area `cols`
/// wide: the screen wide enough, the view loaded and set up,
/// and at least one item, else as the plain list. The drawing, the layout's
/// sections, the footer's keys and `h`/`l` all ask this, so none of them
/// treats the list as the board.
pub fn pilot_board_shown(v: &View, cols: usize) -> bool {
    v.id == ViewId::Autopilot
        && cols >= PILOT_BOARD_AT
        && v.loaded
        && v.data.setup.is_none()
        && !v.data.items.is_empty()
}

/// Whether the Slack view is drawn as its board: once
/// something is saved.
pub fn slack_board_shown(v: &View) -> bool {
    v.id == ViewId::Slack && v.loaded && v.data.at.is_some()
}

/// Whether the Settings view is drawn as its board: with the screen
/// wide enough.
pub fn settings_board_shown(m: &Mission) -> bool {
    m.laid().0 as usize >= super::settings_board::BOARD_AT
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
    /// A Slack workspace's own emoji (`images::emoji`), from the
    /// snapshot's `slack_emoji` by its name.
    Emoji {
        name: String,
    },
    /// A closed tab's last screen (`images::screen`), the Reopen view's.
    Screen {
        shot: std::sync::Arc<crate::usage::shots::Shot>,
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

    /// Someone's avatar at `x`, `y`, as large as `size` (`kit::avatar`,
    /// board 0d): their photo over the kit's text form, else that text, its
    /// color from their name whatever service `who` is from.
    pub fn avatar(
        name: String,
        (x, y): (usize, usize),
        who: Option<crate::photos::Who>,
        person: &str,
        size: kit::AvatarSize,
    ) -> Image {
        Image {
            name,
            x: x as u16,
            y: y as u16,
            width: size.cols() as u16,
            height: size.rows() as u16,
            shape: Shape::Avatar {
                who,
                label: person.to_string(),
                color: Color::Indexed(kit::avatar_color(person)),
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
            | Shape::Picture { .. }
            | Shape::Emoji { .. }
            | Shape::Screen { .. } => None,
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
    // Under the mouse: the card, or one of a blocked card's after lines.
    let lit = |i: usize| {
        !chosen(i)
            && matches!(m.hover, Some(Spot::Index(at) | Spot::Part(at, _)) if at == i as isize)
    };
    if let Some((at, head)) = items.iter().enumerate().find(|(_, i)| views::is_summary(i)) {
        let spend = head.data.get("spend").and_then(serde_json::Value::as_array);
        let spend_w = if spend.is_some() { SPEND_W + 2 } else { 0 };
        // Its menu's button, lit under the mouse or while Tab's focus is on
        // the heading, before the spend.
        let button_lit = m.hover == Some(Spot::Menu(at as isize))
            || (m.focus == Some(Focus::Item) && chosen(at));
        let button = kit::keycap("m", "☰ Menu", button_lit, m.snap.caps, theme);
        let button_w = text::spans_width(&button);
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
    let side_w = side_width(m, "autopilot", cols);
    let board_w = cols - side_w;
    let columns = crate::autopilot::view::board_columns(items);
    let held: Vec<Vec<usize>> = columns
        .iter()
        .map(|column| {
            (0..items.len())
                .filter(|&i| items[i].group.as_deref() == Some(*column))
                .collect()
        })
        .collect();
    let board_columns: Vec<kit::BoardColumn> = columns
        .iter()
        .zip(&held)
        .map(|(column, indices)| kit::BoardColumn {
            name: column.to_string(),
            tone: pilot_column_tone(column),
            cards: indices
                .iter()
                .map(|&i| pilot_card(&items[i], i, chosen(i), lit(i), m))
                .collect(),
        })
        .collect();
    let drawn = kit::board(
        &board_columns,
        board_w,
        body,
        more_lit(m),
        m.snap.rules.scroll_bars,
        theme,
    );
    // A blocked card's after lines, each a click of its own that goes to
    // the item it names, ahead of the card's own click.
    for p in &drawn.placed {
        let at = held[p.column][p.card];
        let blockers = items[at].data["after"]
            .as_array()
            .map_or(0, Vec::len)
            .min(PILOT_BLOCKERS_SHOWN);
        let title = &board_columns[p.column].cards[p.card].title;
        let first = 2 + kit::title_lines(title, p.w.saturating_sub(4)).len();
        for n in 0..blockers {
            let dy = first + n;
            if dy + 1 >= p.h {
                break;
            }
            hits.push(HitBox {
                y: (top + p.y + dy) as u16,
                x0: (p.x + 1) as u16,
                x1: (p.x + p.w).saturating_sub(2) as u16,
                spot: Spot::Part(at as isize, n),
            });
        }
    }
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

/// How many of the items a blocked item waits on its card names, the rest
/// counted in a line under them.
const PILOT_BLOCKERS_SHOWN: usize = 3;

/// An Autopilot item's card: on its top edge its state's mark and its
/// tab's number (or its place in line, or "closed"), a red ✕ at the right
/// once a stage failed; its first line, where it is and how long it has
/// worked as its facts, and a meter of its stages done. A skipped item's
/// card says "skipped by you", and one held by a board's review says what
/// it waits on, both dim with no meter. A blocked one (board 113) is named
/// "blocked" with the idle mark, and under a dim "after" names each item it
/// waits on, a line each (`blocker_line`), `PILOT_BLOCKERS_SHOWN` at most
/// and then how many more. An issue's card names its priority first.
fn pilot_card(item: &Item, at: usize, chosen: bool, hovered: bool, m: &Mission) -> kit::BoardCard {
    let theme = &m.snap.theme;
    let pilot = item.data.get("pilot");
    let skipped = item.data.get("skipped").is_some();
    let held = item.data.get("waitsOn").is_some();
    let after = item.data["after"].as_array();
    // An item in line whose last start failed (`view::say_failed_starts`).
    let start_failed = item.data["startFailed"].as_str();
    let failed = pilot.and_then(|p| p["failed"].as_bool()) == Some(true) || start_failed.is_some();
    let stages = |p: &serde_json::Value| p["done"].as_f64().unwrap_or(0.0) as f32 / 8.0;
    let (name, where_, meter) = match pilot {
        // A skipped item has no place in line and no stages.
        None if skipped => (String::new(), "skipped by you".to_string(), None),
        // Nor has one blocked by open items, or held by a board's review.
        None if after.is_some() => ("blocked".to_string(), "after".to_string(), None),
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
                match start_failed {
                    Some(why) => format!("start failed: {why}"),
                    None if place == 1 => "opens next".to_string(),
                    None => format!("{} in line", ordinal(place)),
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
            Some("Blocked") => Some(Mark::Idle),
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
    let mut facts = Vec::new();
    // An issue's priority (`view::say_priorities`), before what it waits on.
    if let Some(p) = item.data["priority"].as_u64() {
        facts.push(vec![Span::styled(format!("priority {p}"), fg(theme.dim))]);
    }
    facts.push(vec![Span::styled(
        where_,
        fg(if skipped { theme.faint } else { theme.dim }),
    )]);
    if let Some(words) = pilot.and_then(|p| pilot_timer(p, m.snap.now_secs)) {
        facts.push(vec![Span::styled(words, fg(theme.dim))]);
    }
    // A landing card names the way its repository lands by (board 194).
    if let Some(way) = pilot.and_then(|p| p["way"].as_str()).and_then(Way::parse) {
        facts.push(kit::way_chip(way.chip(), m.snap.caps, theme));
    }
    let blockers = after.map_or(&[][..], Vec::as_slice);
    for (n, on) in blockers.iter().take(PILOT_BLOCKERS_SHOWN).enumerate() {
        let lit = m.hover == Some(Spot::Part(at as isize, n));
        facts.push(blocker_line(on, lit, m.snap.frame, theme));
    }
    // The rest are counted, so a card stays short enough to be drawn; the
    // pane beside the board and the menu have every one.
    if let Some(more) = blockers.len().checked_sub(PILOT_BLOCKERS_SHOWN + 1) {
        facts.push(vec![Span::styled(
            format!("and {} more", more + 1),
            fg(theme.dim),
        )]);
    }
    let name_tone = if after.is_some() {
        Tone::Dim
    } else {
        Tone::Text
    };
    kit::BoardCard {
        mark: mark.map(|k| kit::mark(k, m.snap.frame, theme)),
        name: Span::styled(name, name_tone.bold(theme)),
        state: if failed {
            vec![kit::mark(Mark::Failed, 0, theme)]
        } else {
            Vec::new()
        },
        title,
        title_look: if skipped || (held && after.is_none()) {
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

/// One item a blocked item waits on (`view::blockers`), as a line of its
/// card and of the pane beside the board: the mark of where it stands
/// (its tab's column, idle when it is blocked itself, pending while it
/// waits in line or the queue won't take it), its tab's number
/// when one works it, and its first line, dim, underlined under the mouse.
fn blocker_line(
    on: &serde_json::Value,
    lit: bool,
    frame: usize,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let mark = match on["column"].as_str() {
        Some("Blocked") => Mark::Idle,
        Some("Working") => Mark::Working,
        Some("Waiting on you") => Mark::Needs,
        Some("Landing") => Mark::Landing,
        Some("Landed") => Mark::Passed,
        _ => Mark::Pending,
    };
    let mut line = vec![kit::mark(mark, frame, theme), Span::raw(" ")];
    if let Some(index) = on["index"].as_u64() {
        line.push(Span::styled(format!("{index} "), bold(theme.bright)));
    }
    let words = fg(if lit { theme.bright } else { theme.dim });
    line.push(Span::styled(
        on["item"].as_str().unwrap_or_default().to_string(),
        if lit {
            words.add_modifier(Modifier::UNDERLINED)
        } else {
            words
        },
    ));
    line
}

/// Where an item a blocked item waits on stands, in words, for the pane.
fn blocker_stands(on: &serde_json::Value) -> String {
    match (
        on["index"].as_u64(),
        on["column"].as_str(),
        on["place"].as_u64(),
    ) {
        (Some(index), Some(column), _) => format!("tab {index}, {}", column.to_lowercase()),
        (_, Some(_), _) => "blocked itself".to_string(),
        (_, None, Some(place)) => format!("{} in line, no tab yet", ordinal(place)),
        (_, None, None) => "open, not in line".to_string(),
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
    let after = item.data["after"].as_array();
    let detail = match item.data["place"].as_u64() {
        None if after.is_some_and(|a| a.len() == 1) => {
            "Blocked: opens once the item it waits on is ticked".to_string()
        }
        None if after.is_some() => {
            "Blocked: opens once every item it waits on is ticked".to_string()
        }
        None if item.data.get("waitsOn").is_some() => format!(
            "Waits on {}, still open: opens once it is ticked",
            item.data["waitsOn"].as_str().unwrap_or_default()
        ),
        Some(1) => "Opens next".to_string(),
        Some(n) => format!("Opens {} in line", ordinal(n)),
        None => item.detail.clone(),
    };
    let detail = match item.data["startFailed"].as_str() {
        Some(why) => format!("{detail}, its last start failed: {why}"),
        None => detail,
    };
    for l in text::wrap(&detail, inner) {
        out.push(vec![Span::styled(l, fg(theme.dim))]);
    }
    // What a blocked item waits on: each item whole, wrapped under its
    // mark and tab's number, then where it stands on a line of its own.
    if let Some(after) = after {
        out.push(Vec::new());
        out.push(kit::label("After", Tone::Dim, None, inner, theme));
        for on in after {
            let mut lead = blocker_line(on, false, 0, theme);
            lead.pop();
            let indent = text::spans_width(&lead);
            let name = on["item"].as_str().unwrap_or_default();
            for (n, l) in text::wrap(name, inner.saturating_sub(indent).max(1))
                .into_iter()
                .enumerate()
            {
                let mut line = if n == 0 {
                    lead.clone()
                } else {
                    vec![Span::raw(" ".repeat(indent))]
                };
                line.push(Span::styled(l, fg(theme.text)));
                out.push(line);
            }
            for l in text::wrap(&blocker_stands(on), inner.saturating_sub(indent).max(1)) {
                out.push(vec![
                    Span::raw(" ".repeat(indent)),
                    Span::styled(l, fg(theme.dim)),
                ]);
            }
        }
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
    let right_w = text::spans_width(&right);
    let mut out = text::fit_spans(left, cols.saturating_sub(right_w + 1), Style::new());
    out.extend(right);
    out.push(Span::raw(" "));
    out
}

/// The CI view: its sections' labels with their counts; each
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
        let ci = &item.data["ci"];
        // What the run means for landing in its repository's way (board
        // 196), under its title: its repository, the way and where it is
        // there, and the run's counts.
        let landing = ci["landing"].as_str().filter(|w| !w.is_empty());
        if state != "failing" {
            if let Some(words) = landing {
                let short = repo.rsplit('/').next().unwrap_or(repo);
                let line = kit::row(
                    vec![Span::styled(
                        format!("  {short} · {words} · {}", item.detail),
                        fg(theme.dim),
                    )],
                    list_w,
                    row_chosen,
                    theme,
                );
                entries.push((if hover { lit(line, theme) } else { line }, Some(spot)));
            }
            continue;
        }
        let mut facts: Vec<String> = [ci["repo"].as_str(), ci["branch"].as_str()]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if let (Some(f), Some(t)) = (ci["failed"].as_u64(), ci["total"].as_u64()) {
            facts.push(format!("{f} of {t} failed"));
        }
        // A train has no checks of its own: its detail says why it fails,
        // in place of its branch, which its title names.
        if ci["train"].as_bool() == Some(true) && !item.detail.is_empty() {
            facts.retain(|f| Some(f.as_str()) != ci["branch"].as_str());
            facts.push(item.detail.clone());
        }
        facts.extend(landing.map(str::to_string));
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
    let pane_x = if wide { list_w + 2 } else { 2 };
    let (about, bar) = match about {
        Some(a) => {
            let top = m.side_top().min(a.last_top(pane_h));
            let total = a.lines.len();
            let bar_x = pane_x + pane_w - 3;
            let bar = pane_bar((total, top, pane_h), (bar_x, pane_top), hits, m);
            (a.window(top, pane_h).lines, bar)
        }
        None => (Vec::new(), Vec::new()),
    };
    let pane_inner = pane_w - 2 - usize::from(!bar.is_empty());
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
        let mut spans = text::fit_spans(
            shown.get(i).map(|(l, _)| l.clone()).unwrap_or_default(),
            list_w,
            Style::new(),
        );
        if wide {
            spans.push(kit::rule_v(theme));
            spans.push(Span::raw(" "));
            spans.extend(text::fit_spans(
                about.get(i).cloned().unwrap_or_default(),
                pane_inner,
                Style::new(),
            ));
            spans.extend(bar.get(i).cloned());
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
            spans.extend(text::fit_spans(
                about.get(i).cloned().unwrap_or_default(),
                pane_inner,
                Style::new(),
            ));
            spans.extend(bar.get(i).cloned());
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

/// The keys the footer names for the view in front, each a key and what it
/// does (a pair with no key is words of its own).
fn hints(m: &Mission, view: Option<&View>, beside: bool, resizes: bool) -> Vec<(String, String)> {
    let pairs = |p: &[(&str, &str)]| -> Vec<(String, String)> {
        p.iter()
            .map(|(k, w)| (k.to_string(), w.to_string()))
            .collect()
    };
    // After `C-w`, its second key.
    if m.window_waits(crate::util::now_ms()) {
        let mut out = pairs(&[
            ("C-w", ""),
            ("h l", "left, right pane"),
            ("w W", "next, last pane"),
        ]);
        if !m.sections().is_empty() {
            out.extend(pairs(&[("j k", "next, last section")]));
        }
        return out;
    }
    // Several views at once: Tab goes between them, Esc back to one; the
    // focused view's pane beside, when it has the focus, scrolls.
    let grid_moves = if m.pane_now() == super::Pane::Beside {
        ("jk", "scroll")
    } else {
        ("jk", "move")
    };
    if m.grid.is_some() && !m.filter.as_ref().is_some_and(|f| f.is_open()) && m.jump.is_none() {
        let mut out = pairs(&[
            ("tab", "next view"),
            grid_moves,
            ("Enter", "actions"),
            ("m", "view actions"),
        ]);
        if beside && crate::settings::is_on("ctrl_w_panes") {
            out.extend(pairs(&[("C-w h l", "panes")]));
        }
        if resizes && !m.snap.side_keys.is_empty() {
            out.push((m.snap.side_keys.clone(), "width".to_string()));
        }
        out.extend(pairs(&[("esc", "one view")]));
        return out;
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
    if let Some(f) = m.filter.as_ref().filter(|f| f.is_open()) {
        let typed: String = f
            .spans(Style::new(), Style::new())
            .into_iter()
            .map(|s| s.content)
            .collect();
        let mut out = vec![(typed, String::new())];
        out.extend(pairs(&[("↑↓", "move")]));
        out.extend(if m.picker {
            pairs(&[("Enter", "actions"), ("esc", "close")])
        } else {
            pairs(&[("Enter", "keep it"), ("esc", "clear")])
        });
        return out;
    }
    // The bar names the keys through its groups and views.
    let next: &[(&str, &str)] = if m.snap.tab_focus {
        &[("tab", "focus")]
    } else {
        &[("tab", "next view")]
    };
    let close: &[(&str, &str)] = &[("?", "keys"), ("esc", "close")];
    let panel = view.is_some_and(|v| v.id == ViewId::Settings) && settings_board_shown(m);
    let with = |first: &[(&str, &str)]| {
        let mut out = pairs(first);
        if beside {
            out.extend(pairs(&[("J K", "scroll")]));
        }
        if (beside || panel) && crate::settings::is_on("ctrl_w_panes") {
            out.extend(pairs(&[("C-w h l", "panes")]));
        }
        if resizes && !m.snap.side_keys.is_empty() {
            out.push((m.snap.side_keys.clone(), "width".to_string()));
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
    match m.pane_now() {
        super::Pane::Panel if view.is_some_and(|v| v.id == ViewId::Slack) => {
            return with(&[("jk", "section"), ("l", "its messages"), ("m", VIEW)]);
        }
        super::Pane::Panel => return with(&[("jk", "category"), ("Enter", "its options")]),
        super::Pane::Beside => {
            return with(&[
                ("jk", "scroll"),
                ("g G", "top, end"),
                ("Enter", ACTS),
                ("m", VIEW),
            ]);
        }
        super::Pane::List => {}
    }
    let Some(view) = view else {
        return with_acts(&[("hjkl", "move")], ACTS, VIEW, filter);
    };
    match view.id {
        ViewId::Reminders => with_acts(super::reminders::hints(), ACTS, VIEW, &[]),
        ViewId::Routines => with_acts(&[("jk", "move")], ACTS, VIEW, filter),
        ViewId::Settings if settings_board_shown(m) => with_acts(
            &[("hl", "category"), ("jk", "option")],
            "its choices",
            VIEW,
            &[("/", "search every category")],
        ),
        ViewId::Settings => with_acts(&[("jk", "move")], "its choices", VIEW, &[("/", "search")]),
        ViewId::Relay => with_acts(&[("jk", "move")], "its choices", VIEW, &[("/", "search")]),
        ViewId::Keys if keys_board_shown(m, view, m.laid().0 as usize) => with_acts(
            &[("hl", "group"), ("jk", "key")],
            ACTS,
            VIEW,
            &[("/", "filter keys, words, tags")],
        ),
        ViewId::Slack if slack_board_shown(view) => {
            let mut keys = Vec::new();
            let threads = crate::settings::is_on("slack_threads");
            if threads && !super::slack_board::beside(m.laid().0 as usize) {
                keys.push(("i", "thread"));
            }
            keys.push(("/", "filter"));
            let hl = if m.panes().contains(&super::Pane::Panel) {
                ("hl", "sections, messages, thread")
            } else {
                ("hl", "section")
            };
            with_acts(&[("jk", "move"), hl], ACTS, VIEW, &keys)
        }
        ViewId::Landing => {
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
        ViewId::Autopilot if pilot_board_shown(view, m.laid().0 as usize) => with_acts(
            &[("hl", "column"), ("jk", "move")],
            ACTS,
            "view actions: its settings, pause",
            &[],
        ),
        ViewId::Autopilot => with_acts(
            &[("jk", "move")],
            ACTS,
            "view actions: its settings, pause",
            filter,
        ),
        // No letter keys (boards 18, 22): `i` still shows the chosen row's
        // pane on a narrow screen, as the view's menu does.
        ViewId::Inbox if super::inbox_board::shown(view) => {
            with_acts(&[("jk", "move")], "actions, marks read", VIEW, filter)
        }
        ViewId::Reopen if super::reopen_board::shown(view) => with_acts(
            &[("jk", "move")],
            "reopen, forget",
            "view actions: tabs or sessions only",
            filter,
        ),
        ViewId::Resources => with_acts(
            &[("jk", "move")],
            "actions: go to the tab, end a process",
            VIEW,
            filter,
        ),
        ViewId::Prs | ViewId::Tickets => {
            with_acts(&[("hl", "column"), ("jk", "move")], ACTS, VIEW, &[])
        }
        ViewId::Usage if m.laid().0 as usize >= super::usage_board::BOARD_AT => {
            with_acts(&[("jk", "tab or service")], ACTS, VIEW, filter)
        }
        _ => with_acts(&[("jk", "move")], ACTS, VIEW, filter),
    }
}

/// The Notifications view as a history by day (`history`):
/// a keycap for each kind with its week's count, the one shown lit, and
/// the week's count and the sidebar's at the right; the column heads over
/// a dashed rule; then each day's label and its notifications, a row each:
/// its time, its kind's mark, its title (bright while the sidebar shows
/// it), what it led to, and whether the sidebar still shows it.
///
/// By where they come from (`notices_grouped`, board 115) the headings take
/// the keycaps' and the days' place: each source's, ▾ open or ▸ folded with
/// its count, which the cursor rests on; a sub-heading dim under it where it
/// is split; and each row's time with its day once it isn't today's.
/// The narrowest the grouped board says a row's day with its time.
const DATED_FROM: usize = 72;

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
    let grouped = m.snap.notices_grouped;
    // A chip for each kind, which a click shows alone (as `m`'s menu does).
    let mut spans = vec![Span::raw(" ")];
    let mut x = 1;
    for key in (1..=7).filter(|_| !grouped) {
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
        let w = text::spans_width(&cap);
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
    if !grouped {
        lines.push(Row::default().line(cols, None));
    }
    // The rows a column narrower while the scroll bar is beside them: the
    // heads and their rule take the two lines before them.
    let items = &view.data.items;
    let room = rows.saturating_sub(1 + lines.len() + 2).max(1);
    let full = cols;
    let cols = if m.snap.rules.scroll_bars && items.len() > room && cols > 1 {
        cols - 1
    } else {
        cols
    };
    // The column heads.
    // Wider where a row's time names its day too ("Nov 13 12:05 PM").
    // In a narrow cell (several views at once) the clock time alone, so
    // the title keeps its room.
    let dated = grouped && cols >= DATED_FROM;
    let when: usize = if dated {
        crate::ui::when::SHORT_WIDTH + 1
    } else {
        9
    };
    const SIDEBAR: usize = 10;
    let rest = cols.saturating_sub(1 + when + 3 + SIDEBAR);
    let led_w = (rest * 2 / 5).min(36);
    let what_w = rest.saturating_sub(led_w + 2);
    let head = |text: &str| Span::styled(text.to_uppercase(), Tone::Dim.bold(theme));
    let pad = |n: usize| Span::raw(" ".repeat(n));
    lines.push(Line {
        spans: kit::fit(
            vec![
                pad(2),
                head(&format!("{:>w$}", "When", w = when - 2)),
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
    let cursor = m.cursor.max(0) as usize;
    let first = (cursor + 1).saturating_sub(room);
    let rows_from = lines.len();
    for (index, item) in items.iter().enumerate().skip(first).take(room) {
        let chosen = index == cursor && m.cursor >= 0;
        let lit = !chosen && m.hover == Some(Spot::Index(index as isize));
        hits.push(HitBox {
            y: (top_row + lines.len()) as u16,
            x0: 0,
            x1: cols.saturating_sub(1) as u16,
            spot: Spot::Index(index as isize),
        });
        let count_of = |item: &Item| {
            item.data
                .get("count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0)
        };
        let spans: Vec<Span<'static>> = if let Some(name) = super::history::group_heading(item) {
            let folded = super::history::folded(item);
            let mut spans = vec![
                Span::styled(if folded { " ▸ " } else { " ▾ " }, Tone::Dim.style(theme)),
                Span::styled(
                    name.to_uppercase(),
                    if chosen {
                        Tone::Text.bold(theme)
                    } else {
                        Tone::Dim.bold(theme)
                    },
                ),
                // Under a filter its rows are the matches, not its count.
                Span::styled(
                    if super::history::under_filter(item) {
                        String::new()
                    } else {
                        format!(" {}", count_of(item))
                    },
                    Tone::Dim.style(theme),
                ),
            ];
            if folded {
                spans.push(Span::styled("   folded", Tone::Faint.style(theme)));
            }
            spans
        } else if super::history::is_part(item) {
            vec![
                Span::styled(format!("   {}", item.title), Tone::Dim.style(theme)),
                Span::styled(format!("  {}", count_of(item)), Tone::Faint.style(theme)),
            ]
        } else if views::is_header(item) {
            let count = count_of(item) as usize;
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
            // Its day and time by source, its day alone in a narrow cell,
            // the clock time alone under its day's heading; a meeting to
            // come's start too, so it fits the column.
            let at = item
                .time
                .map(|t| {
                    if dated {
                        crate::ui::when::short(t, &now)
                    } else if grouped {
                        crate::ui::when::compact(t, &now)
                    } else {
                        crate::ui::when::clock(t, &now)
                    }
                })
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
                Span::styled(format!("{at:>w$} ", w = when - 1), Tone::Faint.style(theme)),
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
    rows_bar(&mut lines[rows_from..], (items.len(), first, room), full, m);
}

/// The key a footer's label names: its arrows and words as the keys
/// they are, a single key as it is written; none for a pair (`jk`, `J K`),
/// whose rows and pane take the click themselves.
fn footer_key(label: &str) -> Option<crate::mux::input::Key> {
    let name = match label {
        "↵" | "Enter" => "Enter",
        "esc" => "Escape",
        "tab" => "Tab",
        l if l.chars().count() == 1 || l.starts_with("M-") || l == "C-w" => l,
        _ => return None,
    };
    crate::mux::input::Key::parse(name)
}

/// The footer, whose keys take a click and a Tab stop: the kit's hints,
/// each key that names one a spot (`Spot::Key`) at row `y`, lit under the
/// mouse.
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
        // The kit's hints, each key in the accent.
        let mut spans = kit::hints(&p, theme);
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
            // Keys whole, one or a space apart (`C-h`, `M-h M-l`, the pane
            // resize keys as keys.json binds them), each a control of its
            // own; a group of single keys (`J K`, `C-w h l`) is no such.
            None if key.split(' ').all(|t| t.chars().count() > 1)
                && key
                    .split(' ')
                    .all(|t| crate::mux::input::Key::parse(t).is_some()) =>
            {
                let mut at = 0;
                key.split(' ')
                    .filter_map(|t| {
                        let start = at;
                        at += t.len() + 1;
                        crate::mux::input::Key::parse(t).map(|k| (k, start, t.len()))
                    })
                    .collect()
            }
            // `C-w` and its directions are a sequence, no key each.
            None if key.starts_with("C-") => Vec::new(),
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
        x += text::spans_width(&spans);
        out.extend(spans);
    }
    out
}

/// The bar's first row, with easy mode's × at its right end when it has
/// one, and `right` flush against that end, a column in, where the row
/// leaves it room two columns clear of what is before it.
fn with_close(
    mut head: Row,
    right: Vec<Span<'static>>,
    m: &Mission,
    cols: usize,
    hits: &mut Vec<HitBox>,
) -> Line {
    let closes = m.snap.close && cols > 8;
    let room = if closes {
        cols - width(CLOSE) - 1
    } else {
        cols
    };
    let right_w = text::spans_width(&right);
    if right_w > 0 && head.width() + 2 + right_w < room {
        head.put(" ".repeat(room - 1 - right_w - head.width()), Style::new());
        head.spans.extend(right);
    }
    if !closes {
        return head.line(cols, None);
    }
    let mut spans = head.fitted(room, Style::new());
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

/// How many tabs the Tabs view holds and how many of them need you, as
/// board 3 ends its header: "11 tabs · 3 need you".
fn tabs_words(tabs: &[TabModel], r: &Rules) -> String {
    let n = tabs.len();
    let needs = model::needs_you(tabs, r).len();
    let all = format!("{n} tab{}", if n == 1 { "" } else { "s" });
    match needs {
        0 => all,
        1 => format!("{all} · 1 needs you"),
        _ => format!("{all} · {needs} need you"),
    }
}

/// What the header shows besides the view's name: the view's facts (the
/// Reminders' or the Routines' words; on Tabs, how many tabs there are and
/// how many need you, at its right end), the filter, and whether the
/// filter takes a click.
struct Bar {
    counts: Vec<Span<'static>>,
    filter: Vec<Span<'static>>,
    filter_hit: bool,
}

/// The header over the view in front (board 64), since its
/// groups and views are listed down the side (src/mission/nav.rs): the
/// view's name, its group and its facts (the Reminders' or the Routines'
/// words, with several views at once how many), then the filter, and on
/// Tabs how many tabs there are and how many need you at its right end
/// (board 3); a rule under it and a line clear, so the view starts where
/// it did under the two-row bar.
fn header(m: &Mission, mut bar: Bar, cols: usize, lines: &mut Vec<Line>, hits: &mut Vec<HitBox>) {
    let theme = &m.snap.theme;
    let gridded = m.grid_shown().len();
    let (name, group) = super::nav::header_words(m, m.view);
    let mut head = Row::default();
    head.put(" ", Style::new())
        .put(name, Tone::Text.bold(theme));
    if !group.is_empty() {
        head.put(format!("   {group}"), Tone::Dim.style(theme));
    }
    if let Some(repo) = m.snap.repo_filter.as_ref().filter(|_| m.view == 0) {
        head.put(format!("   Filtered to {repo}"), Tone::Dim.style(theme));
    }
    if gridded > 0 {
        head.put(
            format!(" · {gridded} views at once"),
            Tone::Dim.style(theme),
        );
    } else if !bar.counts.is_empty()
        && m.current()
            .is_some_and(|v| matches!(v.id, ViewId::Reminders | ViewId::Routines))
    {
        head.put("   ", Style::new());
        head.spans.extend(std::mem::take(&mut bar.counts));
    }
    let right = if gridded == 0 && m.current().is_none() {
        std::mem::take(&mut bar.counts)
    } else {
        Vec::new()
    };
    if !bar.filter.is_empty() && bar.filter_hit {
        let x = head.width() + 2;
        hits.push(HitBox {
            y: 0,
            x0: x as u16,
            x1: (head.width() + text::spans_width(&bar.filter)).saturating_sub(1) as u16,
            spot: Spot::Filter,
        });
    }
    head.spans.extend(bar.filter);
    lines.push(with_close(head, right, m, cols, hits));
    lines.push(Line {
        spans: vec![kit::rule(cols, theme)],
        fill: None,
    });
    lines.push(Line {
        spans: vec![Span::raw(" ".repeat(cols))],
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

/// The bar: the header over the view in front (`header`),
/// else one row naming every view.
pub(super) fn bar(m: &Mission, cols: usize, hits: &mut Vec<HitBox>) -> Vec<Line> {
    let snap = &m.snap;
    let theme = &snap.theme;
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
        filter.extend(f.spans(reverse(fg(YELLOW), filter_lit), fg(theme.dim)));
    } else if filter_stop {
        filter.push(Span::raw("  "));
        filter.push(Span::styled(
            " / filter ",
            reverse(fg(theme.dim), filter_lit),
        ));
    }
    // With the Reminders view in front, the bar's right counts them.
    // And with Routines, them and when the next runs.
    let counts = match view.filter(|v| matches!(v.id, ViewId::Reminders | ViewId::Routines)) {
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
        None => vec![Span::styled(
            tabs_words(&snap.tabs, &snap.rules),
            Tone::Dim.style(theme),
        )],
    };
    let mut lines = Vec::new();
    let bar = Bar {
        counts,
        filter,
        filter_hit: filter_stop || (m.filter.is_some() && snap.tab_focus),
    };
    header(m, bar, cols, &mut lines, hits);
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
            let pilot_board_shown = pilot_board_shown(v, cols);
            let ci_shown = v.id == ViewId::Ci
                && v.loaded
                && v.data.setup.is_none()
                && !v.data.items.is_empty();
            let settings_shown =
                v.id == ViewId::Settings && cols >= super::settings_board::BOARD_AT && v.loaded;
            let relay_shown = v.id == ViewId::Relay && cols >= super::relay_board::BOARD_AT;
            let resources_shown = v.id == ViewId::Resources
                && cols >= super::resources_board::BOARD_AT
                && v.loaded
                && !v.data.items.is_empty();
            if pilot_board_shown {
                pictures = pilot_board(v, cols, rows, m, lines, hits, meters);
            } else if settings_shown {
                super::settings_board::board(v, m, cols, rows, lines, hits, meters);
            } else if relay_shown {
                super::relay_board::board(v, m, cols, rows, lines, hits);
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
                    if matches!(v.id, ViewId::Prs | ViewId::Tickets) {
                        pictures = board_cards(v, columns, cols, room, m, theme, top, lines, hits);
                    } else {
                        board(v, columns, cols, room, m, theme, top, lines, hits);
                    }
                }
            } else if v.id == ViewId::Usage
                && cols >= super::usage_board::BOARD_AT
                && super::usage_board::board(v, m, cols, rows, lines, hits, meters)
            {
            } else if super::inbox_board::shown(v) {
                super::inbox_board::board(v, cols, rows, m, lines, hits, meters);
            } else if super::reopen_board::shown(v) {
                super::reopen_board::board(v, cols, rows, m, lines, hits, meters);
            } else if keys_board_shown(m, v, cols) {
                super::keys_board::board(v, cols, rows, m, lines, hits, meters);
            } else if slack_board_shown(v) {
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
                    thread_open: m.alone.contains(&ViewId::Slack),
                    failed: snap.slack_thread_failed.as_deref(),
                    files_failed: &snap.slack_files_failed,
                    files_kept: &snap.slack_files_kept,
                    hover: match m.hover {
                        Some(Spot::Index(i)) => usize::try_from(i).ok(),
                        _ => None,
                    },
                    hover_section: match m.hover {
                        Some(Spot::Choose(i)) => usize::try_from(i).ok(),
                        _ => None,
                    },
                    sections_focused: m.pane_now() == super::Pane::Panel,
                    emoji_on: crate::day::slack_emoji::on(),
                    emoji: &snap.slack_emoji,
                };
                let (board, at, images) = super::slack_board::board(v, &look);
                lines.extend(board);
                hits.extend(at);
                meters.extend(images);
            } else if v.id == ViewId::Reminders {
                super::reminders::month(v, m, cols, rows, lines, hits);
            } else if v.id == ViewId::Routines {
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

/// With the pane beside a list focused (`C-w l`, a click in it), the rule
/// between them in the accent along the pane's rows: the rule a few
/// columns left of where each row's pane starts, whichever board drew it
/// (alone, or with the space after it, as the PR and ticket boards draw it).
fn light_divider(lines: &mut [Line], hits: &[HitBox], theme: &Theme) {
    let accent = Tone::Accent.style(theme);
    for h in hits.iter().filter(|h| matches!(h.spot, Spot::Side(_))) {
        let Some(line) = lines.get_mut(h.y as usize) else {
            continue;
        };
        let from = (h.x0 as usize).saturating_sub(super::RULE_ROOM as usize);
        let mut x = 0;
        for span in line.spans.iter_mut() {
            let w = width(&span.content);
            if span.content.trim() == "│" && x >= from && x < h.x0 as usize {
                span.style = span.style.patch(accent);
            }
            x += w;
            if x >= h.x0 as usize {
                break;
            }
        }
    }
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
            let resizes = hits.iter().any(|h| matches!(h.spot, Spot::SideEdge(_)));
            let pairs = hints(m, view, beside, resizes);
            // Every footer's keys take a click and a Tab stop,
            // or without.
            let y = rows.saturating_sub(1) as u16;
            row.spans
                .extend(key_footer(&pairs, m, theme, (y, cols), &mut keys_hit));
            row
        }
    };
    lines.push(foot.line(cols, None));
    if m.pane_now() == super::Pane::Beside {
        light_divider(&mut lines, &hits, theme);
    }
    hits.retain(|h| (h.y as usize) < rows.saturating_sub(1));
    hits.extend(keys_hit);
    meters.retain(|image| (image.y + image.height) as usize <= rows.saturating_sub(1));
    (lines, hits, meters)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Landing view's section heading for a repository landing `way`.
    fn landing_header(way: &str, id: &str, title: &str, count: usize) -> Item {
        let mut h = views::header(id, title, count);
        h.data["way"] = serde_json::json!(way);
        h
    }
    use crate::items::ViewData;
    use crate::mission::Snapshot;
    use crate::mux::TabId;
    use crate::sidebar::claude::SessionRef;
    use crate::sidebar::model::tests::rules;

    #[test]
    fn a_focused_pane_beside_lights_the_rule_before_it_alone_or_with_its_space() {
        let theme = Mission::new().snap.theme;
        let accent = Tone::Accent.style(&theme).fg;
        for rule in ["│", "│ "] {
            let mut lines = vec![Line {
                spans: vec![Span::raw("list"), Span::raw(rule), Span::raw("pane")],
                fill: None,
            }];
            let x0 = 4 + width(rule) as u16;
            let hits = [HitBox {
                y: 0,
                x0,
                x1: 20,
                spot: Spot::Side(3),
            }];
            light_divider(&mut lines, &hits, &theme);
            assert_eq!(lines[0].spans[1].style.fg, accent, "{rule:?}");
            assert_ne!(lines[0].spans[2].style.fg, accent);
        }
    }

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

    #[test]
    fn a_cards_foot_says_how_long_its_tab_has_been_open_and_drops_it_first() {
        let words = |spans: Vec<Span<'static>>| -> String {
            spans.iter().map(|s| s.content.as_ref()).collect()
        };
        let mut t = tab(1, "work");
        t.opened = Some(1_000);
        let now = 1_000 + 72 * 60;
        let wide = words(card_foot(&t, Some("main"), 50, now, &Theme::DARK));
        assert!(wide.starts_with("main  no PR"), "{wide}");
        assert!(
            wide.ends_with("opened 1h12m") && width(&wide) == 50,
            "{wide}"
        );
        assert!(wide.contains("opened 1h12m"), "{wide}");
        let narrow = words(card_foot(&t, Some("main"), 16, now, &Theme::DARK));
        assert!(!narrow.contains("opened"), "{narrow}");
        t.opened = None;
        let none = words(card_foot(&t, Some("main"), 50, now, &Theme::DARK));
        assert!(!none.contains("opened"), "{none}");
        // A branch too long for the card is cut, never the checks.
        let long = words(card_foot(
            &t,
            Some("note/a-branch-with-a-long-name"),
            24,
            now,
            &Theme::DARK,
        ));
        assert!(long.ends_with("…  no PR") && width(&long) == 24, "{long}");
    }

    #[test]
    fn a_merged_tabs_card_says_merged_at_its_foot_while_it_works_again() {
        let words = |t: &TabModel| -> String {
            card_foot(t, None, 50, 0, &Theme::DARK)
                .iter()
                .map(|s| s.content.to_string())
                .collect()
        };
        let mut t = tab(1, "work");
        t.state = TabState::Working;
        t.pr = Some(12345);
        t.merge = Some(model::MergeState::Merged);
        assert!(words(&t).contains("#12345 merged"), "{}", words(&t));
        t.landed_on = Some(("abc1234".into(), "main".into()));
        assert!(words(&t).contains("landed abc1234"), "{}", words(&t));
    }

    #[test]
    fn a_landing_outside_switchboard_s_clone_has_its_leak_check_not_run() {
        let landing = serde_json::json!({
            "holding": true,
            "phase": "pushing",
            "record": { "leak": { "skipped": true } },
        });
        let cells = landing_stages(&landing, 0);
        assert_eq!(cells[2], stage_cell(Stage::Waiting, "not run"));
        let found = serde_json::json!({ "holding": true, "record": { "leak": { "found": 0 } } });
        assert_eq!(landing_stages(&found, 0)[2].label, "✓ none");
    }

    #[test]
    fn a_merge_left_with_conflicts_is_a_red_lock_cell_naming_its_files() {
        let landing =
            serde_json::json!({ "holding": true, "phase": "merge conflict src/a.rs b.md" });
        let cells = landing_stages(&landing, 0);
        assert_eq!(cells[3].stage, Stage::Failed);
        assert_eq!(cells[3].label, "✕ conflict");
        assert_eq!(cells[3].note, "src/a.rs b.md");
        assert!(then_words(&landing).contains("resolve the merge"));
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
            tab_focus: true,
            nav_follow: false,
            side_keys: String::new(),
            close: false,
            now_secs: rules().now_secs(),
            tz: jiff::tz::TimeZone::UTC,
            frame: 0,
            screens: Default::default(),
            pilot: None,
            all_tabs: Vec::new(),
            repo_filter: None,
            caps: false,
            cell: None,
            diagrams: Default::default(),
            notice_counts: None,
            notices_grouped: false,
            photos: Default::default(),
            calendar_next: None,
            calendar_notes: Vec::new(),
            inbox_ticket: None,
            inbox_links: Default::default(),
            slack_thread: None,
            slack_pictures: Default::default(),
            slack_emoji: Default::default(),
            slack_work: Vec::new(),
            slack_thread_failed: None,
            slack_files_failed: Default::default(),
            slack_files_kept: Default::default(),
            reopen_shot: None,
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
    fn a_filter_being_typed_leads_the_keys_line() {
        let mut m = mission(vec![tab(1, "one"), tab(2, "two")], Vec::new());
        m.filter = Some(crate::ui::menu::Filter::of("tw", true));
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
    fn a_card_waiting_for_a_slot_shows_its_mark_how_long_and_its_place() {
        use crate::slots::{Hold, Kind, TabSlot};
        let now = rules().now_secs();
        let mut m = board_3();
        let slot = |hold, place| TabSlot {
            kind: Kind::Build,
            hold,
            place,
            since: now - 20,
            command: "cargo build --release".into(),
        };
        m.snap.tabs[1].slot = Some(slot(Hold::Waiting, 2));
        m.snap.tabs[2].slot = Some(TabSlot {
            kind: Kind::Tests,
            ..slot(Hold::Holding { index: 0, cap: 2 }, 0)
        });
        let (lines, _) = screen(&m, 160, 60);
        let text = texts(&lines);
        let now_row = text
            .iter()
            .find(|l| l.contains("cargo test --lib mission"))
            .unwrap_or_else(|| panic!("{text:#?}"));
        assert!(now_row.contains("◌ Bash"), "{text:#?}");
        // How long it has waited, rather than how long the call has run (1m).
        assert!(now_row.contains("<1m"), "how long it has waited: {text:#?}");
        assert!(
            text.iter()
                .any(|l| l.contains("waits for a build slot · 2nd")),
            "{text:#?}"
        );
        assert!(
            text.iter().any(|l| l.contains("test slot 1 of 2")),
            "{text:#?}"
        );
    }

    #[test]
    fn the_tabs_are_cards_under_their_groups_labels_and_a_shelf_under_them() {
        let mut m = board_3();
        m.cursor = 1;
        let (lines, hits) = screen(&m, 160, 60);
        let text = texts(&lines);
        let row = |has: &str| {
            text.iter()
                .position(|l| l.contains(has))
                .unwrap_or_else(|| panic!("{has}: {text:#?}"))
        };
        // The header ends with how many tabs there are and how many need
        // you, in place of the counts' chips.
        assert!(text[0].ends_with("5 tabs · 1 needs you"), "{text:#?}");
        // Each group a label with its count at the right edge, no frame
        // round it, over closed cards whose edges are the plain rule, the
        // group's tone left to their marks and state words (board 143).
        let needs = row("NEEDS YOU");
        assert!(
            text[needs].ends_with('1') && width(&text[needs]) == 159,
            "{text:#?}"
        );
        let working = row("WORKING");
        assert!(text[working].ends_with('2'), "{text:#?}");
        // A working card shows its review loop's round among its chips.
        row("round 2");
        let edge_of = |y: usize, corner: char| {
            lines[y]
                .spans
                .iter()
                .find(|s| s.content.contains(corner))
                .unwrap_or_else(|| panic!("{corner} on {y}: {text:#?}"))
                .style
                .fg
        };
        assert_eq!(edge_of(needs + 1, '╭'), Some(kit::border(&Theme::DARK)));
        assert!(
            text[needs + 1].starts_with(" ╭ ● 1 relay images ─"),
            "{text:#?}"
        );
        // The chosen card's edge alone in the accent, its rows unfilled,
        // its top edge an accent bar with its name dark on it.
        assert_eq!(
            edge_of(working + 1, '╭'),
            Some(Tone::Accent.fg(&Theme::DARK))
        );
        let bar = &lines[working + 1].spans;
        let name = bar
            .iter()
            .find(|s| s.content.contains("summaries"))
            .unwrap_or_else(|| panic!("{text:#?}"));
        assert_eq!(name.style.bg, Some(Tone::Accent.fg(&Theme::DARK)));
        assert_ne!(name.style.fg, Some(Tone::Accent.fg(&Theme::DARK)));
        assert_eq!(
            lines[working + 1]
                .spans
                .iter()
                .rfind(|s| s.content.contains('╮'))
                .and_then(|s| s.style.fg),
            Some(kit::border(&Theme::DARK)),
            "the card beside it the plain rule"
        );
        let chosen_shade = kit::Surface::Chosen.color(&Theme::DARK);
        assert!(
            lines[working + 1..working + 6]
                .iter()
                .all(|l| l.fill.is_none()
                    && l.spans.iter().all(|s| s.style.bg != Some(chosen_shade))),
            "{text:#?}"
        );
        // The waiting card: how long on its edge, its question word for
        // word under its kind, its options with no numbers, and why it asked.
        assert!(text[needs + 1].contains("waiting 4m"), "{text:#?}");
        assert!(text[row("QUESTION · LAYOUT")].starts_with(" │ "));
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
        // No branch known, its checks alone at its foot, over its bottom edge.
        let checks = row("✕ 1 of 13");
        assert!(text[checks].starts_with(" │ ✕ 1 of 13"), "{text:#?}");
        assert!(text[checks + 1].starts_with(" ╰"), "{text:#?}");
        // The working card: this turn, then the call it is on and for how
        // long, then its chips, and its branch with no PR at its foot.
        assert!(text[working + 1].contains("working 2m"), "{text:#?}");
        row("THIS TURN");
        row("Wrote the summary module.");
        let now_line = row("cargo test --lib mission");
        assert!(text[now_line].contains("Bash") && text[now_line].contains("1m"));
        row("+10 −2");
        let foot = row("note/summaries");
        assert!(text[foot].contains("note/summaries  no PR"), "{text:#?}");
        // Each row of cards as tall as its tallest holds: the working
        // cards end at their chips, shorter than the one that asks.
        assert_eq!(foot, row("+10 −2") + 1, "{text:#?}");
        assert!(foot + 1 - working < working - 1 - needs, "{text:#?}");
        // Its screen shows only once s asks for it.
        assert!(!text.iter().any(|l| l.contains("line 40")));
        m.card_screen = Some((TabId(2), 1));
        let shown = texts(&screen(&m, 160, 60).0);
        assert!(shown.iter().any(|l| l.contains("line 40")), "{shown:#?}");
        // Every other stage is the shelf, a line a tab under the cards.
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
    fn a_command_too_long_for_its_card_is_never_answered_from_it() {
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
        let shown = mission(vec![spaced.clone()], Vec::new());
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
    fn a_long_shelf_scrolls_to_its_chosen_line() {
        let tabs: Vec<TabModel> = (1..=40).map(|n| tab(n, &format!("idle {n}"))).collect();
        let mut m = mission(tabs, Vec::new());
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
    fn a_card_follows_the_question_its_screen_shows_and_its_options() {
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
    fn a_waiting_card_s_options_have_no_numbers_or_clicks_of_their_own() {
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
    fn a_tab_with_only_its_background_running_shows_its_last_turn() {
        let mut watching = tab(1, "watching");
        watching.state = TabState::Background;
        watching.background = 1;
        watching.summary = Some("Started the deploy watch.".into());
        let m = mission(vec![watching], Vec::new());
        let text = texts(&screen(&m, 160, 40).0);
        assert!(text.iter().any(|l| l.contains("LAST TURN")), "{text:#?}");
        assert!(!text.iter().any(|l| l.contains("thinking")), "{text:#?}");
    }

    #[test]
    fn a_card_s_foot_is_its_branch_and_checks_with_no_context_meter() {
        let mut t = tab(1, "a tab with quite a long name");
        t.state = TabState::Working;
        t.context = Some(62);
        let mut m = mission(vec![t], Vec::new());
        m.snap.screens.insert(
            TabId(1),
            crate::mission::Screen {
                branch: Some("note/a-branch-named-at-length".into()),
                ..Default::default()
            },
        );
        let text = texts(&screen(&m, 160, 40).0);
        assert!(!text.iter().any(|l| l.contains("ctx ")), "{text:#?}");
        assert!(
            text.iter()
                .any(|l| l.contains("note/a-branch-named-at-length  no PR")),
            "{text:#?}"
        );
        // Its name whole on an edge with room for it, cut only on one
        // without.
        assert!(
            text.iter()
                .any(|l| l.contains("1 a tab with quite a long name  ")),
            "{text:#?}"
        );
        let narrow = texts(&screen(&m, 44, 40).0);
        assert!(
            narrow
                .iter()
                .any(|l| l.contains("1 a tab with quite a long …") && l.contains("working  ╮")),
            "{narrow:#?}"
        );
    }

    #[test]
    fn a_request_that_fits_keeps_every_option_on_its_card() {
        let mut m = board_3();
        if let Some(a) = m.snap.tabs[0].ask.as_mut() {
            // Six lines of question at a card's least width, and three
            // options with a description each.
            a.words = (1..=6)
                .map(|n| format!("line {n} of the question, as long"))
                .collect::<Vec<_>>()
                .join(" ");
            a.options = vec!["First".into(), "Second".into(), "Third".into()];
            a.details = vec![
                "one way".into(),
                "another way".into(),
                "the last way".into(),
            ];
        }
        assert!(ask_fits(&m.snap.tabs[0], None));
        let text = texts(&screen(&m, 125, 60).0);
        let row = |has: &str| {
            text.iter()
                .position(|l| l.contains(has))
                .unwrap_or_else(|| panic!("{has}: {text:#?}"))
        };
        assert!(row("line 6 of the") < row("First"), "{text:#?}");
        assert_eq!(row("the last way"), row("Third") + 1, "{text:#?}");
        // No room left for what it said before asking, so no label of it.
        assert!(
            !text.iter().any(|l| l.contains("BEFORE IT ASKED")),
            "{text:#?}"
        );
        // Its checks still at its foot, under them.
        assert!(row("✕ 1 of 13") > row("the last way"), "{text:#?}");
        // On a screen of 24 rows the card keeps its rows, so the request
        // is still whole on it.
        let short = texts(&screen(&m, 125, 24).0);
        let third = short.iter().position(|l| l.contains("Third")).unwrap();
        assert!(short[third + 1].contains("the last way"), "{short:#?}");
        // With descriptions of two lines each it is more than a card
        // holds, so it isn't answered from the card, which says so.
        if let Some(a) = m.snap.tabs[0].ask.as_mut() {
            a.details = vec!["a description long enough to wrap onto a second line".into(); 3];
        }
        assert!(!ask_fits(&m.snap.tabs[0], None));
        // Away from a card it is still known whole, for a menu that shows
        // the request itself.
        assert!(ask_whole(&m.snap.tabs[0], None));
        let text = texts(&screen(&m, 125, 60).0);
        assert!(
            text.iter().any(|l| l.contains("the tab shows all of it")),
            "{text:#?}"
        );
        // Nor is one whose description has a second paragraph the card
        // would cut.
        if let Some(a) = m.snap.tabs[0].ask.as_mut() {
            a.words = "Which one?".into();
            a.details = vec![
                "Uses the new layout.\n\nRemoves the saved one.".into(),
                String::new(),
                String::new(),
            ];
        }
        assert!(!ask_fits(&m.snap.tabs[0], None));
    }

    #[test]
    fn a_scroll_bar_takes_the_last_column_and_the_cards_end_before_it() {
        let tabs: Vec<TabModel> = (1..=9)
            .map(|n| {
                let mut t = tab(n, &format!("work {n}"));
                t.state = TabState::Working;
                t
            })
            .collect();
        let mut m = mission(tabs, Vec::new());
        m.snap.rules.scroll_bars = true;
        // Three rows of cards on a screen with room for two.
        let text = texts(&screen(&m, 125, 20).0);
        let tops: Vec<&String> = text.iter().filter(|l| l.contains('╭')).collect();
        assert!(!tops.is_empty(), "{text:#?}");
        for top in tops {
            assert_eq!(top.matches('╭').count(), 3, "{text:#?}");
            assert_eq!(top.matches('╮').count(), 3, "{text:#?}");
            let edge = top.rfind('╮').map(|i| width(&top[..i]));
            assert_eq!(edge, Some(123), "the bar's column clear: {text:#?}");
        }
        // With all of it on the screen, they start under their label and
        // end at the screen's edge.
        let text = texts(&screen(&m, 125, 60).0);
        let top = text.iter().find(|l| l.contains('╭')).unwrap();
        assert!(top.starts_with(" ╭") && width(top) == 125, "{text:#?}");
    }

    #[test]
    fn only_what_needs_you_and_what_works_are_cards_and_the_rest_the_shelf() {
        let now = rules().now_secs();
        let mut finished = tab(1, "full titles");
        finished.state = TabState::Done;
        finished.since = Some(now - 60);
        finished.summary = Some("Found why two tests fail.".into());
        let mut reviewing = tab(2, "review round");
        reviewing.state = TabState::Working;
        reviewing.loop_skill = Some("review".into());
        let mut monitoring = tab(3, "fork parity");
        monitoring.state = TabState::Background;
        monitoring.monitors = vec!["fork parity".into()];
        let mut shell = tab(4, "a shell");
        shell.claude = None;
        let idle = tab(5, "quiet");
        let m = mission(
            vec![finished, reviewing, monitoring, shell, idle],
            Vec::new(),
        );
        let text = texts(&screen(&m, 160, 60).0);
        let row = |has: &str| {
            text.iter()
                .position(|l| l.contains(has))
                .unwrap_or_else(|| panic!("{has}: {text:#?}"))
        };
        // A finished tab is a card under Needs you: its last turn, done.
        let needs = row("NEEDS YOU");
        assert!(
            text[needs + 1].starts_with(" ╭ ✓ 1 full titles  ")
                && text[needs + 1].contains("done 1m"),
            "{text:#?}"
        );
        assert_eq!(row("LAST TURN"), needs + 3, "{text:#?}");
        // A review loop's tab and one watching a monitor count as Working,
        // with no section of their own.
        let working = row("WORKING");
        assert!(text[working].ends_with('2'), "{text:#?}");
        assert!(
            text[working + 1].contains("2 review round")
                && text[working + 1].contains("3 fork parity"),
            "{text:#?}"
        );
        assert!(
            !text
                .iter()
                .any(|l| l.contains("REVIEW LOOP") || l.contains("MONITORING")),
            "{text:#?}"
        );
        // An idle tab and one with no Claude are lines on the shelf, the
        // shell's saying where it is.
        let cards = text.iter().filter(|l| l.contains('╭')).count();
        assert_eq!(cards, 2, "{text:#?}");
        let idle = row("IDLE");
        assert!(text[idle + 1].contains("quiet"), "{text:#?}");
        let other = row("OTHER");
        assert!(other > idle, "{text:#?}");
        assert!(
            text[other + 1].contains("a shell") && text[other + 1].contains("/work"),
            "{text:#?}"
        );
        // Every stage is a group of cards or on the shelf's list, so no
        // tab is left out.
        for stage in model::Stage::ALL {
            assert!(
                card_group(stage).is_some() || SHELF.contains(&stage),
                "{stage:?}"
            );
        }
        // A tab whose Claude works while its build fails is a Working
        // card; idle, it is a line under Build failing.
        let mut failing = tab(7, "red build");
        failing.pr = Some(12345);
        failing.pr_state = Some(crate::facts::PrState::Open);
        failing.checks = Some(crate::facts::Checks {
            pass: 1,
            fail: 1,
            pending: 0,
        });
        let mut fixing = failing.clone();
        fixing.state = TabState::Working;
        let mut both = mission(vec![failing.clone()], Vec::new());
        both.snap.rules.build_failing = true;
        assert!(on_shelf_tab(&failing, &both.snap.rules));
        assert!(!on_shelf_tab(&fixing, &both.snap.rules));
        both.snap.tabs = vec![fixing];
        let fixed = texts(&screen(&both, 160, 40).0);
        assert!(
            fixed.iter().any(|l| l.contains("WORKING"))
                && fixed
                    .iter()
                    .any(|l| l.contains("╭") && l.contains("7 red build"))
                && !fixed.iter().any(|l| l.contains("BUILD FAILING")),
            "{fixed:#?}"
        );
        // The keys move through them in that order.
        let g = grid(&m.snap.tabs, &m.snap.rules, 160, false, false);
        assert_eq!(g.order, vec![0, 1, 2, 4, 3]);
        assert_eq!(g.rows, vec![vec![0], vec![1, 2], vec![3], vec![4]]);
    }

    #[test]
    fn the_tabs_follow_the_sidebar_s_repository_filter() {
        // The server marks the tabs of another repository than the filter's
        // (`sidebar_all_marked`): elsewhere, or with a reminder come due.
        let mut here = tab(1, "here work");
        here.state = TabState::Waiting;
        let mut away = tab(2, "away work");
        away.state = TabState::Waiting;
        away.elsewhere = true;
        let mut also = tab(3, "also away");
        also.elsewhere = true;
        let mut due = tab(4, "due elsewhere");
        due.outside_filter = true;
        due.from_repo = Some("api".into());
        let mut m = mission(vec![here, away, also, due], Vec::new());
        m.snap.repo_filter = Some("app".into());
        let text = texts(&screen(&m, 160, 60).0);
        assert!(text[0].contains("Filtered to app"), "{text:#?}");
        assert!(text.iter().any(|l| l.contains("here work")), "{text:#?}");
        // The other repository's tabs folded under one line with their
        // count, a waiting one among them, at the foot.
        let other = text
            .iter()
            .position(|l| l.contains("OTHER REPOS"))
            .expect("Other repos");
        assert!(text[other].trim_end().ends_with('2'), "{text:#?}");
        assert!(
            !text
                .iter()
                .any(|l| l.contains("away work") || l.contains("also away")),
            "{text:#?}"
        );
        // The reminder come due elsewhere above it.
        let due_at = text
            .iter()
            .position(|l| l.contains("due elsewhere"))
            .expect("the reminder");
        assert!(due_at < other, "{text:#?}");
        assert!(text[due_at].contains("api ·"), "{text:#?}");
        // The fold one place the keys rest on, its first tab standing for it.
        let g = grid(&m.snap.tabs, &m.snap.rules, 160, false, true);
        assert_eq!(g.order, vec![0, 3, 1]);
        // Opened, its tabs are lines of their own.
        m.other_repos_open = true;
        let text = texts(&screen(&m, 160, 60).0);
        assert!(
            text.iter().any(|l| l.contains("away work"))
                && text.iter().any(|l| l.contains("also away")),
            "{text:#?}"
        );
        // With no filter, every tab is where its state puts it.
        let plain = mission(vec![tab(1, "here work"), tab(2, "away work")], Vec::new());
        let text = texts(&screen(&plain, 160, 60).0);
        assert!(
            !text[0].contains("Filtered") && !text.iter().any(|l| l.contains("OTHER REPOS")),
            "{text:#?}"
        );
        assert!(text.iter().any(|l| l.contains("away work")), "{text:#?}");
    }

    #[test]
    fn a_landing_tab_s_card_wears_its_way_s_chip() {
        let tabs: Vec<TabModel> = (1..=3)
            .map(|n| {
                let mut t = tab(n, &format!("land {n}"));
                t.state = TabState::Working;
                t.landing = Some(crate::facts::Landing {
                    holding: n == 1,
                    line: n,
                    method: crate::facts::Method::Pr,
                });
                t.way = Some(Way::SwitchboardQueue);
                t
            })
            .collect();
        let m = mission(tabs, Vec::new());
        let text = texts(&screen(&m, 240, 60).0);
        assert!(
            text.iter().any(|l| l.contains("Switchboard's queue")),
            "{text:#?}"
        );
    }

    #[test]
    fn the_cards_fill_any_width_and_give_way_in_board_45_s_order() {
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
        m.cursor = 5;
        for (cols, across) in [(120, 2), (160, 3), (240, 4)] {
            let (lines, _) = screen(&m, cols, 80);
            let text = texts(&lines);
            let first = text.iter().position(|l| l.contains("work 1")).unwrap();
            // Each card the kit's, closed: a corner each on the row of
            // their top edges, the first under its label's first letter
            // and the last ending at the screen's edge.
            let bars: Vec<usize> = text[first]
                .char_indices()
                .filter(|(_, c)| *c == '╭')
                .map(|(i, _)| width(&text[first][..i]))
                .collect();
            assert_eq!(bars.len(), across, "{cols}: {text:#?}");
            assert_eq!(bars[0], 1, "{cols}: {text:#?}");
            assert!(
                text[first].ends_with('╮') && width(&text[first]) == cols,
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
        let g = grid(&m.snap.tabs, &m.snap.rules, 160, false, false);
        assert_eq!(g.rows, vec![vec![0, 1, 2], vec![3, 4], vec![5]]);
        // With room for one card, the shelf folds to its label until Enter
        // on it opens it.
        let g = grid(&m.snap.tabs, &m.snap.rules, 60, false, false);
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
            weekly_limit: 0,
            weekly: None,
            lands_alone: false,
            hands_free: false,
            target: String::new(),
            drained: false,
            failing: String::new(),
            way: None,
        });
        m.cursor = -1;
        let (lines, hits) = screen(&m, 120, 30);
        assert_eq!(lines.len(), 30);
        for l in &lines {
            assert_eq!(width(&l.text()), 120, "{:?}", l.text());
        }
        let text = texts(&lines);
        assert!(
            text[3].starts_with(
                " AUTO Work  2 of 3 working · 4 of 5 open · next: Next thing · 1 skipped"
            ),
            "{}",
            text[3]
        );
        assert_eq!(lines[3].fill, Some(Theme::DARK.highlight), "chosen");
        assert!(
            hits.contains(&HitBox {
                y: 3,
                x0: 0,
                x1: 119,
                spot: Spot::Index(-1)
            }),
            "a click on it chooses it"
        );
        assert!(
            hits.iter()
                .filter(|h| matches!(h.spot, Spot::Index(i) if i >= 0))
                .all(|h| h.y >= 4),
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
        assert!(
            !text.contains("this month and later"),
            "no letter keys: {text}"
        );
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
        // A click.
        let (_, hits) = screen(&m, 120, 20);
        assert!(hits.iter().any(|h| h.spot == Spot::Key(menu)));
        // A filter being typed is no key, whatever its letters.
        m.filter = Some(crate::ui::menu::Filter::of("queue", true));
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
        {
            let m = mission(vec![tab(1, "one")], Vec::new());
            let pairs = hints(&m, None, false, false);
            assert!(has(&pairs, "Enter", "actions") && has(&pairs, "m", "view actions"));
            for id in [
                ViewId::Reopen,
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
                m.view = 1;
                let pairs = hints(&m, m.current(), false, false);
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
            when_missed: crate::routines::WhenMissed::OnWaking,
            prompt: "Read the new issues and sort them by area.".into(),
            model: String::new(),
            tools: vec!["Read".into(), "Glob".into(), "Grep".into()],
            needs: Vec::new(),
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
            live: &live,
            runs: &runs,
        };
        let items = crate::routines::view::items(&[triage, once], &facts);
        let view = list_view(ViewId::Routines, "Routines", items);
        let mut nine = tab(9, "Morning triage");
        nine.index = 9;
        let draw = |cols: usize, pane: bool| {
            let mut m = mission(vec![nine.clone()], vec![view.clone()]);
            m.snap.all_tabs = vec![nine.clone()];
            m.view = 1;
            m.size = (cols as u16, 44);
            if pane {
                m.alone.insert(ViewId::Routines);
            }
            let (lines, hits, _, _) = drawn(&m, cols, 44);
            (texts(&lines).join("\n"), hits)
        };
        let (text, hits) = draw(240, false);
        for want in [
            "RECURRING",
            "ONCE",
            "Morning triage",
            "Every day at 8:45 AM",
            "Read, Glob, Grep",
            "THIS WEEK",
            "PROMPT",
            "added to Claude's system prompt",
            "run now, edit, pause or delete it",
            "ALLOWED TOOLS",
            "another tool fails the run, naming it",
            "HOW IT RUNS",
            "A headless Claude in its directory, opening no tab.",
            "RUNS TODAY",
            "Sorted four new issues",
            "1m  $0.12",
            "2 routines · next at",
            "– missed or still running, so skipped",
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

        assert!(!text.contains("tab 9"), "no run names a tab: {text}");

        // Under 160: the rows' schedule goes.
        let (text, _) = draw(150, false);
        assert!(text.contains("1m  $0.12"), "{text}");
        assert!(!text.contains("Every day at 8:45 AM  "), "{text}");
        assert!(text.contains("PROMPT"), "{text}");

        // Under 140 the pane is behind i.
        let (text, _) = draw(120, false);
        assert!(!text.contains("PROMPT"), "{text}");
        assert!(
            !text.contains("i the chosen routine"),
            "no letter keys: {text}"
        );
        let (text, _) = draw(120, true);
        assert!(
            text.contains("PROMPT") && text.contains("run now, edit, pause or delete it"),
            "{text}"
        );
        assert!(!text.contains("THIS WEEK"), "{text}");

        // A run that reached for a tool off its list: failed, naming it, in
        // the row's chip, the pane's runs and Enter's keycap (board 29).
        let mut needing = routine(
            "r1",
            "Morning triage",
            Schedule::Days {
                days: Days::ALL,
                at: 8 * 60 + 45,
            },
        );
        needing.needs = vec!["Bash".into()];
        // A long list of tools, as the starters have, leaves room for it.
        needing.tools = (0..10)
            .map(|i| format!("mcp__claude_ai_Slack__slack_tool_{i}"))
            .collect();
        let failed = HashMap::from([(
            "r1".to_string(),
            vec![Run {
                due: now_secs - 600,
                ran: now_secs - 600,
                tab: String::new(),
                secs: 22,
                cost: 0.02,
                outcome: Outcome::Failed,
                words: "needs Bash".into(),
            }],
        )]);
        let facts = crate::routines::view::Facts {
            now: facts.now.clone(),
            live: &live,
            runs: &failed,
        };
        let items = crate::routines::view::items(&[needing], &facts);
        let view = list_view(ViewId::Routines, "Routines", items);
        let mut m = mission(vec![nine.clone()], vec![view]);
        m.view = 1;
        m.size = (240, 44);
        let (lines, _, _, _) = drawn(&m, 240, 44);
        let text = texts(&lines).join("\n");
        assert!(
            text.contains("· failed: needs Bash"),
            "the row's chip: {text}"
        );
        assert!(
            text.contains("✕ failed: needs Bash"),
            "the pane's run: {text}"
        );
        assert!(
            text.contains("actions: allow Bash and run it again first"),
            "{text}"
        );
    }

    /// The Reopen view: a closed tab that landed, chosen, with
    /// its last screen and 41 turns, and a past session.
    fn reopen_mission() -> (Mission, i64) {
        use crate::usage::closed::Closing;
        use crate::usage::shots::{Shot, ShotLine};
        let now = 1_700_000_000_000;
        let closing = Closing {
            at: now - 60_000,
            name: "api".into(),
            kind: "closed".into(),
            path: Some("/w/api".into()),
            session: Some("s1".into()),
            lived: 3_600_000,
            prompt: "make the api answer faster".into(),
            outcome: "landed a3f1c2e".into(),
            panes: 2,
            screen: Some("1-1.json".into()),
            ..Closing::default()
        };
        let sessions = vec![crate::cmd::wt_revive::SessionRow {
            id: "s1".into(),
            dir: "/w/api".into(),
            when: (now - 120_000) / 1000,
            prompt: String::new(),
            turns: 41,
            resume: String::new(),
        }];
        let items = crate::usage::reopen::items(
            &[("s1".into(), closing)],
            &sessions,
            &Default::default(),
            &Default::default(),
            now,
            &|_| "today".into(),
        );
        let mut m = mission(Vec::new(), vec![list_view(ViewId::Reopen, "Reopen", items)]);
        m.snap.now_secs = now / 1000;
        m.snap.reopen_shot = Some(std::sync::Arc::new(Shot {
            lines: vec![ShotLine {
                text: "$ cargo test".into(),
            }],
        }));
        m.view = 1;
        m.cursor = 1;
        (m, now)
    }

    #[test]
    fn reopen_s_worktrees_are_a_line_each_the_merged_ones_folded() {
        let (mut m, now) = reopen_mission();
        let row = |tree: &str, pr: Option<&str>, standing: &str, merged: bool| {
            Item::new(tree, tree)
                .target(crate::items::Target::Dir(tree.into()))
                .data(serde_json::json!({
                    "worktree": tree.trim_start_matches("/w/"),
                    "repo": "/code/web-app",
                    "branch": tree.trim_start_matches("/w/"),
                    "pr": pr,
                    "standing": standing,
                    "standingTone": "done",
                    "merged": merged,
                    "when": now / 1000 - 3600,
                    "prompt": "draw the condensed sidebar",
                    "counts": if merged { serde_json::Value::Null } else {
                        serde_json::json!({ "default": "main", "ahead": 4, "toPush": 0 })
                    },
                }))
        };
        let map = vec![
            row("/w/condensed", None, "no PR", false),
            row("/w/rate-limits", Some("#12345"), "Approved", false),
            row("/w/retry", Some("#12345"), "Merged", true),
        ];
        let mut items = m.snap.views[0].data.items.clone();
        items.extend(crate::usage::reopen::worktrees(&map));
        crate::usage::reopen::fold_merged(&mut items, false);
        crate::usage::reopen::keep(&mut items, crate::usage::reopen::Kind::Both);
        let chosen = items.iter().position(|i| i.title == "condensed").unwrap();
        m.snap.views[0].data.items = items;
        m.cursor = chosen as isize;
        let t = texts(&drawn(&m, 160, 40).0);
        let at = t
            .iter()
            .position(|l| l.contains("condensed") && l.contains("web-app"))
            .unwrap();
        assert!(
            t[at].contains("no PR")
                && t[at].contains("4 commits ahead")
                && t[at].ends_with("1h ago"),
            "{:?}",
            t[at]
        );
        assert_eq!(width(&t[at]), 160, "{:?}", t[at]);
        assert!(t[at + 1].contains("rate-limits") && t[at + 1].contains("PR 12345 · approved"));
        assert!(
            t[at + 2].contains("▸ 1 merged worktree, folded"),
            "{:?}",
            t[at + 2]
        );
        assert!(!t.iter().any(|l| l.contains("retry")));
        assert!(
            t.iter()
                .any(|l| l.contains("Branch condensed · 4 commits ahead of main"))
        );
        assert!(t.iter().any(|l| l.contains("> draw the condensed sidebar")));
    }

    #[test]
    fn reopen_rows_and_the_chosen_one_s_pane_fill_any_width() {
        let (mut m, _) = reopen_mission();
        for cols in [240, 160] {
            let (lines, _, images, _) = drawn(&m, cols, 40);
            let t = texts(&lines);
            let row = t
                .iter()
                .position(|l| l.contains("api") && l.contains("tab"))
                .unwrap();
            assert!(
                t[row].contains("/w/api") && t[row].contains("✓ landed a3f1c2e"),
                "{t:#?}"
            );
            assert!(
                t[row].ends_with("1m ago"),
                "when at the right edge: {:?}",
                t[row]
            );
            assert_eq!(width(&t[row]), cols, "{:?}", t[row]);
            assert!(t[row + 1].contains("> make the api answer faster"));
            assert!(
                t.iter()
                    .any(|l| l.contains("closed 1m ago · 2 panes · Claude session s1…"))
            );
            assert!(t.iter().any(|l| l.contains(
                "Reopening brings back a tab in /w/api, named as it was, with its Claude resumed."
            )));
            let shot = images
                .iter()
                .find(|i| matches!(i.shape, Shape::Screen { .. }))
                .expect("the last screen");
            assert!(
                shot.width as usize > cols / 2,
                "the picture stretches: {shot:?}"
            );
            let facts = t.iter().position(|l| l.contains("41 turns")).unwrap();
            assert!(
                (shot.y as usize..(shot.y + shot.height) as usize).contains(&facts),
                "the facts beside the picture at {cols}"
            );
        }
        // At 120: the facts, the directory and the outcome's words gone,
        // and the pane behind `i`.
        let (lines, _, images, _) = drawn(&m, 120, 40);
        let t = texts(&lines);
        let row = t
            .iter()
            .position(|l| l.contains("api") && l.contains("tab"))
            .unwrap();
        assert!(
            !t[row].contains("/w/api") && !t[row].contains("landed"),
            "{:?}",
            t[row]
        );
        assert!(
            t[row].contains('✓') && t[row].ends_with("1m ago"),
            "{:?}",
            t[row]
        );
        assert!(!t.iter().any(|l| l.contains("Reopening brings back")));
        assert!(images.is_empty());
        assert!(
            !t.last().unwrap().contains("i what it brings back"),
            "no letter keys: {:?}",
            t.last()
        );
        m.alone.insert(ViewId::Reopen);
        let (lines, _, images, _) = drawn(&m, 120, 40);
        let t = texts(&lines);
        assert!(t.iter().any(|l| l.contains("Reopening brings back")));
        let shot = images
            .iter()
            .find(|i| matches!(i.shape, Shape::Screen { .. }))
            .unwrap();
        let facts = t.iter().position(|l| l.contains("41 turns")).unwrap();
        assert!(
            facts >= (shot.y + shot.height) as usize,
            "the facts under the picture"
        );
        // With no row chosen (a view beside it has the focus), the rows.
        m.cursor = -100;
        let t = texts(&drawn(&m, 120, 40).0);
        assert!(
            t.iter().any(|l| l.contains("api") && l.contains("tab")),
            "{t:#?}"
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
    fn a_train_is_a_run_of_its_own_and_each_run_says_what_it_means_for_landing() {
        let failing = Item::new("f", "#2027 sidebar header").data(serde_json::json!({ "ci": {
            "number": 2027, "repo": "acme/switchboard", "state": "failing",
            "failed": 2, "total": 9,
            "landing": "Switchboard's merge queue, 3rd in Switchboard's queue",
        } }));
        let github = Item::new("g", "#208 billing page")
            .detail("4 of 5 passed, 1 running")
            .data(serde_json::json!({ "ci": {
                "number": 208, "repo": "acme/web-app", "state": "running",
                "landing": "GitHub's merge queue, 3rd",
            } }));
        let train = Item::new("train:/r:ci/train-3", "train 3 #2040 + #219")
            .detail("testing")
            .data(serde_json::json!({ "ci": {
                "number": 0, "repo": "acme/switchboard", "state": "running", "train": true,
                "landing": "Switchboard's merge queue",
            } }));
        let stuck = Item::new("train:/r:ci/train-5", "train 5 #31")
            .detail("passed, merge failed 2 times")
            .data(serde_json::json!({ "ci": {
                "number": 0, "repo": "acme/switchboard", "state": "failing", "train": true,
                "branch": "ci/train-5", "landing": "Switchboard's merge queue",
            } }));
        let items = views::ci_sections(&[failing, github, train, stuck]);
        let mut m = mission(Vec::new(), vec![list_view(ViewId::Ci, "CI", items)]);
        m.view = 1;
        let text = texts(&screen(&m, 200, 30).0);
        let line = |words: &str| {
            text.iter()
                .position(|l| l.contains(words))
                .unwrap_or_else(|| panic!("{words}: {text:#?}"))
        };
        assert!(
            text[line("#2027 sidebar header") + 1]
                .contains("2 of 9 failed · Switchboard's merge queue, 3rd")
        );
        assert!(
            text[line("#208 billing page") + 1]
                .contains("web-app · GitHub's merge queue, 3rd · 4 of 5 passed")
        );
        assert!(
            text[line("train 5 #31") + 1].contains("passed, merge failed 2 times"),
            "{text:#?}"
        );
        let at = line("train 3 #2040 + #219");
        assert!(
            text[at + 1].contains("switchboard · Switchboard's merge queue · testing"),
            "{text:#?}"
        );
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
    fn the_ci_view_heads_its_sections_and_shows_the_chosen_check_s_step_beside() {
        let mut working = tab(6, "cache");
        working.pr = Some(27);
        working.pr_repo = Some("acme/app".into());
        let mut m = mission(vec![working], vec![ci_view()]);
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
    fn the_ci_view_draws_on_a_screen_with_room_for_almost_nothing() {
        let mut m = mission(Vec::new(), vec![ci_view()]);
        m.view = 1;
        m.cursor = 1;
        for rows in 0..4 {
            let mut lines = Vec::new();
            let mut hits = Vec::new();
            ci_rows(&m.snap.views[0], 100, rows, &m, &mut lines, &mut hits);
        }
    }

    #[test]
    fn the_chosen_check_is_the_one_beside_and_a_narrow_screen_puts_it_under() {
        let mut m = mission(Vec::new(), vec![ci_view()]);
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
    fn the_header_names_the_view_and_its_group_over_a_rule() {
        let mut m = mission(
            vec![tab(1, "api")],
            vec![
                list_view(ViewId::WorkMap, "Work map", vec![]),
                list_view(ViewId::Prs, "PRs", vec![]),
                list_view(ViewId::Settings, "Settings", vec![]),
            ],
        );
        m.view = 2;
        let (lines, hits) = screen(&m, 100, 12);
        let t = texts(&lines);
        assert!(t[0].starts_with(" PRs   Code"), "{:?}", t[0]);
        assert_eq!(t[1], "─".repeat(100));
        assert_eq!(t[2].trim(), "");
        let name = lines[0].spans.iter().find(|s| s.content == "PRs").unwrap();
        assert!(name.style.add_modifier.contains(Modifier::BOLD));
        assert!(
            hits.iter()
                .all(|h| !matches!(h.spot, Spot::Group(_) | Spot::View(_))),
            "the groups and views are listed down the side"
        );
        m.view = 1;
        let t = texts(&screen(&m, 100, 12).0);
        assert!(t[0].starts_with(" Map   Work"), "{:?}", t[0]);
        m.view = 0;
        let t = texts(&screen(&m, 100, 12).0);
        assert!(t[0].starts_with(" Tabs   Work"), "{:?}", t[0]);
    }

    fn work_views() -> Vec<View> {
        vec![
            list_view(ViewId::Landing, "Landing", vec![]),
            list_view(ViewId::Autopilot, "Autopilot", vec![]),
            list_view(ViewId::Routines, "Routines", vec![]),
            list_view(ViewId::WorkMap, "Work map", vec![]),
            list_view(ViewId::Reopen, "Reopen", vec![]),
        ]
    }

    #[test]
    fn the_list_of_views_scrolls_to_keep_the_chosen_one_in_sight() {
        let mut m = mission(
            vec![tab(1, "api")],
            vec![
                list_view(ViewId::WorkMap, "Work map", vec![]),
                list_view(ViewId::Prs, "PRs", vec![]),
                list_view(ViewId::Settings, "Settings", vec![]),
                list_view(ViewId::Keys, "Keys", vec![]),
            ],
        );
        let text = |m: &Mission| {
            let (lines, hits, top) = super::super::nav::lines(m, 34, 10, false);
            (texts(&lines).join("\n"), hits, top)
        };
        let (all, _, top) = text(&m);
        assert_eq!(top, 0);
        assert!(all.contains("Tabs") && !all.contains("Keys"), "{all}");
        // Keys in front: the list moves to keep its row in sight, a click
        // on it landing there.
        m.view = 4;
        let (all, hits, top) = text(&m);
        assert!(top > 0 && all.contains("Keys"), "{all}");
        let keys = hits.iter().find(|h| h.spot == Spot::View(4)).unwrap();
        assert!(
            texts(&super::super::nav::lines(&m, 34, 10, false).0)[keys.y as usize].contains("Keys")
        );
        // Scrolled back up by the wheel, it stays there.
        m.nav_top = 0;
        m.nav_scrolled = true;
        assert_eq!(text(&m).2, 0);
    }

    #[test]
    fn tabs_focus_on_the_filter_is_on_the_header() {
        let mut m = mission(vec![tab(1, "api")], work_views());
        m.snap.tab_focus = true;
        m.focus = Some(Focus::Filter);
        let (lines, hits) = screen(&m, 48, 12);
        let t = texts(&lines);
        assert!(
            t[0].contains(" / filter") && t[0].ends_with("1 tab"),
            "{:?}",
            t[0]
        );
        let filter = hits.iter().find(|h| h.spot == Spot::Filter).unwrap();
        assert_eq!(filter.y, 0);
        assert!(filter.x1 < 48, "{filter:?}");
    }

    #[test]
    fn the_slack_relay_views_plain_list_shows_its_labels_and_whole_counts() {
        let items = super::super::relay_board::items(&[], None);
        let mut m = mission(
            Vec::new(),
            vec![list_view(ViewId::Relay, "Slack relay", items)],
        );
        m.view = 1;
        let text = texts(&screen(&m, 79, 30).0).join("\n");
        assert!(text.contains("Not running"), "{text}");
        assert!(text.contains("0 sent by itself, 0 notified"), "{text}");
        assert!(text.contains("Finished turns"), "{text}");
        assert!(text.contains("Failed commits, pushes"), "{text}");
        assert!(text.contains("Thread only, 0 today"), "{text}");
    }

    #[test]
    fn settings_rows_line_up_as_name_value_and_what_it_does() {
        let items = views::settings_items(&[]);
        let mut m = mission(
            Vec::new(),
            vec![list_view(ViewId::Settings, "Settings", items)],
        );
        m.view = 1;
        let (lines, _) = screen(&m, 100, 20);
        let text = texts(&lines);
        assert!(text[3].starts_with(" THIS COMPUTER"), "{text:#?}");
        assert!(
            text[4].contains("GitHub")
                && text[4].contains("this computer")
                && (text[4].contains(" on ") || text[4].contains(" off ")),
            "{}",
            text[4]
        );
    }

    #[test]
    fn settings_is_a_board_of_categories_and_the_options_of_the_one_in_front() {
        let mut all = crate::settings::schema();
        let changed = all
            .iter_mut()
            .find(|o| o.category == "Sidebar" && o.choices.is_none())
            .unwrap();
        changed.value = serde_json::Value::Bool(!crate::util::truthy(&changed.default));
        let (key, label) = (changed.key.clone(), changed.label.clone());
        let items = views::settings_items_of(&all, &[]);
        let at = items
            .iter()
            .position(|i| i.id == format!("setting:{key}"))
            .unwrap();
        let mut m = mission(
            Vec::new(),
            vec![list_view(ViewId::Settings, "Settings", items.clone())],
        );
        m.view = 1;
        m.size = (160, 48);
        m.cursor = at as isize;
        let (lines, hits, images, _) = drawn(&m, 160, 48);
        let text = texts(&lines);
        let all_text = text.join("\n");
        // The panel: the Sidebar's chip of 1 before its count, the
        // category in front resting while its options have the focus.
        let sidebar = text
            .iter()
            .find(|l| l.starts_with("  Sidebar ") && !l.starts_with("  Sidebar layout"))
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
        m.filter = Some(crate::ui::menu::Filter::of("notif", false));
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
    fn the_pr_board_draws_each_pr_as_a_card() {
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
        m.view = 1;
        m.cursor = 1;
        let (lines, hits) = screen(&m, 180, 30);
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
            text[11].contains("╭ #2  ") && text[11].contains(" ✕  ╮"),
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
        m.view = 1;
        m.cursor = 0;
        m.size = (160, 20);
        let columns = crate::sources::prboard::COLUMNS;
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
    fn the_pr_board_s_landing_column_says_where_each_pr_is_in_its_way() {
        let item = |n: u32, landing: &str, way: &str| {
            Item::new(n.to_string(), format!("#{n} PR {n}"))
                .group(crate::sources::prboard::LANDING)
                .data(serde_json::json!({
                    "pr": { "number": n }, "checks": "success",
                    "landing": landing, "way": way, "body": "",
                }))
        };
        let items = vec![
            item(2040, "train 3 · CI 6m", "switchboard-queue"),
            item(208, "GitHub's · 3rd", "github-queue"),
        ];
        let mut m = mission(Vec::new(), vec![list_view(ViewId::Prs, "PRs", items)]);
        m.view = 1;
        m.cursor = 1;
        let mut lines = Vec::new();
        let mut hits = Vec::new();
        board_cards(
            &m.snap.views[0],
            crate::sources::prboard::COLUMNS,
            200,
            24,
            &m,
            &Theme::DARK,
            0,
            &mut lines,
            &mut hits,
        );
        let text = texts(&lines);
        assert!(text[0].contains("LANDING 2"), "{}", text[0]);
        for words in ["train 3 · CI 6m", "GitHub's · 3rd"] {
            assert!(text.iter().any(|l| l.contains(words)), "{words}: {text:#?}");
        }
        // The pane names the chosen PR's way and when it is done there.
        assert!(
            text.iter()
                .any(|l| l.contains("GitHub's merge queue · done when GitHub merges")),
            "{text:#?}"
        );
    }

    #[test]
    fn the_pr_board_names_reviewers_or_the_tab_and_shows_the_chosen_pr_beside_it() {
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
        m.view = 1;
        m.cursor = 1;
        let columns = crate::sources::prboard::COLUMNS;
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
    fn a_resized_pane_beside_a_board_keeps_its_width_within_bounds() {
        let mut m = Mission::new();
        assert_eq!(side_width(&m, "prs", 160), 50, "its share, unresized");
        assert_eq!(side_width(&m, "slack", 160), 0, "no board of these");
        m.side_widths.insert("prs".into(), 65);
        assert_eq!(side_width(&m, "prs", 160), 65);
        assert_eq!(side_width(&m, "tickets", 160), 50, "each board its own");
        assert_eq!(side_width(&m, "prs", 120), 0, "under the board there");
        m.side_widths.insert("prs".into(), 10);
        assert_eq!(side_width(&m, "prs", 160), SIDE_MIN);
        m.side_widths.insert("prs".into(), 150);
        assert_eq!(side_width(&m, "prs", 160), 160 - BOARD_MIN);
        m.side_widths.insert("autopilot".into(), 150);
        assert_eq!(side_width(&m, "autopilot", 220), 220 - PILOT_BOARD_AT);
    }

    #[test]
    fn a_resized_pane_lays_the_description_out_at_its_width_with_an_edge_to_drag() {
        let body = "word ".repeat(40);
        let items = vec![
            Item::new("1", "ABC-7 Fix it")
                .group("Todo")
                .data(serde_json::json!({ "identifier": "ABC-7", "description": body })),
        ];
        let mut m = mission(
            Vec::new(),
            vec![list_view(ViewId::Tickets, "Tickets", items)],
        );
        m.snap.side_keys = "C-M-S-h C-M-S-l".into();
        m.view = 1;
        m.cursor = 0;
        let columns = crate::sources::tickets::COLUMNS;
        let drawn = |m: &Mission| {
            let mut l = Vec::new();
            let mut h = Vec::new();
            board_cards(
                &m.snap.views[0],
                columns,
                160,
                20,
                m,
                &Theme::DARK,
                0,
                &mut l,
                &mut h,
            );
            (texts(&l), h)
        };
        let (text, hits) = drawn(&m);
        assert_eq!(
            text[0].chars().position(|c| c == '│'),
            Some(110),
            "{}",
            text[0]
        );
        m.side_widths.insert("tickets".into(), 70);
        let (wide, hits_wide) = drawn(&m);
        assert_eq!(
            wide[0].chars().position(|c| c == '│'),
            Some(90),
            "{}",
            wide[0]
        );
        let words = |text: &[String]| {
            text.iter()
                .filter_map(|l| l.split_once('│').map(|(_, r)| r.matches("word").count()))
                .max()
                .unwrap_or(0)
        };
        assert!(words(&wide) > words(&text), "laid out wider: {wide:#?}");
        assert!(
            hits.iter()
                .any(|h| h.spot == Spot::SideEdge(50) && h.x0 == 110)
        );
        assert!(
            hits_wide
                .iter()
                .any(|h| h.spot == Spot::SideEdge(70) && h.x0 == 90)
        );
        let footer = hints(&m, m.current(), true, true);
        assert!(
            footer.contains(&("C-M-S-h C-M-S-l".to_string(), "width".to_string())),
            "{footer:?}"
        );
        // Keys bound whole, each a control of its own that presses it.
        let pressed = |m: &Mission| -> Vec<String> {
            let mut keys = Vec::new();
            key_footer(
                &hints(m, m.current(), true, true),
                m,
                &Theme::DARK,
                (0, 200),
                &mut keys,
            );
            keys.iter()
                .filter_map(|h| match h.spot {
                    Spot::Key(k) => Some(k.notation()),
                    _ => None,
                })
                .collect()
        };
        let defaults = pressed(&m);
        assert!(
            defaults.contains(&"C-M-S-h".to_string()) && defaults.contains(&"C-M-S-l".to_string()),
            "{defaults:?}"
        );
        m.snap.side_keys = "C-h C-l".into();
        let ctrl = pressed(&m);
        assert!(
            ctrl.contains(&"C-h".to_string()) && ctrl.contains(&"C-l".to_string()),
            "{ctrl:?}"
        );
        m.snap.side_keys = "C-h".into();
        assert!(pressed(&m).contains(&"C-h".to_string()), "one bound alone");
        m.snap.side_keys = "M-h M-l".into();
        let pressed = pressed(&m);
        assert!(
            pressed.contains(&"M-h".to_string()) && pressed.contains(&"M-l".to_string()),
            "{pressed:?}"
        );
        assert_eq!(
            pressed.iter().filter(|k| *k == "h").count(),
            1,
            "h once, for moving: {pressed:?}"
        );
        // With several views shown, the focused one's pane too.
        m.grid = Some(super::super::grid::Grid::new(Default::default()));
        let footer = hints(&m, m.current(), true, true);
        assert!(footer.iter().any(|(_, w)| w == "width"), "{footer:?}");
        m.grid = None;
        let footer = hints(&m, m.current(), true, false);
        assert!(
            !footer.iter().any(|(_, w)| w == "width"),
            "no pane beside: {footer:?}"
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
    fn the_pr_and_ticket_boards_draw_the_kit_s_cards_at_any_width() {
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
                crate::sources::prboard::COLUMNS,
                "IN REVIEW 1",
                "╭ #1  ",
            ),
            (
                ViewId::Tickets,
                "Tickets",
                ticket,
                crate::sources::tickets::COLUMNS,
                "TODO 1",
                "╭ AB-1  ",
            ),
        ] {
            let mut m = mission(Vec::new(), vec![list_view(id, title, vec![item])]);
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
    fn the_ticket_board_draws_each_ticket_as_a_card_and_the_chosen_one_beside_it() {
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
        m.view = 1;
        m.cursor = 0;
        let columns = crate::sources::tickets::COLUMNS;
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
            text[2].contains("╭ AB-1  ") && text[2].contains(" !!!  ╮"),
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
    fn the_prs_merged_this_week_are_a_row_under_the_columns() {
        let item = |n: u32, column: &str| {
            let mut i = Item::new(n.to_string(), format!("#{n} PR {n}"))
                .group(column)
                .data(serde_json::json!({ "pr": { "number": n }, "mergedBy": "ana" }));
            i.time = Some((rules().now_secs() - 86_400) * 1000);
            i
        };
        let items = vec![item(1, "In review"), item(2, "Merged"), item(3, "Merged")];
        let mut m = mission(Vec::new(), vec![list_view(ViewId::Prs, "PRs", items)]);
        m.cursor = 2;
        let columns = crate::sources::prboard::COLUMNS;
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
    fn a_narrow_pr_board_shows_the_chosen_pr_s_column() {
        let item = |n: u32, column: &str| {
            Item::new(n.to_string(), format!("#{n} PR {n}"))
                .group(column)
                .data(serde_json::json!({ "pr": { "number": n } }))
        };
        let columns = crate::sources::prboard::COLUMNS;
        let last = columns[columns.len() - 2];
        let items = vec![item(1, columns[0]), item(2, last)];
        let mut m = mission(Vec::new(), vec![list_view(ViewId::Prs, "PRs", items)]);
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
            text[1].starts_with(&columns[0].to_uppercase()) && !text[1].contains("LANDING"),
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
        assert!(text[1].contains("LANDING 1"), "{}", text[1]);
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
            text[3].contains("Tab") && text[3].contains("% CPU"),
            "{}",
            text[3]
        );
        assert!(text[4].contains("1 build") && text[4].contains("93.0"));
        assert!(text[10].contains("All tabs"), "{text:#?}");
    }

    #[test]
    fn resources_lists_each_kind_of_slot_its_holders_free_slots_and_waiters() {
        use crate::mux::PaneId;
        use crate::slots::{Board, Hold, Kind, Use};
        use crate::usage::resources::{self, History, Proc, TabPanes};
        let now = rules().now_secs();
        let tabs = [
            TabPanes {
                tab: TabId(7),
                index: 7,
                name: "rust notes".into(),
                pids: vec![700],
            },
            TabPanes {
                tab: TabId(3),
                index: 3,
                name: "uncommitted".into(),
                pids: vec![300],
            },
        ];
        let list = vec![
            Proc {
                pid: 700,
                ppid: 1,
                name: "zsh".into(),
                cpu: 0.0,
                memory: 1 << 20,
            },
            Proc {
                pid: 300,
                ppid: 1,
                name: "zsh".into(),
                cpu: 0.0,
                memory: 1 << 20,
            },
        ];
        let mut data = resources::view(&tabs, Some(&list), Some(1), &History::default(), None);
        let mut board = Board::default();
        let report = |pid, pane, kind, hold, command: &str| Use {
            pid,
            child: None,
            pane: PaneId(pane),
            kind,
            hold,
            since: now - 130,
            came: pid as i64,
            command: command.into(),
        };
        board.report(report(
            1,
            7,
            Kind::Build,
            Hold::Holding { index: 0, cap: 1 },
            "cargo build",
        ));
        board.report(report(
            2,
            3,
            Kind::Build,
            Hold::Waiting,
            "cargo check --all",
        ));
        let tab_of = |p: PaneId| Some((TabId(p.0), p.0 as usize, format!("tab {}", p.0)));
        resources::with_slots(&mut data, &board.kinds(|_| 1), tab_of, now);
        let view = View {
            id: ViewId::Resources,
            title: "Resources".into(),
            data,
            loaded: true,
        };
        let mut m = mission(
            vec![tab(7, "rust notes"), tab(3, "uncommitted")],
            vec![view],
        );
        m.view = 1;
        super::super::resources_board::Sort::Cpu.sort(&mut m.snap.views[0].data.items);
        m.cursor = 1;
        let (lines, hits, _, _) = drawn(&m, 160, 30);
        let text = texts(&lines);
        let all = text.join("\n");
        let at = |has: &str| {
            text.iter()
                .position(|l| l.contains(has))
                .unwrap_or_else(|| panic!("{has}: {all}"))
        };
        let head = at("BUILDS");
        assert!(text[head].contains("1 of 1 held"), "{all}");
        assert!(
            text[head].contains("TEST RUNS") && text[head].contains("0 of 1 held"),
            "{all}"
        );
        assert!(
            text[head + 1].contains("7 tab 7") && text[head + 1].contains("cargo build"),
            "{all}"
        );
        assert!(
            text[head + 1].contains("· a free slot"),
            "the test runs' free slot: {all}"
        );
        let waiter = at("◌ 3 tab 3");
        assert!(
            text[waiter].contains("1st") && text[waiter].contains("2m"),
            "{all}"
        );
        assert!(all.contains("nothing waits"), "{all}");
        // The waiter is the chosen row, and a click on it chooses it.
        assert!(text[waiter].contains('▌'), "{all}");
        assert!(
            hits.iter()
                .any(|h| h.y as usize == waiter && h.spot == Spot::Index(1))
        );
        // Its tab's row in the table says what it waits for.
        assert!(
            all.contains("cargo check, waiting for a build slot"),
            "{all}"
        );
        assert!(
            !all.contains("CPU …"),
            "no reading of the computer yet: {all}"
        );
    }

    #[test]
    fn resources_is_a_board_of_the_computer_and_each_tab() {
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
            way: None,
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
            weekly_limit: 0,
            weekly: None,
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
            phases: Vec::new(),
            now: 0,
            leak_checked: true,
        }
    }

    #[test]
    fn autopilot_keeps_the_chosen_card_in_sight() {
        let rows: Vec<_> = (1..=10)
            .map(|n| pilot_row(n, &format!("item-{n}"), TabState::Working))
            .collect();
        let list = crate::autopilot::view::items(
            &pilot_summary(),
            "",
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
    fn autopilot_times_each_card_until_it_lands() {
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
    fn an_item_whose_start_failed_says_why_on_its_card_and_beside_the_board() {
        let mut list = crate::autopilot::view::items(
            &pilot_summary(),
            "",
            "",
            &[],
            &["Offline item".into(), "Later".into()],
            &[],
            &[],
            &std::collections::HashMap::new(),
        );
        let failed = std::collections::HashMap::from([(
            "Offline item".to_string(),
            "couldn't fetch origin's main".to_string(),
        )]);
        crate::autopilot::view::say_failed_starts(&mut list, &failed);
        let chosen = list
            .iter()
            .position(|i| i.id == "next:Offline item")
            .unwrap();
        let mut m = mission(
            Vec::new(),
            vec![list_view(ViewId::Autopilot, "Autopilot", list)],
        );
        m.view = 1;
        m.cursor = chosen as isize;
        let text = texts(&drawn(&m, 200, 30).0).join("\n");
        assert!(text.contains("start failed: couldn't"), "{text}");
        assert!(
            text.contains("2nd in line"),
            "the other card as before: {text}"
        );
        assert!(
            text.contains("its last start failed"),
            "beside the board: {text}"
        );
    }

    #[test]
    fn autopilot_names_its_repository_s_way_in_its_heading_and_on_a_landing_card() {
        let mut landing = pilot_row(3, "Give notes stable IDs", TabState::Working);
        landing.loop_skill = Some("queue".into());
        landing.phase = Some("pushing".into());
        landing.landing = Some(serde_json::json!({ "holding": true, "phase": "pushing" }));
        let summary = crate::autopilot::Summary {
            way: Some(Way::SwitchboardQueue),
            ..pilot_summary()
        };
        let list = crate::autopilot::view::items(
            &summary,
            "",
            "",
            &[
                landing,
                pilot_row(4, "Port the PR board", TabState::Working),
            ],
            &[],
            &[],
            &[],
            &std::collections::HashMap::new(),
        );
        let mut m = mission(
            Vec::new(),
            vec![list_view(ViewId::Autopilot, "Autopilot", list)],
        );
        m.view = 1;
        let text = texts(&drawn(&m, 200, 30).0);
        assert!(
            text.iter()
                .any(|l| l.contains("lands by Switchboard's merge queue · without asking")),
            "{text:#?}"
        );
        // The landing card's chip, and none on the working one's.
        assert_eq!(
            text.iter()
                .filter(|l| l.contains("Switchboard's queue"))
                .count(),
            1,
            "{text:#?}"
        );
    }

    #[test]
    fn autopilot_is_a_board_of_its_items_with_the_skipped_last() {
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
                    file: None,
                    },
                ),
                (
                    "Third".to_string(),
                    crate::cmd::worktree_from_note::PlanNote {
                        name: "Plan - Third".into(),
                        text: None,
                    file: None,
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
        for column in crate::autopilot::view::board_columns(&m.snap.views[0].data.items) {
            assert!(heads.contains(&column.to_uppercase()), "{heads}");
        }
        assert!(!heads.contains("BLOCKED"), "nothing is blocked: {heads}");
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
    fn autopilot_has_no_skipped_column_while_nothing_is_skipped() {
        let rows = vec![pilot_row(3, "Give notes stable IDs", TabState::Working)];
        let list = crate::autopilot::view::items(
            &pilot_summary(),
            "",
            "",
            &rows,
            &["Third".into()],
            &[],
            &[],
            &std::collections::HashMap::new(),
        );
        let mut m = mission(
            Vec::new(),
            vec![list_view(ViewId::Autopilot, "Autopilot", list)],
        );
        m.view = 1;
        m.size = (200, 30);
        let text = texts(&drawn(&m, 200, 30).0);
        let heads = text
            .iter()
            .find(|l| l.contains("NEXT UP 1"))
            .unwrap_or_else(|| panic!("{text:#?}"));
        for column in ["Next up", "Working", "Waiting on you", "Landing", "Landed"] {
            assert!(heads.contains(&column.to_uppercase()), "{heads}");
        }
        assert!(!text.iter().any(|l| l.contains("SKIPPED")), "{text:#?}");
        assert!(!text.iter().any(|l| l.contains("BLOCKED")), "{text:#?}");
    }

    #[test]
    fn a_blocked_item_stands_in_its_own_column_naming_each_item_it_waits_on() {
        let rows = vec![pilot_row(3, "The base", TabState::Working)];
        let held = [crate::autopilot::pilot::Held {
            text: "Builds on both".into(),
            waits_on: "The base".into(),
            after: vec![
                ("The base".into(), crate::autopilot::pilot::Stands::Open),
                ("Third".into(), crate::autopilot::pilot::Stands::Queued(1)),
            ],
        }];
        let list = crate::autopilot::view::items(
            &pilot_summary(),
            "",
            "",
            &rows,
            &["Third".into()],
            &held,
            &[],
            &std::collections::HashMap::new(),
        );
        let blocked = list
            .iter()
            .position(|i| i.id == "held:Builds on both")
            .unwrap();
        let mut m = mission(
            Vec::new(),
            vec![list_view(ViewId::Autopilot, "Autopilot", list)],
        );
        m.view = 1;
        m.size = (200, 40);
        m.cursor = blocked as isize;
        let (lines, hits, _, _) = drawn(&m, 200, 40);
        let text = texts(&lines);
        let heads = text
            .iter()
            .find(|l| l.contains("NEXT UP 1"))
            .unwrap_or_else(|| panic!("{text:#?}"));
        let col = |l: &str, s: &str| {
            l[..l.find(s).unwrap_or_else(|| panic!("{s} in {l}"))]
                .chars()
                .count()
        };
        // Between Next up and Working.
        assert!(col(heads, "NEXT UP") < col(heads, "BLOCKED 1"), "{heads}");
        assert!(col(heads, "BLOCKED 1") < col(heads, "WORKING"), "{heads}");
        // Its card: named blocked, its item, a dim "after", then a line for
        // each item it waits on: the tab working one, the other in line.
        let row_of = |s: &str| {
            text.iter()
                .position(|l| l.contains(s))
                .unwrap_or_else(|| panic!("{s} in {text:#?}"))
        };
        let top = row_of("blocked  ");
        let under = col(heads, "BLOCKED 1");
        assert!(col(&text[top], "blocked  ") >= under, "{}", text[top]);
        assert!(
            text[top + 1].contains("Builds on both"),
            "{}",
            text[top + 1]
        );
        assert!(text[top + 2].contains("after"), "{}", text[top + 2]);
        assert!(text[top + 3].contains("3 The base"), "{}", text[top + 3]);
        assert!(text[top + 4].contains("◌ Third"), "{}", text[top + 4]);
        // Each of those lines is a click of its own, ahead of its card's.
        for (n, dy) in [(0, 3), (1, 4)] {
            let spot = Spot::Part(blocked as isize, n);
            let hit = hits
                .iter()
                .find(|h| h.y as usize == top + dy && h.x0 as usize >= under)
                .unwrap_or_else(|| panic!("a click on line {dy}"));
            assert_eq!(hit.spot, spot, "the after line before the card");
        }
        // Under the mouse, the line is underlined and its card lit.
        m.hover = Some(Spot::Part(blocked as isize, 1));
        let (lines, _, _, _) = drawn(&m, 200, 40);
        let lit = lines[top + 4]
            .spans
            .iter()
            .find(|s| s.content.contains("Third"))
            .unwrap();
        assert!(lit.style.add_modifier.contains(Modifier::UNDERLINED));
        // A card names three of them at most, then counts the rest, which
        // take no click.
        let item = &mut m.snap.views[0].data.items[blocked];
        item.data["after"] = serde_json::json!(
            (1..=6)
                .map(|n| serde_json::json!({ "item": format!("Dep {n}") }))
                .collect::<Vec<_>>()
        );
        m.hover = None;
        let (lines, hits, _, _) = drawn(&m, 200, 40);
        let card = texts(&lines);
        assert!(card[top + 5].contains("◌ Dep 3"), "{}", card[top + 5]);
        assert!(card[top + 6].contains("and 3 more"), "{}", card[top + 6]);
        assert!(card[top + 7].contains("╰"), "{}", card[top + 7]);
        assert!(
            !hits
                .iter()
                .any(|h| matches!(h.spot, Spot::Part(_, n) if n >= 3)),
            "only the lines shown take a click"
        );
        // The pane beside it says where each one stands.
        let board = text.join("\n");
        assert!(
            board.contains("Blocked: opens once every item it waits on is ticked"),
            "{board}"
        );
        assert!(board.contains("AFTER"), "{board}");
        // Each item it waits on whole, where it stands on the line under.
        let stands = |name: &str, words: &str| {
            assert!(
                text.windows(2)
                    .any(|w| w[0].contains(name) && w[1].contains(words)),
                "{name}, then {words}: {board}"
            );
        };
        stands("3 The base", "tab 3, working");
        stands("◌ Third", "1st in line, no tab yet");
        let blocked_itself = serde_json::json!({ "item": "Middle", "column": "Blocked" });
        assert_eq!(blocker_stands(&blocked_itself), "blocked itself");
        assert_eq!(
            blocker_line(&blocked_itself, false, 0, &m.snap.theme)[0].content,
            kit::mark(Mark::Idle, 0, &m.snap.theme).content
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
    fn notifications_are_a_history_by_day_with_their_kinds() {
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

    // Board 115: a heading per source in place of the keycaps and the days.
    #[test]
    fn notifications_grouped_are_under_their_sources_headings() {
        use crate::sidebar::history::{Entry, Sidebar};
        const DAY: i64 = 86_400_000;
        let now = rules().now_ms;
        let entry = |source: &str, id: &str, title: &str, at: i64, place: &str| Entry {
            source: source.into(),
            id: id.into(),
            at,
            item: Item::new(id, title).data(serde_json::json!({ "where": place })),
            sidebar: Sidebar::Shown,
            led_to: None,
            replaced: false,
        };
        let mut all: Vec<Entry> = (0..4)
            .map(|i| {
                entry(
                    "slack",
                    &format!("g{i}"),
                    "In general",
                    now - DAY - i,
                    "#general",
                )
            })
            .collect();
        all.push(entry("slack", "d1", "DM Ada", now - 2 * DAY, "Ada"));
        all.push(entry("slack", "d2", "DM Ada again", now - 9 * DAY, "Ada"));
        all.push(entry("autolanded", "l", "Landed abc1234", now, ""));
        all.push(entry(
            "inbox",
            "i",
            "A ticket assigned to you",
            now - 3 * DAY,
            "",
        ));
        let refs: Vec<&Entry> = all.iter().collect();
        let tz = jiff::tz::TimeZone::UTC;
        let today = crate::util::local_at_ms(now)
            .with_time_zone(tz.clone())
            .date();
        let rows = |folded: &[&str]| {
            crate::mission::history::grouped(
                all.iter().map(crate::mission::history::row_of).collect(),
                &folded.iter().map(|f| f.to_string()).collect(),
                false,
            )
        };
        let mut m = mission(
            Vec::new(),
            vec![list_view(
                ViewId::Notifications,
                "Notifications",
                rows(&["Inbox"]),
            )],
        );
        m.snap.notices_grouped = true;
        m.snap.notice_counts = Some(crate::mission::history::counts(&refs, today, &tz));
        m.view = 1;
        let (lines, hits) = screen(&m, 140, 30);
        let text = texts(&lines);
        let all_text = text.join("\n");
        let at = |want: &str| {
            text.iter()
                .position(|l| l.contains(want))
                .unwrap_or_else(|| panic!("{want}: {all_text}"))
        };
        // The sources by their newest, Slack split by channel and DM.
        let order = [
            "▾ AUTOPILOT 1",
            "Landed abc1234",
            "▾ SLACK 6",
            "#general  4",
            "In general",
            "Ada  2",
            "DM Ada",
            "▸ INBOX 1   folded",
        ];
        let places: Vec<usize> = order.iter().map(|w| at(w)).collect();
        assert!(places.windows(2).all(|w| w[0] < w[1]), "{all_text}");
        assert!(!all_text.contains("A ticket assigned to you"), "{all_text}");
        // No keycaps and no days; the week's count stays.
        assert!(!hits.iter().any(|h| matches!(h.spot, Spot::Kind(_))));
        for gone in ["All 8", "TODAY", "YESTERDAY"] {
            assert!(!all_text.contains(gone), "{gone}: {all_text}");
        }
        assert!(all_text.contains("in the sidebar"), "{all_text}");
        // A row says its day once it isn't today's.
        assert!(text[at("In general")].contains("Yest "), "{all_text}");
        assert!(text[at("DM Ada again")].contains("Jan 6 "), "{all_text}");
        // A narrow cell keeps the title its room: the day alone.
        let narrow = texts(&screen(&m, 60, 30).0);
        let general = narrow
            .iter()
            .find(|l| l.contains("In general"))
            .cloned()
            .unwrap_or_default();
        assert!(general.contains("Yest "), "{narrow:?}");
        assert!(
            !general.contains(" AM") && !general.contains(" PM"),
            "{narrow:?}"
        );
        for l in &lines {
            assert_eq!(width(&l.text()), 140, "{:?}", l.text());
        }
    }

    #[test]
    fn the_chosen_landing_stays_in_sight_under_two_headings() {
        let landing = |n: usize| {
            Item::new(format!("land:/w/{n}"), format!("1. landing-{n}"))
                .detail("waiting")
                .data(serde_json::json!({ "landing": { "holding": false, "place": n } }))
        };
        let mut items = vec![landing_header(
            "direct",
            "/a:direct",
            "app · straight to main",
            4,
        )];
        items.extend((1..=4).map(landing));
        items.push(landing_header(
            "direct",
            "/b:direct",
            "api · straight to main",
            5,
        ));
        items.extend((5..=9).map(landing));
        let last = items.len() - 1;
        let mut m = mission(
            Vec::new(),
            vec![list_view(ViewId::Landing, "Landing", items)],
        );
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
    fn landing_fills_the_screen_whatever_the_headings_out_of_sight() {
        let landing = |n: usize| {
            Item::new(format!("land:/w/{n}"), format!("1. landing-{n}"))
                .detail("waiting")
                .data(serde_json::json!({ "landing": { "holding": false, "place": 1 } }))
        };
        let mut items = Vec::new();
        for n in 1..=12 {
            items.push(landing_header(
                "direct",
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
        let mut heading =
            landing_header("direct", "/r:direct", "switchboard · straight on main", 3);
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
        m.view = 1;
        m.cursor = 1;
        m
    }

    #[test]
    fn landing_s_columns_fill_any_width_and_narrow_in_board_45_s_order() {
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
    fn the_chosen_landing_s_detail_says_what_it_waits_on_and_its_last_review() {
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
    fn a_landing_s_worktree_under_the_home_is_written_from_tilde() {
        let mut m = landing_board();
        let landing = m
            .snap
            .views
            .iter_mut()
            .find(|v| v.id == ViewId::Landing)
            .unwrap();
        let root = crate::paths::home().join("code/api/.claude/worktrees/stable-ids");
        landing.data.items[1].data["landing"]["root"] = root.to_string_lossy().into();
        let text = texts(&screen(&m, 160, 50).0).join("\n");
        assert!(
            text.contains("~/code/api/.claude/worktrees/stable-ids"),
            "{text}"
        );
    }

    #[test]
    fn a_landing_is_a_row_of_stage_cells_with_its_note_under_its_name() {
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
            landing_header("direct", "/r:direct", "switchboard · straight to main", 2),
            landing("note-stable-ids", true, None, Some("pushing")),
            landing("note-benchmark", false, Some(2), None),
        ];
        let mut m = mission(
            Vec::new(),
            vec![list_view(ViewId::Landing, "Landing", items)],
        );
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
            heads.contains("worktree")
                && heads.contains("leak check")
                && heads.contains("landed")
                && heads.contains("cleaned up"),
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
            landing_header("direct", "/r:direct", "switchboard · straight to main", 1),
            failing,
            landing_header("github-queue", "/r:queue", "web-app · merge queue", 2),
            queued("QUEUED"),
            queued("UNMERGEABLE"),
        ];
        let mut f = mission(
            Vec::new(),
            vec![list_view(ViewId::Landing, "Landing", more)],
        );
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
        // A GitHub merge queue's PR is a row of that way's stages.
        let heads = ftext.iter().find(|l| l.contains("pull request")).unwrap();
        assert!(
            heads.contains("approved") && heads.contains("queued") && heads.contains("cleaned up"),
            "{heads}"
        );
        let pr = ftext
            .iter()
            .position(|l| l.contains("#2031 settings page"))
            .unwrap();
        assert!(
            ftext[pr].contains("✓ passed")
                && ftext[pr].contains("✓ approved")
                && ftext[pr].contains("3rd"),
            "{}",
            ftext[pr]
        );
        assert!(ftext[pr + 2].contains("✕ can't merge"), "{}", ftext[pr + 2]);
    }

    #[test]
    fn each_way_s_section_names_it_and_ends_in_landed_and_cleaned_up() {
        let mut ours = landing_header("switchboard-queue", "/r:prqueue", "switchboard", 2);
        ours.data["words"] = serde_json::json!(
            "lands by Switchboard's merge queue  ·  done when its PR merges on main, then cleaned up"
        );
        let in_train = Item::new("prqueue:/r:2040", "1. #2040 calendar RSVP")
            .detail("testing on ci/train-3 · tab 5")
            .data(serde_json::json!({ "prqueue": {
                "testing": true, "train": "ci/train-3", "place": 1,
                "record": { "review": { "clean_rounds": 2 } },
            } }));
        let merging = Item::new("prqueue:/r:2041", "3. #2041 merging")
            .detail("testing on ci/train-4 · tab 7")
            .data(serde_json::json!({ "prqueue": {
                "testing": true, "train": "ci/train-4", "place": 1, "passed": true,
            } }));
        let stuck = Item::new("prqueue:/r:2042", "4. #2042 stuck")
            .detail("testing on ci/train-6 · tab 8")
            .data(serde_json::json!({ "prqueue": {
                "testing": true, "train": "ci/train-6", "place": 1, "passed": true,
                "mergeFailures": 2,
            } }));
        let waiting = Item::new("prqueue:/r:219", "2. #219 tab colors")
            .detail("waiting · tab 6")
            .data(serde_json::json!({ "prqueue": { "testing": false, "place": 2 } }));
        let landed = Item::new("landed:@9", "3. #208 billing page")
            .detail("merged · cleaning up · tab 4")
            .data(
                serde_json::json!({ "landed": { "way": "switchboard-queue", "cleaning": true } }),
            );
        let mut m = mission(
            Vec::new(),
            vec![list_view(
                ViewId::Landing,
                "Landing",
                vec![ours, in_train, merging, stuck, waiting, landed],
            )],
        );
        m.view = 1;
        let text = texts(&screen(&m, 180, 20).0);
        let at = text
            .iter()
            .position(|l| l.contains("switchboard   lands by Switchboard's merge queue"))
            .unwrap_or_else(|| panic!("{text:#?}"));
        assert!(
            text[at].contains("done when its PR merges on main"),
            "{}",
            text[at]
        );
        let heads = &text[at + 1];
        for head in [
            "pull request",
            "review",
            "checks",
            "queued",
            "train",
            "landed",
            "cleaned up",
        ] {
            assert!(heads.contains(head), "{head}: {heads}");
        }
        let row = |name: &str| text.iter().position(|l| l.contains(name)).unwrap();
        let first = &text[row("#2040 calendar RSVP")];
        assert!(
            first.contains("✓ 2 clean")
                && first.contains("✓ passed")
                && first.contains("✓ 1st")
                && first.contains("testing"),
            "{first}"
        );
        assert!(text[row("#219 tab colors")].contains("2nd"));
        assert!(text[row("#2041 merging")].contains("confirming"));
        assert!(text[row("#2042 stuck")].contains("✕ not merged"));
        let merged = &text[row("#208 billing page")];
        assert!(
            merged.contains("✓ merged") && merged.contains("cleaning…"),
            "{merged}"
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
    fn the_pane_beside_a_board_has_a_scroll_bar_that_pages_and_drags_it() {
        let item = Item::new("menu", "Fix the menu")
            .group("Todo")
            .data(serde_json::json!({
                "ticket": "menu", "state": "Todo", "description": long_description(30),
            }));
        let mut m = mission(
            Vec::new(),
            vec![list_view(ViewId::Tickets, "Tickets", vec![item])],
        );
        m.snap.rules.scroll_bars = true;
        m.view = 1;
        m.size = (160, 20);
        let (lines, hits, _, _) = drawn(&m, 160, 20);
        let bar: Vec<(u16, usize, usize, usize)> = hits
            .iter()
            .filter_map(|h| match h.spot {
                Spot::PaneBar { row, body, total } => Some((h.x0, row, body, total)),
                _ => None,
            })
            .collect();
        assert!(!bar.is_empty(), "the bar's rows are hits");
        assert!(
            bar.iter().all(|b| b.0 == 159),
            "in the last column: {bar:?}"
        );
        let text = texts(&lines);
        assert!(
            text.iter().any(|l| l.ends_with('┃')),
            "the thumb: {text:#?}"
        );
        m.hits = hits;
        let (_, _, body, total) = bar[0];
        assert!(m.side_bar_to((body - 1, body, total), None), "a page down");
        assert_eq!(m.side_top(), body);
        m.side_bar_to((0, body, total), Some((body - 1, body)));
        assert_eq!(m.side_top(), 0, "dragged to the top");
        m.snap.rules.scroll_bars = false;
        let (_, hits, _, _) = drawn(&m, 160, 20);
        assert!(!hits.iter().any(|h| matches!(h.spot, Spot::PaneBar { .. })));
    }

    #[test]
    fn the_autopilot_plan_beside_the_board_scrolls_too() {
        let rows = vec![pilot_row(3, "Give notes stable IDs", TabState::Working)];
        let list = crate::autopilot::view::items(
            &pilot_summary(),
            "",
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
                    file: None,
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
    fn the_usage_view_is_a_board_of_charts_tabs_limits_and_requests() {
        use crate::requests::{ServiceCount, Tally};
        use crate::usage::charts::{self, Limit, Model, ModelSpend, SpanSpend, TabSpend};
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
            by_model: vec![
                ModelSpend {
                    name: "Fable 5.2".into(),
                    spent: 3.0,
                    share: 0.75,
                },
                ModelSpend {
                    name: "Haiku 5.5".into(),
                    spent: 1.0,
                    share: 0.25,
                },
            ],
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
                Limit {
                    key: "spend_limit".into(),
                    name: "Spend".into(),
                    used: 86.0,
                    resets_at: now + 9 * 86400,
                    pace: None,
                    detail: Some("$430.00 of $500 a month".into()),
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
        assert!(text.contains("just now"), "{text}");
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
        assert_eq!(named("usage/limit/"), 3);
        // A gateway's spend limit, its dollars of the month's and its reset.
        let spend = texts(&lines)
            .into_iter()
            .find(|l| l.contains("Spend"))
            .expect("the spend limit's row");
        assert!(
            spend.contains("86%") && spend.contains("$430.00 of $500 a month · resets "),
            "{spend}"
        );
        // Today's spend by model under the limits, a meter of its share each.
        let models_at = text.find("BY MODEL").expect("the models' block");
        assert!(models_at > text.find("LIMITS").unwrap(), "{text}");
        assert!(models_at < text.find("REQUESTS").unwrap(), "{text}");
        let fable = texts(&lines)
            .into_iter()
            .find(|l| l.contains("Fable 5.2"))
            .expect("a row a model");
        assert!(fable.contains("75%") && fable.contains("$3.00"), "{fable}");
        assert_eq!(named("usage/model/"), 2);
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
    fn the_keys_view_is_a_board_of_groups_a_heatmap_the_chosen_key_and_its_table() {
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
            easy: false,
        };
        let items = keymap::items(&bindings, shown, &counts);
        let mut view = list_view(ViewId::Keys, "Keys", items.clone());
        view.data.head = keymap::head(&counts, true);
        let mut m = mission(Vec::new(), vec![view]);
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
            easy: false,
        };
        let items = keymap::items(&bindings, shown, &counts);
        let mut view = list_view(ViewId::Keys, "Keys", items.clone());
        view.data.head = keymap::head(&counts, true);
        let mut m = mission(Vec::new(), vec![view]);
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
