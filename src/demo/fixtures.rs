//! The demo's made-up data, and how its services answer with it: GitHub's
//! GraphQL endpoint and REST API, Linear's GraphQL endpoint and Slack's Web
//! API, each through `super::services`, so a request the real service would
//! refuse is refused here too before any of this is asked.
//!
//! Every name in it is made up: the repositories are `acme/…`, the tickets
//! `ABC-…`, the people go by first names, and you are `demo-user`, Alex.
//!
//! The answers are read off the query's text, as little of it as tells one
//! query from another: each lookup the workspace makes asks for its own root
//! fields (`search(`, `pullRequests(headRefName:`, `assignedIssues`,
//! `notifications(`, ...). A PR or a ticket is answered whole, with every
//! field any of the queries asks for, since GraphQL's answer having more
//! than was asked changes nothing for the code reading it; a PR's `commits`
//! carries both the count the PR view reads and the last commit's checks
//! the board reads. A query the demo has no answer for gets GraphQL's error,
//! `NOT_FOUND`, so the view asking it says it found nothing rather than
//! showing someone else's data.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use super::services::{Reply, Request};

/// The made-up token GitHub's stand-in takes.
pub const GITHUB_TOKEN: &str = "demo-github-token";
/// The made-up key Linear's stand-in takes.
pub const LINEAR_KEY: &str = "demo-linear-key";
/// The made-up bot token Slack's stand-in takes.
pub const SLACK_TOKEN: &str = "demo-slack-bot-token";

/// You, in the demo.
pub const LOGIN: &str = "demo-user";
pub const NAME: &str = "Alex";

/// A made-up repository: its name under `acme/`, and what it is about.
pub struct Repo {
    pub name: &'static str,
    pub about: &'static str,
}

pub const REPOS: &[Repo] = &[
    Repo {
        name: "api",
        about: "The HTTP API behind the app",
    },
    Repo {
        name: "web",
        about: "The web app",
    },
    Repo {
        name: "docs",
        about: "The developer guides",
    },
];

/// How a PR's checks stand.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Checks {
    Passing,
    Failing,
    Running,
}

/// A made-up pull request.
pub struct Pr {
    pub repo: &'static str,
    pub number: u64,
    pub title: &'static str,
    pub branch: &'static str,
    /// `OPEN` or `MERGED`.
    pub state: &'static str,
    pub draft: bool,
    /// `APPROVED`, `CHANGES_REQUESTED`, `REVIEW_REQUIRED`.
    pub review: &'static str,
    pub checks: Checks,
    pub queued: Option<u64>,
    pub additions: u64,
    pub deletions: u64,
    pub reviewers: &'static [&'static str],
    pub body: &'static str,
    /// Hours ago it was opened (or merged, once merged).
    pub hours_ago: i64,
}

