//! Every pull request a tab follows: the ones its Claude session opened or
//! looked at (`agent::prs`, from its `PostToolUse` hooks), in the order
//! opened, kept as the tab's `@tab_prs` option (saved with the layout and
//! handed over on an upgrade, as every tab option is), joined by its
//! branch's PR as the tab names find it (`names`), which is never stored, so
//! moving to another branch takes the old branch's PR along only when the
//! session opened it. New code, with nothing of the tmux version's before
//! it.
//!
//! One of them is the tab's current PR, which its label leads with (as
//! `wt-jump` reads it), and M-v, M-d, the sidebar's facts and mission
//! control's cards read: the one picked with "Show another of this tab's
//! PRs" (`@tab_pr_current`) while it is still listed, else the branch's,
//! else the newest opened that the lookups know is open. With Settings' `tab_prs` off, a tab has only its
//! branch's PR, as before.
//!
//! Plain data, read and written as text, with nothing in it that reads a
//! file or asks a service, so the drawing path can read it.

use crate::facts::{PrFacts, PrState};
use crate::integrations::github::{self, Repo};
use crate::ui::kit::Mark;

/// The PRs the tab's session opened, `<owner>/<name>#<n>` each, separated
/// by spaces, oldest first.
pub const LIST: &str = "@tab_prs";
/// The ones of `LIST` the session made with `gh pr create`, written as it
/// is: each may stay current once merged, while the tab is on its branch or
/// its repository's default branch (`merged_stays_current`), where a PR
/// only looked at is current only while open.
pub const MADE: &str = "@tab_prs_made";
/// The one picked as current, `<owner>/<name>#<n>`.
pub const CURRENT: &str = "@tab_pr_current";
/// Settings' switch.
pub const SETTING: &str = "tab_prs";

/// One PR a tab follows. Its repository is known for every PR the session
/// opened; only the branch's may lack it (a `wtpr` directory's number),
/// which is then its directory's origin's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabPr {
    pub number: u64,
    pub repo: Option<Repo>,
}

impl TabPr {
    /// `<owner>/<name>#<n>`, or `#<n>` with no repository known.
    pub fn key(&self) -> String {
        match &self.repo {
            Some(repo) => format!("{repo}#{}", self.number),
            None => format!("#{}", self.number),
        }
    }

    /// Reads one written by `key`, its repository required.
    pub fn parse(text: &str) -> Option<TabPr> {
        let (repo, number) = text.trim().rsplit_once('#')?;
        Some(TabPr {
            number: number.parse().ok().filter(|n| *n > 0)?,
            repo: Some(github::parse_name_with_owner(repo)?),
        })
    }

    /// Whether it is the same PR as `other`: the same number in the same
    /// repository, either one's unknown repository matching any.
    pub fn same(&self, other: &TabPr) -> bool {
        self.number == other.number
            && match (&self.repo, &other.repo) {
                (Some(a), Some(b)) => {
                    a.owner.eq_ignore_ascii_case(&b.owner) && a.name.eq_ignore_ascii_case(&b.name)
                }
                _ => true,
            }
    }
}

/// The PRs a `@tab_prs` value lists, anything unreadable left out.
pub fn read(value: &str) -> Vec<TabPr> {
    let mut list: Vec<TabPr> = Vec::new();
    for pr in value.split_whitespace().filter_map(TabPr::parse) {
        if !list.iter().any(|p| p.same(&pr)) {
            list.push(pr);
        }
    }
    list
}

/// A `@tab_prs` value for `list`.
pub fn write(list: &[TabPr]) -> String {
    list.iter().map(TabPr::key).collect::<Vec<_>>().join(" ")
}

/// `list` with `pr` added at its end, as the newest; `None` when it was
/// listed already.
pub fn with(list: &[TabPr], pr: TabPr) -> Option<Vec<TabPr>> {
    if list.iter().any(|p| p.same(&pr)) {
        return None;
    }
    let mut out = list.to_vec();
    out.push(pr);
    Some(out)
}

/// Every PR the tab follows, in the order opened: the branch's first when
/// the session didn't open it, else where it was opened, with the
/// repository the branch's lookup found when the listed one has none.
pub fn joined(branch: Option<&TabPr>, opened: &[TabPr]) -> Vec<TabPr> {
    let mut all = opened.to_vec();
    if let Some(branch) = branch {
        match all.iter_mut().find(|p| p.same(branch)) {
            Some(p) => {
                if p.repo.is_none() {
                    p.repo.clone_from(&branch.repo);
                }
            }
            None => all.insert(0, branch.clone()),
        }
    }
    all
}

