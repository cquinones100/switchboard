//! The built-in `github` plugin: your open pull requests and the ones
//! waiting on your review, as a view of their own and, at its top, three in
//! the sidebar. Ported from lib/sidebar/plugins/github.js, which asked
//! through the gh command and its login; this asks GitHub's GraphQL API
//! directly, with the token `integrations::github` finds.
//!
//! Needs your attention: a review requested of you, or a PR of yours whose
//! checks failed or that has changes requested. Active: yours, otherwise.
//! Drafts are idle.
//!
//! Options: `{ "query": "<extra search terms>" }`, added to both searches,
//! such as `org:my-company`.

use anyhow::Result;
use serde_json::{Value, json};

use crate::integrations::github;
use crate::items::{Item, State, Target};

pub const TITLE: &str = "Pull requests";

const FIELDS: &str = "
  ... on PullRequest {
    number title url isDraft updatedAt reviewDecision
    repository { nameWithOwner }
    commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
  }";

fn query() -> String {
    format!(
        "query($mine: String!, $review: String!) {{
  mine: search(query: $mine, type: ISSUE, first: 30) {{ nodes {{ {FIELDS} }} }}
  review: search(query: $review, type: ISSUE, first: 30) {{ nodes {{ {FIELDS} }} }}
}}"
    )
}

/// One pull request as an item.
pub fn item(pr: &Value, reviewing: bool) -> Option<Item> {
    let url = pr.get("url").and_then(Value::as_str)?;
    let checks = pr
        .pointer("/commits/nodes/0/commit/statusCheckRollup/state")
        .and_then(Value::as_str);
    let failing = matches!(checks, Some("FAILURE" | "ERROR"));
    let decision = pr.get("reviewDecision").and_then(Value::as_str);
    let changes = decision == Some("CHANGES_REQUESTED");
    let why = if reviewing {
        "review requested"
    } else if failing {
        "checks failing"
    } else if changes {
        "changes requested"
    } else if decision == Some("APPROVED") {
        "approved"
    } else if checks == Some("PENDING") {
        "checks running"
    } else {
        "open"
    };
    let state = if reviewing || failing || changes {
        State::Attention
    } else if pr.get("isDraft").and_then(Value::as_bool) == Some(true) {
        State::Idle
    } else {
        State::Active
    };
    let repo = pr
        .pointer("/repository/nameWithOwner")
        .and_then(Value::as_str)
        .unwrap_or("");
    let number = pr.get("number").and_then(Value::as_u64).unwrap_or(0);
    let title = pr.get("title").and_then(Value::as_str).unwrap_or("");
    let mut item = Item::new(url, format!("#{number} {title}"))
        .detail(format!("{repo} · {why}"))
        .state(state)
        .target(Target::Url(url.to_string()))
        .data(json!({ "number": number, "repository": repo }));
    if let Some(t) = pr
        .get("updatedAt")
        .and_then(Value::as_str)
        .and_then(|t| t.parse::<jiff::Timestamp>().ok())
    {
        item.time = Some(t.as_millisecond());
    }
    Some(item)
}

fn rank(state: State) -> u8 {
    match state {
        State::Attention => 0,
        State::Active => 1,
        State::Idle => 2,
    }
}

/// The items from GraphQL's answer: the reviews asked of you first, then
/// yours, the ones needing you first and each kind newest first.
pub fn items_from(data: &Value) -> Vec<Item> {
    let nodes = |key: &str| {
        data.pointer(&format!("/{key}/nodes"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let mut items: Vec<Item> = nodes("review")
        .iter()
        .filter_map(|pr| item(pr, true))
        .chain(nodes("mine").iter().filter_map(|pr| item(pr, false)))
        .collect();
    items.sort_by(|a, b| {
        rank(a.state)
            .cmp(&rank(b.state))
            .then(b.time.unwrap_or(0).cmp(&a.time.unwrap_or(0)))
    });
    items
}

/// Asks GitHub.
pub async fn fetch(options: &Value) -> Result<Vec<Item>> {
    let extra = options
        .get("query")
        .and_then(Value::as_str)
        .filter(|q| !q.is_empty())
        .map(|q| format!(" {q}"))
        .unwrap_or_default();
    let variables = json!({
        "mine": format!("is:pr is:open archived:false author:@me{extra}"),
        "review": format!("is:pr is:open archived:false review-requested:@me{extra}"),
    });
    let data = github::graphql(&query(), variables, "plugin: github").await?;
    Ok(items_from(&data))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(number: u64, extra: Value) -> Value {
        let mut pr = json!({
            "number": number,
            "title": format!("PR {number}"),
            "url": format!("https://github.com/acme/w/pull/{number}"),
            "isDraft": false,
            "updatedAt": format!("2026-10-{:02}T10:00:00Z", number),
            "reviewDecision": null,
            "repository": { "nameWithOwner": "acme/w" },
            "commits": { "nodes": [{ "commit": { "statusCheckRollup": { "state": "SUCCESS" } } }] },
        });
        for (k, v) in extra.as_object().unwrap() {
            pr[k] = v.clone();
        }
        pr
    }

    #[test]
    fn reviews_and_trouble_come_first_then_yours_then_drafts() {
        let data = json!({
            "review": { "nodes": [pr(1, json!({}))] },
            "mine": { "nodes": [
                pr(2, json!({ "isDraft": true })),
                pr(3, json!({ "commits": { "nodes": [{ "commit": { "statusCheckRollup": { "state": "FAILURE" } } }] } })),
                pr(4, json!({ "reviewDecision": "APPROVED" })),
                pr(5, json!({ "reviewDecision": "CHANGES_REQUESTED" })),
                pr(6, json!({ "commits": { "nodes": [{ "commit": { "statusCheckRollup": { "state": "PENDING" } } }] } })),
                json!({}),
            ] },
        });
        let rows: Vec<(String, String, State)> = items_from(&data)
            .into_iter()
            .map(|i| (i.title, i.detail, i.state))
            .collect();
        let row = |t: &str, d: &str, s| (t.to_string(), d.to_string(), s);
        assert_eq!(
            rows,
            vec![
                row("#5 PR 5", "acme/w · changes requested", State::Attention),
                row("#3 PR 3", "acme/w · checks failing", State::Attention),
                row("#1 PR 1", "acme/w · review requested", State::Attention),
                row("#6 PR 6", "acme/w · checks running", State::Active),
                row("#4 PR 4", "acme/w · approved", State::Active),
                row("#2 PR 2", "acme/w · open", State::Idle),
            ]
        );
    }
}
