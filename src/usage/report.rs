//! How you use Switchboard, from what the habits setting has counted, as
//! M-p's "How you use Switchboard" shows it: the keys pressed this week and
//! month, the commands picked in M-p, mission control's views opened, and
//! the suggestions waiting now. Ported from lib/habits-report.js's
//! `reportLines`, which built the lines from the counts alone, so its test
//! needs no server or terminal; whatever shows them draws each piece in its
//! tone.

use indexmap::IndexMap;
use serde_json::Value;

use super::habits::{self, Suggestion, Tally, VIEWS};

/// How many keys and picks are listed.
const TOP: usize = 10;

/// How a piece of a line is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Plain,
    /// A section's heading: bold and bright.
    Heading,
    /// Faint: what a column means, a note after a row.
    Dim,
}

/// A line of the report, in pieces each with its tone.
pub type Line = Vec<(String, Tone)>;

/// A line's text, its tones dropped.
pub fn plain(line: &Line) -> String {
    line.iter().map(|(text, _)| text.as_str()).collect()
}

fn text(t: impl Into<String>) -> Line {
    vec![(t.into(), Tone::Plain)]
}

fn dim(t: impl Into<String>) -> Line {
    vec![(t.into(), Tone::Dim)]
}

fn heading(t: &str, after: &str) -> Line {
    vec![
        (t.to_string(), Tone::Heading),
        (after.to_string(), Tone::Dim),
    ]
}

/// Padded on the right to `width` columns, as text.js's `pad`.
fn pad(t: &str, width: usize) -> String {
    format!("{t:<width$}")
}

/// What the report is made from: a month's and a week's counts, a setting's
/// value, the root keys' descriptions, the palette-only commands'
/// descriptions (by `@palette <action>`), and the suggestions waiting now.
pub struct Inputs<'a> {
    pub month: &'a Tally,
    pub week: &'a Tally,
    pub value: &'a dyn Fn(&str) -> Option<Value>,
    pub notes: &'a IndexMap<String, String>,
    pub palette_notes: &'a IndexMap<String, String>,
    pub suggestions: &'a [Suggestion],
}

fn by_count_desc(mut list: Vec<(String, u64)>) -> Vec<(String, u64)> {
    // Stable, so ties keep the order first seen.
    list.sort_by_key(|a| std::cmp::Reverse(a.1));
    list
}

fn is_tab_number(key: &str) -> bool {
    key.len() == 3 && key.starts_with("M-") && matches!(key.as_bytes()[2], b'2'..=b'9')
}

/// The report's lines.
pub fn report_lines(i: &Inputs) -> Vec<Line> {
    if i.month.tracked == 0 {
        return vec![
            text(""),
            text(" Nothing counted yet: counting starts as you use the workspace."),
            text(""),
        ];
    }
    let days = i.month.tracked.min(30);
    let mut lines = vec![
        text(""),
        text(format!(
            " Counted on this computer only, for the last {days} day{}{}.",
            if days == 1 { "" } else { "s" },
            if days < 30 { " of up to 30" } else { "" }
        )),
        text(""),
    ];

    let key_week: IndexMap<String, u64> = i.week.of_kind("key").into_iter().collect();
    let keys = by_count_desc(i.month.of_kind("key"));
    lines.push(heading(
        " Keys you press most",
        " · this week, last 30 days",
    ));
    if keys.is_empty() {
        lines.push(dim("   none yet"));
    }
    for (key, n) in keys.into_iter().take(TOP) {
        // M-2 to M-9 take M-1's description, as tmux.conf gave only M-1 one.
        let note = i
            .notes
            .get(&key)
            .or_else(|| is_tab_number(&key).then(|| i.notes.get("M-1")).flatten())
            .map(String::as_str)
            .unwrap_or("");
        lines.push(text(format!(
            "   {}{}{}{}",
            pad(&key, 10),
            pad(
                &format!("{:>5}", key_week.get(&key).copied().unwrap_or(0)),
                7
            ),
            pad(&format!("{n:>6}"), 8),
            habits::untagged(note)
        )));
    }
    lines.push(text(""));

    let picks = by_count_desc(i.month.of_kind("palette"));
    lines.push(heading(" Picked in M-p", " · last 30 days"));
    if picks.is_empty() {
        lines.push(dim("   none yet"));
    }
    for (key, n) in picks.into_iter().take(TOP) {
        let own = i.notes.get(&key);
        let words = own
            .or_else(|| i.palette_notes.get(&key))
            .map(|n| habits::untagged(n).to_string())
            .unwrap_or_else(|| key.clone());
        let mut line = text(format!("   {}{words}", pad(&format!("{n:>5}"), 8)));
        if own.is_some() {
            line.push((format!("  · {key} does it"), Tone::Dim));
        }
        lines.push(line);
    }
    lines.push(text(""));

    // Every view counted, most opened first (Tabs and Settings, and one whose
    // setting is now off, too), then the views that are on and weren't.
    let opened = by_count_desc(i.month.of_kind("view"));
    lines.push(heading(
        " Mission control's views opened",
        " · last 30 days",
    ));
    for (title, n) in &opened {
        lines.push(text(format!("   {}{title}", pad(&format!("{n:>5}"), 8))));
    }
    for v in VIEWS
        .iter()
        .filter(|v| habits::view_on(v, i.value) && !opened.iter().any(|(t, _)| t == v.title))
    {
        let mut line = text(format!("   {}{}", pad(&format!("{:>5}", 0), 8), v.title));
        line.push(("  · not opened".to_string(), Tone::Dim));
        lines.push(line);
    }
    lines.push(text(""));

    lines.push(heading(" Suggestions now", ""));
    if i.suggestions.is_empty() {
        lines.push(dim("   none"));
    }
    for s in i.suggestions {
        lines.push(text(format!("   {}", s.title)));
    }
    lines.push(text(""));
    lines.push(dim(
        " Each suggestion is in the sidebar's notifications, where Enter acts on it.",
    ));
    lines
}

