//! A Linear ticket as the ticket view shows it (tmux/ticket-view.sh, and
//! pr-view.js's `ticket`, which laid it out as its page in Linear does):
//! its title, the facts from its side panel, who opened it and when, the
//! description, sub-issues, related tickets and links, then the comments,
//! oldest first, each reply under the comment it answers.
//!
//! The ticket is the one the pane's branch names, else the one the tab's
//! name gives (a "Shaping ABC-123: ..." tab before there is a branch), which
//! the server works out before it opens the pane. As the PR view does, the
//! copy last read is shown at once (`~/.cache/switchboard/ticket-view/`)
//! and the ticket read again in the background, through Linear's API with
//! the key `integrations::linear` finds.
//!
//! The ticket is laid out as board 14 draws it (`board`,
//! through `super::board`): the description, sub-issues and comment
//! threads, with its facts, pull requests, related tickets and links beside
//! them.

use std::process::ExitCode;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use super::board::{self, Board, Part, Piece};
use super::follow;
use super::pager::{self, Content};
use crate::integrations::linear;
use crate::photos::{Service, Who};
use crate::ui::kit::{self, Mark, Tone};
use crate::{paths, util};

const SOURCE: &str = "M-i ticket view";
/// Where a diagram that can't be drawn here is shown properly.
pub const PLACE: &str = "M-l in Linear";

const QUERY: &str = "query($id: String!) { issue(id: $id) { \
    identifier title url description createdAt updatedAt priorityLabel estimate \
    state { name type } assignee { name } creator { id name } project { name } cycle { number name } \
    labels { nodes { name } } parent { identifier title } \
    children { nodes { identifier title url state { name type } } } \
    relations { nodes { type relatedIssue { identifier title url state { name type } } } } \
    attachments { nodes { title url metadata } } \
    comments(first: 100) { nodes { id body createdAt parent { id } user { id name } botActor { name } } } } }";

static LINEAR_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^|[^(<])<?(https://linear\.app/[^/\s]+/issue/([A-Z][A-Z0-9]*-\d+)[^\s)>]*)>?")
        .unwrap()
});

/// The view with no ticket to show, saying `words`.
pub fn none(words: String) -> ExitCode {
    pager::run(Content::Failed(words), None, None)
}

/// A bare link to a Linear ticket as a link named by its identifier, which
/// then reads as the identifier, clickable (pr-view.js's `tidy`).
pub fn tidy(text: &str) -> String {
    LINEAR_URL
        .replace_all(text, "${1}[${3}](${2})")
        .into_owned()
}

fn name_at(v: &Value, path: &str) -> Option<String> {
    v.pointer(path)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn nodes<'a>(v: &'a Value, path: &str) -> Vec<&'a Value> {
    v.pointer(path)
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default()
}

/// A Linear user, for their avatar.
fn who_of(user: &Value) -> Option<Who> {
    name_at(user, "/id").map(|id| Who::new(Service::Linear, &id))
}

/// A ticket's state as the kit marks it, by the kind Linear gives it.
fn state_mark(issue: &Value) -> Mark {
    match name_at(issue, "/state/type").as_deref() {
        Some("completed") => Mark::Passed,
        Some("started") => Mark::Pending,
        Some("canceled") => Mark::Failed,
        _ => Mark::Idle,
    }
}

static PR_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^https://github\.com/[^/]+/[^/]+/pull/(\d+)").unwrap());

