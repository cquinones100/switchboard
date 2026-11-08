//! M-T: a tab's whole title in a box, since the sidebar cuts it at three
//! lines and the status line at its width: the title as the tab bar has it
//! (`@tab_label`, its PR number in front), wrapped, and under it the
//! branch, the directory, the PR and the Linear ticket, as far as they are
//! known. Copy the title (or `y`) copies it; Close, Esc or a click off the
//! options closes it. The sidebar's `i` and a tab's menu in mission control
//! ("Show its full title") open the same box (the tab_title setting).
//! Ported from tmux/tab-title.sh.
//!
//! It reads what the server holds alone, the branch from git's files and
//! the PR and the ticket as the tab's name was last worked out, never the
//! network, so it opens at once.
//!
//! A tab opened for a note's item (the palette's wtn list, the work-a-note
//! mode) is named with the item's first 50 characters cut back to a word
//! and ending in … (`short_name`), for the tab bar and the second status
//! line, and keeps the item's whole first line as its full title
//! (`@tab_title`), which this box, the sidebar and mission control show in
//! its place, its PR number in front, only while the tab's name is still
//! that item cut short (`cut_of`), so a tab renamed since shows its new
//! name. A tab opened before falls back to the work-a-note mode's
//! `@autopilot_item`, and this box then to the item its branch is for
//! (`wtnItem` in git's config, read from its file, the note named by its ID
//! or, from before notes had IDs, its path). Ported from
//! wtn-open.zsh's `open_tab`, model.js's `cutOf` and tab-title.sh's
//! `cut_of`.

use std::sync::LazyLock;

use regex::Regex;

/// A name is the item whole up to this many characters, else cut short.
const SHORT: usize = 50;

static TRAILING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\s\p{P}\p{S}]+$").expect("a regex"));

/// The name wtn's tab gets for an item's first line: the line whole up to
/// 50 characters, else its first 49 cut back to a word, its trailing
/// spaces and marks taken off, ending in ….
pub fn short_name(whole: &str) -> String {
    let chars: Vec<char> = whole.chars().collect();
    if chars.len() <= SHORT {
        return whole.to_string();
    }
    let mut cut: String = chars[..SHORT - 1].iter().collect();
    if let Some(at) = cut.rfind(' ') {
        cut.truncate(at);
    }
    format!("{}…", TRAILING.replace(&cut, ""))
}

/// Whether a tab's name is still the one wtn gave it for `whole`: the item
/// whole, its word cut (`short_name`), or its first 50 characters, as tabs
/// were named before; a `#<PR> ` in front of the name doesn't count.
pub fn cut_of(name: &str, whole: &str) -> bool {
    if name.is_empty() || whole.is_empty() {
        return false;
    }
    let name = strip_pr(name);
    if name == whole {
        return true;
    }
    let chars: Vec<char> = whole.chars().collect();
    if chars.len() <= SHORT {
        return false;
    }
    name == chars[..SHORT].iter().collect::<String>() || name == short_name(whole)
}

fn strip_pr(name: &str) -> &str {
    name.strip_prefix('#')
        .and_then(|rest| rest.split_once(' '))
        .filter(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        .map_or(name, |(_, rest)| rest)
}

/// The item a branch is for, from the text of a repository's git config:
/// `branch.<branch>.wtnItem`, `<note's ID>: <item's first line>`, or
/// `<note's path>: <item's first line>` from before notes had IDs, the note
/// left off.
pub fn wtn_item(config: &str, branch: &str) -> Option<String> {
    let mut in_branch = false;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            let section = line.trim_start_matches('[').trim_end_matches(']').trim();
            in_branch = section
                .strip_prefix("branch")
                .map(str::trim)
                .and_then(|s| s.strip_prefix('"')?.strip_suffix('"'))
                .is_some_and(|b| b == branch);
            continue;
        }
        if !in_branch {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("wtnitem") {
            continue;
        }
        let value = unquote(value.trim());
        if let Some((_, item)) = crate::notes::id::claim_of(&value) {
            return Some(item.to_string());
        }
        let item = match value.split_once(".md: ") {
            Some((_, item)) => item.to_string(),
            None => value,
        };
        return Some(item);
    }
    None
}