/// The made-up tabs' branches each have a PR here, and the board has a few
/// more in each column.
pub const PRS: &[Pr] = &[
    Pr {
        repo: "api",
        number: 128,
        title: "Rate-limit the login endpoint",
        branch: "abc-12-rate-limit-login",
        state: "OPEN",
        draft: false,
        review: "REVIEW_REQUIRED",
        checks: Checks::Running,
        queued: None,
        additions: 214,
        deletions: 31,
        reviewers: &["maya"],
        body: "Login attempts are limited to ten a minute for each address, with a `Retry-After` header on the 429.\n\nCloses ABC-12.",
        hours_ago: 3,
    },
    Pr {
        repo: "web",
        number: 342,
        title: "Add a dark mode toggle to settings",
        branch: "abc-15-dark-mode-toggle",
        state: "OPEN",
        draft: false,
        review: "CHANGES_REQUESTED",
        checks: Checks::Failing,
        queued: None,
        additions: 388,
        deletions: 120,
        reviewers: &["theo", "priya"],
        body: "A toggle in Settings › Appearance, kept per account, following the system until it is changed.\n\nCloses ABC-15.",
        hours_ago: 26,
    },
    Pr {
        repo: "docs",
        number: 57,
        title: "Write the API quickstart guide",
        branch: "abc-18-api-guide",
        state: "OPEN",
        draft: false,
        review: "APPROVED",
        checks: Checks::Passing,
        queued: None,
        additions: 96,
        deletions: 4,
        reviewers: &["sam"],
        body: "From an API key to the first request in five minutes, with examples in curl and JavaScript.\n\nCloses ABC-18.",
        hours_ago: 20,
    },
    Pr {
        repo: "api",
        number: 121,
        title: "Move sessions to Redis",
        branch: "abc-9-sessions-redis",
        state: "OPEN",
        draft: true,
        review: "REVIEW_REQUIRED",
        checks: Checks::Passing,
        queued: None,
        additions: 540,
        deletions: 210,
        reviewers: &[],
        body: "Work in progress: sessions kept in Redis rather than the database.",
        hours_ago: 72,
    },
    Pr {
        repo: "api",
        number: 125,
        title: "Paginate the audit log",
        branch: "abc-14-paginate-audit",
        state: "OPEN",
        draft: false,
        review: "APPROVED",
        checks: Checks::Passing,
        queued: Some(1),
        additions: 120,
        deletions: 45,
        reviewers: &["jonas"],
        body: "The audit log in pages of fifty, newest first.",
        hours_ago: 30,
    },
    Pr {
        repo: "web",
        number: 338,
        title: "Fix the avatar upload on Safari",
        branch: "abc-11-safari-avatar",
        state: "MERGED",
        draft: false,
        review: "APPROVED",
        checks: Checks::Passing,
        queued: None,
        additions: 18,
        deletions: 6,
        reviewers: &["priya"],
        body: "Safari sends the file without a type; it is read from its first bytes instead.",
        hours_ago: 22,
    },
];

/// A made-up ticket.
pub struct Ticket {
    pub number: u64,
    pub title: &'static str,
    /// Linear's state type: `unstarted`, `started`, `completed`, `canceled`.
    pub kind: &'static str,
    pub state: &'static str,
    /// 1 urgent … 4 low, 0 none.
    pub priority: u64,
    pub project: &'static str,
    pub description: &'static str,
    pub hours_ago: i64,
}

pub const TICKETS: &[Ticket] = &[
    Ticket {
        number: 12,
        title: "Rate-limit the login endpoint",
        kind: "started",
        state: "In Progress",
        priority: 2,
        project: "Security review",
        description: "Too many attempts from one address should be slowed down, with a clear error.",
        hours_ago: 3,
    },
    Ticket {
        number: 15,
        title: "Dark mode toggle in settings",
        kind: "started",
        state: "In Review",
        priority: 3,
        project: "Settings refresh",
        description: "Let people pick light, dark or the system's appearance.",
        hours_ago: 26,
    },
    Ticket {
        number: 18,
        title: "API quickstart guide",
        kind: "started",
        state: "In Review",
        priority: 3,
        project: "Developer docs",
        description: "A guide from an API key to the first request.",
        hours_ago: 20,
    },
    Ticket {
        number: 20,
        title: "Password reset emails arrive late",
        kind: "unstarted",
        state: "Todo",
        priority: 1,
        project: "Security review",
        description: "Some reset emails take ten minutes; the queue's worker looks stuck.",
        hours_ago: 2,
    },
    Ticket {
        number: 21,
        title: "Export the billing history as CSV",
        kind: "unstarted",
        state: "Todo",
        priority: 4,
        project: "Settings refresh",
        description: "A button on the billing page that downloads every invoice as CSV.",
        hours_ago: 50,
    },
    Ticket {
        number: 11,
        title: "Avatar upload fails on Safari",
        kind: "completed",
        state: "Done",
        priority: 2,
        project: "Settings refresh",
        description: "Safari sends no file type.",
        hours_ago: 22,
    },
    Ticket {
        number: 7,
        title: "Old onboarding checklist",
        kind: "canceled",
        state: "Canceled",
        priority: 0,
        project: "",
        description: "Replaced by the new onboarding.",
        hours_ago: 40,
    },
];

