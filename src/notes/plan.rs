//! A plan note read for the panel UX 2.0 board 8 draws beside the note:
//! its sections, and its open choice as options, each with its letter, its
//! words, whether it is the recommended one and the mock drawn under it. And
//! a choice made: the option ticked and the headings renamed, as the
//! sessions that build a plan read it (`worktree_from_note::open_choice`).
//! New with UX 2.0; the tmux version opened a plan as any other note.
//!
//! A plan is the shape `plan-item` writes: `# Plan - <name>`, then `## `
//! sections (`Today`, `Decision needed` or `Decision`, `Steps`). An option
//! is a top-level checklist line under `## Decision needed`, `- [ ] A: …`,
//! "(recommended)" anywhere in it marking that one, the lines indented under
//! it more of its words, and a fenced block indented under it opened with
//! ```` ```mock ```` what it would look like drawn in text. Plain data, read
//! and written as text, so it is tested without the app.

use std::sync::LazyLock;

use regex::Regex;

/// A plan note, read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Its name, its title without "Plan - ".
    pub name: String,
    /// Its `## ` sections in order, the decision's among them.
    pub sections: Vec<Section>,
}

/// A `## ` section: its heading's words and what is under it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Section {
    pub heading: String,
    /// Its lines, blank ones at either end left off.
    pub lines: Vec<String>,
    /// The open choice, for a `## Decision needed` section with no option
    /// ticked; its lines then are the words around the options.
    pub choice: Option<Choice>,
}

/// An open choice's options.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Choice {
    pub options: Vec<Opt>,
}

/// One option of a choice.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Opt {
    /// Its letter, A, B, ….
    pub letter: char,
    /// Its words, "(recommended)" taken out.
    pub words: String,
    pub recommended: bool,
    /// Its mock's lines, if it has one.
    pub mock: Vec<String>,
}

static OPTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[-*+] \[([ xX])\] ([A-Z])\b\s*(.*)$").unwrap());
static RECOMMENDED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s*\(recommended\)").unwrap());
static NEEDED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^##[ \t]+decisions?[ \t]+needed\b").unwrap());
static STEPS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^##[ \t]+Steps(?:[ \t]+\(([^)]*)\))?[ \t]*$").unwrap());
static ANY_DECISION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^##[ \t]+decisions?\b").unwrap());
static CHOSE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*(?:[-*+]\s+)?chose\b").unwrap());

impl Plan {
    /// The plan in a note's text, its ID line left out.
    pub fn parse(text: &str) -> Plan {
        let text = super::id::strip(text);
        let mut plan = Plan::default();
        let mut current: Option<Section> = None;
        for (i, line) in text.lines().enumerate() {
            if i == 0 && line.starts_with("# ") {
                let title = line[2..].trim();
                plan.name = title
                    .strip_prefix("Plan - ")
                    .or_else(|| title.strip_prefix("Plan -"))
                    .unwrap_or(title)
                    .trim()
                    .to_string();
                continue;
            }
            if let Some(heading) = line.strip_prefix("## ") {
                plan.sections.extend(current.take().map(Section::settle));
                current = Some(Section {
                    heading: heading.trim().to_string(),
                    ..Section::default()
                });
                continue;
            }
            match current.as_mut() {
                Some(s) => s.lines.push(line.to_string()),
                None if !line.trim().is_empty() => {
                    // Words before the first section, a section of no name.
                    current = Some(Section {
                        lines: vec![line.to_string()],
                        ..Section::default()
                    });
                }
                None => {}
            }
        }
        plan.sections.extend(current.map(Section::settle));
        plan
    }

    /// Its open choice, if any.
    pub fn choice(&self) -> Option<&Choice> {
        self.sections.iter().find_map(|s| s.choice.as_ref())
    }

    /// The option the plan recommends, else its first.
    pub fn recommended(&self) -> Option<char> {
        let options = &self.choice()?.options;
        options
            .iter()
            .find(|o| o.recommended)
            .or(options.first())
            .map(|o| o.letter)
    }
}

impl Section {
    /// Blank lines off its ends, and its options read when it is an open
    /// decision.
    fn settle(mut self) -> Section {
        while self.lines.first().is_some_and(|l| l.trim().is_empty()) {
            self.lines.remove(0);
        }
        while self.lines.last().is_some_and(|l| l.trim().is_empty()) {
            self.lines.pop();
        }
        if NEEDED.is_match(&format!("## {}", self.heading)) {
            let (choice, rest) = options(&self.lines);
            if let Some(choice) = choice {
                self.choice = Some(choice);
                self.lines = rest;
            }
        }
        self
    }
}

