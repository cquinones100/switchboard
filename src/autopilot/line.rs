//! The mode in mission control (render.js's line above the cards and
//! autopilot.js's `summary` and `choices`): while it is on, a line leading
//! the Tabs view says which note it works, how many of its tabs work and
//! are open against its slots and cap, the item it takes next, and whether
//! it is paused; the cursor's -1 is on it, and Enter there opens the menu
//! `choices` gives, which pauses or resumes it, skips the next item or
//! takes the skipped ones again, narrows it to items with a settled plan or
//! widens it again, cycles when its landed tabs close (the autopilot_close
//! setting, as Settings changes it), lands without asking or asks again,
//! turns hands-free on or off, sets it up again, or stops it. Its notification's menu
//! (`notice_choices`) goes to that line or puts the notification away.
//! While the mode has its own view (`has_view`, UX 2.0), the line is not
//! drawn: the view's heading says what it said and opens the same menu.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use super::mode::{Mode, Skipped};
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
    /// How many of the note's items are skipped.
    pub skipped: usize,
    pub spend_limit: f64,
    /// Today's Claude spend, as the Usage view sums it, read with the next
    /// items while there is a limit; 0 otherwise.
    pub spent: f64,
    /// Its tabs land without asking.
    pub lands_alone: bool,
    /// Its tabs take the recommended option of what they would ask.
    pub hands_free: bool,
    /// The branch its tabs land on; empty for the default branch.
    pub target: String,
    /// The note had nothing more to start at the last look, while its tabs
    /// still work.
    pub drained: bool,
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
        let mut facts = Vec::new();
        if !self.target.is_empty() {
            facts.push(format!("→ {}", self.target));
        }
        facts.extend([
            how,
            if self.next.is_empty() {
                "nothing left to take".to_string()
            } else {
                format!("next: {}", self.next)
            },
        ]);
        if self.skipped > 0 {
            facts.push(format!("{} skipped", self.skipped));
        }
        if self.ready_only {
            facts.push("planned items only".into());
        }
        if self.spend_limit > 0.0 {
            facts.push(format!("stops at ${} today", self.spend_limit));
        }
        if self.hands_free {
            facts.push("hands-free".into());
        } else if self.lands_alone {
            facts.push("lands on its own".into());
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
    /// The next item skipped, by its first line, in the mode's note alone.
    Skip(String),
    /// The note's skipped items taken again: the one named, or all of them.
    Unskip(Option<String>),
    /// Only items with a settled plan, or those with none too.
    PlannedOnly(bool),
    /// The note, slots, cap and spend limit asked for again.
    Change,
    /// "Land on:" asked again.
    ChangeTarget,
    Stop,
    /// Mission control's Tabs, where its line is.
    ToTabs,
    /// Its notification put away.
    Dismiss,
    /// The autopilot_close setting to its next choice.
    CycleClose,
    /// Its tabs land on the default branch without asking to push, once
    /// the question `land_question` asks says yes.
    LandWithoutAsking,
    /// Its tabs ask before they land again.
    AskBeforeLanding,
    /// Its tabs take the recommended option of what they would ask, and
    /// land without asking, once the question `hands_free_question` asks
    /// says yes.
    HandsFree,
    /// Its tabs ask you again; landing without asking stays as it was.
    AskMe,
}

/// The question before landing without asking, naming what it skips and
/// what it keeps.
pub fn land_question() -> (String, Vec<String>) {
    (
        "Land its tabs on their target branch without asking to push?".into(),
        vec![
            "Skipped: the question before each push, and medium review findings, fixed without asking".into(),
            "Kept: the tests, two clean review rounds, the landing lock, leak-check; a repository whose landing runs no tests or no review still asks".into(),
            "Asked after all: anything leak-check finds, an unclear conflict, a failure the branch didn't cause".into(),
            "Told after each landing, in the notifications and on Slack".into(),
            "Only for landings straight on a branch; tabs open now too".into(),
        ],
    )
}

