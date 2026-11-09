//! Every tab's pull request, from GitHub, in one GraphQL query: whether it is
//! open, closed or merged, its place in its merge queue, its checks counted
//! by outcome, its review and its size, as lib/sidebar/prs.js asked them.
//! The sidebar places a tab by these (Build failing, In merge queue, Merged)
//! and its cards show them, from `Server.facts`, which the server fills from
//! the last answer, so a draw never waits on the network.
//!
//! Owners, names, numbers and branches go into the query's text, so only
//! what can be one is let in (`PrKey::safe`). One PR or repository GitHub
//! can't find fails only its own part of the answer, which keeps what was
//! known of it; the rest are read all the same.
//!
//! A branch asked of the project a fork's clone's `upstream` names (a key
//! with `from`) takes only a PR from that fork, its newest: the head
//! repository's owner is asked for with each PR and matched here, so another
//! fork's branch of the same name never counts.

use indexmap::IndexMap;
use serde_json::{Value, json};

use super::repo::PrKey;
use crate::facts::{Checks, Merge, PrFacts, PrState};
use crate::integrations::github;

/// What is asked of each PR: where it stands, and for the cards its checks
/// counted by outcome (one count each, however many checks), its review and
/// its size.
pub const PR_FIELDS: &str = "number url state reviewDecision additions deletions mergeQueueEntry { position } commits(last: 1) { nodes { commit { statusCheckRollup { contexts(first: 1) { checkRunCountsByState { state count } statusContextCountsByState { state count } } } } } }";

/// One PR's place in the query: its repository's alias (`r0`), its own
/// (`a0`), and what it was asked for (by a branch, whose answer is a list,
/// or by its number).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alias {
    pub repo: String,
    pub alias: String,
    pub key: PrKey,
}

/// The query for every key, grouped by repository, each key once; `None`
/// when no key can be asked about.
pub fn query_for(keys: &[PrKey]) -> (Option<String>, Vec<Alias>) {
    let mut by_repo: IndexMap<String, Vec<&PrKey>> = IndexMap::new();
    for key in keys.iter().filter(|k| k.safe()) {
        let wanted = by_repo.entry(key.repo().to_string()).or_default();
        if !wanted.contains(&key) {
            wanted.push(key);
        }
    }
    let mut aliases = Vec::new();
    let parts: Vec<String> = by_repo
        .iter()
        .enumerate()
        .map(|(i, (_, wanted))| {
            let repo = wanted[0].repo();
            let asks: Vec<String> = wanted
                .iter()
                .enumerate()
                .map(|(j, key)| {
                    aliases.push(Alias {
                        repo: format!("r{i}"),
                        alias: format!("a{j}"),
                        key: (*key).clone(),
                    });
                    match key {
                        PrKey::Number { number, .. } => {
                            format!("a{j}: pullRequest(number: {number}) {{ {PR_FIELDS} }}")
                        }
                        PrKey::Branch {
                            branch, from: None, ..
                        } => format!(
                            "a{j}: pullRequests(headRefName: \"{branch}\", first: 1, orderBy: {{ field: CREATED_AT, direction: DESC }}) {{ nodes {{ {PR_FIELDS} }} }}"
                        ),
                        PrKey::Branch {
                            branch,
                            from: Some(_),
                            ..
                        } => format!(
                            "a{j}: pullRequests(headRefName: \"{branch}\", first: 20, orderBy: {{ field: CREATED_AT, direction: DESC }}) {{ nodes {{ {PR_FIELDS} isCrossRepository headRepositoryOwner {{ login }} }} }}"
                        ),
                    }
                })
                .collect();
            format!(
                "r{i}: repository(owner: \"{}\", name: \"{}\") {{ {} }}",
                repo.owner,
                repo.name,
                asks.join(" ")
            )
        })
        .collect();
    let query = (!parts.is_empty()).then(|| format!("query {{ {} }}", parts.join(" ")));
    (query, aliases)
}