/// An ISO 8601 time `hours` ago.
pub fn ago(hours: i64) -> String {
    ago_minutes(hours * 60)
}

pub fn ago_minutes(minutes: i64) -> String {
    let at = jiff::Timestamp::now() - jiff::SignedDuration::from_mins(minutes);
    at.strftime("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn priority_label(p: u64) -> &'static str {
    match p {
        1 => "Urgent",
        2 => "High",
        3 => "Medium",
        4 => "Low",
        _ => "No priority",
    }
}

/// A PR as GitHub answers it, with every field the workspace's queries ask.
pub fn pr_json(pr: &Pr) -> Value {
    let repo = format!("acme/{}", pr.repo);
    let (rollup, counts, runs) = match pr.checks {
        Checks::Passing => (
            "SUCCESS",
            json!([{ "state": "SUCCESS", "count": 6 }]),
            vec![
                ("test", "COMPLETED", "SUCCESS"),
                ("lint", "COMPLETED", "SUCCESS"),
            ],
        ),
        Checks::Failing => (
            "FAILURE",
            json!([{ "state": "SUCCESS", "count": 4 }, { "state": "FAILURE", "count": 1 }]),
            vec![
                ("test", "COMPLETED", "FAILURE"),
                ("lint", "COMPLETED", "SUCCESS"),
            ],
        ),
        Checks::Running => (
            "PENDING",
            json!([{ "state": "SUCCESS", "count": 3 }, { "state": "IN_PROGRESS", "count": 2 }]),
            vec![
                ("test", "IN_PROGRESS", ""),
                ("lint", "COMPLETED", "SUCCESS"),
            ],
        ),
    };
    let merged = pr.state == "MERGED";
    let nodes: Vec<Value> = runs
        .iter()
        .map(|(name, status, conclusion)| {
            json!({ "__typename": "CheckRun", "name": name, "status": status,
                    "conclusion": if conclusion.is_empty() { Value::Null } else { json!(conclusion) } })
        })
        .collect();
    let review_state = match pr.review {
        "APPROVED" => "APPROVED",
        "CHANGES_REQUESTED" => "CHANGES_REQUESTED",
        _ => "COMMENTED",
    };
    let latest: Vec<Value> = pr
        .reviewers
        .iter()
        .take(if pr.review == "REVIEW_REQUIRED" { 0 } else { 1 })
        .map(|r| json!({ "author": { "login": r }, "state": review_state }))
        .collect();
    let requested: Vec<Value> = pr
        .reviewers
        .iter()
        .skip(latest.len())
        .map(|r| json!({ "requestedReviewer": { "login": r } }))
        .collect();
    let threads: Vec<Value> = if pr.review == "CHANGES_REQUESTED" {
        vec![json!({
            "isResolved": false, "path": "src/settings/Appearance.tsx", "line": 42,
            "comments": { "totalCount": 2, "nodes": [{ "author": { "login": pr.reviewers[0] },
                "body": "This reads the system setting on every render; could it be read once?" }] },
            "latest": { "nodes": [{ "author": { "login": LOGIN }, "body": "Good point, moving it into a hook." }] }
        })]
    } else {
        Vec::new()
    };
    json!({
        "id": format!("PR_demo_{}_{}", pr.repo, pr.number),
        "number": pr.number,
        "title": pr.title,
        "url": format!("https://github.com/{repo}/pull/{}", pr.number),
        "state": pr.state,
        "isDraft": pr.draft,
        "reviewDecision": pr.review,
        "mergedAt": if merged { json!(ago(pr.hours_ago)) } else { Value::Null },
        "mergedBy": if merged { json!({ "login": LOGIN }) } else { Value::Null },
        "createdAt": ago(pr.hours_ago + 2),
        "updatedAt": ago(pr.hours_ago),
        "body": pr.body,
        "additions": pr.additions,
        "deletions": pr.deletions,
        "changedFiles": 4,
        "author": { "login": LOGIN },
        "baseRefName": "main",
        "headRefName": pr.branch,
        "isCrossRepository": false,
        "headRepositoryOwner": { "login": "acme" },
        "repository": { "nameWithOwner": repo },
        "mergeQueueEntry": pr.queued.map(|p| json!({ "position": p })).unwrap_or(Value::Null),
        "mergeable": "MERGEABLE",
        "commitCount": { "totalCount": 3 },
        "commits": { "totalCount": 3, "nodes": [{ "commit": { "oid": "0d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e", "statusCheckRollup": {
            "state": rollup,
            "contexts": {
                "checkRunCountsByState": counts,
                "statusContextCountsByState": [],
                "nodes": nodes,
            }
        } } }] },
        "last": { "nodes": [{ "commit": { "statusCheckRollup": {
            "state": rollup,
            "contexts": { "checkRunCountsByState": counts, "statusContextCountsByState": [], "nodes": nodes }
        } } }] },
        "reviewRequests": { "nodes": requested },
        "latestReviews": { "nodes": latest },
        "reviewThreads": { "pageInfo": { "hasNextPage": false, "endCursor": null }, "nodes": threads },
        "files": { "nodes": [
            { "path": "src/index.ts", "additions": pr.additions / 2, "deletions": pr.deletions / 2 },
            { "path": "test/index.test.ts", "additions": pr.additions - pr.additions / 2, "deletions": pr.deletions - pr.deletions / 2 }
        ] },
        "comments": { "nodes": [] },
    })
}

/// A ticket as Linear answers it, with every field the workspace's queries
/// ask.
pub fn ticket_json(t: &Ticket) -> Value {
    let id = format!("ABC-{}", t.number);
    let done = t.kind == "completed";
    let canceled = t.kind == "canceled";
    json!({
        "id": format!("issue-demo-{}", t.number),
        "identifier": id,
        "title": t.title,
        "description": t.description,
        "url": format!("https://linear.app/acme/issue/{id}"),
        "priority": t.priority,
        "priorityLabel": priority_label(t.priority),
        "estimate": 2,
        "createdAt": ago(t.hours_ago + 24),
        "updatedAt": ago(t.hours_ago),
        "completedAt": if done { json!(ago(t.hours_ago)) } else { Value::Null },
        "canceledAt": if canceled { json!(ago(t.hours_ago)) } else { Value::Null },
        "state": { "name": t.state, "type": t.kind },
        "project": if t.project.is_empty() { Value::Null } else { json!({ "name": t.project }) },
        "assignee": { "name": NAME },
        "creator": { "id": "user-maya", "name": "Maya" },
        "cycle": { "number": 14, "name": "Cycle 14" },
        "labels": { "nodes": [] },
        "parent": null,
        "children": { "nodes": [] },
        "relations": { "nodes": [] },
        "attachments": { "nodes": [] },
        "comments": { "nodes": comments_of(t) },
        "team": { "key": "ABC", "states": { "nodes": [{ "id": "state-done", "name": "Done", "position": 1 }] } },
    })
}

/// A demo ticket's comments: Maya's, and the Inbox's comment on it
/// when it has one, so its board shows that comment marked new.
fn comments_of(t: &Ticket) -> Vec<Value> {
    let mut out = vec![json!({
        "id": format!("comment-{}", t.number), "body": "Looks good to start on.",
        "createdAt": ago(t.hours_ago + 1), "parent": null,
        "user": { "id": "user-maya", "name": "Maya" }, "botActor": null
    })];
    let ticket = format!("ABC-{}", t.number);
    for n in inbox() {
        if n["issue"]["identifier"] == ticket.as_str() && n["comment"].is_object() {
            out.push(json!({
                "id": n["comment"]["id"], "body": n["comment"]["body"],
                "createdAt": n["createdAt"], "parent": null,
                "user": n["actor"], "botActor": null
            }));
        }
    }
    out
}

fn graphql_error(message: &str) -> Reply {
    Reply::json(
        200,
        json!({ "data": null, "errors": [{ "type": "NOT_FOUND", "message": message }] }),
    )
}

fn data(value: Value) -> Reply {
    Reply::json(200, json!({ "data": value }))
}

static REPO_ALIAS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(\w+): repository\(owner: "([^"]+)", name: "([^"]+)"\)"#).unwrap()
});
static PR_ALIAS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(\w+): pullRequest(?:s)?\((?:number: (\d+)|headRefName: "([^"]+)")"#).unwrap()
});