/// The ticket as board 14 draws it: its title and who opened it, the
/// description through Markdown, its sub-issues and the comment threads,
/// with its facts, pull requests, related tickets and links beside them.
pub fn board(issue: &Value, now: jiff::Timestamp) -> Board {
    let text = |path: &str| name_at(issue, path).unwrap_or_default();
    let creator = name_at(issue, "/creator/name").unwrap_or_else(|| "Someone".into());
    let mut main = vec![
        Part::Row(vec![
            Piece::Name(text("/identifier"), Tone::Accent),
            Piece::Name(format!("  {}", text("/title")), Tone::Text),
        ]),
        Part::Said {
            who: who_of(issue.get("creator").unwrap_or(&Value::Null)),
            name: creator.clone(),
            head: vec![Piece::Text(
                format!(
                    "{creator} opened it {}",
                    board::when(&text("/createdAt"), now)
                ),
                Tone::Dim,
            )],
            body: String::new(),
            indent: 0,
        },
        Part::Rule,
        Part::Blank,
    ];
    let description = tidy(&text("/description"));
    main.push(Part::Text {
        markdown: if description.trim().is_empty() {
            "_No description._".into()
        } else {
            description
        },
        indent: 0,
    });
    let children = nodes(issue, "/children/nodes");
    if !children.is_empty() {
        let done = children
            .iter()
            .filter(|c| state_mark(c) == Mark::Passed)
            .count();
        main.extend([
            Part::Blank,
            Part::Label(
                "Sub-issues".into(),
                Some(format!("{done} of {} done", children.len())),
            ),
        ]);
        for c in children {
            main.push(Part::Row(vec![
                Piece::Mark(state_mark(c)),
                Piece::Text(" ".into(), Tone::Text),
                Piece::Link(
                    name_at(c, "/identifier").unwrap_or_default(),
                    Tone::Dim,
                    name_at(c, "/url").unwrap_or_default(),
                ),
                Piece::Text(
                    format!("  {}", name_at(c, "/title").unwrap_or_default()),
                    Tone::Text,
                ),
            ]));
        }
    }
    let mut comments = nodes(issue, "/comments/nodes");
    comments.sort_by_key(|c| name_at(c, "/createdAt").unwrap_or_default());
    let parent_of = |c: &Value| name_at(c, "/parent/id");
    if !comments.is_empty() {
        main.extend([
            Part::Blank,
            Part::Label("Comments".into(), Some(comments.len().to_string())),
        ]);
    }
    let comment = |c: &Value, indent: usize| {
        let name = name_at(c, "/user/name")
            .or_else(|| name_at(c, "/botActor/name"))
            .unwrap_or_else(|| "Someone".into());
        Part::Said {
            who: who_of(c.get("user").unwrap_or(&Value::Null)),
            name: name.clone(),
            head: vec![
                Piece::Name(name, Tone::Text),
                Piece::Text(
                    format!(
                        "  {}",
                        board::when(&name_at(c, "/createdAt").unwrap_or_default(), now)
                    ),
                    Tone::Dim,
                ),
            ],
            body: tidy(c.get("body").and_then(Value::as_str).unwrap_or("")),
            indent,
        }
    };
    for c in comments.iter().filter(|c| parent_of(c).is_none()) {
        main.push(Part::Blank);
        main.push(comment(c, 0));
        let id = name_at(c, "/id");
        for r in comments
            .iter()
            .filter(|r| parent_of(r).is_some() && parent_of(r) == id)
        {
            main.push(Part::Blank);
            // A reply's avatar at its parent's words.
            main.push(comment(r, kit::SAID_INDENT));
        }
    }

    let mut panel = Vec::new();
    let fact = |key: &str, value: Piece| {
        Part::Row(vec![Piece::Text(format!("{key:<11}"), Tone::Dim), value])
    };
    if let Some(state) = name_at(issue, "/state/name") {
        let tone = match name_at(issue, "/state/type").as_deref() {
            Some("completed") => Tone::Done,
            Some("started") => Tone::Needs,
            Some("canceled") => Tone::Failed,
            _ => Tone::Dim,
        };
        panel.push(fact("Status", Piece::Chip(state, tone)));
    }
    // A space where a chip has its left end, so every value starts in the
    // same column as the status's.
    let plain = |v: String| Piece::Text(format!(" {v}"), Tone::Text);
    if let Some(p) = name_at(issue, "/priorityLabel").filter(|p| p != "No priority") {
        panel.push(fact("Priority", plain(p)));
    }
    panel.push(fact(
        "Assignee",
        plain(name_at(issue, "/assignee/name").unwrap_or_else(|| "unassigned".into())),
    ));
    if let Some(project) = name_at(issue, "/project/name") {
        panel.push(fact("Project", plain(project)));
    }
    if let Some(cycle) = issue.get("cycle").filter(|c| c.is_object()) {
        let named = name_at(cycle, "/name").or_else(|| {
            cycle
                .get("number")
                .filter(|n| !n.is_null())
                .map(|n| format!("Cycle {}", util::js_string(n)))
        });
        panel.push(fact("Cycle", plain(named.unwrap_or_default())));
    }
    if let Some(estimate) = issue.get("estimate").filter(|e| !e.is_null()) {
        panel.push(fact("Estimate", plain(util::js_string(estimate))));
    }
    let labels: Vec<String> = nodes(issue, "/labels/nodes")
        .iter()
        .filter_map(|l| name_at(l, "/name"))
        .collect();
    if !labels.is_empty() {
        panel.push(fact("Labels", plain(labels.join(" · "))));
    }
    if let Some(parent) = name_at(issue, "/parent/identifier") {
        panel.push(fact("Parent", plain(parent)));
    }
    let attachments = nodes(issue, "/attachments/nodes");
    let (prs, links): (Vec<&Value>, Vec<&Value>) = attachments
        .iter()
        .partition(|a| PR_LINK.is_match(&name_at(a, "/url").unwrap_or_default()));
    if !prs.is_empty() {
        panel.extend([
            Part::Blank,
            Part::Label("Pull requests".into(), Some(prs.len().to_string())),
        ]);
        for a in prs {
            let url = name_at(a, "/url").unwrap_or_default();
            let number = PR_LINK
                .captures(&url)
                .map(|c| c[1].to_string())
                .unwrap_or_default();
            let mut row = vec![
                Piece::Link(format!("#{number}"), Tone::Accent, url.clone()),
                Piece::Text(
                    format!(" {}", name_at(a, "/title").unwrap_or_default()),
                    Tone::Text,
                ),
            ];
            // Linear's GitHub integration keeps the PR's state with it.
            let (mark, said) = match name_at(a, "/metadata/status").as_deref() {
                Some("merged") => (Some(Mark::Passed), "merged"),
                Some("closed") => (Some(Mark::Failed), "closed"),
                Some("draft") => (Some(Mark::Idle), "draft"),
                Some("open" | "inReview" | "approved") => (Some(Mark::Pending), "open"),
                _ => (None, ""),
            };
            if let Some(m) = mark {
                row.extend([
                    Piece::Text("  ".into(), Tone::Text),
                    Piece::Mark(m),
                    Piece::Text(format!(" {said}"), Tone::Dim),
                ]);
            }
            panel.push(Part::Row(row));
        }
    }
    let relations = nodes(issue, "/relations/nodes");
    if !relations.is_empty() {
        panel.extend([
            Part::Blank,
            Part::Label("Related".into(), Some(relations.len().to_string())),
        ]);
        for r in relations {
            let other = r.get("relatedIssue").unwrap_or(&Value::Null);
            let kind = match r.get("type").and_then(Value::as_str) {
                Some("blocks") => "blocks ",
                Some("duplicate") => "duplicates ",
                _ => "",
            };
            let mut row = vec![
                Piece::Mark(state_mark(other)),
                Piece::Text(format!(" {kind}"), Tone::Dim),
                Piece::Link(
                    name_at(other, "/identifier").unwrap_or_default(),
                    Tone::Dim,
                    name_at(other, "/url").unwrap_or_default(),
                ),
                Piece::Text(
                    format!("  {}", name_at(other, "/title").unwrap_or_default()),
                    Tone::Text,
                ),
            ];
            if let Some(state) = name_at(other, "/state/name") {
                row.push(Piece::Text(format!(" · {state}"), Tone::Dim));
            }
            panel.push(Part::Row(row));
        }
    }
    if !links.is_empty() {
        panel.extend([Part::Blank, Part::Label("Links".into(), None)]);
        for a in links {
            let url = name_at(a, "/url").unwrap_or_default();
            let title = name_at(a, "/title").unwrap_or_else(|| url.clone());
            panel.push(Part::Row(vec![Piece::Link(title, Tone::Text, url)]));
        }
    }
    Board {
        main,
        panel,
        place: PLACE.into(),
        keys: [
            ("j/k", "scroll"),
            ("/", "search"),
            ("r", "read again"),
            ("M-l", "in Linear"),
            ("M-v", "its PR"),
            ("M-c", "back to Claude"),
        ]
        .map(|(k, w)| (k.to_string(), w.to_string()))
        .to_vec(),
        actions: Vec::new(),
        title: String::new(),
    }
}

