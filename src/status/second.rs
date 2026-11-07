//! The second status line (tmux/tab-status.sh, tmux.conf's
//! `status-format[1]`), under the tabs while the `second_status` setting is
//! on: the current tab's label whole, where the tab may cut it short, its
//! PR, the program in front of its pane, and in a Claude pane what the
//! session is doing, its name and your last prompt.
//!
//! tab-status.sh ran on every redraw and kept its Claude summary in a cache,
//! since working it out took seven processes. Here everything comes from
//! what the server already holds: the tab's name and PR (src/tabs), the
//! pane's state (src/agent), and its Claude session and last turn as the
//! sidebar read them from Claude Code's files on the tick (src/sidebar), so
//! drawing it starts nothing and reads nothing.
//!
//! tab-status.sh also said whether the PR had every commit made here
//! ("↑2 to push", "pushed"), from `git rev-list --count`. Counting commits
//! means walking git's object store, which isn't done without git, so that
//! part is left out.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use super::theme::{GREEN, Palette, YELLOW};
use crate::agent::State;
use crate::mux::compose::put;

/// The longest prompt shown before it is cut with "...".
const PROMPT_MAX: usize = 60;

/// What the line says, as the server knows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Second {
    /// The tab's label as tab-label.sh gave it: without the PR in front,
    /// which comes after it here.
    pub label: String,
    /// Whether the tab has a name of its own, drawn bold.
    pub own_name: bool,
    pub pr: Option<u64>,
    /// The program in front of the pane; `claude` for a Claude pane.
    pub command: Option<String>,
    /// The tab's color, the program drawn in it.
    pub color: Option<Color>,
    /// The Claude pane's state and since when (seconds).
    pub state: Option<(State, i64)>,
    /// The name the session was given (not one derived from its first
    /// prompt, which goes stale as the session moves on).
    pub session_name: Option<String>,
    /// Your last prompt to it.
    pub prompt: Option<String>,
}

