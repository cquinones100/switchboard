//! UX 2.0's tab header: the rows at the top of a tab's area, above its
//! panes, in place of the status line's tab at the bottom. New with UX 2.0,
//! with no tmux-version file behind it, since tmux drew its status line
//! and nothing over a window's panes.
//!
//! Its first row is filled with the tab's own color (the one its number
//! has in the sidebar, src/tabs/color.rs), and on it, dark, the state's
//! mark, the tab's name in bold, where it works and what Claude is doing;
//! while the tab's history shows a past moment, which ("viewing 13:41, 51
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
//! It is drawn with the kit's atoms (src/ui/kit.rs): the state's mark, a
//! keycap for each key (the half circles at its ends where the client's
//! terminal draws them) and the rule.
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
    Show(Shown),
    /// M-h, the tab's history (src/history).
    History,
    Commands,
}

impl Button {
    /// What a click on it does: the key's action, or the palette's (a
    /// click while the palette is open lands outside it, which closes it).
    pub fn action(self) -> Action {
        match self {
            Button::Show(shown) => shown.action(),
            Button::History => Action::History,
            Button::Commands => Action::Palette,
        }
    }
}

/// What the header says about a tab.
#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    pub name: String,
    /// Where the tab works, as it is shown (`~/code/app`).
    pub path: String,
    /// The branch it is on, as tab naming last worked it out; empty when
    /// not known.
    pub branch: String,
    pub state: Option<State>,
    /// How long it has been in that state (`4m`), empty when not known.
    pub age: String,
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
    /// The moment the history shows, as "viewing 13:41, 51 minutes ago".
    pub viewing: Option<String>,
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
/// terminal draws the kit's half circles (`kit::caps`). `palette`: the
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
    x = compose::put(buf, x, first, &mark.content, mark_style);
    x = compose::put(
        buf,
        x + 1,
        first,
        &h.name,
        band.fg(name_fg).add_modifier(Modifier::BOLD),
    );
    let mut facts = vec![h.path.clone()];
    if !h.branch.is_empty() {
        facts.push(h.branch.clone());
    }
    if let Some(s) = h.state {
        // "waiting 4m": how long a tab has waited is what tells you where
        // M-a should take you first.
        facts.push(if h.age.is_empty() {
            s.name().to_string()
        } else {
            format!("{} {}", s.name(), h.age)
        });
    }
    // The moment a tab's history shows, at the right end in place of where
    // M-a goes next, its room kept: the facts give way to it.
    let viewing_at = h.viewing.as_ref().map(|v| {
        let width = crate::sidebar::text::width(v) as u16 + 1;
        area.right().saturating_sub(width).max(area.x)
    });
    let facts_room = Rect {
        width: viewing_at.map_or(first.width, |at| {
            at.saturating_sub(2).saturating_sub(first.x)
        }),
        ..first
    };
    let end = compose::put(
        buf,
        x + 3,
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
    } else if let Some((number, name)) = &h.next {
        let next = format!("M-a next: {number} {name} ");
        let width = crate::sidebar::text::width(&next) as u16;
        let at = area.right().saturating_sub(width);
        if at > end + 1 {
            compose::put(buf, at, first, &next, band.fg(words_fg));
        }
    }
    if area.height < 3 {
        return keys;
    }
    let row = Rect::new(area.x, area.y + 2, area.width, 1);
    let mut x = area.x + 1;
    for shown in Shown::ALL {
        let mut cap = kit::keycap(shown.key(), shown.label(), shown == h.shown, caps, &theme);
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
            name: "planning".into(),
            path: "~/switchboard".into(),
            branch: String::new(),
            state: Some(State::Working),
            age: "2m".into(),
            color: None,
            shown,
            next: None,
            commands: false,
            history: false,
            history_open: false,
            viewing: None,
        }
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
            name: "planning".into(),
            path: "~/switchboard".into(),
            branch: String::new(),
            state: None,
            age: String::new(),
            color: None,
            shown: Shown::Claude,
            next: Some((3, "billing".into())),
            commands: false,
            history: true,
            history_open: true,
            viewing: Some("viewing 13:41, 51 minutes ago".into()),
        };
        let keys = draw(&mut buf, area, &h, None, &Palette::DARK, false, false);
        let row: String = (0..120).map(|x| buf[(x, 2)].symbol().to_string()).collect();
        assert!(row.contains("M-e  Note   M-h History"), "{row}");
        assert_eq!(keys.last().unwrap().1, Button::History);
        assert_eq!(Button::History.action(), Action::History);
        let first: String = (0..120).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert!(first.contains("viewing 13:41, 51 minutes ago"), "{first}");
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
            name: "a long tab name".into(),
            path: "~/code/a/very/long/path/to/somewhere".into(),
            branch: "note/a-long-branch-name".into(),
            state: None,
            age: String::new(),
            color: None,
            shown: Shown::Claude,
            next: None,
            commands: false,
            history: true,
            history_open: true,
            viewing: Some("viewing 13:41, 51 minutes ago".into()),
        };
        draw(&mut buf, area, &h, None, &Palette::DARK, false, false);
        let first: String = (0..70).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert!(
            first.trim_end().ends_with("viewing 13:41, 51 minutes ago"),
            "{first}"
        );
    }
}