/// The tab's current PR among `all`: the one picked while it is listed,
/// else the branch's, else the newest that `eligible` takes (the server's:
/// one the session made, else one the lookups know is open), so a merged
/// PR the session only looked at never becomes a branchless tab's PR and
/// files it under Merged, while one it made stays current, marked merged,
/// until it leaves the list.
pub fn current(
    all: &[TabPr],
    branch: Option<&TabPr>,
    picked: Option<&TabPr>,
    eligible: impl Fn(&TabPr) -> bool,
) -> Option<TabPr> {
    let listed = |pr: &TabPr| all.iter().find(|p| p.same(pr)).cloned();
    picked
        .and_then(listed)
        .or_else(|| branch.and_then(listed))
        .or_else(|| all.iter().rev().find(|p| eligible(p)).cloned())
}

/// Whether a merged PR the session made, `head` its branch, may still be
/// the tab's current PR: while the tab is on that branch (before the
/// landing's cleanup) or on its repository's default branch (the worktree
/// removed, the tab back in the main clone), so the tab stays under Merged
/// with the PR leading its label until it takes up other work on another
/// branch.
pub fn merged_stays_current(head: &str, branch: Option<&str>, on_default: bool) -> bool {
    on_default || branch.is_some_and(|b| !head.is_empty() && b == head)
}

/// `all` with the current one first, the rest in the order opened, as the
/// sidebar and the Tabs card list them.
pub fn current_first(all: &[TabPr], current: Option<&TabPr>) -> Vec<TabPr> {
    let mut out: Vec<TabPr> = current.into_iter().cloned().collect();
    out.extend(
        all.iter()
            .filter(|p| current.is_none_or(|c| !c.same(p)))
            .cloned(),
    );
    out
}

/// A PR's mark by where it stands (boards 127 to 129): merged or its checks
/// passed ✓, closed or a check failing ✕, checks running ◌, else ○ (no
/// checks yet, or not looked up yet).
pub fn mark_of(facts: Option<&PrFacts>) -> Mark {
    let Some(f) = facts else {
        return Mark::Idle;
    };
    match (f.state, f.checks) {
        (PrState::Merged, _) => Mark::Passed,
        (PrState::Closed, _) => Mark::Failed,
        (_, Some(c)) if c.fail > 0 => Mark::Failed,
        (_, Some(c)) if c.pending > 0 => Mark::Pending,
        (_, Some(c)) if c.pass > 0 => Mark::Passed,
        _ if f.review_decision.as_deref() == Some("APPROVED") => Mark::Passed,
        _ => Mark::Idle,
    }
}

/// Where a PR stands in a few words after its mark, as the Tabs card says
/// it (board 128): `3 of 5`, `1 failing`, `approved`, `merged`.
pub fn standing(f: &PrFacts) -> String {
    match (f.state, f.checks) {
        (PrState::Merged, _) => "merged".into(),
        (PrState::Closed, _) => "closed".into(),
        (_, Some(c)) if c.fail > 0 => format!("{} failing", c.fail),
        (_, Some(c)) if c.pending > 0 => format!("{} of {}", c.pass, c.pass + c.fail + c.pending),
        _ if f.review_decision.as_deref() == Some("APPROVED") => "approved".into(),
        (_, Some(c)) if c.pass > 0 => "passed".into(),
        _ if f.draft => "draft".into(),
        _ => "open".into(),
    }
}

