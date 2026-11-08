//! The merge queue of each repository your tabs are in, for mission
//! control's Landing view (`landing`), ported from lib/sidebar/queues.js:
//! every pull request in it, everyone's, in queue order, the ones a tab of
//! yours has open marked with that tab. One GraphQL query asks for every
//! repository's default branch's queue, only while the github and
//! view_landing settings are on. A repository GitHub reports an error for
//! keeps its last answer rather than dropping out of the view; one with
//! nothing from before is said above the list rather than taken as having
//! no queue.
//!
//! The tabs are matched as the view is built, so a tab opened or renumbered
//! since the last answer shows at once.

use std::collections::{BTreeMap, HashSet};

use serde_json::{Value, json};

use super::{TabInfo, ms_of, text};
use crate::integrations::github;
use crate::items::{Item, State, Target};

const ENTRY: &str = "position state enqueuedAt estimatedTimeToMerge pullRequest { number title url author { login } }";

/// A repository's merge queue, as GitHub last answered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Queue {
    pub url: String,
    pub entries: Vec<Value>,
}

/// Every repository's queue, and those GitHub couldn't answer for with
/// nothing from before.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Queues {
    pub by_repo: BTreeMap<String, Queue>,
    pub unanswered: Vec<String>,
}

/// What GitHub calls an entry's state, in words.
fn state_words(state: &str) -> String {
    match state {
        "QUEUED" => "queued".into(),
        "AWAITING_CHECKS" => "awaiting checks".into(),
        "MERGEABLE" => "ready to merge".into(),
        "UNMERGEABLE" => "can't merge".into(),
        "LOCKED" => "merging".into(),
        other => other.to_lowercase(),
    }
}

/// About how long until it merges, as GitHub estimates it in seconds.
pub fn eta(seconds: u64) -> String {
    match seconds {
        0 => String::new(),
        s if s < 90 => "merges in ~1m".into(),
        s if s < 5400 => format!("merges in ~{}m", (s as f64 / 60.0).round()),
        s => format!("merges in ~{}h", (s as f64 / 3600.0).round()),
    }
}

/// The view's rows: each repository's queue in order, a tab's PR marked.
pub fn queue_items(by_repo: &BTreeMap<String, Queue>, tabs: &[TabInfo]) -> Vec<Item> {
    let mut items = Vec::new();
    for (repo, queue) in by_repo {
        let mut entries: Vec<&Value> = queue.entries.iter().collect();
        entries.sort_by_key(|e| e.get("position").and_then(Value::as_u64).unwrap_or(0));
        for entry in entries {
            let Some(number) = entry.pointer("/pullRequest/number").and_then(Value::as_u64) else {
                continue;
            };
            let position = entry.get("position").and_then(Value::as_u64).unwrap_or(0);
            let state = text(entry, "/state");
            let author = text(entry, "/pullRequest/author/login");
            let url = text(entry, "/pullRequest/url");
            let tab = tabs.iter().find(|t| {
                t.pr == Some(number) && t.repo.as_ref().is_some_and(|r| r.to_string() == *repo)
            });
            let detail: Vec<String> = [
                repo.split('/').nth(1).unwrap_or("").to_string(),
                state_words(state),
                eta(entry
                    .get("estimatedTimeToMerge")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)),
                author.to_string(),
                tab.map(|t| format!("tab {}", t.index)).unwrap_or_default(),
            ]
            .into_iter()
            .filter(|w| !w.is_empty())
            .collect();
            // A tab's PR that can't merge needs you; a tab's otherwise is
            // yours to watch.
            let colour = match tab {
                Some(_) if state == "UNMERGEABLE" => State::Attention,
                Some(_) => State::Active,
                None => State::Idle,
            };
            let mut item = Item::new(
                url,
                format!(
                    "{position}. #{number} {}",
                    text(entry, "/pullRequest/title")
                ),
            )
            .detail(detail.join(" · "))
            .state(colour)
            .target(Target::Url(url.to_string()))
            .data(json!({
                "queue": {
                    "repo": repo, "number": number, "url": url, "queueUrl": queue.url,
                    "tab": tab.map(|t| t.id.to_string()),
                },
                "search": format!("{repo} {author}"),
            }));
            item.time = ms_of(entry.get("enqueuedAt").and_then(Value::as_str));
            items.push(item);
        }
    }
    items
}