fn find_pr(repo: &str, number: u64) -> Option<&'static Pr> {
    PRS.iter()
        .find(|p| format!("acme/{}", p.repo) == repo && p.number == number)
}

fn prs_on(repo: &str, branch: &str) -> Vec<Value> {
    PRS.iter()
        .filter(|p| format!("acme/{}", p.repo) == repo && p.branch == branch)
        .map(pr_json)
        .collect()
}

/// What GitHub answers, by the request's path and query.
pub fn github(request: &Request) -> Option<Reply> {
    if request.path() != "/graphql" {
        return github_rest(request);
    }
    let query = request.query();
    let vars = request.json()["variables"].clone();
    let var = |name: &str| vars[name].as_str().unwrap_or("").to_string();
    let repo_of_vars = || format!("{}/{}", var("owner"), var("name"));
    if query.contains("mutation") {
        return Some(graphql_error("The demo changes nothing on GitHub."));
    }
    // The tabs' PRs in one query, an alias for each repository and each PR.
    if REPO_ALIAS.is_match(&query) {
        let mut answer = serde_json::Map::new();
        let mut marks: Vec<(usize, bool, regex::Captures)> = Vec::new();
        for c in REPO_ALIAS.captures_iter(&query) {
            marks.push((c.get(0).unwrap().start(), true, c));
        }
        for c in PR_ALIAS.captures_iter(&query) {
            marks.push((c.get(0).unwrap().start(), false, c));
        }
        marks.sort_by_key(|(at, _, _)| *at);
        let mut current: Option<(String, String)> = None;
        for (_, is_repo, c) in marks {
            if is_repo {
                let alias = c[1].to_string();
                let repo = format!("{}/{}", &c[2], &c[3]);
                answer.insert(alias.clone(), json!({}));
                current = Some((alias, repo));
                continue;
            }
            let Some((alias, repo)) = &current else {
                continue;
            };
            let value = match (c.get(2), c.get(3)) {
                (Some(n), _) => n
                    .as_str()
                    .parse()
                    .ok()
                    .and_then(|n| find_pr(repo, n))
                    .map(pr_json)
                    .unwrap_or(Value::Null),
                (_, Some(branch)) => json!({ "nodes": prs_on(repo, branch.as_str()) }),
                _ => Value::Null,
            };
            answer[alias][&c[1]] = value;
        }
        return Some(data(Value::Object(answer)));
    }
    if query.contains("search(") {
        let mut out = serde_json::Map::new();
        for (alias, words) in vars.as_object().into_iter().flatten() {
            let words = words.as_str().unwrap_or("");
            let merged = words.contains("is:merged");
            let repos: Vec<&str> = words
                .split_whitespace()
                .filter_map(|w| w.strip_prefix("repo:"))
                .collect();
            let nodes: Vec<Value> = PRS
                .iter()
                .filter(|p| (p.state == "MERGED") == merged)
                .filter(|p| {
                    repos.is_empty() || repos.contains(&format!("acme/{}", p.repo).as_str())
                })
                .map(pr_json)
                .collect();
            out.insert(
                alias.clone(),
                json!({ "issueCount": nodes.len(), "nodes": nodes }),
            );
        }
        return Some(data(Value::Object(out)));
    }
    if query.contains("pullRequests(headRefName") {
        let nodes = prs_on(&repo_of_vars(), &var("head"));
        let mut out = json!({ "repository": { "pullRequests": { "nodes": nodes } } });
        if query.contains("upstream:") {
            out["upstream"] = json!({ "pullRequests": { "nodes": [] } });
        }
        return Some(data(out));
    }
    if query.contains("pullRequest(number") {
        let number = vars["number"].as_u64().unwrap_or(0);
        let pr = find_pr(&repo_of_vars(), number).map(pr_json);
        return Some(data(json!({ "repository": { "pullRequest": pr } })));
    }
    if query.contains("isFork") {
        return Some(data(json!({ "repository": { "isFork": false } })));
    }
    if query.contains("defaultBranchRef") {
        return Some(data(
            json!({ "repository": { "defaultBranchRef": { "name": "main" } } }),
        ));
    }
    if query.contains("viewer") {
        return Some(data(
            json!({ "viewer": { "login": LOGIN, "name": NAME, "avatarUrl": null } }),
        ));
    }
    Some(graphql_error("The demo has no answer for this query."))
}

