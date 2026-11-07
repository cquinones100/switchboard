//! The mode in mission control (render.js's line above the cards and
//! autopilot.js's `summary` and `choices`): while it is on, a line leading
//! the Tabs view says which note it works, how many of its tabs work and
//! are open against its slots and cap, the item it takes next, and whether
//! it is paused; the cursor's -1 is on it, and Enter there opens the menu
//! `choices` gives, which pauses or resumes it, skips the next item or
//! takes the skipped ones again, narrows it to items with a settled plan or
//! widens it again, sets it up again, or stops it. Its notification's menu
//! (`notice_choices`) goes to that line or puts the notification away.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use super::mode::Mode;
use crate::sidebar::render::Line;
use crate::sidebar::text::{cut, width};
use crate::ui::Theme;

/// What the line shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    /// The note's title.
    pub note: String,
    /// Its tabs working, and open.
    pub busy: usize,
    pub open: usize,
    pub slots: u32,
    pub cap: u32,
    /// The item it takes next, by its first line; empty for none.
    pub next: String,
    pub paused: bool,
    pub away: bool,
    pub ready_only: bool,
    /// How many items are skipped.
    pub skipped: usize,
    pub spend_limit: f64,
}

impl Summary {
    /// What follows the note's title on the line.
    pub fn facts(&self) -> String {
        let how = if self.paused {
            "paused".to_string()
        } else if self.away {
            "paused while you're away".to_string()
        } else {
            format!(
                "{} of {} working · {} of {} open",
                self.busy, self.slots, self.open, self.cap
            )
        };
        let mut facts = vec![
            how,
            if self.next.is_empty() {
                "nothing left to take".to_string()
            } else {
                format!("next: {}", self.next)
            },
        ];
        if self.skipped > 0 {
            facts.push(format!("{} skipped", self.skipped));
        }
        if self.ready_only {
            facts.push("planned items only".into());
        }
        if self.spend_limit > 0.0 {
            facts.push(format!("stops at ${} today", self.spend_limit));
        }
        facts.join(" · ")
    }
}

/// The line, `cols` wide: on the highlight when the cursor is on it, in the
/// panel's shade when the mouse is.
pub fn line(s: &Summary, cols: usize, chosen: bool, lit: bool, theme: &Theme) -> Line {
    let fill = if chosen {
        Some(theme.highlight)
    } else if lit {
        Some(theme.panel)
    } else {
        None
    };
    let name = if chosen {
        Style::new().fg(theme.bright).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.text)
    };
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(
            "AUTO",
            Style::new()
                .fg(Color::Indexed(81))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ];
    let head = 6;
    if cols <= head {
        return Line {
            spans: vec![Span::raw(" ".repeat(cols))],
            fill,
        };
    }
    let note = cut(&s.note, cols - head);
    let mut used = head + width(&note);
    spans.push(Span::styled(note, name));
    if cols > used + 3 {
        let facts = cut(&s.facts(), cols - used - 2);
        used += 2 + width(&facts);
        spans.push(Span::raw("  "));
        spans.push(Span::styled(facts, Style::new().fg(theme.dim)));
    }
    if used < cols {
        spans.push(Span::raw(" ".repeat(cols - used)));
    }
    Line { spans, fill }
}

/// What an option of its menu, or of its notification's, does.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Pause,
    Resume,
    /// The next item skipped, by its first line.
    Skip(String),
    /// The skipped items taken again.
    Unskip,
    /// Only items with a settled plan, or those with none too.
    PlannedOnly(bool),
    /// The note, slots, cap and spend limit asked for again.
    Change,
    Stop,
    /// Mission control's Tabs, where its line is.
    ToTabs,
    /// Its notification put away.
    Dismiss,
}

/// The menu Enter on its line opens, given the item it takes next: each
/// option's key, label and what it does.
pub fn choices(mode: &Mode, next: &str) -> Vec<(char, String, Act)> {
    let mut out = vec![if mode.paused {
        ('r', "Resume".to_string(), Act::Resume)
    } else {
        (
            'p',
            "Pause: open no more tabs for now".to_string(),
            Act::Pause,
        )
    }];
    if !next.is_empty() {
        out.push((
            's',
            format!("Skip the next item: {next}"),
            Act::Skip(next.to_string()),
        ));
    }
    let n = mode.skipped.len();
    if n > 0 {
        out.push((
            'c',
            format!(
                "Take the {n} skipped item{} again",
                if n == 1 { "" } else { "s" }
            ),
            Act::Unskip,
        ));
    }
    out.push(if mode.ready_only {
        (
            'a',
            "Take items without a plan too".to_string(),
            Act::PlannedOnly(false),
        )
    } else {
        (
            'a',
            "Take only items with a settled plan".to_string(),
            Act::PlannedOnly(true),
        )
    });
    out.push((
        'n',
        "Change its note, slots, cap or spend limit…".to_string(),
        Act::Change,
    ));
    out.push(('x', "Stop working the note".to_string(), Act::Stop));
    out
}