// A git config value as git reads it: quotes taken off, `\"` and `\\`
// unescaped.
fn unquote(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {}
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => {}
            },
            c => out.push(c),
        }
    }
    out
}

/// The Linear ticket a tab's label names, `ABC-123: <title>`, as the label
/// path-facts.sh gave a directory with no name set.
pub fn ticket_of(facts_label: &str) -> Option<&str> {
    let (id, _) = facts_label.split_once(": ")?;
    let (team, number) = id.split_once('-')?;
    let team_ok = team.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && team
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
    let number_ok = !number.is_empty() && number.chars().all(|c| c.is_ascii_digit());
    (team_ok && number_ok).then_some(id)
}

/// The facts under the title, a line each, as far as they are known: the
/// branch (none when detached), the directory, the PR and the ticket.
pub fn facts(
    branch: Option<&str>,
    dir: Option<&str>,
    pr: Option<u64>,
    ticket: Option<&str>,
) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(b) = branch.filter(|b| !b.is_empty() && *b != "HEAD") {
        out.push(format!("Branch     {b}"));
    }
    if let Some(d) = dir.filter(|d| !d.is_empty()) {
        out.push(format!("Directory  {d}"));
    }
    if let Some(n) = pr {
        out.push(format!("PR         #{n}"));
    }
    if let Some(t) = ticket {
        out.push(format!("Ticket     {t}"));
    }
    out
}

/// What the box shows above its options: the title, then the facts after a
/// blank line.
pub fn body(title: &str, facts: &[String]) -> String {
    if facts.is_empty() {
        title.to_string()
    } else {
        format!("{title}\n\n{}", facts.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // test/tab-title.test.js's and test/tmux-sidebar.test.js's cut_of.
    #[test]
    fn a_note_items_tab_is_named_for_it_cut_at_a_word_while_its_name_is_that() {
        let whole = "Copy Claude's last reply into the clipboard, with a key of its own and a tip";
        let short = short_name(whole);
        assert_eq!(short, "Copy Claude's last reply into the clipboard…");
        assert!(short.chars().count() <= 50);
        assert_eq!(short_name("Short item"), "Short item");
        assert!(cut_of(&short, whole));
        assert!(cut_of(&format!("#12 {short}"), whole), "a PR in front");
        let first_fifty: String = whole.chars().take(50).collect();
        assert!(cut_of(&first_fifty, whole), "as tabs were named before");
        assert!(cut_of("Short item", "Short item"));
        assert!(
            !cut_of("Copy Claude's last reply", whole),
            "renamed since, even to the item's first words"
        );
        assert!(!cut_of("", whole));
        let config = "[core]\n\tbare = false\n[branch \"note/copy\"]\n\twtnItem = \"Work.md: Copy \\\"the\\\" reply\"\n";
        assert_eq!(
            wtn_item(config, "note/copy").as_deref(),
            Some("Copy \"the\" reply")
        );
        assert_eq!(wtn_item(config, "main"), None);
        let by_id = "[branch \"note/copy\"]\n\twtnItem = n-0123456789ab: Copy the .md: reply\n";
        assert_eq!(
            wtn_item(by_id, "note/copy").as_deref(),
            Some("Copy the .md: reply"),
            "a claim by the note's ID too"
        );
    }

    #[test]
    fn the_box_says_the_title_then_what_is_known() {
        assert_eq!(ticket_of("ENG-42: Fix the login"), Some("ENG-42"));
        assert_eq!(ticket_of("app"), None);
        assert_eq!(ticket_of("eng-42: lower"), None);
        assert_eq!(ticket_of("Notes: today"), None);
        let lines = facts(Some("fix-login"), Some("~/app"), Some(12), Some("ENG-42"));
        assert_eq!(
            lines,
            [
                "Branch     fix-login",
                "Directory  ~/app",
                "PR         #12",
                "Ticket     ENG-42"
            ]
        );
        assert!(facts(Some("HEAD"), None, None, None).is_empty(), "detached");
        assert_eq!(
            body("#12 ENG-42: Fix the login", &lines[..1]),
            "#12 ENG-42: Fix the login\n\nBranch     fix-login"
        );
    }
}