/// The question before going hands-free, naming what it decides and what
/// still stops.
pub fn hands_free_question() -> (String, Vec<String>) {
    (
        "Have its tabs decide for you, hands-free?".into(),
        vec![
            "Decided: a question box, a question that ends a turn, and an item whose plan needs a choice, each taking the recommended option".into(),
            "Written down: each decision, as a \"Decided hands-free\" line in the item's plan, and counted when it lands".into(),
            "Turned on with it: Land without asking, and off with it: Ask before landing".into(),
            "Still asked: what your rules or a skill say must be asked, permission prompts, and a question a tab ends on a fourth time".into(),
            "Only for its own tabs; tabs open now too".into(),
        ],
    )
}

/// The menu Enter on its line opens, given the item it takes next and the
/// autopilot_close setting's choice in words: each option's key, label and
/// what it does. `offer_hands_free` is the autopilot_handsfree setting.
pub fn choices(
    mode: &Mode,
    next: &str,
    close: &str,
    offer_hands_free: bool,
) -> Vec<(char, String, Act)> {
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
    let n = mode.skipped_here().count();
    if n > 0 {
        out.push((
            'c',
            format!(
                "Take the {n} skipped item{} again",
                if n == 1 { "" } else { "s" }
            ),
            Act::Unskip(None),
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
    out.push(('l', format!("Close landed tabs: {close}"), Act::CycleClose));
    out.push(if mode.land_without_asking {
        ('w', "Ask before landing".to_string(), Act::AskBeforeLanding)
    } else {
        (
            'w',
            "Land without asking".to_string(),
            Act::LandWithoutAsking,
        )
    });
    if mode.hands_free {
        out.push(('h', "Ask me again: hands-free off".to_string(), Act::AskMe));
    } else if offer_hands_free {
        out.push(('h', "Hands-free: decide for me".to_string(), Act::HandsFree));
    }
    out.push((
        'b',
        "Change the target branch…".to_string(),
        Act::ChangeTarget,
    ));
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
            let skipped = Skipped {
                note: mode.note_id.clone(),
                text: text.clone(),
            };
            if !mode.skipped.contains(&skipped) {
                mode.skipped.push(skipped);
            }
            format!("Skipped: {text}")
        }
        Act::Unskip(None) => {
            let here = mode.note_id.clone();
            mode.skipped.retain(|x| x.note != here);
            "The skipped items are back in line".to_string()
        }
        Act::Unskip(Some(text)) => {
            let here = mode.note_id.clone();
            mode.skipped.retain(|x| x.note != here || x.text != *text);
            format!("Back in line: {text}")
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
        Act::LandWithoutAsking => {
            mode.land_without_asking = true;
            "Its tabs land without asking; you're told after each".to_string()
        }
        // Hands-free lands without asking, so it goes with it.
        Act::AskBeforeLanding => {
            mode.land_without_asking = false;
            mode.hands_free = false;
            "Its tabs ask before they land again; those open now too".to_string()
        }
        Act::HandsFree => {
            mode.hands_free = true;
            mode.land_without_asking = true;
            "Its tabs decide for you and land without asking; each decision goes in the item's plan"
                .to_string()
        }
        Act::AskMe => {
            mode.hands_free = false;
            "Its tabs ask you again; those open now too".to_string()
        }
        Act::Change | Act::ChangeTarget | Act::ToTabs | Act::Dismiss | Act::CycleClose => {
            return None;
        }
    })
}

/// Its notification's menu, in mission control's Notifications view.
pub fn notice_choices() -> Vec<(char, &'static str, Act)> {
    vec![
        (
            'o',
            if super::has_view() {
                "Open mission control's Autopilot"
            } else {
                "Open mission control's Tabs"
            },
            Act::ToTabs,
        ),
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
            spent: 0.0,
            lands_alone: false,
            hands_free: false,
            target: String::new(),
            drained: false,
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
        let alone = Summary {
            lands_alone: true,
            ..summary()
        };
        assert!(
            line(&alone, 160, false, false, &Theme::DARK)
                .text()
                .contains(" · lands on its own")
        );
        let targeted = Summary {
            target: "team/next".into(),
            ..summary()
        };
        assert!(
            line(&targeted, 160, false, false, &Theme::DARK)
                .text()
                .starts_with(" AUTO Work  → team/next · 2 of 3 working")
        );
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
            choices(mode, next, "when seen", true)
                .into_iter()
                .map(|(_, l, _)| l)
                .collect()
        };
        assert_eq!(
            labels(&mode, ""),
            vec![
                "Pause: open no more tabs for now",
                "Take only items with a settled plan",
                "Close landed tabs: when seen",
                "Land without asking",
                "Hands-free: decide for me",
                "Change the target branch…",
                "Change its note, slots, cap or spend limit…",
                "Stop working the note"
            ]
        );
        let (_, _, pause) = choices(&mode, "", "when seen", true).remove(0);
        assert_eq!(apply(&pause, &mut mode).as_deref(), Some("Paused"));
        assert!(mode.paused);
        let (_, label, skip) = choices(&mode, "One", "when seen", true).remove(1);
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
        apply(&Act::LandWithoutAsking, &mut mode);
        assert!(labels(&mode, "").iter().any(|l| l == "Ask before landing"));
        apply(&Act::AskBeforeLanding, &mut mode);
        assert!(!mode.land_without_asking);
        let (question, list) = land_question();
        assert!(question.ends_with("without asking to push?"));
        assert!(list.iter().any(|l| l.starts_with(
            "Kept: the tests, two clean review rounds, the landing lock, leak-check"
        )));
    }

    #[test]
    fn hands_free_lands_without_asking_too_and_off_leaves_landing_as_it_was() {
        let mut mode = Mode {
            on: true,
            note: "Work.md".into(),
            ..Mode::default()
        };
        let has = |mode: &Mode, offer: bool, label: &str| {
            choices(mode, "", "when seen", offer)
                .iter()
                .any(|(_, l, _)| l == label)
        };
        assert!(
            !has(&mode, false, "Hands-free: decide for me"),
            "the setting off offers none"
        );
        apply(&Act::HandsFree, &mut mode);
        assert!(mode.hands_free && mode.land_without_asking);
        assert!(has(&mode, false, "Ask me again: hands-free off"));
        apply(&Act::AskMe, &mut mode);
        assert!(!mode.hands_free && mode.land_without_asking);
        apply(&Act::HandsFree, &mut mode);
        apply(&Act::AskBeforeLanding, &mut mode);
        assert!(!mode.hands_free, "asking before landing isn't hands-free");
        let (question, list) = hands_free_question();
        assert!(question.contains("hands-free"));
        assert!(
            list.iter()
                .any(|l| l.starts_with("Still asked: what your rules or a skill say"))
        );
        let s = Summary {
            hands_free: true,
            lands_alone: true,
            ..summary()
        };
        assert!(s.facts().ends_with(" · hands-free"));
    }

    #[test]
    fn a_skipped_item_hides_in_its_own_note_alone_and_comes_back_there_alone() {
        let mut mode = Mode {
            on: true,
            note: "Work.md".into(),
            note_id: "n-0123456789ab".into(),
            skipped: vec![Skipped {
                note: "n-ffffffffffff".into(),
                text: "Elsewhere".into(),
            }],
            ..Mode::default()
        };
        assert!(
            !choices(&mode, "", "when seen", true)
                .iter()
                .any(|(_, l, _)| l.starts_with("Take the")),
            "another note's skipped item isn't this note's"
        );
        apply(&Act::Skip("One".into()), &mut mode);
        assert_eq!(mode.skipped_here().collect::<Vec<_>>(), ["One"]);
        apply(&Act::Unskip(None), &mut mode);
        assert_eq!(mode.skipped.len(), 1, "the other note's kept");
        assert_eq!(mode.skipped[0].text, "Elsewhere");
    }

    #[test]
    fn one_skipped_item_comes_back_alone() {
        let mut mode = Mode {
            on: true,
            note: "Work.md".into(),
            note_id: "n-0123456789ab".into(),
            skipped: vec![Skipped {
                note: "n-ffffffffffff".into(),
                text: "One".into(),
            }],
            ..Mode::default()
        };
        apply(&Act::Skip("One".into()), &mut mode);
        apply(&Act::Skip("Two".into()), &mut mode);
        apply(&Act::Unskip(Some("One".into())), &mut mode);
        assert_eq!(mode.skipped_here().collect::<Vec<_>>(), ["Two"]);
        assert_eq!(
            mode.skipped.len(),
            2,
            "another note's item by the same words kept"
        );
    }
}