/// What one of the menu's options changes in the mode, and what it says it
/// did; `None` for those that do more than change the mode (`Change`,
/// `ToTabs`, `Dismiss`).
pub fn apply(act: &Act, mode: &mut Mode) -> Option<String> {
    Some(match act {
        Act::Pause => {
            mode.paused = true;
            "Paused".to_string()
        }
        Act::Resume => {
            mode.paused = false;
            "Resumed".to_string()
        }
        Act::Skip(text) => {
            if !mode.skipped.contains(text) {
                mode.skipped.push(text.clone());
            }
            format!("Skipped: {text}")
        }
        Act::Unskip => {
            mode.skipped.clear();
            "The skipped items are back in line".to_string()
        }
        Act::PlannedOnly(true) => {
            mode.ready_only = true;
            "Taking only items with a settled plan".to_string()
        }
        Act::PlannedOnly(false) => {
            mode.ready_only = false;
            "Taking items without a plan too, after the planned ones".to_string()
        }
        Act::Stop => {
            mode.on = false;
            mode.paused = false;
            mode.said.clear();
            "Stopped: no more tabs open for the note; those open carry on".to_string()
        }
        Act::Change | Act::ToTabs | Act::Dismiss => return None,
    })
}

/// Its notification's menu, in mission control's Notifications view.
pub fn notice_choices() -> Vec<(char, &'static str, Act)> {
    vec![
        ('o', "Open mission control's Tabs", Act::ToTabs),
        ('x', "Dismiss it", Act::Dismiss),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> Summary {
        Summary {
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
        }
    }

    #[test]
    fn the_line_names_the_note_its_tabs_against_slots_and_cap_and_the_next_item() {
        let s = summary();
        let l = line(&s, 120, true, false, &Theme::DARK);
        assert_eq!(width(&l.text()), 120);
        assert!(
            l.text().starts_with(
                " AUTO Work  2 of 3 working · 4 of 5 open · next: Next thing · 1 skipped"
            ),
            "{}",
            l.text()
        );
        assert_eq!(l.fill, Some(Theme::DARK.highlight));
        let paused = Summary {
            paused: true,
            next: String::new(),
            ..summary()
        };
        assert!(
            line(&paused, 120, false, false, &Theme::DARK)
                .text()
                .contains("Work  paused · nothing left to take")
        );
        for cols in [0, 4, 6, 9, 30] {
            assert_eq!(
                width(&line(&s, cols, false, true, &Theme::DARK).text()),
                cols
            );
        }
    }

    // test/tmux-sidebar.test.js's "k from the top row of tabs reaches the
    // line of the note worked continuously, and Enter there opens its menu,
    // whose Pause pauses it".
    #[test]
    fn its_menu_pauses_skips_narrows_changes_and_stops_it() {
        let mut mode = Mode {
            on: true,
            note: "Work.md".into(),
            ..Mode::default()
        };
        let labels = |mode: &Mode, next: &str| -> Vec<String> {
            choices(mode, next).into_iter().map(|(_, l, _)| l).collect()
        };
        assert_eq!(
            labels(&mode, ""),
            vec![
                "Pause: open no more tabs for now",
                "Take only items with a settled plan",
                "Change its note, slots, cap or spend limit…",
                "Stop working the note"
            ]
        );
        let (_, _, pause) = choices(&mode, "").remove(0);
        assert_eq!(apply(&pause, &mut mode).as_deref(), Some("Paused"));
        assert!(mode.paused);
        let (_, label, skip) = choices(&mode, "One").remove(1);
        assert_eq!(label, "Skip the next item: One");
        apply(&skip, &mut mode);
        assert_eq!(
            labels(&mode, "Two")[..3],
            [
                "Resume".to_string(),
                "Skip the next item: Two".to_string(),
                "Take the 1 skipped item again".to_string()
            ]
        );
        apply(&Act::Stop, &mut mode);
        assert!(!mode.on && !mode.paused);
    }
}
