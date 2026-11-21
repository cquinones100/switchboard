//! The tab header (UX 2.0's design): the rows at the top of a tab's area,
//! above its panes, in place of the status line tmux drew at the bottom.
//! New, with no tmux-version file behind it, since tmux drew nothing over a
//! window's panes.
//!
//! With the sidebar hidden, its first row starts with the workspace's own
//! buttons, as the sidebar's top line does (boards 57 to 60): ← and → back
//! and forward through the tabs visited, and ⊞ mission control.
//!
//! Its first row is filled with the tab's own color (the one its number
//! has in the sidebar, src/tabs/color.rs), and on it, dark, the state's
//! mark, the tab's name in bold (cut to keep the path, the state and
//! where M-a goes next on the line), the repository it works in (its main
//! clone, never a worktree's long path), its branch, cut first where the
//! row is narrow, what Claude is doing and
//! how full its context is (`ctx 62%`, as its statusline last told the
//! server);
//! while the tab's history shows a past moment, which ("viewing 1:41 PM, 51
//! minutes ago") in place of where M-a goes next;
//! under a blank row, the keys of what the tab can show in Claude's place
//! (M-c Claude, M-d its diff, M-v its PR, M-i its ticket, M-e its note), the
//! one it shows now highlighted, then M-h for the tab's history (src/history),
//! lit while it is up, and at the row's right end the ⌘ Commands
//! button (the palette_button setting), lit while the palette is open, which
//! gives way where the header is too narrow for both; and a rule under them. What it shows is read
//! from the tab's focused pane: a tool in Claude's place (`@in_place`), a
//! view marked as the PR's or the ticket's, or the notes pane.
//!
//! It is drawn with the kit's atoms (crates/ui/src/kit.rs): the state's mark, a
//! keycap for each key (the half circles at its ends where the client's
//! terminal draws them) and the rule.
//!
//! Notes' own tab goes without it while the sidebar shows
//! (`Session::headerless`): the app draws its own top line (board 7), and
//! its pane takes the header's rows.
//!
//! It is drawn from what the server already holds, as everything on the
//! drawing path is: no file is read and no process started for it.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;

use crate::agent::State;
use crate::mux::compose;
use crate::mux::keys::Action;
use crate::sidebar::text;
use crate::status::theme::Palette;
use crate::ui::Theme;
use crate::ui::kit::{self, Mark};

/// The rows the header takes: the name, a blank row, the keys, the rule.
pub const ROWS: u16 = 4;

/// What a tab shows in Claude's place, each with the key that shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    Claude,
    Diff,
    Pr,
    Ticket,
    Note,
}

impl Shown {
    pub const ALL: [Shown; 5] = [
        Shown::Claude,
        Shown::Diff,
        Shown::Pr,
        Shown::Ticket,
        Shown::Note,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Shown::Claude => "M-c",
            Shown::Diff => "M-d",
            Shown::Pr => "M-v",
            Shown::Ticket => "M-i",
            Shown::Note => "M-e",
        }
    }

    /// What its key is bound to, which a click on it does too.
    pub fn action(self) -> Action {
        match self {
            Shown::Claude => Action::FocusClaude,
            Shown::Diff => Action::Diff(crate::diff::Command::Diff),
            Shown::Pr => Action::View(crate::views::Command::PrView),
            Shown::Ticket => Action::View(crate::views::Command::TicketView),
            Shown::Note => Action::Notes(crate::notes::serve::Command::Beside),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Shown::Claude => "Claude",
            Shown::Diff => "Diff",
            Shown::Pr => "PR",
            Shown::Ticket => "Ticket",
            Shown::Note => "Note",
        }
    }

    /// What a pane with these options shows: a tool in its pane's place by
    /// its `@in_place`, a view by its mark, the notes pane by its own.
    pub fn of_pane(opt: impl Fn(&str) -> Option<String>) -> Shown {
        let on = |k: &str| opt(k).is_some_and(|v| v == "1");
        // The tool names src/diff and src/views give `open_in_place`, and
        // the views' marks (src/views/serve.rs's PR_VIEW and TICKET_VIEW).
        match opt("@in_place").as_deref() {
            Some("diff") => return Shown::Diff,
            Some("pr-view") => return Shown::Pr,
            Some("ticket-view") => return Shown::Ticket,
            _ => {}
        }
        if on("@pr_view") {
            Shown::Pr
        } else if on("@ticket_view") {
            Shown::Ticket
        } else if on("@notes_pane") {
            Shown::Note
        } else {
            Shown::Claude
        }
    }
}

/// What a click in the header lands on: a key, or the ⌘ Commands button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    /// ← and →, back and forward through the tabs visited, and ⊞,
    /// mission control, at the first row's left end with the sidebar
    /// hidden.
    Back,
    Forward,
    Mission,
    Show(Shown),
    /// M-h, the tab's history (src/history).
    History,
    Commands,
    /// A row of the hints in the spare columns beside a lone pane
    /// (src/mux/beside.rs), which go where the header's keys go.
    Beside(super::beside::Beside),
}