// GitHub's check states, in gh pr checks' buckets; anything else is pending.
fn bucket(state: &str) -> usize {
    match state {
        "SUCCESS" | "NEUTRAL" | "COMPLETED" | "SKIPPED" => 0,
        "FAILURE" | "ERROR" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE"
        | "STALE" => 1,
        _ => 2,
    }
}

/// A PR's checks counted by outcome, or `None` when it has no checks.
pub fn checks_of(pr: &Value) -> Option<Checks> {
    let contexts = pr.pointer("/commits/nodes/0/commit/statusCheckRollup/contexts")?;
    if contexts.is_null() {
        return None;
    }
    let mut counts = [0u32; 3];
    for list in ["checkRunCountsByState", "statusContextCountsByState"] {
        for entry in contexts
            .get(list)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let state = entry.get("state").and_then(Value::as_str).unwrap_or("");
            let count = entry.get("count").and_then(Value::as_u64).unwrap_or(0);
            counts[bucket(state)] += count as u32;
        }
    }
    Some(Checks {
        pass: counts[0],
        fail: counts[1],
        pending: counts[2],
    })
}

/// What GitHub said of one PR, as the tab keeps it.
pub fn facts_from(pr: &Value, key: &PrKey) -> Option<PrFacts> {
    let number = pr.get("number").and_then(Value::as_u64)?;
    let state = match pr.get("state").and_then(Value::as_str)? {
        "OPEN" => PrState::Open,
        "CLOSED" => PrState::Closed,
        "MERGED" => PrState::Merged,
        _ => return None,
    };
    let merge = if state == PrState::Merged {
        Some(Merge::Merged)
    } else {
        pr.pointer("/mergeQueueEntry/position")
            .and_then(Value::as_u64)
            .map(|position| Merge::Queued {
                position: position as u32,
            })
    };
    let repo = key.repo().to_string();
    Some(PrFacts {
        number,
        url: pr
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("https://github.com/{repo}/pull/{number}")),
        repo,
        state,
        merge,
        checks: if state == PrState::Merged {
            None
        } else {
            checks_of(pr)
        },
        review_decision: pr
            .get("reviewDecision")
            .and_then(Value::as_str)
            .filter(|r| !r.is_empty())
            .map(str::to_string),
        additions: pr.get("additions").and_then(Value::as_u64).unwrap_or(0),
        deletions: pr.get("deletions").and_then(Value::as_u64).unwrap_or(0),
    })
}

/// The answers in what GitHub gave: each alias it answered, leaving out
/// those an error's `path` names (its repository's alias, or the PR's).
pub fn answers(aliases: &[Alias], data: &Value, errors: &[Value]) -> Vec<(PrKey, PrFacts)> {
    let failed: Vec<String> = errors
        .iter()
        .map(|e| {
            e.get("path")
                .and_then(Value::as_array)
                .map(|p| {
                    p.iter()
                        .take(2)
                        .map(|s| {
                            s.as_str()
                                .map(str::to_string)
                                .unwrap_or_else(|| s.to_string())
                        })
                        .collect::<Vec<_>>()
                        .join(".")
                })
                .unwrap_or_default()
        })
        .collect();
    aliases
        .iter()
        .filter(|a| {
            !failed.contains(&a.repo) && !failed.contains(&format!("{}.{}", a.repo, a.alias))
        })
        .filter_map(|a| {
            let answer = data.get(&a.repo)?.get(&a.alias)?;
            let pr = match &a.key {
                PrKey::Branch { from: None, .. } => answer.pointer("/nodes/0")?,
                // The fork's PR, an open one before the rest, as
                // `github::branch_pr` chooses.
                PrKey::Branch {
                    from: Some(from), ..
                } => {
                    let fork_s: Vec<&Value> = answer
                        .get("nodes")?
                        .as_array()?
                        .iter()
                        .filter(|pr| from_fork(pr, from))
                        .collect();
                    let open = fork_s
                        .iter()
                        .find(|pr| pr.get("state").and_then(Value::as_str) == Some("OPEN"));
                    *open.or_else(|| fork_s.first())?
                }
                PrKey::Number { .. } => answer,
            };
            Some((a.key.clone(), facts_from(pr, &a.key)?))
        })
        .collect()
}