/// How long ago, as tab-status.sh put it: `<1m`, `4m`, `2h`.
pub fn took(secs: i64) -> String {
    if secs < 60 {
        "<1m".into()
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

/// The session's name and last prompt as one: `name: "prompt"`, either
/// alone, or nothing. A prompt under three words ("yes", "do it") says
/// nothing on its own and is left out; a long one is cut at `PROMPT_MAX`.
pub fn summary(name: Option<&str>, prompt: Option<&str>) -> String {
    let prompt = prompt
        .map(|p| {
            p.chars()
                .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .filter(|p| p.split_whitespace().count() >= 3)
        .map(|p| {
            if p.chars().count() > PROMPT_MAX {
                let cut: String = p.chars().take(PROMPT_MAX - 3).collect();
                format!("\"{cut}...\"")
            } else {
                format!("\"{p}\"")
            }
        });
    match (name.filter(|n| !n.is_empty()), prompt) {
        (Some(n), Some(p)) => format!("{n}: {p}"),
        (Some(n), None) => n.to_string(),
        (None, Some(p)) => p,
        (None, None) => String::new(),
    }
}

/// The line's pieces and their styles, with `now` in seconds for how long
/// a state has lasted.
pub fn pieces(s: &Second, p: &Palette, now: i64) -> Vec<(String, Style)> {
    let mut out = vec![("  ".to_string(), Style::new())];
    let mut label = Style::new().fg(p.bright);
    if s.own_name {
        label = label.add_modifier(Modifier::BOLD);
    }
    out.push((s.label.clone(), label));
    if let Some(pr) = s.pr {
        out.push(("   ".into(), Style::new()));
        out.push((format!("#{pr}"), Style::new().fg(p.status_fg)));
    }
    if let Some(command) = s.command.as_deref().filter(|c| !c.is_empty()) {
        out.push(("  ".into(), Style::new()));
        out.push((
            command.to_string(),
            Style::new().fg(s.color.unwrap_or(p.status_fg)),
        ));
    }
    if let Some((state, since)) = s.state {
        let ago = (since > 0).then(|| took(now - since));
        let (words, color) = match state {
            State::Waiting => (
                match &ago {
                    Some(t) => format!("waiting on you for {t}"),
                    None => "waiting on you".into(),
                },
                YELLOW,
            ),
            State::Done => (
                match &ago {
                    Some(t) => format!("finished {t} ago"),
                    None => "finished".into(),
                },
                GREEN,
            ),
            State::Working => ("working".into(), p.dim),
        };
        out.push((" ".into(), Style::new()));
        out.push((words, Style::new().fg(color)));
    }
    let summary = summary(s.session_name.as_deref(), s.prompt.as_deref());
    if !summary.is_empty() {
        out.push(("   ".into(), Style::new()));
        out.push((summary, Style::new().fg(p.dim)));
    }
    out
}

/// Draws the line in `area` (one row) on the status line's background.
pub fn draw(buf: &mut Buffer, area: Rect, pieces: &[(String, Style)], p: &Palette) {
    let base = Style::new().bg(p.status_bg).fg(p.status_fg);
    buf.set_style(area, base);
    for x in area.left()..area.right() {
        buf[(x, area.y)].set_symbol(" ");
    }
    let mut x = area.x;
    for (text, style) in pieces {
        x = put(buf, x, area, text, base.patch(*style));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(buf: &Buffer) -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, 0)].symbol().to_string())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn a_prompt_says_something_or_is_left_out() {
        assert_eq!(summary(None, Some("yes")), "");
        assert_eq!(summary(None, Some("do it")), "");
        assert_eq!(
            summary(None, Some("fix the\nfailing test  ")),
            "\"fix the failing test\""
        );
        assert_eq!(
            summary(Some("Docs"), Some("add the status line")),
            "Docs: \"add the status line\""
        );
        assert_eq!(summary(Some("Docs"), Some("ok")), "Docs");
        let long = "word ".repeat(30);
        let cut = summary(None, Some(&long));
        assert_eq!(cut.chars().count(), PROMPT_MAX + 2);
        assert!(cut.ends_with("...\""), "{cut}");
    }

    #[test]
    fn how_long_reads_as_tab_status_put_it() {
        assert_eq!(took(5), "<1m");
        assert_eq!(took(240), "4m");
        assert_eq!(took(7300), "2h");
    }

    #[test]
    fn the_line_has_the_label_pr_program_state_and_summary() {
        let p = &Palette::DARK;
        let green = Color::Rgb(0x23, 0xef, 0x59);
        let s = Second {
            label: "ENG-1: Fix it".into(),
            own_name: false,
            pr: Some(12),
            command: Some("claude".into()),
            color: Some(green),
            state: Some((State::Waiting, 1000)),
            session_name: None,
            prompt: Some("make the tests pass".into()),
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 120, 1));
        let pieces = pieces(&s, p, 1240);
        let area = buf.area;
        draw(&mut buf, area, &pieces, p);
        assert_eq!(
            row(&buf),
            "  ENG-1: Fix it   #12  claude waiting on you for 4m   \"make the tests pass\""
        );
        let at = |text: &str| row(&buf).find(text).unwrap() as u16;
        assert_eq!(buf[(at("claude"), 0)].fg, green);
        assert_eq!(buf[(at("waiting"), 0)].fg, YELLOW);
        assert_eq!(buf[(at("\"make"), 0)].fg, p.dim);
        assert_eq!(buf[(2, 0)].fg, p.bright);
        assert_eq!(buf[(2, 0)].bg, p.status_bg);
        assert!(!buf[(2, 0)].modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn a_tab_named_by_you_is_bold_and_a_shell_has_no_state() {
        let p = &Palette::LIGHT;
        let s = Second {
            label: "Shaping: things".into(),
            own_name: true,
            command: Some("zsh".into()),
            ..Second::default()
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 1));
        let area = buf.area;
        draw(&mut buf, area, &pieces(&s, p, 0), p);
        assert_eq!(row(&buf), "  Shaping: things  zsh");
        assert!(buf[(2, 0)].modifier.contains(Modifier::BOLD));
        assert_eq!(buf[(2, 0)].bg, Palette::LIGHT.status_bg);
    }
}