impl Button {
    /// What a click on it does: the key's action, or the palette's (a
    /// click while the palette is open lands outside it, which closes it).
    pub fn action(self) -> Action {
        match self {
            Button::Back => Action::TabBack,
            Button::Forward => Action::TabForward,
            Button::Mission => Action::MissionControl,
            Button::Show(shown) => shown.action(),
            Button::History => Action::History,
            Button::Commands => Action::Palette,
            Button::Beside(beside) => beside.action(),
        }
    }
}

/// What the header says about a tab.
#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    /// The tab's number, as the sidebar shows it; 0 for a window that
    /// takes none (Notifications), which shows none.
    pub number: usize,
    pub name: String,
    /// Where the tab works, as it is shown (`~/code/app`).
    pub path: String,
    /// The branch it is on, as tab naming last worked it out; empty when
    /// not known.
    pub branch: String,
    pub state: Option<State>,
    /// How long it has been in that state (`4m`), empty when not known.
    pub age: String,
    /// How long the tab has been open (`1h12m`), empty when not known or
    /// with the tab_age setting off.
    pub opened: String,
    pub color: Option<Color>,
    pub shown: Shown,
    /// The tab M-a goes to next, its number and name.
    pub next: Option<(usize, String)>,
    /// The ⌘ Commands button is shown (the palette_button setting).
    pub commands: bool,
    /// M-h's key is shown (the tab_history setting), and lit while this
    /// client shows the tab's history.
    pub history: bool,
    pub history_open: bool,
    /// The moment the history shows, as "viewing 1:41 PM, 51 minutes ago".
    pub viewing: Option<String>,
    /// How full its Claude's context is, in percent, once its statusline
    /// has said.
    pub context: Option<u8>,
    /// With several PRs (`tabs::prs`), the current one's place among them
    /// (from 1), how many, and its number: the first row says how many, and
    /// M-v's key which it shows (board 127).
    pub prs: Option<(usize, usize, u64)>,
    /// The workspace's own buttons, while the sidebar that otherwise
    /// holds them is hidden.
    pub nav: Option<Nav>,
}

/// What the workspace's own buttons show: whether ← and → are drawn and
/// have a tab to go to, and ⊞'s count of tabs that need you.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Nav {
    /// Back and forward step through the tabs visited (tab_back_forward):
    /// whether each has a tab that way.
    pub trail: Option<(bool, bool)>,
    pub needs: usize,
}

/// A state's mark in the kit.
fn mark_of(state: Option<State>) -> Mark {
    match state {
        Some(State::Waiting) => Mark::Needs,
        Some(State::Done) => Mark::Passed,
        Some(State::Working) => Mark::Working,
        None => Mark::Idle,
    }
}

/// Where each key and the button went, for a click on it.
pub type Keys = Vec<(Rect, Button)>;

/// The button's key and what it does, drawn as a keycap.
const COMMANDS: (&str, &str) = ("⌘", "Commands");

/// The tab's history's key and what it shows.
const HISTORY: (&str, &str) = ("M-h", "History");

