//! The link commands over mission control: with a view in front, "Copy a
//! link from this pane" lists what the view shows (a Slack message's links,
//! a PR's, a ticket's description's), each marked with where it came from,
//! never the links of the tab's pane under it.

use crate::common;
use crate::mission::{all_settings, open};

use common::TestServer;
use common::http::{Fake, Reply};
use serde_json::json;

/// Copies a link from M-p with mission control open, waits for the list,
/// and returns it, closing it after.
fn copy_link_list(client: &mut common::TestClient, wait: &str) -> String {
    client.type_text("\x1bp");
    client.wait_text("› ");
    client.type_text("Copy a link from this pane");
    client.wait_text("Copy a link from this pane to the clipboard");
    client.type_text("\r");
    client.wait_text("Copy: ");
    let screen = client.wait_text(wait).join("\n");
    // Esc goes back to M-p, and again closes it.
    client.type_text("\x1b");
    client.wait_text("─ Actions ");
    client.type_text("\x1b");
    client.wait_for("M-p closed", |s| {
        !s.iter().any(|l| l.contains("─ Actions "))
    });
    screen
}

#[test]
fn copy_a_link_on_the_slack_view_lists_its_messages_links_not_the_panes() {
    let secs = switchboard::util::now_ms() / 1000;
    let server = TestServer::start_with_settings(
        &all_settings(json!({ "view_slack": true, "slack_background": false, "photos": false }))
            .to_string(),
    );
    std::fs::create_dir_all(&server.state).unwrap();
    std::fs::write(
        server.state.join("slack.json"),
        json!({ "checked": secs * 1000, "items": [
            { "kind": "dm", "channel": "D1", "from": "Ada", "where": "Ada",
              // As the look saves a link behind words: the words in the
              // text, the link beside it.
              "text": "the spec is at the spec",
              "links": ["[the spec](http://127.0.0.1:9/spec)"],
              "ts": format!("{}.000100", secs - 120) },
            { "kind": "mention", "channel": "C1", "from": "Bo", "where": "#dev",
              "text": "see http://127.0.0.1:9/design please",
              "ts": format!("{}.000100", secs - 60) },
        ] })
        .to_string(),
    )
    .unwrap();
    let mut client = server.attach(200, 40);
    client.wait_text("$");
    client.type_text("printf 'pane http://127.0.0.1:9/pane\\n'\r");
    client.wait_text("pane http://127.0.0.1:9/pane");
    open(&mut client);
    client.type_text("vslack\r");
    client.wait_text("see http://127.0.0.1:9/design");
    let list = copy_link_list(&mut client, "   2/2 ");
    assert!(
        list.contains("the spec · Slack · Ada  http://127.0.0.1:9/spec")
            && list.contains("Slack · #dev · Bo  http://127.0.0.1:9/design"),
        "the messages' links, marked with where they came from:\n{list}"
    );
    assert!(
        !list.contains("http://127.0.0.1:9/pane"),
        "not the pane's:\n{list}"
    );
}

