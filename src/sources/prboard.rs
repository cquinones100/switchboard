//! Mission control's PR board, ported from lib/sidebar/prboard.js: your pull
//! requests in the repositories your tabs are in, as columns by where they
//! stand (Draft, In review, Approved, In merge queue, and Merged in the last
//! `MERGED_DAYS`), from two GitHub searches. Only those repositories, so old
//! pull requests elsewhere stay off the board: each tab's origin, and in a
//! fork's clone the project its `upstream` remote names, where the PRs from
//! your fork are (`super::pr_repos_of`).
//!
//! The merged column changes far less than the rest, so it is searched for
//! only every `MERGED_EVERY`, or when the repositories change; in between,
//! the last answer is kept and only the open PRs are asked for.
//!
//! The view's items come column by column, each with its column as `group`;
//! `COLUMNS` names them in order, so an empty column is still drawn.

use std::time::Duration;

use serde_json::{Value, json};

use super::{in_columns, ms_of, text};
use crate::integrations::github;
use crate::items::{Item, State, Target};

pub const COLUMNS: [&str; 5] = ["Draft", "In review", "Approved", "In merge queue", "Merged"];
/// UX 2.0's columns: a PR with changes asked for in a column of its own.
pub const COLUMNS_UX2: [&str; 6] = [
    "Draft",
    "In review",
    "Changes asked",
    "Approved",
    "In merge queue",
    "Merged",
];

/// The board's columns, with UX 2.0 on or off.
pub fn columns(ux2: bool) -> &'static [&'static str] {
    if ux2 { &COLUMNS_UX2 } else { &COLUMNS }
}
pub const MERGED_DAYS: i64 = 7;
pub const MERGED_EVERY: Duration = Duration::from_secs(10 * 60);

const FIELDS: &str = "... on PullRequest { number title url isDraft state reviewDecision mergedAt mergedBy { login } body additions deletions author { login } repository { nameWithOwner } mergeQueueEntry { position } commitCount: commits { totalCount } commits(last: 1) { nodes { commit { statusCheckRollup { state } } } } reviewRequests(first: 10) { nodes { requestedReviewer { ... on User { login } ... on Team { name } } } } latestReviews(first: 10) { nodes { author { login } } } }";

fn column_of(pr: &Value, ux2: bool) -> &'static str {
    if text(pr, "/state") == "MERGED" {
        "Merged"
    } else if pr.get("mergeQueueEntry").is_some_and(|e| !e.is_null()) {
        "In merge queue"
    } else if pr.get("isDraft").and_then(Value::as_bool) == Some(true) {
        "Draft"
    } else if ux2 && text(pr, "/reviewDecision") == "CHANGES_REQUESTED" {
        "Changes asked"
    } else if text(pr, "/reviewDecision") == "APPROVED" {
        "Approved"
    } else {
        "In review"
    }
}