fn github_rest(request: &Request) -> Option<Reply> {
    let path = request.path();
    if path == "/user" {
        return Some(Reply::json(
            200,
            json!({ "login": LOGIN, "name": NAME, "id": 1 }),
        ));
    }
    None
}

/// What Linear answers, by the query.
pub fn linear(request: &Request) -> Option<Reply> {
    let query = request.query();
    let vars = request.json()["variables"].clone();
    if query.contains("mutation") {
        // Said plainly, as GitHub's stand-in does: a change reported done
        // would come back undone at the next look.
        return Some(Reply::json(
            200,
            json!({ "data": null, "errors": [{ "message": "The demo changes nothing on Linear.", "extensions": { "code": "INVALID_INPUT", "userPresentableMessage": "The demo changes nothing on Linear." } }] }),
        ));
    }
    if query.contains("assignedIssues") {
        let (open, canceled): (Vec<&Ticket>, Vec<&Ticket>) =
            TICKETS.iter().partition(|t| t.kind != "canceled");
        return Some(data(json!({ "viewer": {
            "assignedIssues": { "nodes": open.iter().map(|t| ticket_json(t)).collect::<Vec<_>>() },
            "canceled": { "nodes": canceled.iter().map(|t| ticket_json(t)).collect::<Vec<_>>() },
        } })));
    }
    if query.contains("notifications(") {
        return Some(data(json!({ "notifications": { "nodes": inbox() } })));
    }
    if query.contains("issue(id") {
        let id = vars["id"].as_str().unwrap_or("").to_ascii_uppercase();
        let issue = TICKETS
            .iter()
            .find(|t| format!("ABC-{}", t.number) == id)
            .map(ticket_json);
        return Some(data(json!({ "issue": issue })));
    }
    if query.contains("viewer") {
        return Some(data(json!({ "viewer": {
            "id": "user-demo", "name": NAME, "displayName": "alex", "email": "alex@example.com",
            "organization": { "urlKey": "acme", "name": "Acme" },
        } })));
    }
    Some(Reply::json(
        200,
        json!({ "data": null, "errors": [{ "message": "The demo has no answer for this query.", "extensions": { "code": "INVALID_INPUT" } }] }),
    ))
}