#[test]
fn copy_a_link_on_the_tickets_view_lists_the_chosen_tickets_description_first() {
    let linear = Fake::linear("test-key", |request| {
        if !request.query().contains("assignedIssues") {
            return None;
        }
        let issue = |id: &str, link: &str, priority: u64| {
            json!({
                "identifier": id, "title": format!("Ticket {id}"),
                "description": format!("The plan is at {link}"),
                "url": format!("https://linear.app/acme/issue/{id}"),
                "priority": priority, "priorityLabel": "High",
                "updatedAt": jiff::Timestamp::now().to_string(),
                "completedAt": null, "canceledAt": null,
                "state": { "name": "Todo", "type": "unstarted" }, "project": null,
            })
        };
        Some(Reply::json(
            200,
            json!({ "data": { "viewer": {
                "assignedIssues": { "nodes": [
                    issue("ABC-1", "http://127.0.0.1:9/one", 1),
                    issue("ABC-2", "http://127.0.0.1:9/two", 3),
                ] },
                "canceled": { "nodes": [] },
            } } }),
        ))
    });
    let server = TestServer::start_with_env_and_settings(
        &[
            ("LINEAR_API_KEY", "test-key"),
            ("SWITCHBOARD_LINEAR_API", &linear.graphql_url()),
        ],
        &all_settings(json!({"linear": true, "photos": false})).to_string(),
    );
    let mut client = server.attach(200, 40);
    client.wait_text("$");
    open(&mut client);
    client.type_text("vtickets\r");
    client.wait_text("The plan is at http://127.0.0.1:9/one");
    let list = copy_link_list(&mut client, "   4/4 ");
    let first = list
        .lines()
        .find(|l| l.contains('›') && l.contains("ABC-"))
        .unwrap_or_default();
    assert!(
        first.contains("ABC-1  http://127.0.0.1:9/one"),
        "the chosen ticket's description's link first:\n{list}"
    );
    assert!(
        list.contains("ABC-2  http://127.0.0.1:9/two"),
        "then the other ticket's:\n{list}"
    );
}

#[test]
fn copy_a_link_on_the_prs_view_lists_the_chosen_prs_links_first() {
    let pr = json!({
        "number": 12, "url": "https://github.com/acme/widgets/pull/12",
        "title": "Fix the login", "state": "OPEN", "isDraft": false,
        "baseRefName": "main", "headRefName": "fix-login",
        "headRepositoryOwner": {"login": "acme"}, "reviewDecision": "APPROVED",
        "additions": 30, "deletions": 4, "mergedAt": null,
        "body": "Follows [the design](http://127.0.0.1:9/design).",
        "repository": {"nameWithOwner": "acme/widgets"},
        "commits": {"nodes": []},
    });
    let fake = Fake::github("test-token", move |request| {
        if request.path() != "/graphql" {
            return None;
        }
        let asked = request.text();
        let answer = if asked.contains("pullRequest(number:") {
            json!({"data": {"r0": {"a0": pr}}})
        } else if asked.contains("pullRequests(headRefName") {
            json!({"data": {"repository": {"pullRequests": {"nodes": [pr]}}}})
        } else if asked.contains("search(") {
            json!({"data": {"open": {"nodes": [pr]}, "merged": {"nodes": []}, "search": {"nodes": []}}})
        } else {
            json!({"data": {}})
        };
        Some(Reply::json(200, answer))
    });
    let server = TestServer::start_with_env_and_settings(
        &[
            ("GH_TOKEN", "test-token"),
            ("SWITCHBOARD_GITHUB_GRAPHQL", &fake.graphql_url()),
            ("SWITCHBOARD_GITHUB_API", &fake.url),
        ],
        &all_settings(json!({"github": true, "photos": false})).to_string(),
    );
    let work = server.dir.path().join("widgets");
    std::fs::create_dir_all(&work).unwrap();
    let git = |args: &[&str]| {
        let done = std::process::Command::new("git")
            .arg("-C")
            .arg(&work)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .unwrap();
        assert!(done.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&["symbolic-ref", "HEAD", "refs/heads/fix-login"]);
    git(&["commit", "-q", "--allow-empty", "-m", "first"]);
    git(&["remote", "add", "origin", "git@github.com:acme/widgets.git"]);
    let mut client = server.attach(200, 40);
    client.wait_text("$");
    client.type_text(&format!("cd {}\r", work.display()));
    open(&mut client);
    client.type_text("vprs\r");
    client.wait_text("#12 Fix the login");
    let list = copy_link_list(&mut client, "   2/2 ");
    assert!(
        list.lines()
            .any(|l| l.contains("› the design · PR #12  http://127.0.0.1:9/design")),
        "the PR's description's link first:\n{list}"
    );
    assert!(
        list.contains("PR #12  https://github.com/acme/widgets/pull/12"),
        "then the PR's own:\n{list}"
    );
}