// Whether a PR's head is in `owner`'s fork: from that owner, and from
// another repository than the one it is opened in.
fn from_fork(pr: &Value, owner: &str) -> bool {
    pr.pointer("/headRepositoryOwner/login")
        .and_then(Value::as_str)
        .is_some_and(|login| login.eq_ignore_ascii_case(owner))
        && pr.get("isCrossRepository").and_then(Value::as_bool) != Some(false)
}

/// Asks GitHub about every key, in one query.
pub async fn fetch(keys: Vec<PrKey>) -> anyhow::Result<Vec<(PrKey, PrFacts)>> {
    let (query, aliases) = query_for(&keys);
    let Some(query) = query else {
        return Ok(Vec::new());
    };
    let (data, errors) = github::graphql_partial(&query, json!({}), "sidebar: tab PRs").await?;
    Ok(answers(&aliases, &data, &errors))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> PrKey {
        PrKey::parse(s).unwrap()
    }

    #[test]
    fn the_tabs_prs_are_asked_about_in_one_query_a_gone_worktree_s_by_its_branch() {
        // lib/sidebar's "the tabs' PRs are asked about in one query, a gone
        // worktree's by its branch, and only real names reach it".
        let keys: Vec<PrKey> = [
            "acme/widgets#12",
            "acme/widgets#13",
            "acme/widgets#14",
            "acme/widgets#13",
            "acme/widgets@feat-x",
        ]
        .iter()
        .map(|k| key(k))
        .collect();
        let (query, aliases) = query_for(&keys);
        let query = query.unwrap();
        let shape = regex::Regex::new(
            r#"r0: repository\(owner: "acme", name: "widgets"\) \{ a0: pullRequest\(number: 12\).* a2: pullRequest\(number: 14\).* a3: pullRequests\(headRefName: "feat-x""#,
        )
        .unwrap();
        assert!(shape.is_match(&query), "{query}");
        assert_eq!(query.matches("number: 13)").count(), 1, "each PR once");

        let answer = json!({ "r0": {
            "a0": { "number": 12, "state": "OPEN", "mergeQueueEntry": { "position": 2 } },
            "a1": { "number": 13, "state": "MERGED", "mergeQueueEntry": null },
            "a2": {
                "number": 14, "state": "OPEN", "mergeQueueEntry": null, "reviewDecision": "APPROVED",
                "additions": 340, "deletions": 80,
                "commits": { "nodes": [{ "commit": { "statusCheckRollup": { "contexts": {
                    "checkRunCountsByState": [
                        { "state": "SUCCESS", "count": 10 }, { "state": "SKIPPED", "count": 2 },
                        { "state": "FAILURE", "count": 1 }, { "state": "IN_PROGRESS", "count": 3 }
                    ],
                    "statusContextCountsByState": [{ "state": "PENDING", "count": 0 }]
                } } } }] }
            },
            "a3": { "nodes": [{ "number": 15, "state": "MERGED", "mergeQueueEntry": null }] }
        } });
        let got: std::collections::HashMap<String, PrFacts> = answers(&aliases, &answer, &[])
            .into_iter()
            .map(|(k, f)| (k.to_string(), f))
            .collect();
        let twelve = &got["acme/widgets#12"];
        assert_eq!(twelve.state, PrState::Open);
        assert_eq!(twelve.merge, Some(Merge::Queued { position: 2 }));
        assert_eq!(got["acme/widgets#13"].merge, Some(Merge::Merged));
        assert_eq!(got["acme/widgets#13"].checks, None);
        let fourteen = &got["acme/widgets#14"];
        assert_eq!(
            fourteen.checks,
            Some(Checks {
                pass: 12,
                fail: 1,
                pending: 3
            })
        );
        assert_eq!(fourteen.review_decision.as_deref(), Some("APPROVED"));
        assert_eq!((fourteen.additions, fourteen.deletions), (340, 80));
        assert_eq!(fourteen.merge, None);
        // Found by its branch, PR number and all.
        let gone = &got["acme/widgets@feat-x"];
        assert_eq!((gone.number, gone.merge), (15, Some(Merge::Merged)));
        assert_eq!(gone.url, "https://github.com/acme/widgets/pull/15");

        // Only real names reach the query.
        let mixed: Vec<PrKey> = [
            "acme/widgets#1) { x } #",
            "a\"b/c#2",
            "acme/widgets@x\" }",
            "acme/widgets#7",
        ]
        .iter()
        .filter_map(|k| PrKey::parse(k))
        .collect();
        assert_eq!(
            query_for(&mixed).0.unwrap(),
            format!(
                "query {{ r0: repository(owner: \"acme\", name: \"widgets\") {{ a0: pullRequest(number: 7) {{ {PR_FIELDS} }} }} }}"
            )
        );
        assert_eq!(query_for(&[]).0, None);
    }

    #[test]
    fn a_fork_s_branch_in_upstream_is_only_the_fork_s() {
        let keys = vec![key("me/widgets@feature"), key("acme/widgets@me:feature")];
        let (query, aliases) = query_for(&keys);
        let query = query.unwrap();
        assert!(
            query.contains(r#"r1: repository(owner: "acme", name: "widgets") { a0: pullRequests(headRefName: "feature", first: 20"#)
                && query.contains("isCrossRepository headRepositoryOwner { login }"),
            "{query}"
        );
        let pr = |number: u64, owner: &str, cross: bool| {
            json!({ "number": number, "state": "OPEN", "isCrossRepository": cross,
                    "headRepositoryOwner": { "login": owner },
                    "url": format!("https://github.com/acme/widgets/pull/{number}") })
        };
        // Another fork's, newer, and one from the project's own branch of
        // the name come before the fork's own.
        let answer = json!({
            "r0": { "a0": { "nodes": [] } },
            "r1": { "a0": { "nodes": [pr(52, "someone", true), pr(50, "me", false), pr(41, "Me", true)] } }
        });
        let got = answers(&aliases, &answer, &[]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0.to_string(), "acme/widgets@me:feature");
        assert_eq!(
            (got[0].1.number, got[0].1.repo.as_str()),
            (41, "acme/widgets")
        );
        // With only another fork's, none.
        let answer = json!({ "r1": { "a0": { "nodes": [pr(52, "someone", true)] } } });
        assert!(answers(&aliases, &answer, &[]).is_empty());
        // The fork's open PR before a newer merged one of its own.
        let mut merged = pr(60, "me", true);
        merged["state"] = json!("MERGED");
        let answer = json!({ "r1": { "a0": { "nodes": [merged.clone(), pr(41, "me", true)] } } });
        assert_eq!(answers(&aliases, &answer, &[])[0].1.number, 41);
        // With none open, the newest.
        let answer = json!({ "r1": { "a0": { "nodes": [merged] } } });
        assert_eq!(answers(&aliases, &answer, &[])[0].1.number, 60);
    }

    #[test]
    fn an_error_for_one_pr_or_repository_leaves_the_rest() {
        let keys = vec![key("acme/a#1"), key("acme/a#2"), key("acme/b#3")];
        let (_, aliases) = query_for(&keys);
        let data = json!({
            "r0": { "a0": { "number": 1, "state": "OPEN" }, "a1": null },
            "r1": null
        });
        let errors = vec![
            json!({ "path": ["r0", "a1"], "message": "Could not resolve" }),
            json!({ "path": ["r1"], "message": "Could not resolve to a Repository" }),
        ];
        let got = answers(&aliases, &data, &errors);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0.to_string(), "acme/a#1");
    }
}
