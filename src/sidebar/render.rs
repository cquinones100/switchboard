//! The sidebar's lines (lib/sidebar/render.js's `sidebarLines`): a plain
//! function from what is known to the lines to draw, each exactly the
//! sidebar's width, so the tests can read them as text. At the top the
//! session's name and its counts; then the tabs in their groups, each with
//! its badge, number and name in its project's color, and under it what it
//! is doing; then, pinned to the bottom under their own heading, the
//! notifications, which never scroll away. The header stays at the top just
//! as they do, so on a short screen only the groups and tabs scroll, the
//! filter line saying how many tabs are above (`▲ 3 more`) and the list's
//! last row how many are cut below (`▼ 2 more`); beside a tab in sight
//! where the list was last shown, the list holds there, so going from tab
//! to tab doesn't move it.
//!
//! Each line says what it belongs to (`Owner`), for a click on it, and the
//! list says where it scrolled to (`Scroll`), for C-d and C-u to move by.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use super::model::{self, Rules, Stage, TabModel};
use super::text::{self, width};
use crate::items::{Item, State};
use crate::mux::TabId;
use crate::ui::Theme;
use crate::ui::kit::{self, Mark, Tone};

/// The most lines a tab's name wraps to.
pub const NAME_LINES: usize = 3;

/// The yellow of the notifications and of a filter being typed.
const YELLOW: Color = Color::Indexed(220);

/// What a line of the sidebar belongs to: a tab, or a notification (its
/// source and its item's id), for the cursor and a click; or the row saying
/// how many tabs are out of sight above or below, which a click scrolls;
/// or, while the sidebar is filtered to a repository, its chip on the top
/// line, which a click clears; or a group, whose heading folds or unfolds
/// it on a click and whose folded line unfolds it on a click or Enter;
/// or the notifications' heading and folded line, which a click folds
/// or unfolds; or the top line's ⏰ chip while the sidebar leaves the
/// Reminders group out, which a click opens mission control's Reminders
/// view from; or, in select mode, its chip on the top line, which a click
/// opens the menu of; or, one of the workspace's own buttons
/// first on the top line (back, forward, mission control).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Owner {
    Tab(TabId),
    Notice {
        source: String,
        id: String,
    },
    ScrollUp,
    ScrollDown,
    RepoChip,
    Group(Stage),
    Notices,
    Reminders,
    SelectChip,
    /// M-p's restart as it goes (src/mux/build_row.rs), whose steps a
    /// click opens.
    BuildRow,
    /// ← and →, which step back and forward through the tabs visited as
    /// M-{ and M-} do.
    Back,
    Forward,
    /// ⊞, which opens mission control as M-Space does.
    Mission,
    /// The coordinator's row in the footer, which a click shows or hides
    /// as M-C does (src/coordinator).
    Coordinator,
}

impl Owner {
    pub fn tab(&self) -> Option<TabId> {
        match self {
            Owner::Tab(t) => Some(*t),
            _ => None,
        }
    }

    /// Whether two stops share a row of the sidebar: the same tab, or
    /// notifications of one kind, which the sidebar shows as one row.
    pub fn same_row(&self, other: &Owner) -> bool {
        match (self, other) {
            (Owner::Notice { source: a, .. }, Owner::Notice { source: b, .. }) => a == b,
            _ => self == other,
        }
    }
}

/// A notification as the sidebar lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub owner: Owner,
    pub title: String,
    pub detail: String,
    /// When the newest of its kind came, in seconds since the epoch, shown
    /// as how long ago; none for a kind that keeps no time.
    pub time: Option<i64>,
}

/// A plugin's short list under the tabs (its `sidebar` count of items, from
/// sidebar.json or what it prints): its title, how many items it has and
/// how many need you, why its last run failed, and its first items.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Section {
    pub title: String,
    pub count: usize,
    pub attention: usize,
    pub error: Option<String>,
    pub items: Vec<Item>,
}

impl Section {
    /// A plugin's list as the sidebar shows it: its first `shown` items.
    pub fn of(title: &str, items: &[Item], error: Option<&str>, shown: usize) -> Section {
        Section {
            title: title.to_string(),
            count: items.len(),
            attention: items.iter().filter(|i| i.state == State::Attention).count(),
            error: error.map(str::to_string),
            items: items.iter().take(shown).cloned().collect(),
        }
    }
}

/// A / filter on the tabs, being typed or kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    pub text: String,
    pub typing: bool,
}

/// Where ← or → goes: a tab, or a mission control view by its title.
#[derive(Clone, Debug)]
pub enum Going {
    Tab(Box<TabModel>),
    View(String),
}

/// The workspace's own buttons on the top line (boards 57
/// to 60): ← and → while back and forward step through the tabs visited
/// (tab_back_forward), with the place each goes to, and ⊞ mission control.
#[derive(Clone, Debug, Default)]
pub struct Buttons {
    /// The places back and forward go to, while they step through the
    /// places visited; none draws no ← and →.
    pub trail: Option<(Option<Going>, Option<Going>)>,
    /// Mission control is open on this client, which lights ⊞.
    pub mission_open: bool,
    /// How many tabs need you, beside ⊞ while over 0.
    pub needs: usize,
}

/// How the sidebar is being looked at.
#[derive(Clone, Copy, Debug)]
pub struct View<'a> {
    /// Whether it has the focus.
    pub focused: bool,
    /// The cursor, while it has the focus.
    pub cursor: Option<&'a Owner>,
    /// The spinner's frame.
    pub frame: usize,
    /// The tab it sits beside.
    pub here: TabId,
    pub filter: Option<&'a Filter>,
    /// Where its list was scrolled to, kept while it has the focus.
    pub top: Option<usize>,
    /// Where the session's list was last shown, which it holds to while
    /// the tab it sits beside is in sight there, so going from tab to tab
    /// doesn't move the list.
    pub keep: Option<usize>,
    /// What the mouse is over.
    pub hover: Option<&'a Owner>,
    /// The computer held awake (src/awake): a ☕ after the tab count.
    pub awake: bool,
    /// The repository the tabs are filtered to, by its folder's name, its
    /// chip on the top line; every other one's tabs are under Other repos.
    pub repo: Option<&'a str>,
    /// The groups folded to one line: its tabs' marks and their count,
    /// Other repos' with their repositories.
    pub folded: &'a std::collections::BTreeSet<Stage>,
    /// The notifications folded to one line of their kinds.
    pub notices_folded: bool,
    /// How many tabs wait on a reminder while the sidebar leaves the
    /// Reminders group out (sidebar_reminders off), counted on the top
    /// line's ⏰ chip; 0 shows no chip.
    pub reminders: usize,
    /// M-p's restart as it goes, on the line under the count
    /// (board 56's build row).
    pub build: Option<&'a crate::mux::build_row::Row>,
    /// Select mode: the tabs selected, each drawn with the selected mark, and how
    /// many on the top line's chip.
    pub selected: Option<&'a std::collections::BTreeSet<TabId>>,
    /// The workspace's own buttons, first on the top line.
    pub buttons: Option<&'a Buttons>,
    /// Whether its buttons and chips have the kit's rounded ends, from the
    /// client's terminal (src/mux/graphics.rs's `round_ends`), as the
    /// header's do; padded with a space of their fill otherwise.
    pub caps: bool,
    /// The coordinator's row over the keys at the bottom, while it runs:
    /// its mark and whether it works, waits on you or sleeps.
    pub coordinator: Option<(Mark, &'static str)>,
}

/// One line: its spans, exactly the sidebar's width, and a background for
/// the whole line (the chosen tab's highlight, a hovered row's shade).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    pub spans: Vec<Span<'static>>,
    pub fill: Option<Color>,
}

impl Line {
    fn blank(cols: usize) -> Line {
        Line {
            spans: vec![Span::raw(" ".repeat(cols))],
            fill: None,
        }
    }

    /// The line as text.
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.content.as_ref()).collect()
    }
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

    /// The line, cut to `cols` (an ellipsis in place of what didn't fit)
    /// and padded to fill them.
    fn fit(self, cols: usize, fill: Option<Color>) -> Line {
        let mut spans = Vec::new();
        let total: usize = self.spans.iter().map(|s| width(&s.content)).sum();
        let mut used = 0;
        if total <= cols {
            spans = self.spans;
            used = total;
        } else if cols > 0 {
            // As text.js's `cut` over the whole line: what fits before the
            // last column, then the ellipsis, in the style it cut into.
            for span in self.spans {
                let w = width(&span.content);
                if used + w < cols {
                    used += w;
                    spans.push(span);
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
                spans.push(Span::styled(kept, span.style));
                break;
            }
        }
        if used < cols {
            spans.push(Span::raw(" ".repeat(cols - used)));
        }
        Line { spans, fill }
    }
}

