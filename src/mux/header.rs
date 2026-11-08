//! UX 2.0's tab header: the rows at the top of a tab's area, above its
//! panes, in place of the status line's tab at the bottom. New with UX 2.0,
//! with no tmux-version file behind it, since tmux drew its status line
//! and nothing over a window's panes.
//!
//! Its first row names the tab in bold, in its color, after its state's
//! mark, and says dimmed where it works and what Claude is doing; under a
//! blank row, the keys of what the tab can show in Claude's place (M-c
//! Claude, M-d its diff, M-v its PR, M-i its ticket, M-e its note), the one
//! it shows now highlighted; and a rule under them. What it shows is read
//! from the tab's focused pane: a tool in Claude's place (`@in_place`), a
//! view marked as the PR's or the ticket's, or the notes pane.
//!
//! It is drawn from what the server already holds, as everything on the
//! drawing path is: no file is read and no process started for it.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::agent::State;
use crate::mux::compose;
use crate::mux::keys::Action;
use crate::status::theme::Palette;

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
}

/// A state's mark and color, as the sidebar's badges have them.
fn mark(state: Option<State>) -> (&'static str, Color) {
    match state {
        Some(State::Waiting) => ("●", Color::Indexed(221)),
        Some(State::Done) => ("✓", Color::Indexed(114)),
        Some(State::Working) => ("⠹", Color::Indexed(111)),
        None => ("○", Color::Indexed(244)),
    }
}

/// Where each key went, for a click on it.
pub type Keys = Vec<(Rect, Shown)>;

/// Draws the header into `area` (its full width, `ROWS` high), and gives
/// where each key went.
/// The key under the mouse, with hover_highlight on, is lit.
pub fn draw(buf: &mut Buffer, area: Rect, h: &Header, hover: Option<Shown>, p: &Palette) -> Keys {
    let mut keys = Keys::new();
    if area.height == 0 || area.width == 0 {
        return keys;
    }
    let band = Style::new().bg(p.status_bg);
    let first = Rect::new(area.x, area.y, area.width, 1);
    buf.set_style(first, band);
    let (glyph, state_color) = mark(h.state);
    let mut x = area.x + 1;
    x = compose::put(buf, x, first, glyph, band.fg(state_color));
    x = compose::put(
        buf,
        x + 1,
        first,
        &h.name,
        band.fg(h.color.unwrap_or(p.bright))
            .add_modifier(Modifier::BOLD),
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
    let end = compose::put(buf, x + 3, first, &facts.join(" · "), band.fg(p.dim));
    if let Some((number, name)) = &h.next {
        let next = format!("M-a next: {number} {name} ");
        let width = crate::sidebar::text::width(&next) as u16;
        let at = area.right().saturating_sub(width);
        if at > end + 1 {
            compose::put(buf, at, first, &next, band.fg(p.dim));
        }
    }
    if area.height < 3 {
        return keys;
    }
    let row = Rect::new(area.x, area.y + 2, area.width, 1);
    let mut x = area.x + 1;
    for shown in Shown::ALL {
        let on = shown == h.shown;
        let key_style = if on {
            Style::new()
                .bg(compose::colors::ACCENT)
                .fg(Color::Indexed(232))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().bg(p.current_bg).fg(compose::colors::ACCENT)
        };
        let label_style = if on {
            Style::new()
                .bg(p.current_bg)
                .fg(p.bright)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(p.dim)
        };
        let key_style = if hover == Some(shown) {
            key_style.add_modifier(Modifier::REVERSED)
        } else {
            key_style
        };
        let start = x;
        x = compose::put(buf, x, row, &format!(" {} ", shown.key()), key_style);
        x = compose::put(buf, x, row, &format!(" {} ", shown.label()), label_style);
        if x > start {
            keys.push((Rect::new(start, row.y, x - start, 1), shown));
        }
        x += 1;
    }
    if area.height >= 4 {
        let rule = Rect::new(area.x, area.y + 3, area.width, 1);
        compose::put(
            buf,
            area.x,
            rule,
            &"─".repeat(area.width as usize),
            Style::new().fg(p.border),
        );
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

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
        }
    }

    #[test]
    fn names_the_tab_and_lists_the_keys() {
        let area = Rect::new(0, 0, 90, ROWS);
        let mut buf = Buffer::empty(area);
        let keys = draw(&mut buf, area, &header(Shown::Claude), None, &Palette::DARK);
        assert_eq!(text(&buf, 0), " ⠹ planning   ~/switchboard · working 2m");
        assert_eq!(text(&buf, 1), "");
        assert_eq!(
            text(&buf, 2),
            "  M-c  Claude   M-d  Diff   M-v  PR   M-i  Ticket   M-e  Note"
        );
        assert!(text(&buf, 3).starts_with("───"));
        let shown: Vec<Shown> = keys.iter().map(|(_, s)| *s).collect();
        assert_eq!(shown, Shown::ALL.to_vec());
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
        draw(&mut buf, area, &h, None, &Palette::DARK);
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
            Some(Shown::Diff),
            &Palette::DARK,
        );
        let (diff, _) = keys.iter().find(|(_, s)| *s == Shown::Diff).unwrap();
        assert!(buf[(diff.x, diff.y)].modifier.contains(Modifier::REVERSED));
        let (pr, _) = keys.iter().find(|(_, s)| *s == Shown::Pr).unwrap();
        assert!(!buf[(pr.x, pr.y)].modifier.contains(Modifier::REVERSED));
        assert_eq!(Shown::Claude.action(), Action::FocusClaude);
        assert_eq!(
            Shown::Note.action(),
            Action::Notes(crate::notes::serve::Command::Beside)
        );
    }

    #[test]
    fn highlights_what_the_tab_shows() {
        let area = Rect::new(0, 0, 90, ROWS);
        let mut buf = Buffer::empty(area);
        let keys = draw(&mut buf, area, &header(Shown::Pr), None, &Palette::DARK);
        let (pr, _) = keys.iter().find(|(_, s)| *s == Shown::Pr).unwrap();
        assert_eq!(buf[(pr.x, pr.y)].bg, compose::colors::ACCENT);
        let (claude, _) = keys.iter().find(|(_, s)| *s == Shown::Claude).unwrap();
        assert_ne!(buf[(claude.x, claude.y)].bg, compose::colors::ACCENT);
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