/// The options among a decision section's lines, and the lines that are no
/// option's; no choice when any option is ticked, or a `Chose` line says it
/// was made, or there are none.
fn options(lines: &[String]) -> (Option<Choice>, Vec<String>) {
    let mut options: Vec<Opt> = Vec::new();
    let mut rest = Vec::new();
    let mut in_option = false;
    let mut in_mock = false;
    // The mock's fence's indent, its left edge.
    let mut fence = 0;
    let mut blanks = 0;
    for line in lines {
        if let Some(m) = OPTION.captures(line) {
            if m[1].trim() != "" {
                return (None, lines.to_vec());
            }
            let words = m[3].trim_start_matches([':', '·', '.', '-']).trim();
            let recommended = RECOMMENDED.is_match(&m[3]);
            let words = RECOMMENDED.replace_all(words, "").to_string();
            let words = words
                .trim_start_matches([':', '·', '.', '-'])
                .trim()
                .to_string();
            options.push(Opt {
                letter: m[2].chars().next().unwrap_or('A'),
                words,
                recommended,
                mock: Vec::new(),
            });
            in_option = true;
            in_mock = false;
            continue;
        }
        if CHOSE.is_match(line) && !line.starts_with(char::is_whitespace) {
            return (None, lines.to_vec());
        }
        let indented = line.starts_with(char::is_whitespace);
        // A blank line between an option and more of it (its mock, its
        // words) waits for the next line to say whether the option goes on.
        if in_option && !in_mock && line.trim().is_empty() {
            blanks += 1;
            continue;
        }
        if !indented && !in_mock {
            rest.extend(std::iter::repeat_n(String::new(), blanks));
        }
        blanks = 0;
        match options.last_mut() {
            Some(opt) if in_option && (indented || in_mock) => {
                let trimmed = line.trim();
                if in_mock {
                    if trimmed.starts_with("```") {
                        in_mock = false;
                    } else {
                        // The mock's own indent kept, less the fence's.
                        let lead = fence.min(line.len() - line.trim_start().len());
                        opt.mock.push(line[lead..].trim_end().to_string());
                    }
                } else if trimmed.starts_with("```mock") {
                    in_mock = true;
                    fence = line.len() - line.trim_start().len();
                } else if !trimmed.is_empty() {
                    if !opt.words.is_empty() {
                        opt.words.push(' ');
                    }
                    opt.words.push_str(trimmed.trim_start_matches("- "));
                }
            }
            _ => {
                in_option = false;
                in_mock = false;
                rest.push(line.clone());
            }
        }
    }
    rest.extend(std::iter::repeat_n(String::new(), blanks));
    while rest.first().is_some_and(|l| l.trim().is_empty()) {
        rest.remove(0);
    }
    if options.is_empty() {
        return (None, lines.to_vec());
    }
    (Some(Choice { options }), rest)
}

/// The plan's text with option `letter` chosen: its box ticked, `## Decision
/// needed` renamed `## Decision`, and the steps' heading `## Steps (<old>)`
/// renamed `## Steps (<letter>)` with a line under it saying the steps were
/// written for the other option; `None` when the plan has no open choice
/// with that option. The rest of the text, its ID line included, is kept.
pub fn choose(text: &str, letter: char) -> Option<String> {
    let plan = Plan::parse(text);
    let choice = plan.choice()?;
    choice.options.iter().find(|o| o.letter == letter)?;
    // The section the choice is in: the first "Decision needed" heading
    // whose options are still open, as `Plan::choice` reads them; a settled
    // one keeping that heading is passed by, and one after stays open, to be
    // chosen in its turn.
    let all: Vec<&str> = text.split('\n').collect();
    let target = (0..all.len()).find(|&i| {
        NEEDED.is_match(all[i]) && {
            let body: Vec<String> = all[i + 1..]
                .iter()
                .take_while(|l| !l.starts_with("## "))
                .map(|l| l.to_string())
                .collect();
            options(&body).0.is_some()
        }
    })?;
    let mut out: Vec<String> = Vec::new();
    let mut in_decision = false;
    let mut ticked = false;
    // The steps the choice decides are the first after its decision; any
    // other's are left as they are.
    let mut steps_done = false;
    let mut lines = all.iter().copied().enumerate().peekable();
    while let Some((i, line)) = lines.next() {
        if line.starts_with("## ") {
            in_decision = i == target;
            // Another decision after this one has steps of its own.
            if i > target && ANY_DECISION.is_match(line) {
                steps_done = true;
            }
            if in_decision {
                out.push("## Decision".to_string());
                continue;
            }
            if let Some(m) = STEPS.captures(line).filter(|_| i > target && !steps_done) {
                steps_done = true;
                let old = m.get(1).map(|o| o.as_str().trim().to_string());
                out.push(format!("## Steps ({letter})"));
                if let Some(old) = old.filter(|o| !o.is_empty() && *o != letter.to_string()) {
                    out.push(String::new());
                    out.push(format!(
                        "Steps written for {old}; rewrite them for {letter} before starting."
                    ));
                    if lines.peek().is_some_and(|(_, l)| !l.trim().is_empty()) {
                        out.push(String::new());
                    }
                }
                continue;
            }
        }
        if in_decision
            && !ticked
            && let Some(m) = OPTION.captures(line)
            && m[2].starts_with(letter)
        {
            let at = line.find("[ ]").unwrap_or(2);
            out.push(format!("{}[x]{}", &line[..at], &line[at + 3..]));
            ticked = true;
            continue;
        }
        out.push(line.to_string());
    }
    Some(out.join("\n"))
}