/// Linear's notifications in the demo: a mention, a comment and an
/// assignment.
pub fn inbox() -> Vec<Value> {
    let issue = |n: u64| {
        let t = TICKETS.iter().find(|t| t.number == n).expect("a ticket");
        json!({ "identifier": format!("ABC-{n}"), "title": t.title, "url": format!("https://linear.app/acme/issue/ABC-{n}") })
    };
    vec![
        json!({ "id": "note-1", "type": "issueCommentMention", "createdAt": ago_minutes(25),
                "readAt": null, "archivedAt": null, "snoozedUntilAt": null, "actor": { "id": "user-priya", "name": "Priya" },
                "issue": issue(15), "comment": { "id": "comment-15-1", "body": "@alex can the toggle default to the system setting?", "url": "https://linear.app/acme/issue/ABC-15#comment-1" } }),
        json!({ "id": "note-2", "type": "issueAssignedToYou", "createdAt": ago_minutes(90),
                "readAt": null, "archivedAt": null, "snoozedUntilAt": null, "actor": { "id": "user-maya", "name": "Maya" },
                "issue": issue(20), "comment": null }),
        json!({ "id": "note-3", "type": "issueNewComment", "createdAt": ago(5),
                "readAt": null, "archivedAt": null, "snoozedUntilAt": null, "actor": { "id": "user-sam", "name": "Sam" },
                "issue": issue(18), "comment": { "id": "comment-18-2", "body": "Added a section on API keys, take a look when you can.", "url": "https://linear.app/acme/issue/ABC-18#comment-2" } }),
    ]
}

