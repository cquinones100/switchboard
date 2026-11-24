//! GitHub issues as a ticket source: an issue by its link or
//! `owner/repo#12`, the issue a branch was started for (`wt --issue`), and
//! the repository's open issues to choose a tab's from. GitHub's API is
//! `integrations::github`, behind the `github` setting.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use super::{Lists, Ticket, Yours};
use crate::integrations::github;

/// A GitHub issue named in text: its link, or its `owner/repo#12`.
static ISSUE_IN_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:https://github\.com/([\w.-]+)/([\w.-]+)/issues/([0-9]+)\b)|(?:(?:^|[^\w./-])([\w.-]+)/([\w.-]+)#([0-9]+)\b)",
    )
    .unwrap()
});
static ISSUE_BRANCH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^issue/([0-9]+)(?:-|$)").unwrap());

/// An issue as `Ticket::arg` writes it, `owner/repo#12`.
pub(super) fn issue_arg(text: &str) -> Option<Ticket> {
    let (repo, number) = crate::autopilot::issues::parse_reference(text)?;
    Some(Ticket::Issue { repo, number })
}

/// The issue the branch checked out in `dir` was started for, from its
/// repository's config file and origin (`issue_of_branch`).
pub(super) fn of_branch(dir: &Path, common_dir: &Path, branch: &str) -> Vec<Ticket> {
    let config = std::fs::read_to_string(common_dir.join("config")).unwrap_or_default();
    let origin = crate::sources::repo::origin_of(dir);
    issue_of_branch(&config, branch, origin.as_ref())
        .into_iter()
        .collect()
}

/// The GitHub issue a branch was started for (`wt --issue`), from the
/// text of its repository's git config and its origin: its
/// `branch.<branch>.wtnIssue`, else an `issue/<number>-<slug>` branch's
/// number in origin's repository.
pub fn issue_of_branch(
    config: &str,
    branch: &str,
    origin: Option<&github::Repo>,
) -> Option<Ticket> {
    if let Some(issue) = crate::tabs::title::branch_value(config, branch, "wtnIssue")
        .and_then(|r| Ticket::parse(&r))
        .filter(|t| matches!(t, Ticket::Issue { .. }))
    {
        return Some(issue);
    }
    let number = issue_number_of_branch(branch)?;
    Some(Ticket::Issue {
        repo: origin?.clone(),
        number,
    })
    .filter(Ticket::plain)
}

/// The number an `issue/<number>-<slug>` branch was named for.
pub fn issue_number_of_branch(branch: &str) -> Option<u64> {
    ISSUE_BRANCH.captures(branch)?[1].parse().ok()
}

/// Every GitHub issue `text` names by its link or `owner/repo#12`, each
/// once, in order.
pub fn issues_in(text: &str) -> Vec<Ticket> {
    let mut out: Vec<Ticket> = Vec::new();
    for c in ISSUE_IN_TEXT.captures_iter(text) {
        let part = |at: usize| c.get(at).or_else(|| c.get(at + 3)).map(|m| m.as_str());
        let (Some(owner), Some(name), Some(number)) = (part(1), part(2), part(3)) else {
            continue;
        };
        let Ok(number) = number.parse() else {
            continue;
        };
        let issue = Ticket::Issue {
            repo: github::Repo {
                owner: owner.to_string(),
                name: name.to_string(),
            },
            number,
        };
        if issue.plain() && !out.contains(&issue) {
            out.push(issue);
        }
    }
    out
}

/// The open issues of the repository at `dir`, those assigned to you
/// first, for the picker that chooses the tab's ticket in a repository
/// whose `tickets` is `github`: its origin's, the repository that
/// setting is kept for (`repos::tickets_known`). Asks GitHub, so off the
/// server's task.
pub async fn open_issues_at(dir: &Path) -> Result<Lists, String> {
    const SOURCE: &str = "choose a tab's ticket";
    let repo = crate::sources::repo::origin_of(dir)
        .ok_or_else(|| "This repository's origin isn't on GitHub.".to_string())?;
    let issues = github::open_issues(&repo, SOURCE)
        .await
        .map_err(|e| e.to_string())?;
    Ok(Lists {
        found: open_issue_rows(&repo, issues),
        missing: Vec::new(),
    })
}

/// `open_issues_at`'s rows, in GitHub's order.
pub fn open_issue_rows(repo: &github::Repo, issues: Vec<github::OpenIssue>) -> Vec<Yours> {
    issues
        .into_iter()
        .map(|i| {
            let title: String = i.title.chars().filter(|c| !c.is_control()).collect();
            let yours = if i.yours { " · assigned to you" } else { "" };
            Yours {
                row: format!("#{}  {title}{yours}", i.number),
                ticket: Ticket::Issue {
                    repo: repo.clone(),
                    number: i.number,
                },
                created: String::new(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issues_are_found_in_text_by_their_link_or_reference() {
        assert_eq!(
            issues_in("see https://github.com/acme/app/issues/12, acme/app#7 and acme/app#12"),
            [
                Ticket::parse("acme/app#12").unwrap(),
                Ticket::parse("acme/app#7").unwrap()
            ]
        );
        assert!(issues_in("a.b/c#1x and https://github.com/acme/app/pull/3").is_empty());
        assert_eq!(Ticket::parse("acme/app#12").unwrap().short(), "#12");
    }
}