/// A board card: the PR and its title, then its repository, its checks, its
/// review and its place in the queue. Failing checks or changes asked for
/// need you. With UX 2.0 on, a PR with changes asked for has a column of
/// its own, and the card carries its reviewers and what its description
/// beside the board shows.
pub fn item_of(pr: &Value, ux2: bool) -> Option<Item> {
    let number = pr.get("number").and_then(Value::as_u64)?;
    let url = text(pr, "/url");
    let repo = text(pr, "/repository/nameWithOwner");
    let checks = text(pr, "/commits/nodes/0/commit/statusCheckRollup/state");
    let failing = checks == "FAILURE" || checks == "ERROR";
    let changes = text(pr, "/reviewDecision") == "CHANGES_REQUESTED";
    let position = pr
        .pointer("/mergeQueueEntry/position")
        .and_then(Value::as_u64);
    let marks: Vec<String> = [
        repo.split('/').nth(1).unwrap_or("").to_string(),
        match checks {
            _ if failing => "✗ checks".into(),
            "PENDING" => format!("{} checks", crate::sidebar::text::pending_mark()),
            "SUCCESS" => "✓ checks".into(),
            _ => String::new(),
        },
        if changes {
            "changes requested".into()
        } else {
            String::new()
        },
        position.map(|p| format!("queue {p}")).unwrap_or_default(),
    ]
    .into_iter()
    .filter(|m| !m.is_empty())
    .collect();
    // UX 2.0's cards show the checks as a mark of their own, and the rest
    // on their last line without them.
    let check = match checks {
        _ if failing => "failure",
        "PENDING" => "pending",
        "SUCCESS" => "success",
        _ => "",
    };
    let meta = marks
        .iter()
        .filter(|m| !m.ends_with("checks"))
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" · ");
    let logins = |path: &str, field: &str| -> Vec<String> {
        pr.pointer(path)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|n| n.pointer(field).and_then(Value::as_str))
            .map(str::to_string)
            .collect()
    };
    let mut reviewers = logins("/reviewRequests/nodes", "/requestedReviewer/login");
    reviewers.extend(logins("/reviewRequests/nodes", "/requestedReviewer/name"));
    for login in logins("/latestReviews/nodes", "/author/login") {
        if !reviewers.contains(&login) {
            reviewers.push(login);
        }
    }
    let state = if failing || changes {
        State::Attention
    } else if text(pr, "/state") == "MERGED" {
        State::Idle
    } else {
        State::Active
    };
    let mut item = Item::new(url, format!("#{number} {}", text(pr, "/title")))
        .detail(marks.join(" · "))
        .state(state)
        .target(Target::Url(url.to_string()))
        .group(column_of(pr, ux2))
        .data(json!({
            "pr": { "number": number, "repo": repo },
            "url": url,
            "checks": check,
            "meta": meta,
            "reviewers": reviewers.join(", "),
            "author": text(pr, "/author/login"),
            "mergedBy": text(pr, "/mergedBy/login"),
            "commits": pr.pointer("/commitCount/totalCount").and_then(Value::as_u64),
            "additions": pr.get("additions").and_then(Value::as_u64),
            "deletions": pr.get("deletions").and_then(Value::as_u64),
            "body": text(pr, "/body"),
        }));
    item.time = ms_of(pr.get("mergedAt").and_then(Value::as_str));
    Some(item)
}

/// Lays out search results as the board: each PR once, column by column,
/// the merged ones newest first.
pub fn board_from(prs: &[Value], ux2: bool) -> Vec<Item> {
    let mut seen = std::collections::HashSet::new();
    let mut items: Vec<Item> = prs
        .iter()
        .filter_map(|pr| item_of(pr, ux2))
        .filter(|i| seen.insert(i.id.clone()))
        .collect();
    // A stable sort, so only the merged column changes order.
    items.sort_by_key(|i| {
        if i.group.as_deref() == Some("Merged") {
            -i.time.unwrap_or(0)
        } else {
            0
        }
    });
    in_columns(columns(ux2), items)
}

/// The merged PRs as last searched for: the repositories asked about, when,
/// and what came back.
#[derive(Clone, Debug, Default)]
pub struct Board {
    merged: Option<(String, i64, Vec<Value>)>,
}

impl Board {
    /// Whether the next look asks for merged PRs too: the first time, when
    /// the repositories (`scope`) changed, or `MERGED_EVERY` after the last.
    pub fn wants_merged(&self, scope: &str, now_ms: i64) -> bool {
        match &self.merged {
            None => true,
            Some((was, at, _)) => was != scope || now_ms - at >= MERGED_EVERY.as_millis() as i64,
        }
    }

    /// Takes an answer, keeping the merged PRs when it had none, and gives
    /// the board's items.
    pub fn take(
        &mut self,
        scope: &str,
        now_ms: i64,
        open: Vec<Value>,
        merged: Option<Vec<Value>>,
    ) -> Vec<Item> {
        if let Some(merged) = merged {
            self.merged = Some((scope.to_string(), now_ms, merged));
        }
        let mut all = open;
        if let Some((_, _, merged)) = &self.merged {
            all.extend(merged.iter().cloned());
        }
        board_from(&all, crate::settings::is_on("ux2"))
    }
}