/// What Slack's Web API answers: the bot is signed in, and the rest is a
/// method the demo has nothing for.
pub fn slack(request: &Request) -> Option<Reply> {
    let method = request.path().trim_start_matches("/api/");
    Some(match method {
        "auth.test" => Reply::json(
            200,
            json!({ "ok": true, "url": "https://acme-demo.slack.com/", "team": "Acme", "user": "switchboard", "team_id": "T0DEMO", "user_id": "U0BOT" }),
        ),
        _ => Reply::json(200, json!({ "ok": false, "error": "not_in_demo" })),
    })
}

/// The day's calendar, as `/today` saves it (`day::calendar::save_today`):
/// a meeting about to start, one later, one done.
pub fn calendar(now: jiff::Zoned) -> Value {
    let at = |minutes: i64| {
        (now.clone() + jiff::SignedDuration::from_mins(minutes))
            .strftime("%Y-%m-%dT%H:%M:%S%:z")
            .to_string()
    };
    json!({
        "date": now.strftime("%Y-%m-%d").to_string(),
        "events": [
            { "title": "Standup", "start": at(-120), "end": at(-105), "allDay": false, "you": "going",
              "join": "https://meet.example.com/standup", "link": "https://calendar.example.com/event/standup" },
            { "title": "Design review: settings", "start": at(4), "end": at(34), "allDay": false, "you": "going",
              "join": "https://meet.example.com/design-review", "link": "https://calendar.example.com/event/design" },
            { "title": "1:1 with Maya", "start": at(150), "end": at(180), "allDay": false, "you": "maybe",
              "join": "https://meet.example.com/maya", "link": "https://calendar.example.com/event/maya" },
        ]
    })
}

/// What is new on Slack, as `/slack-updates` saves it
/// (`day::slack::save`): a mention, a direct message, a thread.
pub fn slack_updates(now_ms: i64) -> Value {
    let ts = |minutes: i64| format!("{}.000100", (now_ms / 1000) - minutes * 60);
    json!({ "items": [
        { "kind": "mention", "channel": "C0DEMO1", "where": "#eng-web", "user": "U0PRIYA", "from": "Priya",
          "text": "@alex the dark mode PR fails on the snapshot test, I think it's the new colors", "ts": ts(12),
          "link": "https://acme-demo.slack.com/archives/C0DEMO1/p1" },
        { "kind": "dm", "channel": "D0DEMO2", "where": "Maya", "user": "U0MAYA", "from": "Maya",
          "text": "Could you look at ABC-20 today? Customers are asking about reset emails.", "ts": ts(40),
          "link": "https://acme-demo.slack.com/archives/D0DEMO2/p2" },
        { "kind": "thread", "channel": "C0DEMO3", "where": "#docs", "user": "U0SAM", "from": "Sam",
          "text": "Approved the quickstart, merge when ready", "ts": ts(95), "thread": ts(120),
          "link": "https://acme-demo.slack.com/archives/C0DEMO3/p3" },
    ], "replied": [] })
}

#[cfg(test)]
mod tests {
    use super::super::services::{Fake, Service};
    use super::*;