/// Where the list scrolled to: the line at its top, how many lines it has
/// room for of how many, each line's tab, and the chosen tab's first line,
/// all counted in the list of groups and tabs alone, under the header; and
/// the sidebar's row the list starts on and how many rows it has, its "more"
/// row among them, which the scroll bar runs down beside.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scroll {
    pub top: usize,
    pub list_y: usize,
    pub space: usize,
    pub room: usize,
    pub length: usize,
    /// Each line's stop: a tab, or a folded group's line.
    pub owners: Vec<Option<Owner>>,
    pub cursor_at: usize,
}

/// What `sidebar_lines` gives: the lines to draw (as many as the rows), each
/// one's owner, where the list scrolled to, and whether a badge on it spins.
#[derive(Clone, Debug, Default)]
pub struct Lines {
    pub lines: Vec<Line>,
    pub owners: Vec<Option<Owner>>,
    pub scroll: Scroll,
    pub spins: bool,
    /// The columns the top line's ⏰ chip takes, from and to, while it
    /// shows, which a click there takes for `Owner::Reminders`.
    pub reminders_at: Option<(usize, usize)>,
    /// The columns of the top line each of the workspace's own buttons
    /// takes, from and to, which a click there takes for it.
    pub buttons_at: Vec<(Owner, usize, usize)>,
}

fn fg(c: Color) -> Style {
    Style::new().fg(c)
}

/// An item's mark by its state, as mission control's lists draw it.
fn item_mark(item: &Item, theme: &Theme) -> Span<'static> {
    match item.state {
        State::Attention => Span::styled("●", bold(YELLOW)),
        State::Active => Span::styled("●", bold(Color::Indexed(67))),
        State::Idle => Span::styled("○", fg(theme.faint)),
    }
}

fn bold(c: Color) -> Style {
    Style::new().fg(c).add_modifier(Modifier::BOLD)
}

/// A tab's number in its project's color, as its row shows it.
fn number_color(tab: &TabModel, theme: &Theme) -> Color {
    tab.color
        .map(|c| crate::tabs::color::as_words(c.into(), *theme == Theme::LIGHT))
        .unwrap_or(theme.faint)
}

/// The workspace's own buttons, first on the top line (boards
/// 57 to 60): ← and → (faint with no tab that way) while they step through
/// the tabs visited, then ⊞ with the count of tabs that need you, lit while
/// mission control is open; each on the chosen surface under the mouse,
/// with a column after it. Gives the columns each took.
fn buttons(
    row: &mut Row,
    b: &Buttons,
    hover: Option<&Owner>,
    caps: bool,
    theme: &Theme,
) -> Vec<(Owner, usize, usize)> {
    let press = |owner: &Owner, idle: bool| {
        if hover == Some(owner) {
            kit::Press::Hover
        } else if idle {
            kit::Press::Idle
        } else {
            kit::Press::Rest
        }
    };
    let mut list = Vec::new();
    if let Some((back, forward)) = &b.trail {
        list.push((Owner::Back, "←", None, press(&Owner::Back, back.is_none())));
        list.push((
            Owner::Forward,
            "→",
            None,
            press(&Owner::Forward, forward.is_none()),
        ));
    }
    let count = (b.needs > 0).then(|| (b.needs.to_string(), Tone::Needs));
    let mission = if b.mission_open {
        kit::Press::Lit
    } else {
        press(&Owner::Mission, false)
    };
    list.push((Owner::Mission, "⊞", count, mission));
    let mut at = Vec::new();
    for (owner, glyph, count, press) in list {
        let from: usize = row.spans.iter().map(|s| width(&s.content)).sum();
        row.spans
            .extend(kit::button(glyph, count, press, caps, theme));
        let to: usize = row.spans.iter().map(|s| width(&s.content)).sum();
        row.put(" ", Style::new());
        at.push((owner, from, to));
    }
    at
}