/// Where a PR stands in full, as the picker says it (board 129): its state
/// or review, then its checks: `In review · ◌ 3 of 5`, `Draft · ✕ 1
/// failing`, `Merged`.
pub fn standing_long(f: &PrFacts) -> String {
    let state = match f.state {
        PrState::Merged => return "Merged".into(),
        PrState::Closed => return "Closed".into(),
        PrState::Open if f.draft => "Draft",
        PrState::Open => match f.review_decision.as_deref() {
            Some("APPROVED") => "Approved",
            Some("CHANGES_REQUESTED") => "Changes requested",
            _ => "In review",
        },
    };
    let checks = f.checks.map(|c| {
        if c.fail > 0 {
            format!("✕ {} failing", c.fail)
        } else if c.pending > 0 {
            format!("◌ {} of {}", c.pass, c.pass + c.fail + c.pending)
        } else {
            "✓ passed".into()
        }
    });
    match checks {
        Some(checks) if f.checks.is_some_and(|c| c.pass + c.fail + c.pending > 0) => {
            format!("{state} · {checks}")
        }
        _ => state.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(key: &str) -> TabPr {
        TabPr::parse(key).unwrap()
    }

    fn facts(state: PrState, checks: Option<(u32, u32, u32)>, review: Option<&str>) -> PrFacts {
        PrFacts {
            number: 1,
            title: String::new(),
            head: String::new(),
            draft: false,
            repo: "acme/a".into(),
            url: String::new(),
            state,
            merge: None,
            checks: checks.map(|(pass, fail, pending)| crate::facts::Checks {
                pass,
                fail,
                pending,
            }),
            review_decision: review.map(str::to_string),
            additions: 0,
            deletions: 0,
        }
    }

    #[test]
    fn where_a_pr_stands_as_the_boards_say_it() {
        let running = facts(PrState::Open, Some((3, 0, 2)), None);
        assert_eq!(mark_of(Some(&running)), Mark::Pending);
        assert_eq!(standing(&running), "3 of 5");
        assert_eq!(standing_long(&running), "In review · ◌ 3 of 5");
        let approved = facts(PrState::Open, Some((5, 0, 0)), Some("APPROVED"));
        assert_eq!(mark_of(Some(&approved)), Mark::Passed);
        assert_eq!(standing(&approved), "approved");
        assert_eq!(standing_long(&approved), "Approved · ✓ passed");
        let mut draft = facts(PrState::Open, Some((2, 1, 0)), None);
        draft.draft = true;
        assert_eq!(mark_of(Some(&draft)), Mark::Failed);
        assert_eq!(standing(&draft), "1 failing");
        assert_eq!(standing_long(&draft), "Draft · ✕ 1 failing");
        let merged = facts(PrState::Merged, None, None);
        assert_eq!(
            (mark_of(Some(&merged)), standing_long(&merged)),
            (Mark::Passed, "Merged".into())
        );
        assert_eq!(mark_of(None), Mark::Idle);
    }

    #[test]
    fn a_list_reads_back_as_written_each_pr_once() {
        let list = read(
            "acme/web-app#2024  acme/web-app#2040 bad acme/oncall-tools#318 Acme/Web-App#2024",
        );
        assert_eq!(
            write(&list),
            "acme/web-app#2024 acme/web-app#2040 acme/oncall-tools#318"
        );
        assert_eq!(read(""), Vec::new());
        assert!(TabPr::parse("acme#3").is_none());
        assert!(TabPr::parse("acme/x#0").is_none());
    }

    #[test]
    fn a_pr_opened_again_is_listed_once() {
        let list = read("acme/a#1");
        assert_eq!(with(&list, pr("acme/a#1")), None);
        assert_eq!(
            write(&with(&list, pr("acme/b#1")).unwrap()),
            "acme/a#1 acme/b#1"
        );
    }

    #[test]
    fn the_branch_s_pr_joins_the_list_rather_than_replacing_it() {
        let opened = read("acme/web-app#2024 acme/oncall-tools#318");
        let branch = pr("acme/web-app#2040");
        assert_eq!(
            write(&joined(Some(&branch), &opened)),
            "acme/web-app#2040 acme/web-app#2024 acme/oncall-tools#318"
        );
        // Opened by the session, it stays where it was opened; a branch's
        // number with no repository takes the listed one's.
        let bare = TabPr {
            number: 2024,
            repo: None,
        };
        assert_eq!(
            write(&joined(Some(&bare), &opened)),
            "acme/web-app#2024 acme/oncall-tools#318"
        );
        assert_eq!(write(&joined(None, &[])), "");
    }

    #[test]
    fn the_current_pr_is_the_pick_else_the_branch_s_else_the_newest() {
        let branch = pr("acme/web-app#2040");
        let all = joined(
            Some(&branch),
            &read("acme/web-app#2024 acme/oncall-tools#318"),
        );
        let picked = pr("acme/oncall-tools#318");
        assert_eq!(
            current(&all, Some(&branch), Some(&picked), |_| true),
            Some(picked)
        );
        assert_eq!(
            current(&all, Some(&branch), None, |_| true),
            Some(branch.clone())
        );
        // A pick no longer listed (merged and dropped) gives way.
        assert_eq!(
            current(&all, Some(&branch), Some(&pr("acme/x#9")), |_| true),
            Some(branch)
        );
        let opened = read("acme/a#1 acme/b#2");
        assert_eq!(current(&opened, None, None, |_| true), Some(pr("acme/b#2")));
        // Only an open one becomes current by being newest.
        let open = |p: &TabPr| p.number == 1;
        assert_eq!(current(&opened, None, None, open), Some(pr("acme/a#1")));
        assert_eq!(current(&opened, None, None, |_| false), None);
        assert_eq!(current(&[], None, None, |_| true), None);
    }

    #[test]
    fn a_made_pr_stays_current_after_the_tab_moves_to_the_main_clone() {
        let opened = read("acme/web-app#2024");
        let made = pr("acme/web-app#2024");
        let current_on = |branch: Option<&str>, on_default: bool| {
            current(&opened, None, None, |p| {
                p.same(&made) && merged_stays_current("fix-login", branch, on_default)
            })
        };
        // In the worktree on its head branch, then back in the main clone on
        // the default branch, with no branch PR there.
        assert_eq!(current_on(Some("fix-login"), false), Some(made.clone()));
        assert_eq!(current_on(Some("main"), true), Some(made.clone()));
        // On a new branch for other work, it gives way.
        assert_eq!(current_on(Some("next-thing"), false), None);
        assert_eq!(current_on(None, false), None);
    }

    #[test]
    fn the_current_one_leads_the_rest_in_the_order_opened() {
        let all = read("acme/a#1 acme/a#2 acme/b#3");
        assert_eq!(
            write(&current_first(&all, Some(&pr("acme/a#2")))),
            "acme/a#2 acme/a#1 acme/b#3"
        );
    }
}