/// The search scope for the repositories, `repo:a/b repo:c/d`.
pub fn scope_of(repos: &[String]) -> String {
    repos
        .iter()
        .map(|r| format!("repo:{r}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Searches for your open PRs, and the merged ones too when `with_merged`.
pub async fn fetch(
    scope: String,
    with_merged: bool,
    now_ms: i64,
) -> anyhow::Result<(Vec<Value>, Option<Vec<Value>>)> {
    let since = jiff::Timestamp::from_millisecond(now_ms - MERGED_DAYS * 86_400_000)
        .map(|t| t.to_string()[..10].to_string())
        .unwrap_or_default();
    let (query, variables) = if with_merged {
        (
            format!(
                "query($open: String!, $merged: String!) {{ open: search(query: $open, type: ISSUE, first: 50) {{ nodes {{ {FIELDS} }} }} merged: search(query: $merged, type: ISSUE, first: 30) {{ nodes {{ {FIELDS} }} }} }}"
            ),
            json!({
                "open": format!("is:pr is:open author:@me {scope}"),
                "merged": format!("is:pr is:merged author:@me merged:>={since} {scope}"),
            }),
        )
    } else {
        (
            format!(
                "query($open: String!) {{ open: search(query: $open, type: ISSUE, first: 50) {{ nodes {{ {FIELDS} }} }} }}"
            ),
            json!({ "open": format!("is:pr is:open author:@me {scope}") }),
        )
    };
    let data = github::graphql(&query, variables, "sidebar: PR board").await?;
    let nodes = |name: &str| {
        data.pointer(&format!("/{name}/nodes"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    Ok((nodes("open"), with_merged.then(|| nodes("merged"))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(number: u64, extra: Value) -> Value {
        let mut pr = json!({
            "number": number, "title": format!("PR {number}"),
            "url": format!("https://github.com/acme/w/pull/{number}"),
            "isDraft": false, "state": "OPEN", "reviewDecision": null, "mergedAt": null,
            "repository": { "nameWithOwner": "acme/w" }, "mergeQueueEntry": null,
            "commits": { "nodes": [{ "commit": { "statusCheckRollup": { "state": "SUCCESS" } } }] }
        });
        for (k, v) in extra.as_object().unwrap() {
            pr[k] = v.clone();
        }
        pr
    }

    #[test]
    fn each_pr_goes_in_the_column_for_where_it_stands() {
        // lib/sidebar's "the PR board puts each PR in the column for where
        // it stands".
        let failing = json!({ "commits": { "nodes": [{ "commit": { "statusCheckRollup": { "state": "FAILURE" } } }] } });
        let items = board_from(
            &[
                pr(1, json!({ "isDraft": true })),
                pr(2, failing),
                pr(3, json!({ "reviewDecision": "CHANGES_REQUESTED" })),
                pr(4, json!({ "reviewDecision": "APPROVED" })),
                pr(
                    5,
                    json!({ "reviewDecision": "APPROVED", "mergeQueueEntry": { "position": 2 } }),
                ),
                pr(
                    6,
                    json!({ "state": "MERGED", "mergedAt": "2026-10-20T10:00:00Z" }),
                ),
                pr(
                    7,
                    json!({ "state": "MERGED", "mergedAt": "2026-10-21T10:00:00Z" }),
                ),
                pr(2, json!({})),
            ],
            false,
        );
        let laid: Vec<(String, String)> = items
            .iter()
            .map(|i| (i.group.clone().unwrap(), i.title.clone()))
            .collect();
        let want = [
            ("Draft", "#1 PR 1"),
            ("In review", "#2 PR 2"),
            ("In review", "#3 PR 3"),
            ("Approved", "#4 PR 4"),
            ("In merge queue", "#5 PR 5"),
            ("Merged", "#7 PR 7"),
            ("Merged", "#6 PR 6"),
        ];
        assert_eq!(
            laid,
            want.map(|(g, t)| (g.to_string(), t.to_string())),
            "a PR listed twice once, merged ones newest first"
        );
        assert_eq!(
            (items[1].detail.as_str(), items[1].state),
            ("w · ✗ checks", State::Attention)
        );
        assert_eq!(
            (items[2].detail.as_str(), items[2].state),
            ("w · ✓ checks · changes requested", State::Attention)
        );
        assert_eq!(items[4].detail, "w · ✓ checks · queue 2");
        assert_eq!(
            (
                items[4].data["checks"].as_str(),
                items[4].data["meta"].as_str()
            ),
            (Some("success"), Some("w · queue 2"))
        );
        assert_eq!(items[1].data["checks"], "failure");
        assert_eq!(items[5].state, State::Idle);
        assert_eq!(items[0].data["pr"]["number"], 1);
    }

    #[test]
    fn with_ux2_changes_asked_for_have_a_column_and_the_card_its_reviewers_and_description() {
        let items = board_from(
            &[
                pr(1, json!({})),
                pr(
                    2,
                    json!({
                        "reviewDecision": "CHANGES_REQUESTED",
                        "author": { "login": "ana" }, "additions": 214, "deletions": 88,
                        "commitCount": { "totalCount": 3 }, "body": "Lay it out.",
                        "reviewRequests": { "nodes": [{ "requestedReviewer": { "login": "sam" } }, { "requestedReviewer": { "name": "core" } }] },
                        "latestReviews": { "nodes": [{ "author": { "login": "ben" } }, { "author": { "login": "sam" } }] }
                    }),
                ),
                pr(3, json!({ "reviewDecision": "APPROVED" })),
            ],
            true,
        );
        let groups: Vec<_> = items.iter().map(|i| i.group.clone().unwrap()).collect();
        assert_eq!(groups, ["In review", "Changes asked", "Approved"]);
        let d = &items[1].data;
        assert_eq!(d["reviewers"], "sam, core, ben");
        assert_eq!(
            (
                &d["author"],
                &d["commits"],
                &d["additions"],
                &d["deletions"],
                &d["body"]
            ),
            (
                &json!("ana"),
                &json!(3),
                &json!(214),
                &json!(88),
                &json!("Lay it out.")
            )
        );
    }

    #[test]
    fn merged_prs_are_searched_for_only_every_ten_minutes_keeping_the_last_answer_between() {
        // lib/sidebar's "the PR board searches for merged PRs only every ten
        // minutes, keeping the last answer between".
        let scope = scope_of(&["acme/board".to_string()]);
        assert_eq!(scope, "repo:acme/board");
        let now = "2026-10-29T12:00:00Z"
            .parse::<jiff::Timestamp>()
            .unwrap()
            .as_millisecond();
        let mut board = Board::default();
        assert!(board.wants_merged(&scope, now));
        let merged = pr(
            2,
            json!({ "state": "MERGED", "mergedAt": "2026-10-28T00:00:00Z" }),
        );
        board.take(&scope, now, vec![pr(1, json!({}))], Some(vec![merged]));
        assert!(
            !board.wants_merged(&scope, now + 60_000),
            "a minute later, only the open PRs"
        );
        let items = board.take(&scope, now + 60_000, vec![pr(1, json!({}))], None);
        assert!(
            items.iter().any(|i| i.title.contains("PR 2")),
            "the merged PR stays on the board"
        );
        assert!(
            board.wants_merged(&scope, now + MERGED_EVERY.as_millis() as i64),
            "ten minutes on, merged again"
        );
        assert!(
            board.wants_merged("repo:acme/other", now + 60_000),
            "and when the repositories change"
        );
    }
}
