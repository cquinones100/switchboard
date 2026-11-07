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