    fn post(url: &str, auth: &str, body: Value) -> (u16, Value) {
        use std::io::{Read, Write};
        let addr = url.trim_start_matches("http://");
        let (host, path) = addr.split_once('/').unwrap();
        let mut stream = std::net::TcpStream::connect(host).unwrap();
        let body = body.to_string();
        write!(
            stream,
            "POST /{path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: test\r\nAuthorization: {auth}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut out = String::new();
        stream.read_to_string(&mut out).unwrap();
        let status = out[9..12].parse().unwrap();
        let json =
            serde_json::from_str(out.split("\r\n\r\n").nth(1).unwrap()).unwrap_or(Value::Null);
        (status, json)
    }

    #[test]
    fn the_tabs_prs_are_answered_by_their_aliases() {
        let fake = Fake::start(
            Service::GitHub {
                token: GITHUB_TOKEN.into(),
            },
            github,
        );
        let keys = vec![
            crate::sources::repo::PrKey::parse("acme/api#128").unwrap(),
            crate::sources::repo::PrKey::parse("acme/web@abc-15-dark-mode-toggle").unwrap(),
        ];
        let (query, aliases) = crate::sources::prs::query_for(&keys);
        let (status, answer) = post(
            &fake.graphql_url(),
            &format!("Bearer {GITHUB_TOKEN}"),
            json!({ "query": query.unwrap(), "variables": {} }),
        );
        assert_eq!(status, 200);
        let facts = crate::sources::prs::answers(&aliases, &answer["data"], &[]);
        assert_eq!(facts.len(), 2, "{answer}");
        let numbers: Vec<u64> = facts.iter().map(|(_, f)| f.number).collect();
        assert!(
            numbers.contains(&128) && numbers.contains(&342),
            "{numbers:?}"
        );
    }

    #[test]
    fn the_board_search_answers_open_and_merged_prs_in_its_repositories() {
        let fake = Fake::start(
            Service::GitHub {
                token: GITHUB_TOKEN.into(),
            },
            github,
        );
        let (_, answer) = post(
            &fake.graphql_url(),
            &format!("Bearer {GITHUB_TOKEN}"),
            json!({ "query": "query($open: String!, $merged: String!) { open: search(query: $open) { nodes { number } } merged: search(query: $merged) { nodes { number } } }",
                    "variables": { "open": "is:pr is:open author:@me repo:acme/web", "merged": "is:pr is:merged author:@me repo:acme/web" } }),
        );
        let numbers = |at: &str| -> Vec<u64> {
            answer["data"][at]["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| n["number"].as_u64().unwrap())
                .collect()
        };
        assert_eq!(numbers("open"), [342]);
        assert_eq!(numbers("merged"), [338]);
    }

    #[test]
    fn linear_answers_the_tickets_and_the_inbox_and_refuses_a_wrong_key() {
        let mut fake = Fake::start(
            Service::Linear {
                key: LINEAR_KEY.into(),
            },
            linear,
        );
        let (_, answer) = post(
            &fake.graphql_url(),
            LINEAR_KEY,
            json!({ "query": "query($since: DateTimeOrDuration!) { viewer { assignedIssues { nodes { identifier } } canceled: assignedIssues { nodes { identifier } } } }", "variables": { "since": "x" } }),
        );
        let items = crate::sources::tickets::board_from(
            answer["data"]["viewer"]["assignedIssues"]["nodes"]
                .as_array()
                .unwrap(),
            &[],
            true,
        );
        assert!(items.iter().any(|i| i.id == "ABC-20"));
        let (_, inbox) = post(
            &fake.graphql_url(),
            LINEAR_KEY,
            json!({ "query": "query { notifications(first: 100) { nodes { id } } }" }),
        );
        let items = crate::sources::inbox::inbox_from(
            inbox["data"]["notifications"]["nodes"].as_array().unwrap(),
            crate::util::now_ms(),
        );
        assert_eq!(items.len(), 3);
        fake.expect_refusals();
        let (status, _) = post(
            &fake.graphql_url(),
            "wrong",
            json!({ "query": "query { viewer { id } }" }),
        );
        assert_eq!(status, 400);
        assert_eq!(fake.refused().len(), 1);
    }

    #[test]
    fn a_query_with_no_answer_says_not_found() {
        let fake = Fake::start(
            Service::GitHub {
                token: GITHUB_TOKEN.into(),
            },
            github,
        );
        let (status, answer) = post(
            &fake.graphql_url(),
            &format!("Bearer {GITHUB_TOKEN}"),
            json!({ "query": "query { somethingElse { id } }" }),
        );
        assert_eq!(status, 200);
        assert_eq!(answer["errors"][0]["type"], "NOT_FOUND");
    }
}