/// Draws the header into `area` (its full width, `ROWS` high), and gives
/// where each key and the button went.
/// What is under the mouse, with hover_highlight on, is lit. `caps`: the
/// kit's half circles are written (src/mux/graphics.rs's `round_ends`).
/// `palette`: the
/// palette is open, which lights the button.
pub fn draw(
    buf: &mut Buffer,
    area: Rect,
    h: &Header,
    hover: Option<Button>,
    p: &Palette,
    caps: bool,
    palette: bool,
) -> Keys {
    let mut keys = Keys::new();
    if area.height == 0 || area.width == 0 {
        return keys;
    }
    // Dark words on the tab's color, bright on a dark one; a tab with none
    // yet keeps the plain band, the name bright and the rest dim.
    let (band, name_fg, words_fg) = match h.color {
        Some(color) => {
            let on = crate::tabs::color::words_on(color);
            (Style::new().bg(color).fg(on), on, on)
        }
        None => (Style::new().bg(p.status_bg), p.bright, p.dim),
    };
    let first = Rect::new(area.x, area.y, area.width, 1);
    buf.set_style(first, band);
    let theme = if *p == Palette::LIGHT {
        Theme::LIGHT
    } else if *p == Palette::DARK_TRUE {
        Theme::DARK_TRUE
    } else {
        Theme::DARK
    };
    // The spinner stands still here (its third frame, `⠹`): the header
    // is drawn on focus and tick, not on the spinner's turn.
    let mark = kit::mark(mark_of(h.state), 2, &theme);
    let mark_style = match h.color {
        Some(_) => band.add_modifier(mark.style.add_modifier),
        None => band.patch(mark.style),
    };
    let mut x = area.x + 1;
    if let Some(nav) = h.nav {
        let press = |button: Button, idle: bool| {
            if hover == Some(button) {
                kit::Press::Hover
            } else if idle {
                kit::Press::Idle
            } else {
                kit::Press::Rest
            }
        };
        let mut list = Vec::new();
        if let Some((back, forward)) = nav.trail {
            list.push((Button::Back, "←", None, press(Button::Back, !back)));
            list.push((Button::Forward, "→", None, press(Button::Forward, !forward)));
        }
        let count = (nav.needs > 0).then(|| (nav.needs.to_string(), kit::Tone::Needs));
        list.push((Button::Mission, "⊞", count, press(Button::Mission, false)));
        for (button, glyph, count, press) in list {
            let spans = kit::button(glyph, count, press, caps, &theme);
            let start = x;
            x = crate::ui::draw::put_line(
                buf,
                x,
                first.y,
                area.right().saturating_sub(x),
                &Line::from(spans),
            );
            if x > start {
                keys.push((Rect::new(start, first.y, x - start, 1), button));
            }
            x += 1;
        }
        x += 1;
    }
    x = compose::put(buf, x, first, &mark.content, mark_style);
    // "waiting 4m": how long a tab has waited is what tells you where M-a
    // should take you first.
    let state = h.state.map(|s| {
        if h.age.is_empty() {
            s.name().to_string()
        } else {
            format!("{} {}", s.name(), h.age)
        }
    });
    let opened = (!h.opened.is_empty()).then(|| format!("opened {}", h.opened));
    let context = h.context.map(|used| format!("ctx {used}%"));
    // The moment a tab's history shows, at the right end in place of where
    // M-a goes next, its room kept: the facts give way to it.
    let viewing_at = h.viewing.as_ref().map(|v| {
        let width = crate::sidebar::text::width(v) as u16 + 1;
        area.right().saturating_sub(width).max(area.x)
    });
    let joined = |parts: &[&str]| text::width(&parts.join(" · "));
    let must: Vec<&str> = std::iter::once(h.path.as_str())
        .chain(state.as_deref())
        .collect();
    // The tab's number before its name, bold as the name is: M-1…9,
    // `switchboard tabs` and messages between tabs name a tab by it. It
    // is kept at any width: the name after it is what is cut.
    if h.number > 0 {
        x = compose::put(
            buf,
            x + 1,
            first,
            &h.number.to_string(),
            band.fg(name_fg).add_modifier(Modifier::BOLD),
        );
    }
    // The name, the path and the state, then where M-a goes next at the
    // right: the name is cut to keep their room, down to a few words, then
    // the path, and where even both at their least leave too little, M-a's
    // next gives way.
    let name_x = x + 1;
    const NAME_MIN: usize = 12;
    const PATH_MIN: usize = 8;
    let left_of = |right: u16| right.saturating_sub(2).saturating_sub(name_x) as usize;
    let name_room = |right: u16| left_of(right).saturating_sub(3 + joined(&must));
    let least_facts = joined(&must) - text::width(&h.path) + text::width(&h.path).min(PATH_MIN);
    // Its name is cut to what the least of those leaves (a long item's
    // title is the usual name), and it gives way only below a few columns.
    const NEXT_NAME_MIN: usize = 6;
    // The path whole first, else at its least.
    let left_with =
        |facts: usize| name_x as usize + NAME_MIN.min(text::width(&h.name)) + 3 + facts + 2;
    let next = h
        .next
        .as_ref()
        .filter(|_| h.viewing.is_none())
        .and_then(|(number, name)| {
            let key = format!("M-a next: {number} ");
            let least = NEXT_NAME_MIN.min(text::width(name));
            let room = |facts: usize| {
                (area.right() as usize)
                    .saturating_sub(left_with(facts) + text::width(&key) + 1)
                    .min(text::width(name))
            };
            [joined(&must), least_facts]
                .into_iter()
                .map(room)
                .find(|room| *room >= least)
                .map(|room| format!("{key}{} ", text::cut(name, room)))
        })
        .map(|next| {
            let at = area.right().saturating_sub(text::width(&next) as u16);
            (next, at)
        });
    let right = viewing_at
        .or(next.as_ref().map(|(_, at)| *at))
        .unwrap_or(first.right());
    let name = text::cut(&h.name, name_room(right).max(NAME_MIN));
    x = compose::put(
        buf,
        name_x,
        first,
        &name,
        band.fg(name_fg).add_modifier(Modifier::BOLD),
    );
    let facts_x = x + 3;
    let room = right.saturating_sub(2).saturating_sub(facts_x) as usize;
    // Then the branch, cut to what is left (dropped when that leaves it no
    // more than a few columns), then how long the tab has been open and
    // how full its context is, each only where it fits, so none pushes the
    // state or M-a's next off the line.
    let sep = " · ".chars().count();
    // A path too long for the room left once the name has its least is
    // cut, so the state after it still shows.
    let state_cols = state.as_deref().map_or(0, |s| sep + text::width(s));
    let path = text::cut(&h.path, room.saturating_sub(state_cols).max(PATH_MIN));
    let mut used = text::width(&path) + state_cols;
    let branch = (!h.branch.is_empty())
        .then(|| text::cut(&h.branch, room.saturating_sub(used + sep)))
        .filter(|b| *b == h.branch || text::width(b) >= 6);
    if let Some(b) = &branch {
        used += sep + text::width(b);
    }
    let mut extra = Vec::new();
    let prs = h.prs.map(|(_, all, _)| format!("{all} PRs"));
    for fact in [opened, context, prs].into_iter().flatten() {
        if used + sep + text::width(&fact) <= room {
            used += sep + text::width(&fact);
            extra.push(fact);
        }
    }
    let mut facts = vec![path];
    facts.extend(branch);
    facts.extend(state);
    facts.extend(extra);
    let facts_room = Rect {
        width: viewing_at.map_or(first.width, |at| {
            at.saturating_sub(2).saturating_sub(first.x)
        }),
        ..first
    };
    let end = compose::put(
        buf,
        facts_x,
        facts_room,
        &facts.join(" · "),
        band.fg(words_fg),
    );
    if let (Some(viewing), Some(at)) = (&h.viewing, viewing_at) {
        // Over the name too where the header is too narrow for both.
        compose::put(
            buf,
            at,
            first,
            viewing,
            band.fg(name_fg).add_modifier(Modifier::BOLD),
        );
    } else if let Some((next, at)) = &next
        && *at > end + 1
    {
        compose::put(buf, *at, first, next, band.fg(words_fg));
    }
    if area.height < 3 {
        return keys;
    }
    let row = Rect::new(area.x, area.y + 2, area.width, 1);
    let mut x = area.x + 1;
    for shown in Shown::ALL {
        let label = match (shown, h.prs) {
            (Shown::Pr, Some((at, all, number))) => format!("PR #{number} · {at} of {all}"),
            _ => shown.label().to_string(),
        };
        let mut cap = kit::keycap(shown.key(), &label, shown == h.shown, caps, &theme);
        if hover == Some(Button::Show(shown)) {
            cap[1].style = cap[1].style.add_modifier(Modifier::REVERSED);
        }
        let start = x;
        x = crate::ui::draw::put_line(
            buf,
            x,
            row.y,
            area.right().saturating_sub(x),
            &Line::from(cap),
        );
        if x > start {
            keys.push((Rect::new(start, row.y, x - start, 1), Button::Show(shown)));
        }
        x += 1;
    }
    if h.history {
        let mut cap = kit::keycap(HISTORY.0, HISTORY.1, h.history_open, caps, &theme);
        if hover == Some(Button::History) {
            cap[1].style = cap[1].style.add_modifier(Modifier::REVERSED);
        }
        let start = x;
        x = crate::ui::draw::put_line(
            buf,
            x,
            row.y,
            area.right().saturating_sub(x),
            &Line::from(cap),
        );
        if x > start {
            keys.push((Rect::new(start, row.y, x - start, 1), Button::History));
        }
        x += 1;
    }
    if h.commands {
        let mut cap = kit::keycap(COMMANDS.0, COMMANDS.1, palette, caps, &theme);
        if hover == Some(Button::Commands) {
            cap[1].style = cap[1].style.add_modifier(Modifier::REVERSED);
        }
        let line = Line::from(cap);
        let width = line.width() as u16;
        // At the row's right end, a column in from the edge, and only
        // with a column between it and the last key.
        let at = area.right().saturating_sub(width + 1);
        if at >= x && at > area.x {
            let end = crate::ui::draw::put_line(buf, at, row.y, width, &line);
            keys.push((Rect::new(at, row.y, end - at, 1), Button::Commands));
        }
    }
    if area.height >= 4 {
        let rule = Rect::new(area.x, area.y + 3, area.width, 1);
        let line = kit::rule(area.width as usize, &theme);
        compose::put(buf, area.x, rule, &line.content, line.style.fg(p.border));
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dark words on a light tab color.
    const ON_COLOR: Color = Color::Indexed(232);

    fn text(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    fn header(shown: Shown) -> Header {
        Header {
            number: 0,
            name: "planning".into(),
            path: "~/switchboard".into(),
            branch: String::new(),
            state: Some(State::Working),
            age: "2m".into(),
            opened: String::new(),
            color: None,
            shown,
            next: None,
            commands: false,
            history: false,
            history_open: false,
            viewing: None,
            context: None,
            prs: None,
            nav: None,
        }
    }

    #[test]
    fn with_the_sidebar_hidden_its_buttons_start_the_first_row_and_take_clicks() {
        let area = Rect::new(0, 0, 90, ROWS);
        let mut buf = Buffer::empty(area);
        let h = Header {
            nav: Some(Nav {
                trail: Some((true, false)),
                needs: 3,
            }),
            ..header(Shown::Claude)
        };
        let keys = draw(&mut buf, area, &h, None, &Palette::DARK, false, false);
        let first = text(&buf, 0);
        assert!(first.starts_with("  ←   →   ⊞ 3   ⠹ planning"), "{first:?}");
        let buttons: Vec<Button> = keys.iter().map(|(_, b)| *b).take(3).collect();
        assert_eq!(buttons, [Button::Back, Button::Forward, Button::Mission]);
        assert_eq!(keys[0].0, Rect::new(1, 0, 3, 1));
        assert_eq!(Button::Back.action(), Action::TabBack);
        assert_eq!(Button::Mission.action(), Action::MissionControl);
        // → has no tab that way: faint.
        assert_eq!(buf[(6, 0)].fg, Theme::DARK.faint);
        // With back and forward off, ⊞ alone.
        let h = Header {
            nav: Some(Nav {
                trail: None,
                needs: 0,
            }),
            ..header(Shown::Claude)
        };
        let mut buf = Buffer::empty(area);
        draw(&mut buf, area, &h, None, &Palette::DARK, false, false);
        assert!(
            text(&buf, 0).starts_with("  ⊞   ⠹ planning"),
            "{:?}",
            text(&buf, 0)
        );
    }

    #[test]
    fn names_the_tab_and_lists_the_keys() {
        let area = Rect::new(0, 0, 90, ROWS);
        let mut buf = Buffer::empty(area);
        let keys = draw(
            &mut buf,
            area,
            &header(Shown::Claude),
            None,
            &Palette::DARK,
            false,
            false,
        );
        assert_eq!(text(&buf, 0), " ⠹ planning   ~/switchboard · working 2m");
        assert_eq!(text(&buf, 1), "");
        assert_eq!(
            text(&buf, 2),
            "  M-c Claude    M-d  Diff   M-v  PR   M-i  Ticket   M-e  Note"
        );
        assert!(text(&buf, 3).starts_with("───"));
        let shown: Vec<Button> = keys.iter().map(|(_, s)| *s).collect();
        assert_eq!(shown, Shown::ALL.map(Button::Show).to_vec());
    }

    #[test]
    fn the_tabs_number_comes_before_its_name_and_outlasts_it() {
        let area = Rect::new(0, 0, 90, ROWS);
        let mut buf = Buffer::empty(area);
        let h = Header {
            number: 4,
            ..header(Shown::Claude)
        };
        draw(&mut buf, area, &h, None, &Palette::DARK, false, false);
        assert_eq!(text(&buf, 0), " ⠹ 4 planning   ~/switchboard · working 2m");
        // Bold, as the name is.
        assert!(buf[(3, 0)].modifier.contains(Modifier::BOLD));
        // A narrow header cuts the name and keeps the number.
        let narrow = Rect::new(0, 0, 44, ROWS);
        let mut buf = Buffer::empty(narrow);
        let h = Header {
            number: 12,
            name: "a tab with a long name that can't fit".into(),
            ..header(Shown::Claude)
        };
        draw(&mut buf, narrow, &h, None, &Palette::DARK, false, false);
        let first = text(&buf, 0);
        assert!(first.starts_with(" ⠹ 12 a tab with"), "{first:?}");
        assert!(!first.contains("can't fit"), "{first:?}");
        assert!(first.contains("working 2m"), "{first:?}");
    }

    #[test]
    fn says_how_full_the_context_is_after_the_state() {
        let area = Rect::new(0, 0, 90, ROWS);
        let mut buf = Buffer::empty(area);
        let h = Header {
            context: Some(62),
            prs: None,
            ..header(Shown::Claude)
        };
        draw(&mut buf, area, &h, None, &Palette::DARK, false, false);
        assert!(
            text(&buf, 0).starts_with(" ⠹ planning   ~/switchboard · working 2m · ctx 62%"),
            "{}",
            text(&buf, 0)
        );
    }

    #[test]
    fn says_how_long_the_tab_has_been_open_after_the_state() {
        let area = Rect::new(0, 0, 90, ROWS);
        let mut buf = Buffer::empty(area);
        let h = Header {
            opened: "1h12m".into(),
            context: Some(62),
            prs: None,
            ..header(Shown::Claude)
        };
        draw(&mut buf, area, &h, None, &Palette::DARK, false, false);
        assert!(
            text(&buf, 0)
                .starts_with(" ⠹ planning   ~/switchboard · working 2m · opened 1h12m · ctx 62%"),
            "{}",
            text(&buf, 0)
        );
        // The moment the history shows keeps its room: the age gives way.
        let narrow = Rect::new(0, 0, 60, ROWS);
        let mut buf = Buffer::empty(narrow);
        let h = Header {
            opened: "1h12m".into(),
            viewing: Some("viewing 1:41 PM, 51 minutes ago".into()),
            ..header(Shown::Claude)
        };
        draw(&mut buf, narrow, &h, None, &Palette::DARK, false, false);
        let first = text(&buf, 0);
        assert!(
            first.ends_with("viewing 1:41 PM, 51 minutes ago"),
            "{first}"
        );
        assert!(!first.contains("opened 1h12m"), "{first}");
    }

    #[test]
    fn says_the_branch_and_where_m_a_goes_next() {
        let area = Rect::new(0, 0, 90, ROWS);
        let mut buf = Buffer::empty(area);
        let h = Header {
            branch: "main".into(),
            next: Some((3, "billing".into())),
            ..header(Shown::Claude)
        };
        draw(&mut buf, area, &h, None, &Palette::DARK, false, false);
        let first = text(&buf, 0);
        assert!(first.starts_with(" ⠹ planning   ~/switchboard · main · working 2m"));
        assert!(first.ends_with("M-a next: 3 billing"));
        let at = 90 - "M-a next: 3 billing ".len() as u16;
        assert_eq!(buf[(at, 0)].fg, Palette::DARK.dim);
    }

    // Board 1: a long branch is cut, never the state or where M-a goes
    // next, so they always fit.
    #[test]
    fn a_long_branch_gives_way_to_the_state_and_m_a_next() {
        let area = Rect::new(0, 0, 90, ROWS);
        let mut buf = Buffer::empty(area);
        let h = Header {
            branch: "note/bug-tab-header-and-sidebar-crowd-state-and-miscount".into(),
            next: Some((3, "billing".into())),
            ..header(Shown::Claude)
        };
        draw(&mut buf, area, &h, None, &Palette::DARK, false, false);
        let first = text(&buf, 0);
        assert!(first.contains(" · working 2m"), "{first}");
        assert!(
            first.contains("~/switchboard · note/bug-tab-header"),
            "{first}"
        );
        assert!(first.contains("…"), "{first}");
        assert!(first.ends_with("M-a next: 3 billing"), "{first}");
        // How long it has been open and how full its context is give way
        // too, where they would push M-a's next off.
        let h = Header {
            name: "Fix tab header and sidebar".into(),
            branch: "main".into(),
            opened: "1h12m".into(),
            context: Some(62),
            prs: None,
            next: Some((3, "billing".into())),
            ..header(Shown::Claude)
        };
        let mut buf = Buffer::empty(area);
        draw(&mut buf, area, &h, None, &Palette::DARK, false, false);
        let first = text(&buf, 0);
        assert!(first.contains(" · working 2m"), "{first}");
        assert!(!first.contains("ctx 62%"), "{first}");
        assert!(first.ends_with("M-a next: 3 billing"), "{first}");
        // A long name is cut to keep the path, the state and M-a's next.
        let h = Header {
            name: "Make the header keep its state and the next tab in view always".into(),
            next: Some((3, "billing".into())),
            ..header(Shown::Claude)
        };
        let mut buf = Buffer::empty(area);
        draw(&mut buf, area, &h, None, &Palette::DARK, false, false);
        let first = text(&buf, 0);
        assert!(first.contains("…   ~/switchboard · working 2m"), "{first}");
        assert!(first.ends_with("M-a next: 3 billing"), "{first}");
        // So is a long path, after the name has its least.
        let h = Header {
            path: "~/code/customer-platform/services/notifications-service/backend".into(),
            ..h
        };
        let mut buf = Buffer::empty(area);
        draw(&mut buf, area, &h, None, &Palette::DARK, false, false);
        let first = text(&buf, 0);
        assert!(first.contains("… · working 2m"), "{first}");
        // M-a's next stays while the cut path leaves it room.
        let h = Header {
            name: "planning".into(),
            ..h
        };
        let mut buf = Buffer::empty(area);
        draw(&mut buf, area, &h, None, &Palette::DARK, false, false);
        let first = text(&buf, 0);
        assert!(first.contains("… · working 2m"), "{first}");
        assert!(first.ends_with("M-a next: 3 billing"), "{first}");
        // A long next tab's name is cut rather than dropped.
        let h = Header {
            path: "~/switchboard".into(),
            next: Some((
                3,
                "BUG: Tab header and sidebar crowd state and miscount notifications".into(),
            )),
            ..h
        };
        let mut buf = Buffer::empty(area);
        draw(&mut buf, area, &h, None, &Palette::DARK, false, false);
        let first = text(&buf, 0);
        assert!(first.contains("~/switchboard · working 2m"), "{first}");
        assert!(first.contains("M-a next: 3 BUG: Tab"), "{first}");
        assert!(first.ends_with("…"), "{first}");
    }

    #[test]
    fn lights_the_key_under_the_mouse_and_clicks_do_what_it_does() {
        let area = Rect::new(0, 0, 90, ROWS);
        let mut buf = Buffer::empty(area);
        let keys = draw(
            &mut buf,
            area,
            &header(Shown::Claude),
            Some(Button::Show(Shown::Diff)),
            &Palette::DARK,
            false,
            false,
        );
        let (diff, _) = keys
            .iter()
            .find(|(_, s)| *s == Button::Show(Shown::Diff))
            .unwrap();
        // The key itself, after the keycap's opening end.
        assert!(
            buf[(diff.x + 1, diff.y)]
                .modifier
                .contains(Modifier::REVERSED)
        );
        let (pr, _) = keys
            .iter()
            .find(|(_, s)| *s == Button::Show(Shown::Pr))
            .unwrap();
        assert!(!buf[(pr.x + 1, pr.y)].modifier.contains(Modifier::REVERSED));
        assert_eq!(Shown::Claude.action(), Action::FocusClaude);
        assert_eq!(
            Shown::Note.action(),
            Action::Notes(crate::notes::serve::Command::Beside)
        );
    }

    #[test]
    fn a_terminal_that_draws_half_circles_gets_them_at_each_keys_ends() {
        let area = Rect::new(0, 0, 90, ROWS);
        let mut buf = Buffer::empty(area);
        let keys = draw(
            &mut buf,
            area,
            &header(Shown::Claude),
            None,
            &Palette::DARK,
            true,
            false,
        );
        let (diff, _) = keys
            .iter()
            .find(|(_, s)| *s == Button::Show(Shown::Diff))
            .unwrap();
        assert_eq!(buf[(diff.x, diff.y)].symbol(), kit::CAP_LEFT);
        assert_eq!(buf[(diff.x + 4, diff.y)].symbol(), kit::CAP_RIGHT);
        let mut plain = Buffer::empty(area);
        let same = draw(
            &mut plain,
            area,
            &header(Shown::Claude),
            None,
            &Palette::DARK,
            false,
            false,
        );
        assert_eq!(keys, same, "the keys stay where they were");
    }

    #[test]
    fn highlights_what_the_tab_shows() {
        let area = Rect::new(0, 0, 90, ROWS);
        let mut buf = Buffer::empty(area);
        let keys = draw(
            &mut buf,
            area,
            &header(Shown::Pr),
            None,
            &Palette::DARK,
            false,
            false,
        );
        let (pr, _) = keys
            .iter()
            .find(|(_, s)| *s == Button::Show(Shown::Pr))
            .unwrap();
        assert_eq!(buf[(pr.x, pr.y)].bg, compose::colors::ACCENT);
        let (claude, _) = keys
            .iter()
            .find(|(_, s)| *s == Button::Show(Shown::Claude))
            .unwrap();
        assert_ne!(buf[(claude.x, claude.y)].bg, compose::colors::ACCENT);
    }

    #[test]
    fn fills_the_first_line_with_the_tabs_color_its_words_dark() {
        let area = Rect::new(0, 0, 90, ROWS);
        let mut buf = Buffer::empty(area);
        let color = Color::Rgb(120, 200, 160);
        let h = Header {
            color: Some(color),
            ..header(Shown::Claude)
        };
        draw(&mut buf, area, &h, None, &Palette::DARK, false, false);
        for x in [0, 1, 3, 30, 89] {
            assert_eq!(buf[(x, 0)].bg, color, "column {x}");
        }
        let name = buf[(3, 0)].clone();
        assert_eq!(name.symbol(), "p");
        assert_eq!(name.fg, ON_COLOR);
        assert!(name.modifier.contains(Modifier::BOLD));
        assert_eq!(buf[(1, 0)].fg, ON_COLOR, "the mark");
        assert_eq!(buf[(15, 0)].fg, ON_COLOR, "where it works");
        assert!(!buf[(15, 0)].modifier.contains(Modifier::BOLD));
        assert!(!text(&buf, 0).contains("ctx"));
    }

    #[test]
    fn words_are_light_on_a_dark_vivid_color() {
        let area = Rect::new(0, 0, 90, ROWS);
        // A soft color takes the dark words, a dark vivid one light ones.
        for (color, words) in [
            (Color::Rgb(0xa2, 0xb0, 0xeb), ON_COLOR),
            (Color::Rgb(0x6f, 0x3f, 0xff), Color::Indexed(255)),
        ] {
            // Whatever the theme.
            for palette in [Palette::DARK, Palette::LIGHT] {
                let mut buf = Buffer::empty(area);
                let h = Header {
                    color: Some(color),
                    ..header(Shown::Claude)
                };
                draw(&mut buf, area, &h, None, &palette, false, false);
                assert_eq!(buf[(3, 0)].fg, words, "{color:?}");
                assert_eq!(buf[(15, 0)].fg, words, "{color:?}");
            }
        }
    }

    #[test]
    fn the_commands_button_ends_the_keys_row_and_lights_while_the_palette_is_open() {
        let area = Rect::new(0, 0, 90, ROWS);
        let h = Header {
            commands: true,
            ..header(Shown::Claude)
        };
        let mut buf = Buffer::empty(area);
        let keys = draw(&mut buf, area, &h, None, &Palette::DARK, false, false);
        assert!(text(&buf, 2).ends_with(" ⌘  Commands"), "{}", text(&buf, 2));
        let (button, _) = *keys.last().unwrap();
        assert_eq!(keys.last().unwrap().1, Button::Commands);
        assert_eq!(button.right(), 89, "a column in from the edge");
        assert_eq!(Button::Commands.action(), Action::Palette);
        assert_ne!(buf[(button.x + 1, 2)].bg, compose::colors::ACCENT);
        let mut open = Buffer::empty(area);
        draw(&mut open, area, &h, None, &Palette::DARK, false, true);
        assert_eq!(open[(button.x + 1, 2)].bg, compose::colors::ACCENT);
        let mut off = Buffer::empty(area);
        let keys = draw(
            &mut off,
            area,
            &Header {
                commands: false,
                ..h
            },
            None,
            &Palette::DARK,
            false,
            false,
        );
        assert!(!text(&off, 2).contains("Commands"));
        assert!(!keys.iter().any(|(_, b)| *b == Button::Commands));
    }

    #[test]
    fn the_commands_button_gives_way_where_the_header_is_too_narrow() {
        let h = Header {
            commands: true,
            ..header(Shown::Claude)
        };
        let area = Rect::new(0, 0, 66, ROWS);
        let mut buf = Buffer::empty(area);
        let keys = draw(&mut buf, area, &h, None, &Palette::DARK, false, false);
        assert!(!text(&buf, 2).contains("Commands"));
        assert_eq!(keys.len(), Shown::ALL.len());
        assert!(text(&buf, 2).ends_with("M-e  Note"));
    }

    #[test]
    fn reads_what_a_pane_shows_from_its_options() {
        let of = |pairs: &[(&str, &str)]| {
            let pairs: Vec<(String, String)> = pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            Shown::of_pane(|k| pairs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()))
        };
        assert_eq!(of(&[]), Shown::Claude);
        assert_eq!(of(&[("@in_place", "diff")]), Shown::Diff);
        assert_eq!(of(&[("@in_place", "pr-view")]), Shown::Pr);
        assert_eq!(of(&[("@ticket_view", "1")]), Shown::Ticket);
        assert_eq!(of(&[("@notes_pane", "1")]), Shown::Note);
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;

    #[test]
    fn the_history_key_follows_the_views_and_says_the_moment_shown() {
        let area = Rect::new(0, 0, 120, ROWS);
        let mut buf = Buffer::empty(area);
        let h = Header {
            number: 0,
            name: "planning".into(),
            path: "~/switchboard".into(),
            branch: String::new(),
            state: None,
            age: String::new(),
            opened: String::new(),
            color: None,
            shown: Shown::Claude,
            next: Some((3, "billing".into())),
            commands: false,
            history: true,
            history_open: true,
            viewing: Some("viewing 1:41 PM, 51 minutes ago".into()),
            context: None,
            prs: None,
            nav: None,
        };
        let keys = draw(&mut buf, area, &h, None, &Palette::DARK, false, false);
        let row: String = (0..120).map(|x| buf[(x, 2)].symbol().to_string()).collect();
        assert!(row.contains("M-e  Note   M-h History"), "{row}");
        assert_eq!(keys.last().unwrap().1, Button::History);
        assert_eq!(Button::History.action(), Action::History);
        let first: String = (0..120).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert!(first.contains("viewing 1:41 PM, 51 minutes ago"), "{first}");
        assert!(!first.contains("M-a next"), "in its place");
    }
}

#[cfg(test)]
mod viewing_tests {
    use super::*;

    #[test]
    fn the_moment_shown_keeps_its_room_whatever_the_facts() {
        let area = Rect::new(0, 0, 70, ROWS);
        let mut buf = Buffer::empty(area);
        let h = Header {
            number: 0,
            name: "a long tab name".into(),
            path: "~/code/a/very/long/path/to/somewhere".into(),
            branch: "note/a-long-branch-name".into(),
            state: None,
            age: String::new(),
            opened: String::new(),
            color: None,
            shown: Shown::Claude,
            next: None,
            commands: false,
            history: true,
            history_open: true,
            viewing: Some("viewing 1:41 PM, 51 minutes ago".into()),
            context: None,
            prs: None,
            nav: None,
        };
        draw(&mut buf, area, &h, None, &Palette::DARK, false, false);
        let first: String = (0..70).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert!(
            first
                .trim_end()
                .ends_with("viewing 1:41 PM, 51 minutes ago"),
            "{first}"
        );
    }
}