/// The sidebar for one tab of a session: its session's tabs, most in need of
/// you first, the plugins' lists, then the notifications. `tabs` are the session's (filtered
/// already, when a filter is kept), `frozen` the layout held while browsing.
#[allow(clippy::too_many_arguments)]
pub fn sidebar_lines(
    tabs: &[TabModel],
    frozen: Option<&model::Frozen>,
    notices: &[Notice],
    sections: &[Section],
    cols: usize,
    rows: usize,
    view: &View<'_>,
    r: &Rules,
    theme: &Theme,
) -> Lines {
    // The header (the session's pill and counts, and the filter line) stays
    // at the top whatever the list's scroll, as the notifications stay at
    // the bottom; only the groups and tabs between them scroll.
    let mut head: Vec<Line> = Vec::new();
    let mut lines: Vec<Line> = Vec::new();
    let mut owners: Vec<Option<Owner>> = Vec::new();
    let mut headings: Vec<usize> = Vec::new();
    let mut spins = false;
    let mut cursor_at = 0;
    let mut cursor_end = 0;
    let selected = if view.focused {
        view.cursor.and_then(Owner::tab)
    } else {
        Some(view.here)
    };

    // How many tabs there are.
    let mut pill_row = Row::default();
    // The repository filter's chip, and how many of the tabs are its.
    // In `room` columns: where they are short, "7 of 9" gives way first,
    // then the repository's name is cut, so what follows it keeps its place.
    let repo_chip = |row: &mut Row, room: usize| -> usize {
        let Some(repo) = view.repo else {
            return 0;
        };
        let mine = tabs
            .iter()
            .filter(|t| !t.elsewhere && !t.outside_filter)
            .count();
        let mut said = format!("{mine} of {}", tabs.len());
        // The chip's ends, " ×" and the column after it.
        let around = 5;
        if width(repo) + around + width(&said) > room {
            said.clear();
        }
        // With no room even for a cut name, the chip is its × alone, which
        // still clears the filter on a click.
        let name_room = room.saturating_sub(around);
        let label = if name_room < 2 {
            "×".to_string()
        } else {
            format!("{} ×", text::cut(repo, name_room))
        };
        let chip = kit::chip(&label, Tone::Accent, view.caps, theme);
        let used: usize = chip.iter().map(|s| width(&s.content)).sum::<usize>() + width(&said);
        row.spans.extend(chip);
        row.put(said, fg(theme.dim));
        used
    };
    // The tabs waiting on a reminder, while the Reminders group is left
    // out: their chip, which a click opens the Reminders view from; gives
    // the columns it took.
    let reminders_chip = |row: &mut Row| -> Option<(usize, usize)> {
        if view.reminders == 0 {
            return None;
        }
        let from: usize = row.spans.iter().map(|s| width(&s.content)).sum();
        let chip = kit::chip(
            &format!("⏰ {}", view.reminders),
            Tone::Dim,
            view.caps,
            theme,
        );
        let to = from + chip.iter().map(|s| width(&s.content)).sum::<usize>();
        row.spans.extend(chip);
        Some((from, to))
    };
    let mut reminders_at = None;
    let mut buttons_at = Vec::new();
    // The place ← or → goes to while the mouse is over it, and its arrow.
    let going: Option<(&Going, &str)> = match (view.buttons, view.hover) {
        (Some(b), Some(Owner::Back)) => b
            .trail
            .as_ref()
            .and_then(|(to, _)| to.as_ref())
            .map(|t| (t, "←")),
        (Some(b), Some(Owner::Forward)) => b
            .trail
            .as_ref()
            .and_then(|(_, to)| to.as_ref())
            .map(|t| (t, "→")),
        _ => None,
    };
    // The count, a ☕ while the computer is held awake, and on the
    // right how to filter; the groups' own headings say how many need
    // you or work. Filtered to a repository, its chip in its place.
    let n = tabs.len();
    let said = format!("{n} {}", if n == 1 { "tab" } else { "tabs" });
    pill_row.put(" ", Style::new());
    if let Some(b) = view.buttons {
        buttons_at = buttons(&mut pill_row, b, view.hover, view.caps, theme);
    }
    // Under the mouse, a button names where it goes in place of the
    // rest of the line.
    let named = match (going, view.hover) {
        (Some((Going::View(title), _)), _) => {
            pill_row.put(format!("⊞ {title}"), bold(theme.text));
            true
        }
        (Some((Going::Tab(tab), _)), _) => {
            let state = model::badge_state(tab, r);
            pill_row
                .spans
                .push(kit::mark(Mark::of_badge(state), view.frame, theme));
            pill_row
                .put(" ", Style::new())
                .put(tab.index.to_string(), fg(number_color(tab, theme)))
                .put(" ", Style::new())
                .put(tab.name.clone(), fg(theme.text));
            true
        }
        (None, Some(Owner::Mission)) if view.buttons.is_some() => {
            pill_row.put("Mission control", bold(theme.text));
            true
        }
        _ => false,
    };
    if !named {
        // The ⏰ chip first, so a long repository chip never pushes it off.
        if view.reminders > 0 {
            reminders_at = reminders_chip(&mut pill_row);
            pill_row.put(" ", Style::new());
        }
        // Room for "/ filter" at the right, a column apart, as the
        // board has it beside the chip (43).
        let before: usize = pill_row.spans.iter().map(|s| width(&s.content)).sum();
        let cup = if view.awake { 3 } else { 0 };
        repo_chip(
            &mut pill_row,
            cols.saturating_sub(before + cup + width("/ filter") + 2),
        );
        if view.repo.is_none() {
            pill_row.put(said.clone(), fg(theme.dim));
        }
        let mut used: usize = pill_row.spans.iter().map(|s| width(&s.content)).sum();
        if view.awake {
            let cup = kit::mark(Mark::Awake, 0, theme);
            used += 2 + width(&cup.content);
            pill_row.put("  ", Style::new());
            pill_row.spans.push(cup);
        }
        let gap = cols.saturating_sub(used + width("/ filter") + 1);
        if gap > 0 {
            pill_row
                .put(" ".repeat(gap), Style::new())
                .put("/ filter", fg(theme.faint));
        }
    }
    // In select mode, the top line is how many are selected, a chip a
    // click on opens their menu, and the keys.
    if let Some(picked) = view.selected {
        // Every tab selected, a / filter hiding some or not, as the menu
        // closes them all; a closed tab has dropped out already.
        let n = picked.len();
        pill_row = Row::default();
        reminders_at = None;
        buttons_at.clear();
        pill_row.put(" ", Style::new());
        pill_row.spans.extend(kit::chip(
            &format!("{n} selected"),
            Tone::Accent,
            view.caps,
            theme,
        ));
        pill_row
            .spans
            .extend(kit::hints(&[("↵", "actions"), ("esc", "")], theme));
    }
    // The filter's chip clears it on a click, so its line lights under
    // the mouse as a tab does; select mode's opens its menu.
    let chip_lit = match view.selected {
        Some(_) => view.hover == Some(&Owner::SelectChip),
        None => {
            view.repo.is_some() && view.hover == Some(&Owner::RepoChip)
                || reminders_at.is_some() && view.hover == Some(&Owner::Reminders)
        }
    };
    head.push(pill_row.fit(cols, chip_lit.then_some(theme.panel)));
    // Only the columns of it the line kept are a target.
    let reminders_at = reminders_at
        .filter(|(from, _)| *from < cols)
        .map(|(from, to)| (from, to.min(cols)));
    // A / filter being typed, or kept, in the line under it.
    match view.filter {
        Some(f) => {
            let mut row = Row::default();
            row.put(" ", Style::new())
                .put(format!("/{}", f.text), fg(YELLOW));
            if f.typing {
                row.put("▏", fg(theme.dim));
            }
            head.push(row.fit(cols, None));
        }
        // M-p's restart as it goes, lit under the mouse
        // since a click shows its steps.
        None => match view.build {
            Some(build) => {
                spins |= build.spins();
                let lit = view.hover == Some(&Owner::BuildRow);
                head.push(Line {
                    spans: build.spans(cols, view.frame, theme),
                    fill: lit.then_some(theme.panel),
                });
            }
            None => head.push(Line::blank(cols)),
        },
    }
    let build_row = view.filter.is_none() && view.build.is_some();

    for group in model::grouped(tabs, frozen, r) {
        headings.push(lines.len());
        // Build failing's heading is in the failing badge's red, so it reads
        // at a glance.
        let mut heading = Row::default();
        heading.put(" ", Style::new());
        // Other repos, folded: ▸ and its tabs' marks on its
        // heading, so one that needs you still shows, its count at the
        // right edge (board 43); its repositories by name under it.
        let other_folded = group.stage == Stage::OtherRepos && view.folded.contains(&group.stage);
        if other_folded {
            let tone = Mark::of_badge(group.stage.color()).look().1;
            let n = group.tabs.len().to_string();
            let mut spans = vec![
                Span::raw(" "),
                Span::styled("▸ OTHER REPOS ", tone.bold(theme)),
            ];
            let room = cols.saturating_sub(kit::spans_width(&spans) + width(&n) + 2);
            for t in group.tabs.iter().take(room) {
                let state = model::badge_state(t, r);
                spins |= kit::spins(state);
                spans.push(kit::mark(Mark::of_badge(state), view.frame, theme));
            }
            let gap = cols.saturating_sub(kit::spans_width(&spans) + width(&n) + 1);
            spans.push(Span::raw(" ".repeat(gap.max(1))));
            spans.push(Span::styled(n, tone.style(theme)));
            heading.spans = kit::fit(spans, cols, Style::new());
        } else {
            // The kit's label in its stage's tone, so what needs you reads
            // yellow and what works blue, its count at the right edge.
            let tone = Mark::of_badge(group.stage.color()).look().1;
            heading.spans = kit::label(
                group.stage.title(),
                tone,
                Some(group.tabs.len()),
                cols,
                theme,
            );
        }
        let owner = Owner::Group(group.stage);
        // A heading folds or unfolds its group on a click, so it lights
        // under the mouse; an open group's is a stop of its own, lit as a
        // chosen tab is under the cursor (a folded one's line below it is).
        let heading_chosen =
            view.focused && view.cursor == Some(&owner) && !view.folded.contains(&group.stage);
        if heading_chosen {
            cursor_at = lines.len();
            cursor_end = lines.len();
        }
        let heading_fill = if heading_chosen {
            Some(theme.highlight)
        } else if view.hover == Some(&owner) {
            Some(theme.panel)
        } else {
            None
        };
        lines.push(heading.fit(cols, heading_fill));
        owners.push(Some(owner.clone()));
        if view.folded.contains(&group.stage) {
            // Folded: one line of its tabs' marks, so a waiting one still
            // shows, and their count; Other repos' with their
            // repositories by name.
            let chosen = view.focused && view.cursor == Some(&owner);
            let fill = if chosen {
                Some(theme.highlight)
            } else if view.hover == Some(&owner) {
                Some(theme.panel)
            } else {
                None
            };
            let mut names: Vec<String> = Vec::new();
            for t in group
                .tabs
                .iter()
                .filter(|_| group.stage == Stage::OtherRepos)
            {
                let name = t
                    .repo_dir
                    .as_deref()
                    .and_then(|d| d.file_name())
                    .map(|n| n.to_string_lossy().into_owned());
                if let Some(name) = name.filter(|n| !names.contains(n)) {
                    names.push(name);
                }
            }
            let mut row = Row::default();
            let n = group.tabs.len();
            let tabs_said = format!("{n} {}", if n == 1 { "tab" } else { "tabs" });
            if other_folded {
                // Its marks are on the heading: under it, its repositories.
                let said = if names.is_empty() {
                    tabs_said
                } else {
                    names.join(" · ")
                };
                row.put("  ", Style::new()).put(said, fg(theme.faint));
            } else {
                row.put("▸ ", fg(theme.dim));
                for t in &group.tabs {
                    let state = model::badge_state(t, r);
                    spins |= kit::spins(state);
                    row.spans
                        .push(kit::mark(Mark::of_badge(state), view.frame, theme));
                }
                let mut said = format!("  {tabs_said}");
                if !names.is_empty() {
                    said.push_str(" · ");
                    said.push_str(&names.join(", "));
                }
                row.put(said, fg(theme.dim));
            }
            // Its line holds the list's place as the chosen tab would: the
            // cursor on it, or the tab in front folded into it.
            let holds_here = selected.is_some_and(|s| group.tabs.iter().any(|t| t.id == s));
            if chosen || holds_here {
                cursor_at = lines.len();
                cursor_end = lines.len();
            }
            let line = Line {
                spans: kit::row(
                    row.fit(cols.saturating_sub(1), None).spans,
                    cols,
                    chosen,
                    theme,
                ),
                fill,
            };
            lines.push(line);
            owners.push(Some(owner));
            lines.push(Line::blank(cols));
            owners.push(None);
            continue;
        }
        // In select mode, the selected mark in the accent before a selected tab's
        // mark, and room for one before every other's, so the marks line up.
        let tick_cols = if view.selected.is_some() { 2 } else { 0 };
        let tick = |row: &mut Row, id: TabId| match view.selected {
            Some(picked) if picked.contains(&id) => {
                row.spans.push(kit::mark(Mark::Selected, 0, theme));
                row.put(" ", Style::new());
            }
            Some(_) => {
                row.put("  ", Style::new());
            }
            None => {}
        };
        for tab in &group.tabs {
            let chosen = selected == Some(tab.id);
            if chosen {
                cursor_at = lines.len();
            }
            // Two lines a tab, each the kit's row (the cursor's
            // shaded, with an accent bar at its left). Its state's mark,
            // its number in its project's color, its name on one line;
            // under it, what it is doing.
            let state = model::badge_state(tab, r);
            spins |= kit::spins(state);
            // The tab ← or → goes to lights under the mouse on it too,
            // the arrow at its right edge.
            let arrow = going
                .filter(|(t, _)| matches!(t, Going::Tab(t) if t.id == tab.id))
                .map(|(_, a)| a);
            let fill = if chosen {
                Some(theme.highlight)
            } else if view.hover == Some(&Owner::Tab(tab.id)) || arrow.is_some() {
                Some(theme.panel)
            } else {
                None
            };
            let number = tab.index.to_string();
            let name_style = if chosen || tab.id == view.here {
                bold(theme.bright)
            } else {
                fg(theme.text)
            };
            // A status you set: the kit's chip after its name (board 158).
            let by_you = model::by_hand(tab, r)
                .is_some()
                .then(|| kit::chip(model::BY_YOU, Tone::Dim, view.caps, theme));
            // Landing, the way it lands by on the kit's chip after it
            // (boards 193 to 196).
            let way = tab.way.map(|w| kit::way_chip(w.chip(), view.caps, theme));
            let chip_w = by_you.as_ref().map_or(0, |c| 1 + kit::spans_width(c))
                + way.as_ref().map_or(0, |c| 1 + kit::spans_width(c));
            // A tab following several PRs: the current one's number
            // before its name, how many more after it, dim (board 127).
            let (name, more) = match tab.prs.first() {
                Some(current) if current.current => (
                    format!("#{} {}", current.number, tab.name),
                    format!(" +{}", tab.prs.len() - 1),
                ),
                Some(_) => (tab.name.clone(), format!(" +{}", tab.prs.len())),
                None => (tab.name.clone(), String::new()),
            };
            let room = cols.saturating_sub(4 + width(&number) + tick_cols + chip_w + width(&more));
            let mut first = Row::default();
            tick(&mut first, tab.id);
            first
                .spans
                .push(kit::mark(Mark::of_badge(state), view.frame, theme));
            first
                .put(" ", Style::new())
                .put(number, fg(number_color(tab, theme)))
                .put(" ", Style::new())
                .put(text::cut(&name, room), name_style);
            if !more.is_empty() {
                first.put(more, fg(theme.dim));
            }
            for chip in [by_you, way].into_iter().flatten() {
                first.put(" ", Style::new());
                first.spans.extend(chip);
            }
            let kit_row = |row: Row| Line {
                spans: kit::row(
                    row.fit(cols.saturating_sub(1), None).spans,
                    cols,
                    chosen,
                    theme,
                ),
                fill,
            };
            let first = match arrow {
                Some(a) => {
                    let mut spans = first.fit(cols.saturating_sub(3), None).spans;
                    spans.push(Span::styled(a, Tone::Accent.bold(theme)));
                    Row { spans }
                }
                None => first,
            };
            lines.push(kit_row(first));
            owners.push(Some(Owner::Tab(tab.id)));
            let mut doing: Vec<String> = model::state_words(tab, r)
                .split(" · ")
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect();
            if let Some(c) = tab.checks.filter(|c| c.fail > 0) {
                doing.insert(0, format!("✕ {} failing", c.fail));
            }
            if let Some(n) = tab
                .pr
                .filter(|n| tab.prs.is_empty() && !tab.name.contains(&format!("#{n}")))
            {
                doing.push(format!("#{n}"));
            }
            // The other repository first, so a long state never cuts it.
            if let Some(repo) = &tab.from_repo {
                doing.insert(0, repo.clone());
            }
            if !doing.is_empty() || !tab.prs.is_empty() {
                let mut second = Row::default();
                second.put("    ", Style::new());
                // Each PR's mark by where it stands, the current one
                // first, before what it is doing.
                let mut used = 4;
                for pr in &tab.prs {
                    let words = format!(" #{}  ", pr.number);
                    if used + 1 + width(&words) > cols.saturating_sub(2) {
                        break;
                    }
                    second.spans.push(kit::mark(pr.mark, 0, theme));
                    second.put(words.clone(), fg(theme.dim));
                    used += 1 + width(&words);
                }
                if !doing.is_empty() {
                    second.put(
                        text::cut(&doing.join(" · "), cols.saturating_sub(2 + used)),
                        fg(theme.dim),
                    );
                }
                lines.push(kit_row(second));
                owners.push(Some(Owner::Tab(tab.id)));
            }
            if chosen {
                cursor_end = lines.len() - 1;
            }
        }
        lines.push(Line::blank(cols));
        owners.push(None);
    }

    // Each plugin that asks for it, its first items under its title, after
    // the tabs and scrolling with them.
    for section in sections {
        let mut heading = Row::default();
        heading
            .put(" ", Style::new())
            .put(section.title.to_uppercase(), fg(theme.dim))
            .put(format!(" {}", section.count), fg(theme.faint));
        if section.attention > 0 {
            heading.put(format!(" ! {}", section.attention), bold(YELLOW));
        }
        lines.push(heading.fit(cols, None));
        owners.push(None);
        if let Some(error) = &section.error {
            let mut row = Row::default();
            row.put("  ", Style::new())
                .put(error.clone(), fg(Color::Indexed(167)));
            lines.push(row.fit(cols, None));
            owners.push(None);
        }
        for item in &section.items {
            let mut row = Row::default();
            row.put("  ", Style::new());
            row.spans.push(item_mark(item, theme));
            row.put(" ", Style::new())
                .put(item.title.clone(), fg(theme.text));
            lines.push(row.fit(cols, None));
            owners.push(None);
        }
        lines.push(Line::blank(cols));
        owners.push(None);
    }

    // Notifications, pinned to the bottom under a double rule and their own
    // heading, so they read as no tab and never scroll away.
    let mut notice_lines: Vec<Line> = Vec::new();
    let mut notice_owners: Vec<Option<Owner>> = Vec::new();
    // Notifications open the sidebar, under the header, a line each, and
    // are gone when there are none; they never scroll away.
    let mut top_owners: Vec<Option<Owner>> = Vec::new();
    // Their heading folds or unfolds them on a click, so it lights under
    // the mouse; folded, they are one line of their kinds.
    let notices_lit = view.hover == Some(&Owner::Notices);
    let kinds = notices
        .iter()
        .map(|n| n.title.as_str())
        .collect::<Vec<_>>()
        .join(" · ");
    if !notices.is_empty() {
        let heading = Line {
            spans: kit::label(
                "Notifications",
                Tone::Accent,
                Some(notices.len()),
                cols,
                theme,
            ),
            fill: notices_lit.then_some(theme.panel),
        };
        head.push(heading);
        top_owners.push(Some(Owner::Notices));
        if view.notices_folded {
            let mut row = Row::default();
            row.put("▸ ", fg(theme.dim))
                .put(kinds.clone(), fg(theme.text));
            head.push(Line {
                spans: kit::row(
                    row.fit(cols.saturating_sub(1), None).spans,
                    cols,
                    false,
                    theme,
                ),
                fill: notices_lit.then_some(theme.panel),
            });
            top_owners.push(Some(Owner::Notices));
        }
        for n in notices.iter().filter(|_| !view.notices_folded) {
            let fill = if view.focused && view.cursor.is_some_and(|c| c.same_row(&n.owner)) {
                Some(theme.highlight)
            } else if view.hover == Some(&n.owner) {
                Some(theme.panel)
            } else {
                None
            };
            // Its kind's mark, the title, and how long ago it came at the
            // right edge, dim (board 1), laid out as columns so the title
            // is cut before the time column, a column short of it, whether
            // or not this one keeps a time: the titles end in one place.
            let when = text::ago(n.time, r.now_secs());
            let when_cols = width(&when).max(3);
            let chosen = view.focused && view.cursor.is_some_and(|c| c.same_row(&n.owner));
            let mark = match &n.owner {
                Owner::Notice { source, id } => super::notices::mark_of(source, id),
                _ => Mark::Needs,
            };
            let at = kit::columns(
                &[
                    kit::Col::Fixed {
                        min: 1,
                        natural: 1,
                        cap: 1,
                    },
                    kit::Col::Stretch {
                        weight: 1,
                        min: 1,
                        max: None,
                    },
                    kit::Col::Right {
                        natural: when_cols,
                        cap: when_cols,
                    },
                ],
                cols.saturating_sub(2),
                1,
                &[2],
            );
            let mut spans = Vec::new();
            if let Some(col) = at[0] {
                kit::place(&mut spans, vec![kit::mark(mark, 0, theme)], col);
            }
            if let Some(col) = at[1] {
                kit::place(
                    &mut spans,
                    vec![Span::styled(n.title.clone(), fg(theme.text))],
                    col,
                );
            }
            if let Some((x, w)) = at[2].filter(|_| !when.is_empty()) {
                // Flush right in its column.
                let pad = w.saturating_sub(width(&when));
                kit::place(
                    &mut spans,
                    vec![Span::styled(when, fg(theme.dim))],
                    (x + pad, w - pad),
                );
            }
            let row = Row { spans };
            head.push(Line {
                spans: kit::row(
                    row.fit(cols.saturating_sub(1), None).spans,
                    cols,
                    chosen,
                    theme,
                ),
                fill,
            });
            top_owners.push(Some(n.owner.clone()));
        }
        head.push(Line::blank(cols));
        top_owners.push(None);
    }

    // Over the keys, the coordinator's row while it runs: its mark and
    // whether it works, waits on you or sleeps, lit under the mouse, a
    // click showing it as M-C does.
    if let Some((mark, said)) = view.coordinator {
        let owner = Owner::Coordinator;
        let fill = (view.hover == Some(&owner)).then_some(theme.panel);
        spins |= mark == Mark::Working;
        let mut row = Row::default();
        row.put(" ", Style::new());
        row.spans.push(kit::mark(mark, view.frame, theme));
        row.put(" Coordinator ", fg(theme.text))
            .put(said, fg(theme.dim));
        notice_lines.push(row.fit(cols, fill));
        notice_owners.push(Some(owner));
    }

    // The last row: the keys that start, find and walk tabs, or with
    // the focus in the sidebar, start, find and open the chosen row's menu.
    let mut hint = Row::default();
    hint.put(" ", Style::new());
    // The four don't fit the default width, so the menu's key takes
    // M-a's place while the sidebar has the focus.
    let keys = if view.focused {
        [("M-t", "new"), ("M-j", "jump"), ("m", "menu")]
    } else {
        [("M-t", "new"), ("M-j", "jump"), ("M-a", "next")]
    };
    hint.spans.extend(kit::hints(&keys, theme));
    notice_lines.push(hint.fit(cols, None));
    notice_owners.push(None);

    // The list scrolls to keep the chosen tab in sight between the header
    // and the notifications, from where it was scrolled to (view.top, as C-d
    // and C-u leave it) when it is kept. Else it holds where the session's
    // list was last shown (view.keep) while the tab is in sight there, so
    // going from tab to tab doesn't move it, and otherwise stays at the top
    // while the tab is in sight there, or scrolls to the heading of the
    // tab's group. A list too long for its room keeps its last row to say
    // how many tabs are cut below.
    let length = lines.len();
    let space = rows.saturating_sub(head.len() + notice_lines.len());
    let room = if length > space {
        space.saturating_sub(1)
    } else {
        space
    };
    let in_sight = |at: usize| at <= cursor_at && cursor_end < at + room;
    let steady = match view.keep {
        Some(keep) if in_sight(keep) => keep,
        _ if cursor_end < room => 0,
        _ => headings
            .iter()
            .rev()
            .find(|h| **h <= cursor_at)
            .copied()
            .unwrap_or(0),
    };
    let wanted = view.top.unwrap_or(steady) as isize;
    // Never further than shows the list's end whole.
    let top = wanted
        .max((cursor_end + 1) as isize - room as isize)
        .min(cursor_at as isize)
        .min(length as isize - space as isize)
        .max(0) as usize;
    let whole = length.saturating_sub(top) <= space;
    let end = if whole { length } else { top + room };
    let tabs_in = |from: usize, to: usize| -> Vec<TabId> {
        let mut seen: Vec<TabId> = Vec::new();
        for t in owners[from.min(length)..to.min(length)]
            .iter()
            .flatten()
            .filter_map(Owner::tab)
        {
            if !seen.contains(&t) {
                seen.push(t);
            }
        }
        seen
    };
    // The tabs out of sight above, said on the filter line while no filter
    // is there, and below, on the list's last row; a click on either
    // scrolls as C-u or C-d does, and lights under the mouse as a tab does.
    let more = |mark: &str, owner: &Owner, n: usize| {
        let fill = (view.hover == Some(owner)).then_some(theme.panel);
        let said = if n > 0 {
            format!("{mark} {n} more")
        } else {
            format!("{mark} more")
        };
        let mut row = Row::default();
        row.put(" ", Style::new()).put(said, fg(theme.faint));
        row.fit(cols, fill)
    };
    let mut head_owners: Vec<Option<Owner>> = vec![None; head.len() - top_owners.len()];
    head_owners.extend(top_owners);
    if view.selected.is_some() {
        head_owners[0] = Some(Owner::SelectChip);
    } else if view.repo.is_some() {
        head_owners[0] = Some(Owner::RepoChip);
    }
    if build_row {
        head_owners[1] = Some(Owner::BuildRow);
    } else if top > 0 && view.filter.is_none() {
        // A tab cut at the top counts as above.
        head[1] = more("▲", &Owner::ScrollUp, tabs_in(0, top).len());
        head_owners[1] = Some(Owner::ScrollUp);
    }
    let below = (!whole).then(|| {
        // A tab cut at the bottom counts as below.
        let shown = tabs_in(top, end);
        tabs_in(end, length)
            .into_iter()
            .filter(|t| {
                !shown.contains(t)
                    || owners.get(end).cloned().flatten().and_then(|o| o.tab()) == Some(*t)
            })
            .count()
    });
    let head_rows = head.len();
    let mut shown: Vec<Line> = head;
    let mut shown_owners: Vec<Option<Owner>> = head_owners;
    shown.extend(lines.into_iter().skip(top).take(end - top));
    shown_owners.extend(owners[top..end].iter().cloned());
    if let Some(n) = below {
        shown.push(more("▼", &Owner::ScrollDown, n));
        shown_owners.push(Some(Owner::ScrollDown));
    }
    let list_end = rows.saturating_sub(notice_lines.len()).max(shown.len());
    while shown.len() < list_end {
        shown.push(Line::blank(cols));
        shown_owners.push(None);
    }
    shown.extend(notice_lines);
    shown_owners.extend(notice_owners);
    shown.truncate(rows);
    shown_owners.truncate(rows);
    if r.panel {
        // The sidebar on its shade, as the canvas draws it: every line not
        // filled already (the chosen row, a hovered one) on the sunken
        // surface, which the spans without a background of their own show.
        let shade = kit::Surface::Sunken.color(theme);
        for line in &mut shown {
            line.fill.get_or_insert(shade);
        }
    }
    Lines {
        lines: shown,
        owners: shown_owners,
        scroll: Scroll {
            top,
            list_y: head_rows,
            space,
            room: space,
            length,
            owners: owners
                .iter()
                .map(|o| {
                    o.clone()
                        .filter(|o| matches!(o, Owner::Tab(_) | Owner::Group(_)))
                })
                .collect(),
            cursor_at,
        },
        spins,
        reminders_at,
        buttons_at: buttons_at
            .into_iter()
            .filter(|(_, from, _)| *from < cols)
            .map(|(o, from, to)| (o, from, to.min(cols)))
            .collect(),
    }
}