impl Queues {
    /// Takes GitHub's answer for `repos` (each `r<i>` in the query), keeping
    /// the last answer of a repository named in `failed`.
    pub fn take(&mut self, repos: &[String], data: &Value, failed: &HashSet<String>) {
        let mut by_repo = BTreeMap::new();
        let mut unanswered = Vec::new();
        for (i, repo) in repos.iter().enumerate() {
            if failed.contains(&format!("r{i}")) {
                match self.by_repo.get(repo) {
                    Some(kept) => {
                        by_repo.insert(repo.clone(), kept.clone());
                    }
                    None => unanswered.push(repo.clone()),
                }
                continue;
            }
            let Some(queue) = data
                .pointer(&format!("/r{i}/mergeQueue"))
                .filter(|q| q.is_object())
            else {
                continue;
            };
            let entries = queue
                .pointer("/entries/nodes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|e| e.get("pullRequest").is_some_and(Value::is_object))
                .cloned()
                .collect();
            by_repo.insert(
                repo.clone(),
                Queue {
                    url: text(queue, "/url").to_string(),
                    entries,
                },
            );
        }
        self.by_repo = by_repo;
        self.unanswered = unanswered;
    }
}

/// Asks for every repository's queue (sorted, as `r0`, `r1`, ...): the data
/// and the aliases GitHub reported an error for.
pub async fn fetch(repos: &[String]) -> anyhow::Result<(Value, HashSet<String>)> {
    // Owners and names go into the query's text, so only what can be one is
    // let in; the callers' repositories come from parse_remote, which
    // allows nothing else.
    let parts: Vec<String> = repos
        .iter()
        .enumerate()
        .filter_map(|(i, repo)| {
            let (owner, name) = repo.split_once('/')?;
            Some(format!(
                "r{i}: repository(owner: \"{owner}\", name: \"{name}\") {{ mergeQueue {{ url entries(first: 50) {{ nodes {{ {ENTRY} }} }} }} }}"
            ))
        })
        .collect();
    let (data, errors) = github::graphql_partial(
        &format!("query {{ {} }}", parts.join(" ")),
        json!({}),
        "sidebar: merge queues",
    )
    .await?;
    let failed = errors
        .iter()
        .filter_map(|e| e.pointer("/path/0").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    Ok((data, failed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrations::github::Repo;
    use crate::mux::TabId;

    fn tab(id: u32, index: usize, repo: &str, pr: Option<u64>) -> TabInfo {
        let (owner, name) = repo.split_once('/').unwrap();
        TabInfo {
            id: TabId(id),
            index,
            pr,
            repo: Some(Repo {
                owner: owner.into(),
                name: name.into(),
            }),
            ..TabInfo::default()
        }
    }

    #[test]
    fn each_repository_s_queue_in_order_a_tab_s_pr_marked_and_a_failed_answer_keeping_the_last() {
        // lib/sidebar's "the merge queues are each repository's in order,
        // marking a tab's PR, and a repository GitHub fails to answer for
        // keeps its last answer".
        let repos = vec!["acme/noqueue".to_string(), "acme/queued".to_string()];
        let tabs = vec![
            tab(3, 3, "acme/queued", Some(41)),
            tab(4, 4, "acme/noqueue", None),
        ];
        let answer = json!({ "r0": { "mergeQueue": null }, "r1": { "mergeQueue": {
            "url": "https://github.com/acme/queued/queue/main", "entries": { "nodes": [
            { "position": 2, "state": "UNMERGEABLE", "enqueuedAt": "2026-11-03T10:00:00Z", "estimatedTimeToMerge": 600,
              "pullRequest": { "number": 41, "title": "Mine", "url": "https://github.com/acme/queued/pull/41", "author": { "login": "me" } } },
            { "position": 1, "state": "AWAITING_CHECKS", "enqueuedAt": "2026-11-03T09:00:00Z", "estimatedTimeToMerge": 120,
              "pullRequest": { "number": 40, "title": "Theirs", "url": "https://github.com/acme/queued/pull/40", "author": { "login": "someone" } } }
        ] } } } });
        let mut queues = Queues::default();
        queues.take(&repos, &answer, &HashSet::new());
        let items = queue_items(&queues.by_repo, &tabs);
        let rows: Vec<(&str, &str, State)> = items
            .iter()
            .map(|i| (i.title.as_str(), i.detail.as_str(), i.state))
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    "1. #40 Theirs",
                    "queued · awaiting checks · merges in ~2m · someone",
                    State::Idle
                ),
                (
                    "2. #41 Mine",
                    "queued · can't merge · merges in ~10m · me · tab 3",
                    State::Attention
                ),
            ]
        );
        assert_eq!(
            items[1].data["queue"]["queueUrl"],
            "https://github.com/acme/queued/queue/main"
        );
        assert!(
            queue_items(&queues.by_repo, &[])
                .iter()
                .all(|i| i.data["queue"]["tab"].is_null()),
            "the tab is matched as it is built"
        );

        // A repository GitHub failed to answer for keeps its last answer.
        let failed: HashSet<String> = ["r1".to_string()].into();
        queues.take(
            &repos,
            &json!({ "r0": { "mergeQueue": null }, "r1": null }),
            &failed,
        );
        assert_eq!(queue_items(&queues.by_repo, &tabs).len(), 2);
        assert!(queues.unanswered.is_empty());

        queues.by_repo.clear();
        queues.take(
            &repos,
            &json!({ "r0": { "mergeQueue": null }, "r1": null }),
            &failed,
        );
        assert_eq!(
            queues.unanswered,
            ["acme/queued"],
            "with nothing from before, it is said as unanswered"
        );
    }

    #[test]
    fn about_how_long_until_it_merges() {
        assert_eq!(eta(0), "");
        assert_eq!(eta(60), "merges in ~1m");
        assert_eq!(eta(600), "merges in ~10m");
        assert_eq!(eta(7200), "merges in ~2h");
    }
}