/// What the view shows of a ticket: board 14.
fn content(issue: &Value) -> Content {
    Content::Board(board(issue, jiff::Timestamp::now()))
}

/// The ticket from Linear, or why it couldn't be read.
pub async fn fetch(id: &str) -> Result<Value, String> {
    let data = linear::graphql(QUERY, json!({ "id": id }), SOURCE)
        .await
        .map_err(|e| format!("Couldn't read {id} from Linear: {e}"))?;
    data.get("issue")
        .filter(|i| i.is_object())
        .cloned()
        .ok_or_else(|| format!("Linear has no ticket {id}."))
}

pub(crate) fn cache_file(id: &str) -> std::path::PathBuf {
    paths::cache_dir()
        .join("ticket-view")
        .join(format!("{id}.json"))
}

/// The ticket view: the last copy at once, then the ticket read again, and
/// again as it changes (`follow`).
pub fn main(id: Option<String>) -> ExitCode {
    let Some(id) = id else {
        return pager::run(
            Content::Failed("No Linear ticket in this branch or tab name.".into()),
            None,
            None,
        );
    };
    let cache = cache_file(&id);
    let cached = util::read_json(&cache);
    let initial = match &cached {
        Some(issue) => content(issue),
        None => Content::Loading(format!("Reading {id}...")),
    };
    let read = move || util::block_on(fetch(&id));
    let file = move || Some(cache.clone());
    pager::run_following(initial, follow::start(cached, file, read, content))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue() -> Value {
        json!({
            "identifier": "AB-1", "title": "Make it faster", "url": "https://linear.app/x/issue/AB-1",
            "description": "It is slow. See https://linear.app/x/issue/AB-2/slow-thing for more.",
            "createdAt": "2026-10-01T09:00:00Z", "updatedAt": "2026-10-03T10:00:00Z",
            "priorityLabel": "High", "estimate": 3,
            "state": { "name": "In Progress" }, "assignee": { "name": "Jo" }, "creator": { "name": "Sam" },
            "project": { "name": "Speed" }, "cycle": { "number": 7, "name": null },
            "labels": { "nodes": [{ "name": "perf" }, { "name": "backend" }] },
            "parent": { "identifier": "AB-0", "title": "Speed it all up" },
            "children": { "nodes": [{ "identifier": "AB-3", "title": "Cache it", "state": { "name": "Todo" } }] },
            "relations": { "nodes": [{ "type": "blocks", "relatedIssue": { "identifier": "AB-4", "title": "Ship", "state": null } }] },
            "attachments": { "nodes": [{ "title": "PR #12", "url": "https://github.com/o/r/pull/12" }] },
            "comments": { "nodes": [
                { "id": "c2", "body": "Agreed.", "createdAt": "2026-10-02T00:00:00Z", "parent": { "id": "c1" }, "user": { "name": "Jo" } },
                { "id": "c1", "body": "Where is it slow?", "createdAt": "2026-10-01T12:00:00Z", "parent": null, "user": null, "botActor": { "name": "Bot" } },
                { "id": "c3", "body": "Found it.", "createdAt": "2026-10-03T00:00:00Z", "parent": null, "user": { "name": "Sam" } }
            ] }
        })
    }

    #[test]
    fn the_board_has_sub_issues_threads_and_the_facts_beside() {
        let mut issue = issue();
        issue["state"] = json!({ "name": "In Review", "type": "started" });
        issue["creator"] = json!({ "id": "u-sam", "name": "Sam" });
        issue["children"] = json!({ "nodes": [
            { "identifier": "AB-3", "title": "Cache it", "url": "https://linear.app/x/issue/AB-3", "state": { "name": "Done", "type": "completed" } },
            { "identifier": "AB-5", "title": "Warm it", "state": { "name": "Todo", "type": "unstarted" } }
        ] });
        issue["relations"] = json!({ "nodes": [
            { "type": "blocks", "relatedIssue": { "identifier": "AB-4", "title": "Ship", "state": null } },
            { "type": "related", "relatedIssue": { "identifier": "AB-6", "title": "Tidy", "state": { "name": "Done", "type": "completed" } } }
        ] });
        issue["attachments"] = json!({ "nodes": [
            { "title": "Cache the login", "url": "https://github.com/o/r/pull/12", "metadata": { "status": "merged" } },
            { "title": "Plan", "url": "https://example.com/plan" }
        ] });
        let now = "2026-10-04T12:00:00Z".parse().unwrap();
        let b = board(&issue, now);
        assert!(
            b.wanted(&b.place)
                .contains(&super::super::markdown::Wanted::Photo(Who::new(
                    Service::Linear,
                    "u-sam"
                )))
        );
        let pager = pager::Pager::new(Content::Board(b), 130, 40);
        let screen = pager.screen().join("\n");
        for want in [
            "AB-1  Make it faster",
            "S    Sam opened it 3 days ago",
            "It is slow. See AB-2 for more.",
            "SUB-ISSUES",
            "1 of 2 done",
            "✓ AB-3  Cache it",
            "○ AB-5  Warm it",
            "COMMENTS",
            "  B    Bot  3 days ago",
            "       Where is it slow?",
            "       J    Jo  2 days ago",
            "            Agreed.",
            "  S    Sam  yesterday",
            "       Found it.",
            "Status      In Review",
            "Priority    High",
            "Assignee    Jo",
            "Cycle       Cycle 7",
            "Labels      perf · backend",
            "Parent      AB-0",
            "PULL REQUESTS",
            "#12 Cache the login  ✓ merged",
            "RELATED",
            "○ blocks AB-4  Ship",
            "✓ AB-6  Tidy · Done",
            "LINKS",
            "Plan",
        ] {
            assert!(screen.contains(want), "{want:?} in:\n{screen}");
        }
        assert!(pager.frame().contains("M-l"), "its keys on the footer");
    }

    #[test]
    fn a_bare_linear_link_reads_as_its_identifier() {
        assert_eq!(
            tidy("<https://linear.app/t/issue/AB-1>"),
            "[AB-1](https://linear.app/t/issue/AB-1)"
        );
        assert_eq!(
            tidy("[x](https://linear.app/t/issue/AB-1)"),
            "[x](https://linear.app/t/issue/AB-1)",
            "a link already named stays"
        );
    }
}