/// Draws the lines into `area` of a buffer, a cell for each column as
/// `text::width` counts it, so what was measured and what is drawn agree.
pub fn draw(buf: &mut Buffer, area: Rect, lines: &[Line]) {
    let area = area.intersection(buf.area);
    for (row, line) in lines.iter().enumerate().take(area.height as usize) {
        let y = area.y + row as u16;
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            cell.reset();
            if let Some(fill) = line.fill {
                cell.set_bg(fill);
            }
        }
        let mut x = area.x;
        for span in &line.spans {
            let mut before = 0;
            // A control character never reaches a cell: one from a hook's
            // text (a file named with an escape) would reach the terminal.
            for c in span.content.chars().filter(|c| !c.is_control()) {
                let w = text::width_after(c, before);
                before = w;
                if w == 0 {
                    // Drawn on the character before it.
                    if x > area.x {
                        let cell = &mut buf[(x - 1, y)];
                        let mut symbol = cell.symbol().to_string();
                        symbol.push(c);
                        cell.set_symbol(&symbol);
                    }
                    continue;
                }
                if x + w as u16 > area.right() {
                    break;
                }
                let cell = &mut buf[(x, y)];
                cell.set_char(c);
                cell.set_style(span.style);
                for skip in 1..w as u16 {
                    let next = &mut buf[(x + skip, y)];
                    next.set_symbol("");
                    next.set_style(span.style);
                }
                x += w as u16;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::Checks;
    use crate::sidebar::model::TabState;
    use crate::sidebar::model::tests::{rules, tab};

    static NONE_FOLDED: std::collections::BTreeSet<Stage> = std::collections::BTreeSet::new();

    fn view(here: u32) -> View<'static> {
        View {
            focused: false,
            cursor: None,
            frame: 0,
            here: TabId(here),
            filter: None,
            top: None,
            keep: None,
            hover: None,
            awake: false,
            repo: None,
            folded: &NONE_FOLDED,
            buttons: None,
            caps: false,
            notices_folded: false,
            reminders: 0,
            build: None,
            selected: None,
            coordinator: None,
        }
    }

    fn texts(out: &Lines) -> Vec<String> {
        out.lines
            .iter()
            .map(|l| l.text().trim_end().to_string())
            .collect()
    }

    fn lines(tabs: &[TabModel], cols: usize, rows: usize, v: &View<'_>) -> Lines {
        sidebar_lines(tabs, None, &[], &[], cols, rows, v, &rules(), &Theme::DARK)
    }

    #[test]
    fn every_line_is_the_sidebars_width_however_long_the_names() {
        let mut long = tab(
            1,
            &"a very long name that goes on and on ".repeat(4),
            TabState::Waiting,
        );
        long.wait = Some("permission: Bash a very long command line indeed".into());
        let tabs = vec![long, tab(22, "日本語のタブ名前", TabState::Working)];
        let out = lines(&tabs, 30, 20, &view(1));
        assert_eq!(out.lines.len(), 20);
        for line in &out.lines {
            assert_eq!(width(&line.text()), 30, "{:?}", line.text());
        }
        // The name is cut to its one line.
        let name_lines = out
            .owners
            .iter()
            .zip(&out.lines)
            .filter(|(o, l)| **o == Some(Owner::Tab(TabId(1))) && l.text().contains("a very"))
            .count();
        assert_eq!(name_lines, 1);
        assert!(out.spins, "a working tab's badge spins");
    }

    #[test]
    fn a_tab_landing_through_switchboard_s_queue_wears_its_way_s_chip_under_landing() {
        let mut queued = tab(1, "rsvp", TabState::Idle);
        queued.landing = Some(crate::facts::Landing {
            holding: true,
            line: 1,
            method: crate::facts::Method::Pr,
        });
        queued.way = Some(crate::repos::Way::SwitchboardQueue);
        let tabs = vec![queued, tab(2, "busy", TabState::Working)];
        let out = lines(&tabs, 40, 14, &view(2));
        let text: Vec<String> = out.lines.iter().map(Line::text).collect();
        let heading = text
            .iter()
            .position(|l| l.starts_with(" LANDING"))
            .unwrap_or_else(|| panic!("{text:#?}"));
        let row = &text[heading + 1];
        assert!(
            row.contains("rsvp") && row.contains("Switchboard's queue"),
            "{row}"
        );
        assert!(
            !text
                .iter()
                .any(|l| l.contains("busy") && l.contains("queue"))
        );
    }

    #[test]
    fn a_tab_under_queue_and_clean_is_under_its_heading() {
        let mut landing = tab(1, "landing", TabState::Working);
        landing.loop_skill = Some("queue".into());
        let tabs = vec![landing, tab(2, "busy", TabState::Working)];
        let out = lines(&tabs, 30, 14, &view(1));
        let text: Vec<String> = out.lines.iter().map(Line::text).collect();
        let at = |heading: &str| text.iter().position(|l| l.starts_with(heading));
        let queue = at(" QUEUE-AND-CLEAN").expect("a Queue-and-clean heading");
        let working = at(" WORKING").expect("a Working heading");
        assert!(working < queue, "{text:?}");
        assert_eq!(out.owners[queue + 1], Some(Owner::Tab(TabId(1))));
    }

    #[test]
    fn the_build_row_is_the_line_under_the_count_and_a_click_stop() {
        let t = tab(1, "work", TabState::Idle);
        let r = Rules { ..rules() };
        let row = crate::mux::build_row::Row::Working {
            what: "Building",
            meter: Some((212, 415)),
            secs: Some(41),
            note: "",
        };
        let v = View {
            build: Some(&row),
            hover: Some(&Owner::BuildRow),
            ..view(1)
        };
        let out = sidebar_lines(
            std::slice::from_ref(&t),
            None,
            &[],
            &[],
            30,
            10,
            &v,
            &r,
            &Theme::DARK,
        );
        let text = texts(&out);
        assert!(text[1].starts_with(" ⠋ Building ▰▰▰▰▰"), "{:?}", text[1]);
        assert!(text[1].ends_with("41s"), "{:?}", text[1]);
        assert_eq!(out.owners[1], Some(Owner::BuildRow));
        assert_eq!(
            out.lines[1].fill,
            Some(Theme::DARK.panel),
            "lit under the mouse"
        );
        assert!(out.spins, "its spinner turns");
    }

    // Board 43: filtered to a repository, the top line keeps "/ filter"
    // beside the chip, the count giving way first, and Other repos folded
    // has ▸ and its tabs' marks on its heading, its repositories under it.
    #[test]
    fn the_filtered_sidebar_keeps_its_hint_and_folds_other_repos_onto_its_heading() {
        let mut bench = tab(2, "benchmark harness", TabState::Idle);
        bench.elsewhere = true;
        bench.repo_dir = Some("/code/sb-bench".into());
        let mut oncall = tab(3, "oncall watch", TabState::Waiting);
        oncall.elsewhere = true;
        oncall.repo_dir = Some("/code/oncall-tools".into());
        let tabs = vec![tab(1, "planning", TabState::Working), bench, oncall];
        let r = Rules { ..rules() };
        let folded: std::collections::BTreeSet<Stage> = [Stage::OtherRepos].into();
        let draw = |buttons: Option<&Buttons>, reminders: usize| {
            texts(&sidebar_lines(
                &tabs,
                None,
                &[],
                &[],
                34,
                30,
                &View {
                    repo: Some("switchboard"),
                    folded: &folded,
                    buttons,
                    reminders,
                    ..view(1)
                },
                &r,
                &Theme::DARK,
            ))
        };
        let text = draw(None, 0);
        assert!(text[0].contains("switchboard ×"), "{:?}", text[0]);
        assert!(text[0].contains("1 of 3"), "{:?}", text[0]);
        assert!(text[0].ends_with("/ filter"), "{:?}", text[0]);
        let heading = text.iter().position(|l| l.contains("OTHER REPOS")).unwrap();
        assert!(
            text[heading].starts_with(" ▸ OTHER REPOS ●○"),
            "{:?}",
            text[heading]
        );
        assert!(text[heading].ends_with(" 2"), "{:?}", text[heading]);
        assert_eq!(text[heading + 1], "   oncall-tools · sb-bench");
        // With the workspace's buttons first, the count gives way, then the
        // name is cut.
        let b = Buttons {
            trail: Some((None, None)),
            mission_open: false,
            needs: 1,
        };
        let text = draw(Some(&b), 0);
        assert!(text[0].contains("swi… ×"), "{:?}", text[0]);
        assert!(!text[0].contains("of 3"), "{:?}", text[0]);
        assert!(text[0].ends_with("/ filter"), "{:?}", text[0]);
        // With the reminders' chip too, the chip is its × alone, which
        // still clears the filter.
        let text = draw(Some(&b), 1);
        assert!(!text[0].contains("swi"), "{:?}", text[0]);
        assert!(text[0].ends_with("×"), "{:?}", text[0]);
    }

    // Board 1: each notification wears its kind's mark, and a failing
    // count the kit's ✕.
    #[test]
    fn notifications_wear_their_kinds_marks_and_failing_counts_the_kits() {
        let notice = |source: &str, id: &str, title: &str| Notice {
            owner: Owner::Notice {
                source: source.into(),
                id: id.into(),
            },
            title: title.into(),
            detail: String::new(),
            time: None,
        };
        let notices = [
            notice("meeting", "m", "Design review → 3:00"),
            notice("prs", "merged:x", "PR 212 merged"),
            notice("ci", "c", "main is failing"),
            notice("new-notes", "n", "Standup note written"),
        ];
        let mut red = tab(1, "full titles", TabState::Waiting);
        red.checks = Some(Checks {
            fail: 2,
            ..Checks::default()
        });
        let r = Rules { ..rules() };
        let text = texts(&sidebar_lines(
            std::slice::from_ref(&red),
            None,
            &notices,
            &[],
            34,
            20,
            &view(1),
            &r,
            &Theme::DARK,
        ));
        assert_eq!(text[3], " ◷ Design review → 3:00");
        assert_eq!(text[4], " ✓ PR 212 merged");
        assert_eq!(text[5], " ✕ main is failing");
        assert_eq!(text[6], " ✎ Standup note written");
        assert!(
            text.iter()
                .any(|l| l.ends_with("    ✕ 2 failing · waiting on you")),
            "{text:?}"
        );
    }

    #[test]
    fn the_top_lines_buttons_and_chips_round_their_ends_where_the_terminal_can() {
        let tabs = vec![tab(1, "planning", TabState::Working)];
        let b = Buttons {
            trail: Some((None, None)),
            mission_open: false,
            needs: 1,
        };
        let draw = |caps: bool| {
            sidebar_lines(
                &tabs,
                None,
                &[],
                &[],
                60,
                20,
                &View {
                    buttons: Some(&b),
                    repo: Some("switchboard"),
                    reminders: 2,
                    caps,
                    ..view(1)
                },
                &rules(),
                &Theme::DARK,
            )
        };
        let (l, r) = (kit::CAP_LEFT, kit::CAP_RIGHT);
        let round = draw(true);
        let top = &texts(&round)[0];
        for inner in ["←", "→", "⊞ 1", "switchboard ×", "⏰ 2"] {
            assert!(top.contains(&format!("{l}{inner}{r}")), "{inner}: {top:?}");
        }
        // The click boxes cover the drawn ends, as the padded ones do.
        let square = draw(false);
        assert_eq!(round.buttons_at, square.buttons_at);
        assert_eq!(round.reminders_at, square.reminders_at);
        let top = &texts(&square)[0];
        assert!(!top.contains(l) && !top.contains(r), "{top:?}");
        assert!(top.starts_with("  ←   →   ⊞ 1 "), "{top:?}");
    }

    #[test]
    fn the_top_line_starts_with_back_forward_and_mission_control() {
        let tabs = vec![
            tab(1, "planning", TabState::Working),
            tab(2, "stable note IDs", TabState::Waiting),
        ];
        let r = Rules { ..rules() };
        let draw = |b: &Buttons, hover: Option<&Owner>| {
            sidebar_lines(
                &tabs,
                None,
                &[],
                &[],
                40,
                20,
                &View {
                    buttons: Some(b),
                    hover,
                    ..view(1)
                },
                &r,
                &Theme::DARK,
            )
        };
        // Nothing back or forward: both faint; a tab needs you: its count.
        let b = Buttons {
            trail: Some((None, None)),
            mission_open: false,
            needs: 1,
        };
        let out = draw(&b, None);
        let text = texts(&out);
        assert!(
            text[0].starts_with("  ←   →   ⊞ 1  2 tabs"),
            "{:?}",
            text[0]
        );
        assert_eq!(out.lines[0].spans[2].style.fg, Some(Theme::DARK.faint));
        assert_eq!(
            out.buttons_at,
            vec![
                (Owner::Back, 1, 4),
                (Owner::Forward, 5, 8),
                (Owner::Mission, 9, 14)
            ]
        );
        // With a tab back there, the mouse over ← names it in place of the
        // count, and lights its row with the arrow at its right edge.
        let b = Buttons {
            trail: Some((Some(Going::Tab(Box::new(tabs[1].clone()))), None)),
            mission_open: false,
            needs: 0,
        };
        let out = draw(&b, Some(&Owner::Back));
        let text = texts(&out);
        assert!(
            text[0].starts_with("  ←   →   ⊞  ● 2 stable note IDs"),
            "{:?}",
            text[0]
        );
        assert!(!text[0].contains("tabs"), "{:?}", text[0]);
        let row = out
            .owners
            .iter()
            .position(|o| *o == Some(Owner::Tab(TabId(2))))
            .unwrap();
        assert_eq!(out.lines[row].fill, Some(Theme::DARK.panel));
        // A column in from the edge.
        assert!(text[row].ends_with("←"), "{:?}", text[row]);
        assert_eq!(text[row].chars().count(), 39, "{:?}", text[row]);
        // Over ⊞, "Mission control"; lit while it is open.
        let out = draw(&b, Some(&Owner::Mission));
        assert!(texts(&out)[0].contains("Mission control"));
        let open = Buttons {
            mission_open: true,
            ..b.clone()
        };
        let out = draw(&open, None);
        let lit = out.lines[0]
            .spans
            .iter()
            .find(|s| s.content == "⊞")
            .unwrap();
        assert_eq!(lit.style.bg, Some(crate::mux::compose::colors::ACCENT));
        // With back and forward off, ⊞ alone.
        let off = Buttons {
            trail: None,
            ..b.clone()
        };
        let text = texts(&draw(&off, None));
        assert!(text[0].starts_with("  ⊞  2 tabs"), "{:?}", text[0]);
    }

    #[test]
    fn the_tab_count_and_the_notifications_open_the_sidebar() {
        let t = tab(1, "work", TabState::Idle);
        let notice = Notice {
            owner: Owner::Notice {
                source: "slack".into(),
                id: "n1".into(),
            },
            title: "3 new on Slack".into(),
            detail: "Ada · #general".into(),
            time: Some(rules().now_secs() - 720),
        };
        let r = Rules { ..rules() };
        let out = sidebar_lines(
            std::slice::from_ref(&t),
            None,
            std::slice::from_ref(&notice),
            &[],
            40,
            20,
            &view(1),
            &r,
            &Theme::DARK,
        );
        assert_eq!(out.lines.len(), 20);
        let text = texts(&out);
        assert!(text[0].starts_with(" 1 tab"), "{:?}", text[0]);
        assert!(!text[0].contains(" s "), "no session's name: {:?}", text[0]);
        assert_eq!(text[2], format!(" NOTIFICATIONS{}1", " ".repeat(24)));
        assert_eq!(
            out.lines[2].spans[1].style.fg,
            Some(crate::mux::compose::colors::ACCENT)
        );
        // Its kind's mark, and how long ago it came, dim at the right edge.
        assert_eq!(text[3], format!(" @ 3 new on Slack{}12m", " ".repeat(19)));
        assert_eq!(out.lines[3].spans.last().unwrap().content, " ");
        let when = &out.lines[3].spans[out.lines[3].spans.len() - 2];
        assert_eq!(when.content, "12m");
        assert_eq!(when.style.fg, Some(Theme::DARK.dim));
        assert_eq!(out.owners[3], Some(notice.owner.clone()));
        // A long title is cut before the time column, a column short of
        // it, whether or not it keeps a time; none kept, none shown.
        let long = |time| Notice {
            title: "Landed the stable note IDs and the claims by them".into(),
            time,
            ..notice.clone()
        };
        let at = |n: &Notice| {
            texts(&sidebar_lines(
                std::slice::from_ref(&t),
                None,
                std::slice::from_ref(n),
                &[],
                40,
                20,
                &view(1),
                &r,
                &Theme::DARK,
            ))[3]
                .clone()
        };
        assert_eq!(
            at(&long(Some(rules().now_secs() - 3600))),
            " @ Landed the stable note IDs and …  1h"
        );
        assert_eq!(at(&long(None)), " @ Landed the stable note IDs and …");
        assert_eq!(text[4], "");
        assert!(text[0].ends_with("/ filter"), "{:?}", text[0]);
        // A ☕ after the count while the computer is held awake, the
        // filter's hint still at the right edge.
        let awake = texts(&sidebar_lines(
            std::slice::from_ref(&t),
            None,
            &[],
            &[],
            40,
            20,
            &View {
                awake: true,
                ..view(1)
            },
            &r,
            &Theme::DARK,
        ));
        assert!(awake[0].starts_with(" 1 tab  ☕ "), "{:?}", awake[0]);
        assert!(awake[0].ends_with("/ filter"), "{:?}", awake[0]);
        assert_eq!(
            crate::ui::draw::width(&awake[0]),
            crate::ui::draw::width(&text[0]),
            "the hint where it was"
        );
        // The group's heading with its count at the right edge, then the
        // tab: the cursor's bar, its mark with no pill, its number, its name.
        assert!(text[5].ends_with(" 1"), "{:?}", text[5]);
        assert_eq!(text[6], "▌○ 1 work");
        assert_eq!(out.owners[6], Some(Owner::Tab(TabId(1))));
        assert_eq!(text[19], " M-t new  M-j jump  M-a next");
        // With the focus in it, the menu's key in M-a's place, whole at the
        // default width.
        let focused = texts(&sidebar_lines(
            std::slice::from_ref(&t),
            None,
            std::slice::from_ref(&notice),
            &[],
            34,
            20,
            &View {
                focused: true,
                ..view(1)
            },
            &r,
            &Theme::DARK,
        ));
        assert_eq!(focused[19], " M-t new  M-j jump  m menu");
        assert_eq!(
            out.lines[19].spans[1].style.fg,
            Some(crate::mux::compose::colors::ACCENT)
        );
        assert!(
            !text.iter().any(|l| l.contains("═")),
            "none pinned at the bottom"
        );
        // None, none shown.
        let none = sidebar_lines(
            std::slice::from_ref(&t),
            None,
            &[],
            &[],
            40,
            20,
            &view(1),
            &r,
            &Theme::DARK,
        );
        assert!(!texts(&none).iter().any(|l| l.contains("NOTIFICATIONS")));
    }

    #[test]
    fn with_its_shade_every_line_of_the_sidebar_is_on_the_sunken_surface_but_the_chosen_row() {
        let tabs = [
            tab(1, "work", TabState::Idle),
            tab(2, "other", TabState::Idle),
        ];
        let shaded = |panel: bool, theme: &Theme| {
            let r = Rules { panel, ..rules() };
            sidebar_lines(&tabs, None, &[], &[], 34, 20, &view(1), &r, theme)
        };
        for theme in [Theme::DARK, Theme::DARK_TRUE, Theme::LIGHT] {
            let out = shaded(true, &theme);
            assert_eq!(out.lines.len(), 20);
            let sunken = kit::Surface::Sunken.color(&theme);
            let chosen: Vec<usize> = (0..out.lines.len())
                .filter(|&i| out.lines[i].text().starts_with('▌'))
                .collect();
            assert_eq!(chosen.len(), 1, "the chosen tab's row");
            for (i, line) in out.lines.iter().enumerate() {
                let want = if chosen.contains(&i) {
                    theme.highlight
                } else {
                    sunken
                };
                assert_eq!(line.fill, Some(want), "line {i}: {:?}", line.text());
            }
        }
        assert_eq!(
            kit::Surface::Sunken.color(&Theme::DARK_TRUE),
            Color::Rgb(0x11, 0x12, 0x1a)
        );
        let plain = shaded(false, &Theme::DARK);
        assert!(
            plain
                .lines
                .iter()
                .all(|l| l.fill.is_none() || l.fill == Some(Theme::DARK.highlight))
        );
    }

    #[test]
    fn a_plugins_first_items_sit_under_the_tabs_with_its_count() {
        let t = tab(1, "work", TabState::Idle);
        let items = vec![
            Item::new("1", "api rolling out").state(State::Attention),
            Item::new("2", "web").state(State::Active),
            Item::new("3", "jobs"),
        ];
        let section = Section::of("Deploys", &items, Some("one failed"), 2);
        assert_eq!(section.count, 3);
        assert_eq!(section.attention, 1);
        let out = sidebar_lines(
            std::slice::from_ref(&t),
            None,
            &[],
            std::slice::from_ref(&section),
            40,
            20,
            &view(1),
            &rules(),
            &Theme::DARK,
        );
        let text = texts(&out);
        let at = text.iter().position(|l| l.starts_with(" DEPLOYS")).unwrap();
        assert_eq!(
            text[at..at + 4],
            [
                " DEPLOYS 3 ! 1",
                "  one failed",
                "  ● api rolling out",
                "  ● web",
            ]
        );
        assert_eq!(text[at + 4], "", "a blank line after it");
        assert!(at > 2, "under the tabs");
        assert!(out.owners[at..at + 4].iter().all(Option::is_none));
    }

    #[test]
    fn the_list_holds_where_it_was_scrolled_while_the_cursor_is_in_sight() {
        let tabs: Vec<TabModel> = (1..=8)
            .map(|n| {
                let mut t = tab(n, &format!("tab {n}"), TabState::Idle);
                t.claude = None;
                t
            })
            .collect();
        let at = |cursor: u32, top: Option<usize>| {
            let owner = Owner::Tab(TabId(cursor));
            let v = View {
                focused: true,
                cursor: Some(&owner),
                top,
                ..view(1)
            };
            lines(&tabs, 30, 10, &v).scroll
        };
        let free = at(3, None);
        assert_eq!(free.top, 0, "nothing kept: as little scrolling as it takes");
        assert_eq!(
            at(3, Some(9)).top,
            free.cursor_at,
            "kept past the cursor: back up to it"
        );
        assert_eq!(at(5, Some(2)).top, 2, "the cursor in sight: the list holds");
        let last = at(8, Some(0));
        assert!(last.top > 0 && last.top <= last.cursor_at);
        assert_eq!(
            at(8, Some(99)).top,
            last.length - last.room,
            "never past the end"
        );
    }

    #[test]
    fn a_pinned_tab_wears_the_flag_in_place_of_its_state_mark() {
        let r = Rules { ..rules() };
        let mut pinned = tab(1, "pinned", TabState::Waiting);
        pinned.pinned = true;
        let waiting = tab(2, "waiting", TabState::Waiting);
        let out = sidebar_lines(
            &[pinned, waiting],
            None,
            &[],
            &[],
            40,
            20,
            &view(1),
            &r,
            &Theme::DARK,
        );
        let text = texts(&out);
        let row = |name: &str| {
            text.iter()
                .find(|l| l.contains(name) && !l.contains('·'))
                .unwrap_or_else(|| panic!("{name}: {text:?}"))
        };
        assert!(row("1 pinned").contains('⚑'), "{text:?}");
        assert!(!row("1 pinned").contains('●'), "{text:?}");
        assert!(row("2 waiting").contains('●'), "{text:?}");
        assert!(!row("2 waiting").contains('⚑'), "{text:?}");
        let flag = out
            .lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content == "⚑")
            .expect("the flag is a span of its own");
        assert_eq!(
            flag.style.fg,
            Some(kit::Tone::Needs.bold(&Theme::DARK).fg.unwrap())
        );
    }

    #[test]
    fn in_select_mode_selected_tabs_are_ticked_and_the_top_line_counts_them() {
        let tabs: Vec<TabModel> = (1..=4)
            .map(|n| tab(n, &format!("tab {n}"), TabState::Idle))
            .collect();
        let picked: std::collections::BTreeSet<TabId> = [TabId(1), TabId(2), TabId(4)].into();
        {
            let r = rules();
            let draw = |selected| {
                sidebar_lines(
                    &tabs,
                    None,
                    &[],
                    &[],
                    34,
                    30,
                    &View {
                        focused: true,
                        selected,
                        ..view(1)
                    },
                    &r,
                    &Theme::DARK,
                )
            };
            let on = draw(Some(&picked));
            let text = texts(&on);
            assert!(text[0].contains("3 selected"), "{:?}", text[0]);
            assert_eq!(on.owners[0], Some(Owner::SelectChip));
            let ticked: Vec<&String> = text.iter().filter(|l| l.contains('◆')).collect();
            assert_eq!(ticked.len(), 3, "{text:?}");
            assert!(!ticked.iter().any(|l| l.contains("tab 3")), "{text:?}");

            let off = texts(&draw(None));
            assert!(
                !off.iter()
                    .any(|l| l.contains('◆') || l.contains("selected"))
            );
        }
    }

    #[test]
    fn a_filter_shows_under_the_header_and_the_chosen_tab_is_lit() {
        let tabs = vec![tab(1, "one", TabState::Idle), tab(2, "two", TabState::Idle)];
        let filter = Filter {
            text: "tw".into(),
            typing: true,
        };
        let owner = Owner::Tab(TabId(2));
        let v = View {
            focused: true,
            cursor: Some(&owner),
            filter: Some(&filter),
            hover: Some(&Owner::Tab(TabId(1))),
            ..view(1)
        };
        let out = lines(&tabs, 30, 12, &v);
        assert!(out.lines[1].text().starts_with(" /tw▏"));
        let two = out
            .owners
            .iter()
            .position(|o| *o == Some(owner.clone()))
            .unwrap();
        assert_eq!(out.lines[two].fill, Some(Theme::DARK.highlight));
        assert!(out.lines[two].text().starts_with("▌"));
        let one = out
            .owners
            .iter()
            .position(|o| *o == Some(Owner::Tab(TabId(1))))
            .unwrap();
        assert_eq!(
            out.lines[one].fill,
            Some(Theme::DARK.panel),
            "under the mouse"
        );
    }

    // test/tmux-sidebar.test.js's "a landing without asking marks its tab":
    // the sidebar says so once it has landed.

    #[test]
    fn drawing_puts_each_character_in_the_columns_it_takes() {
        let line = Row::default();
        let mut row = line;
        row.put("a📌b", Style::new());
        let lines = vec![row.fit(6, Some(Color::Indexed(238)))];
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 1));
        draw(&mut buf, Rect::new(1, 0, 6, 1), &lines);
        assert_eq!(buf[(1, 0)].symbol(), "a");
        assert_eq!(buf[(2, 0)].symbol(), "📌");
        assert_eq!(buf[(4, 0)].symbol(), "b");
        assert_eq!(buf[(6, 0)].bg, Color::Indexed(238));
        assert_eq!(buf[(7, 0)].symbol(), " ", "nothing past its area");
    }

    #[test]
    fn drawing_leaves_control_characters_out() {
        let mut row = Row::default();
        row.put("a\u{1b}]0;x\u{7}b\u{9b}c", Style::new());
        let lines = vec![row.fit(8, None)];
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 1));
        draw(&mut buf, Rect::new(0, 0, 8, 1), &lines);
        let drawn: String = (0..8).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert_eq!(drawn.trim_end(), "a]0;xbc");
    }
    #[test]
    fn a_folded_group_is_its_heading_and_one_line_of_its_tabs_marks() {
        let mut waiting = tab(1, "asks", TabState::Waiting);
        waiting.wait = Some("permission".into());
        let tabs = vec![
            waiting,
            tab(2, "busy", TabState::Working),
            tab(3, "also busy", TabState::Working),
        ];
        let folded = std::collections::BTreeSet::from([Stage::Working]);
        let v = View {
            folded: &folded,
            ..view(1)
        };
        let out = lines(&tabs, 34, 20, &v);
        let text = texts(&out);
        let heading = text
            .iter()
            .position(|l| l.starts_with(" WORKING"))
            .expect("Working's heading with its count");
        assert!(text[heading + 1].contains("2 tabs"), "{text:?}");
        assert!(!text.iter().any(|l| l.contains("busy")), "{text:?}");
        assert_eq!(out.owners[heading], Some(Owner::Group(Stage::Working)));
        assert_eq!(out.owners[heading + 1], Some(Owner::Group(Stage::Working)));
        // Needs you, unfolded, still lists its tab, its heading a click
        // target of its own.
        let needs = text
            .iter()
            .position(|l| l.starts_with(" NEEDS YOU"))
            .unwrap();
        assert_eq!(out.owners[needs], Some(Owner::Group(Stage::Needs)));
        assert!(text.iter().any(|l| l.contains("asks")), "{text:?}");
    }

    #[test]
    fn tabs_left_out_for_their_reminders_are_counted_on_the_top_lines_chip() {
        let v = View {
            reminders: 3,
            ..view(1)
        };
        let out = lines(&[tab(1, "work", TabState::Idle)], 40, 10, &v);
        let top = out.lines[0].text();
        let (from, to) = out.reminders_at.expect("the chip's columns");
        let chip: String = top.chars().skip(from).take(to - from).collect();
        assert!(chip.contains("⏰ 3"), "{top:?} {from}..{to}");
        assert!(
            lines(&[tab(1, "work", TabState::Idle)], 40, 10, &view(1))
                .reminders_at
                .is_none()
        );
    }
}
