//! Your PRs' news as notifications (the `prs` kind): one merged, one
//! approved, one whose checks turned failing, found by comparing the PR
//! board's answer with the one before (`prboard`), so it asks GitHub for
//! nothing of its own. The first answer only sets what is known, so a
//! server starting says nothing of what happened before it; a merged PR is
//! news once, when it first shows in the merged column, which is asked for
//! less often than the open ones.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::items::{Item, State};

/// How many are kept until dismissed, the newest.
pub const KEPT: usize = 20;

/// What was known of each PR at the last answer.
#[derive(Clone, Debug, Default)]
pub struct Known {
    /// Each open PR's repository (lowercased), review decision and checks,
    /// by its address; kept for a repository out of the board's scope a
    /// while (its last tab closed), so its PRs aren't news when it comes
    /// back.
    open: HashMap<String, (String, String, String)>,
    /// The merged PRs already seen.
    merged: HashSet<String>,
    /// The repositories an answer has covered, so one coming into the
    /// board's scope later has its PRs merged before then taken as known.
    repos: HashSet<String>,
}

fn text<'a>(item: &'a Item, path: &str) -> &'a str {
    item.data
        .pointer(path)
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// "PR 212" and the repository's short name.
fn named(item: &Item) -> (String, String) {
    let number = item
        .data
        .pointer("/pr/number")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let repo = text(item, "/pr/repo");
    let short = repo.rsplit('/').next().unwrap_or(repo).to_string();
    (format!("PR {number}"), short)
}

fn notice(kind: &str, board: &Item, title: String, state: State, now: i64) -> Item {
    let mut item = board.clone();
    item.id = format!("{kind}:{}", board.id);
    item.title = title;
    item.detail = board.title.clone();
    item.state = state;
    item.time = Some(now);
    item.group = None;
    item
}

impl Known {
    /// The board's answer for `scope` (`repo:a/b repo:c/d`) taken: the
    /// news since the last one, and what is known now. `now` is when it
    /// answered (ms).
    pub fn take(&mut self, board: &[Item], scope: &str, now: i64) -> Vec<Item> {
        let mut news = Vec::new();
        let in_scope: HashSet<String> = scope
            .split_whitespace()
            .filter_map(|r| r.strip_prefix("repo:"))
            .map(str::to_lowercase)
            .collect();
        // What is known of the PRs of repositories this answer leaves out.
        let mut open: HashMap<String, (String, String, String)> = self
            .open
            .iter()
            .filter(|(_, (repo, _, _))| !in_scope.contains(repo))
            .map(|(id, known)| (id.clone(), known.clone()))
            .collect();
        for item in board {
            let column = item.group.clone().unwrap_or_default();
            let checks = text(item, "/checks").to_string();
            // GitHub's names are the same whatever their case.
            let repo_of = text(item, "/pr/repo").to_lowercase();
            let covered = self.repos.contains(&repo_of);
            if column == "Merged" {
                if self.merged.insert(item.id.clone()) && covered {
                    let (pr, repo) = named(item);
                    let mut n = notice(
                        "merged",
                        item,
                        format!("{pr} merged, {repo}"),
                        State::Idle,
                        now,
                    );
                    n.time = item.time.or(Some(now));
                    news.push(n);
                }
                continue;
            }
            let review = text(item, "/review").to_string();
            // A PR new to a repository answered before was neither approved
            // nor failing until now.
            let was = self
                .open
                .get(&item.id)
                .map(|(_, review, checks)| (review.clone(), checks.clone()))
                .or_else(|| covered.then(Default::default));
            if let Some((was_review, was_checks)) = was {
                let (pr, repo) = named(item);
                // Approved whatever column it shows in (the merge queue first).
                if review == "APPROVED" && was_review != "APPROVED" {
                    news.push(notice(
                        "approved",
                        item,
                        format!("{pr} approved, {repo}"),
                        State::Active,
                        now,
                    ));
                }
                if checks == "failure" && was_checks != "failure" {
                    news.push(notice(
                        "failing",
                        item,
                        format!("Checks failed on {pr}, {repo}"),
                        State::Attention,
                        now,
                    ));
                }
            }
            open.insert(item.id.clone(), (repo_of, review, checks));
        }
        self.open = open;
        self.repos.extend(in_scope);
        news
    }
}