static NEEDS_CHOICE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\(needs (?:a )?choices?[^)]*\)").unwrap());

/// An item's line with what it says of its plan's open choice, `(needs a
/// choice: …)`, turned to `(chose <letter>)`; `None` when it says nothing
/// of one.
pub fn item_chose(line: &str, letter: char) -> Option<String> {
    NEEDS_CHOICE.is_match(line).then(|| {
        NEEDS_CHOICE
            .replace(line, format!("(chose {letter})"))
            .into_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN: &str = "# Plan - Condensed sidebar

Item: \"FEAT: a condensed sidebar\"

## Today

- The sidebar is 34 columns wide.

## Decision needed

- [ ] A (recommended): focus widens it
  to full width until focus leaves.
  ```mock
  ● 2 stable
  ⠹ 3 M-d
  ```
- [ ] B: stays condensed

Choosing one renames the heading.

## Steps (A)

1. A setting
2. Tests
";

    #[test]
    fn parse_reads_the_sections_the_options_the_recommended_one_and_a_mock() {
        let plan = Plan::parse(PLAN);
        assert_eq!(plan.name, "Condensed sidebar");
        let headings: Vec<&str> = plan.sections.iter().map(|s| s.heading.as_str()).collect();
        assert_eq!(headings, ["", "Today", "Decision needed", "Steps (A)"]);
        assert_eq!(
            plan.sections[1].lines,
            ["- The sidebar is 34 columns wide."]
        );
        let choice = plan.choice().unwrap();
        assert_eq!(choice.options.len(), 2);
        let a = &choice.options[0];
        assert_eq!(a.letter, 'A');
        assert!(a.recommended);
        assert_eq!(a.words, "focus widens it to full width until focus leaves.");
        assert_eq!(a.mock, ["● 2 stable", "⠹ 3 M-d"]);
        let b = &choice.options[1];
        assert_eq!(
            (b.letter, b.recommended, b.words.as_str()),
            ('B', false, "stays condensed")
        );
        assert!(b.mock.is_empty());
        assert_eq!(
            plan.sections[2].lines,
            ["Choosing one renames the heading."]
        );
        assert_eq!(plan.recommended(), Some('A'));
    }

    #[test]
    fn a_blank_line_between_an_option_and_its_mock_keeps_the_mock_its() {
        let spaced = PLAN
            .replace("  to full width", "\n  to full width")
            .replace("  ```mock", "\n  ```mock");
        let plan = Plan::parse(&spaced);
        let a = &plan.choice().unwrap().options[0];
        assert_eq!(a.mock, ["● 2 stable", "⠹ 3 M-d"]);
        assert_eq!(a.words, "focus widens it to full width until focus leaves.");
        assert_eq!(
            plan.sections[2].lines,
            ["Choosing one renames the heading."]
        );
    }

    #[test]
    fn a_ticked_option_or_a_chose_line_leaves_no_choice_open() {
        let ticked = PLAN.replace("- [ ] B", "- [x] B");
        assert_eq!(Plan::parse(&ticked).choice(), None);
        let chose = PLAN.replace("Choosing one", "Chose B.\n\nChoosing one");
        assert_eq!(Plan::parse(&chose).choice(), None);
        let settled = "# Plan - X\n\n## Decision\n\n- [x] A: this\n\n## Steps\n\n1. Do it\n";
        assert_eq!(Plan::parse(settled).choice(), None);
        assert_eq!(Plan::parse(settled).recommended(), None);
    }

    #[test]
    fn choose_ticks_the_option_and_renames_both_headings() {
        let chosen = choose(PLAN, 'B').unwrap();
        assert!(chosen.contains("## Decision\n"));
        assert!(!chosen.contains("Decision needed"));
        assert!(chosen.contains("- [x] B: stays condensed"));
        assert!(chosen.contains("- [ ] A (recommended)"));
        assert!(chosen.contains(
            "## Steps (B)\n\nSteps written for A; rewrite them for B before starting.\n\n1. A setting"
        ));
        // Read back, the choice is made, as the sessions that build it read it.
        assert_eq!(Plan::parse(&chosen).choice(), None);

        let same = choose(PLAN, 'A').unwrap();
        assert!(same.contains("- [x] A (recommended)"));
        assert!(same.contains("## Steps (A)\n\n1. A setting"));
        assert!(!same.contains("rewrite them"));
    }

    #[test]
    fn choose_leaves_a_plan_without_a_choice_or_that_option_alone() {
        let settled = "# Plan - X\n\n## Steps\n\n1. Do it\n";
        assert_eq!(choose(settled, 'A'), None);
        assert_eq!(choose(PLAN, 'C'), None);
    }

    #[test]
    fn choose_settles_one_decision_and_leaves_the_next_open() {
        let two = PLAN.replace(
            "## Steps (A)",
            "## Decision needed\n\n- [ ] A: one\n- [ ] B: two\n\n## Steps (A)",
        );
        let chosen = choose(&two, 'B').unwrap();
        assert_eq!(chosen.matches("## Decision needed").count(), 1, "{chosen}");
        assert!(chosen.contains("- [x] B: stays condensed"));
        assert!(chosen.contains("- [ ] B: two"));
        assert!(Plan::parse(&chosen).choice().is_some());
    }

    #[test]
    fn choose_passes_a_settled_decision_that_kept_its_heading() {
        let two = PLAN.replace(
            "## Today",
            "## Decision needed\n\n- [x] A: settled\n- [ ] B: not this\n\n## Today",
        );
        let chosen = choose(&two, 'B').unwrap();
        assert!(chosen.contains("- [ ] B: not this"), "{chosen}");
        assert!(chosen.contains("- [x] B: stays condensed"), "{chosen}");
        assert!(Plan::parse(&chosen).choice().is_none());
    }

    #[test]
    fn choose_leaves_an_earlier_decisions_steps_alone() {
        let earlier = PLAN.replace(
            "## Today",
            "## Decision\n\n- [x] A: settled\n\n## Steps (A)\n\n1. Earlier\n\n## Today",
        );
        let chosen = choose(&earlier, 'B').unwrap();
        assert!(chosen.contains("## Steps (A)\n\n1. Earlier"), "{chosen}");
        assert!(
            chosen.contains("## Steps (B)\n\nSteps written for A"),
            "{chosen}"
        );
    }

    #[test]
    fn choose_leaves_a_later_decisions_steps_alone() {
        let later = format!(
            "{}\n## Decision\n\n- [x] A: later\n\n## Steps (A)\n\n1. Later\n",
            PLAN.replace("## Steps (A)\n\n1. A setting\n2. Tests\n", "")
        );
        let chosen = choose(&later, 'B').unwrap();
        assert!(chosen.contains("## Steps (A)\n\n1. Later"), "{chosen}");
    }

    #[test]
    fn steps_with_no_letter_take_the_one_chosen() {
        let plain = PLAN.replace("## Steps (A)", "## Steps");
        let chosen = choose(&plain, 'B').unwrap();
        assert!(chosen.contains("## Steps (B)\n\n1. A setting"));
    }

    #[test]
    fn the_item_says_what_was_chosen_in_place_of_needing_a_choice() {
        assert_eq!(
            item_chose("  Plan: [[Plan - X]] (needs a choice: A, B)", 'B').as_deref(),
            Some("  Plan: [[Plan - X]] (chose B)")
        );
        assert_eq!(item_chose("  Plan: [[Plan - X]]", 'B'), None);
    }
}