/// The report while counting is off.
pub fn off_lines() -> Vec<Line> {
    vec![
        text(""),
        text(" Counting is off: Settings' \"Suggestions from how you work\" turns it on."),
        text(""),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;

    fn map(list: &[(&str, &str)]) -> IndexMap<String, String> {
        list.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn report(
        month: &Tally,
        week: &Tally,
        value: &dyn Fn(&str) -> Option<Value>,
        notes: &IndexMap<String, String>,
        palette_notes: &IndexMap<String, String>,
        suggestions: &[Suggestion],
    ) -> String {
        report_lines(&Inputs {
            month,
            week,
            value,
            notes,
            palette_notes,
            suggestions,
        })
        .iter()
        .map(plain)
        .collect::<Vec<_>>()
        .join("\n")
    }

    fn matches(text: &str, pattern: &str) -> bool {
        Regex::new(pattern).unwrap().is_match(text)
    }

    #[test]
    fn the_report_lists_keys_picks_views_and_suggestions() {
        let month = Tally::of(
            &[
                ("key\tM-d", 40),
                ("key\tM-a", 12),
                ("key\tM-3", 5),
                ("palette\tM-d", 4),
                ("palette\t@palette reset-tips", 2),
                ("view\tCalendar", 3),
                ("view\tSettings", 5),
                ("view\tCI", 1),
            ],
            12,
        );
        let week = Tally::of(&[("key\tM-d", 9)], 12);
        let value = |k: &str| Some(Value::Bool(k != "github"));
        let notes = map(&[
            ("M-d", "Open the branch's diff #git"),
            ("M-a", "Go to the next tab #tabs"),
            ("M-1", "Go to a tab by number (M-1 to M-9) #tabs"),
        ]);
        let palette = map(&[("@palette reset-tips", "Show the tips again #help")]);
        let suggestion = Suggestion {
            id: "key:M-d".into(),
            title: "M-d: Open the branch's diff".into(),
            body: String::new(),
            setting: None,
        };
        let text = report(&month, &week, &value, &notes, &palette, &[suggestion]);
        assert!(text.contains("last 12 days"), "{text}");
        assert!(
            matches(&text, r"(?m)M-d +9 +40 +Open the branch's diff$"),
            "the week's and month's presses, the tags left out: {text}"
        );
        assert!(
            text.find("M-d ").unwrap() < text.find("M-a ").unwrap(),
            "most pressed first"
        );
        assert!(
            matches(&text, r"M-3 .*Go to a tab by number"),
            "M-2 to M-9 take M-1's description"
        );
        assert!(matches(
            &text,
            r"4 +Open the branch's diff {2}· M-d does it"
        ));
        assert!(matches(&text, r"(?m)2 +Show the tips again$"));
        assert!(matches(&text, r"(?m)3 +Calendar$"));
        assert!(matches(&text, r"0 +Slack {2}· not opened"));
        assert!(
            matches(&text, r"5 +Settings\n +3 +Calendar\n +1 +CI\n"),
            "every view counted, most opened first, one now off too"
        );
        assert!(
            !matches(&text, r"PRs|Merge queues"),
            "views of a service turned off aren't listed as not opened"
        );
        assert!(matches(
            &text,
            r"Suggestions now\n +M-d: Open the branch's diff"
        ));
    }

    #[test]
    fn the_period_stops_at_the_30_days_it_counts() {
        let t = Tally::of(&[], 45);
        let on = |_: &str| Some(Value::Bool(true));
        let none = IndexMap::new();
        let text = report(&t, &t, &on, &none, &none, &[]);
        assert!(text.contains("for the last 30 days."));
    }

    #[test]
    fn with_nothing_counted_it_says_so() {
        let t = Tally::of(&[], 0);
        let on = |_: &str| Some(Value::Bool(true));
        let none = IndexMap::new();
        assert!(report(&t, &t, &on, &none, &none, &[]).contains("Nothing counted yet"));
    }
}