/// The notifications kept with the news added, the newest first, at most
/// `KEPT`; a PR's news of the same kind takes the place of its older one.
pub fn kept_with(kept: &[Item], news: Vec<Item>) -> Vec<Item> {
    let mut out: Vec<Item> = news;
    for item in kept {
        if !out.iter().any(|n| n.id == item.id) {
            out.push(item.clone());
        }
    }
    // A merge comes with its own time, which may be before news kept.
    out.sort_by_key(|i| std::cmp::Reverse(i.time.unwrap_or(0)));
    out.truncate(KEPT);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pr(n: u64, column: &str, checks: &str) -> Item {
        let review = if column == "Approved" { "APPROVED" } else { "" };
        let mut i = Item::new(
            format!("https://github.com/o/web-app/pull/{n}"),
            format!("#{n} Fix"),
        )
        .group(column)
        .data(json!({ "pr": { "number": n, "repo": "o/web-app" }, "checks": checks, "review": review }));
        i.time = None;
        i
    }

    const SCOPE: &str = "repo:o/web-app";

    #[test]
    fn the_first_answer_only_sets_what_is_known() {
        let mut k = Known::default();
        assert!(
            k.take(
                &[pr(1, "Merged", ""), pr(2, "Approved", "failure")],
                SCOPE,
                5
            )
            .is_empty()
        );
    }

    #[test]
    fn a_pr_merged_since_the_last_answer_is_news_once() {
        let mut k = Known::default();
        k.take(&[pr(1, "In review", "")], SCOPE, 5);
        // Gone from the open ones before the merged column is asked again.
        assert!(k.take(&[], SCOPE, 6).is_empty());
        let news = k.take(&[pr(1, "Merged", "")], SCOPE, 7);
        assert_eq!(news.len(), 1);
        assert_eq!(news[0].title, "PR 1 merged, web-app");
        assert_eq!(news[0].id, "merged:https://github.com/o/web-app/pull/1");
        assert!(k.take(&[pr(1, "Merged", "")], SCOPE, 8).is_empty());
    }

    #[test]
    fn a_repository_new_to_the_scope_has_its_merged_prs_taken_as_known() {
        let mut k = Known::default();
        k.take(&[], "repo:o/api", 5);
        assert!(
            k.take(&[pr(1, "Merged", "")], "repo:o/api repo:o/web-app", 6)
                .is_empty()
        );
        assert_eq!(
            k.take(&[pr(1, "Merged", ""), pr(2, "Merged", "")], SCOPE, 7)
                .len(),
            1
        );
        // A remote spelled in another case covers the same repository.
        assert_eq!(k.take(&[pr(3, "Merged", "")], "repo:O/Web-App", 8).len(), 1);
    }

    #[test]
    fn approved_and_failing_are_news_when_they_turn() {
        let mut k = Known::default();
        k.take(&[pr(2, "In review", "pending")], SCOPE, 5);
        let news = k.take(&[pr(2, "Approved", "failure")], SCOPE, 6);
        let titles: Vec<&str> = news.iter().map(|n| n.title.as_str()).collect();
        assert_eq!(
            titles,
            ["PR 2 approved, web-app", "Checks failed on PR 2, web-app"]
        );
        assert!(k.take(&[pr(2, "Approved", "failure")], SCOPE, 7).is_empty());
    }

    #[test]
    fn approved_is_news_in_the_merge_queue_too() {
        let mut k = Known::default();
        k.take(&[pr(3, "In review", "")], SCOPE, 5);
        let mut queued = pr(3, "In merge queue", "");
        queued.data["review"] = json!("APPROVED");
        let news = k.take(&[queued], SCOPE, 6);
        assert_eq!(news.len(), 1);
        assert_eq!(news[0].title, "PR 3 approved, web-app");
    }

    #[test]
    fn a_pr_new_to_a_covered_repository_failing_is_news() {
        let mut k = Known::default();
        k.take(&[], SCOPE, 5);
        let news = k.take(&[pr(4, "In review", "failure")], SCOPE, 6);
        assert_eq!(news.len(), 1);
        assert_eq!(news[0].title, "Checks failed on PR 4, web-app");
    }

    #[test]
    fn an_older_merge_doesnt_push_out_newer_news() {
        let kept: Vec<Item> = (0..KEPT as i64)
            .map(|n| {
                let mut i = Item::new(format!("k{n}"), "kept");
                i.time = Some(100 + n);
                i
            })
            .collect();
        let mut merge = Item::new("merged", "old merge");
        merge.time = Some(1);
        let out = kept_with(&kept, vec![merge]);
        assert_eq!(out.len(), KEPT);
        assert!(out.iter().all(|i| i.id != "merged"));
    }

    #[test]
    fn a_repository_back_in_scope_has_its_prs_as_they_were() {
        let mut k = Known::default();
        k.take(&[], SCOPE, 5);
        k.take(&[pr(5, "Approved", "failure")], SCOPE, 6);
        // Its last tab closed, then opened again.
        k.take(&[], "repo:o/api", 7);
        assert!(k.take(&[pr(5, "Approved", "failure")], SCOPE, 8).is_empty());
    }

    #[test]
    fn kept_newest_first_and_capped() {
        let mut a = Item::new("a", "A");
        a.time = Some(1);
        let mut b = Item::new("b", "B");
        b.time = Some(2);
        let kept = kept_with(&[a.clone()], vec![b.clone()]);
        assert_eq!(kept, [b, a]);
    }
}
