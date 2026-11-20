//! A GitHub issue as the ticket view shows it, for a tab whose ticket is
//! the issue its branch was started for (`views::ticket::Ticket::Issue`):
//! its number and title, who opened it and when, its body, then its
//! comments, oldest first, drawn as the PR view draws a PR's; with its
//! state, labels, assignees and milestone beside them, as UX 2.0's board
//! for a ticket, with UX 2.0 off too.
//!
//! As a Linear ticket's view does, it shows the copy last read at once
//! (`~/.cache/switchboard/ticket-view/`) and reads it again in the
//! background through GitHub's API (`integrations::github`), on `r` and
//! when the view takes the focus after a while (`super::follow`). It
//! fetches no pictures, since an image in a comment would tell its
//! author's server when you read the issue. No tmux-version file is
//! behind it.

use std::process::ExitCode;

use serde_json::{Value, json};

use super::board::{self, Board, Part, Piece};
use super::follow;
use super::pager::{self, Content};
use crate::integrations::github::{self, Repo};
use crate::photos::{Service, Who};
use crate::ui::kit::Tone;
use crate::{paths, util};

const SOURCE: &str = "M-i issue view";
/// Where a diagram that can't be drawn here is shown properly.
pub const PLACE: &str = "M-l on GitHub";

fn text(v: &Value, path: &str) -> String {
    v.pointer(path)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn list<'a>(v: &'a Value, path: &str) -> &'a [Value] {
    v.pointer(path)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn names(v: &Value, path: &str, field: &str) -> Vec<String> {
    list(v, path)
        .iter()
        .map(|x| text(x, field))
        .filter(|n| !n.is_empty())
        .collect()
}

/// Its state in words, and how it is drawn: open, closed as done, or
/// closed as not planned.
fn state(issue: &Value) -> (&'static str, Tone) {
    match (
        text(issue, "/state").as_str(),
        text(issue, "/state_reason").as_str(),
    ) {
        ("open", _) => ("Open", Tone::Needs),
        (_, "not_planned") => ("Closed: not planned", Tone::Failed),
        _ => ("Closed", Tone::Done),
    }
}

/// The comments, oldest first.
fn comments(issue: &Value) -> Vec<&Value> {
    let mut all: Vec<&Value> = list(issue, "/comments_read").iter().collect();
    all.sort_by_key(|c| text(c, "/created_at"));
    all
}

/// What a GitHub login wrote, beside their avatar.
fn said(login: &str, then: String, body: String) -> Part {
    Part::Said {
        who: (!login.is_empty()).then(|| Who::new(Service::GitHub, login)),
        name: login.into(),
        head: vec![
            Piece::Name(login.into(), Tone::Text),
            Piece::Text(then, Tone::Dim),
        ],
        body,
        indent: 0,
    }
}

/// The issue as the ticket's board draws a ticket: its number and title,
/// who opened it, its body and its comments, its facts beside them.
pub fn board(issue: &Value, now: jiff::Timestamp) -> Board {
    let opener = text(issue, "/user/login");
    let body = text(issue, "/body");
    let mut main = vec![
        Part::Row(vec![
            Piece::Name(
                format!(
                    "#{}",
                    util::js_string(issue.get("number").unwrap_or(&Value::Null))
                ),
                Tone::Accent,
            ),
            Piece::Name(format!("  {}", text(issue, "/title")), Tone::Text),
        ]),
        said(
            &opener,
            format!(
                "  opened it {}",
                board::when(&text(issue, "/created_at"), now)
            ),
            String::new(),
        ),
        Part::Rule,
        Part::Blank,
        Part::Text {
            markdown: if body.trim().is_empty() {
                "_No description._".into()
            } else {
                body
            },
            indent: 0,
        },
    ];
    let comments = comments(issue);
    if !comments.is_empty() {
        main.extend([
            Part::Blank,
            Part::Label("Comments".into(), Some(comments.len().to_string())),
        ]);
    }
    for c in comments {
        main.push(Part::Blank);
        main.push(said(
            &text(c, "/user/login"),
            format!("  {}", board::when(&text(c, "/created_at"), now)),
            text(c, "/body"),
        ));
    }

    let fact = |key: &str, value: Piece| {
        Part::Row(vec![Piece::Text(format!("{key:<11}"), Tone::Dim), value])
    };
    let plain = |v: String| Piece::Text(format!(" {v}"), Tone::Text);
    let (words, tone) = state(issue);
    let mut panel = vec![fact("Status", Piece::Chip(words.into(), tone))];
    let assignees = names(issue, "/assignees", "/login");
    panel.push(fact(
        "Assignees",
        plain(if assignees.is_empty() {
            "unassigned".into()
        } else {
            assignees.join(", ")
        }),
    ));
    let labels = names(issue, "/labels", "/name");
    if !labels.is_empty() {
        panel.push(fact("Labels", plain(labels.join(" · "))));
    }
    let milestone = text(issue, "/milestone/title");
    if !milestone.is_empty() {
        panel.push(fact("Milestone", plain(milestone)));
    }
    Board {
        main,
        panel,
        place: PLACE.into(),
        keys: [
            ("j/k", "scroll"),
            ("/", "search"),
            ("r", "read again"),
            ("M-l", "on GitHub"),
            ("M-c", "back to Claude"),
        ]
        .map(|(k, w)| (k.to_string(), w.to_string()))
        .to_vec(),
    }
}

/// What the view shows of an issue: its board, with or without UX 2.0,
/// since it reads each person's words apart: an issue's comments are
/// anyone's, and Markdown read as one document would let one comment's
/// links reach into the next.
fn content(issue: &Value) -> Content {
    Content::Board(board(issue, jiff::Timestamp::now()))
}

/// The most pages of an issue's comments read, a hundred each.
const COMMENT_PAGES: u64 = 10;

/// The issue and its comments (under `comments_read`) from GitHub, or why
/// they couldn't be read.
pub async fn fetch(repo: &Repo, number: u64) -> Result<Value, String> {
    let why = |e: anyhow::Error| format!("Couldn't read {repo}#{number} from GitHub: {e}");
    let mut issue = github::rest(
        "GET",
        &format!("/repos/{repo}/issues/{number}"),
        None,
        SOURCE,
    )
    .await
    .map_err(why)?;
    if !issue.is_object() {
        return Err(format!("GitHub has no issue {repo}#{number}."));
    }
    // GitHub answers a pull request's number here too.
    if issue.get("pull_request").is_some_and(|p| !p.is_null()) {
        return Err(format!(
            "{repo}#{number} is a pull request, not an issue; M-l opens it on GitHub."
        ));
    }
    // Its comments, a hundred a page, as many pages as the count GitHub
    // gives (at least one, at most `COMMENT_PAGES`), so a page answered in
    // full at the end never asks for another without end.
    let pages = issue["comments"]
        .as_u64()
        .unwrap_or(0)
        .div_ceil(100)
        .clamp(1, COMMENT_PAGES);
    let mut comments: Vec<Value> = Vec::new();
    for page in 1..=pages {
        let read = github::rest(
            "GET",
            &format!("/repos/{repo}/issues/{number}/comments?per_page=100&page={page}"),
            None,
            SOURCE,
        )
        .await
        .map_err(why)?;
        let read = read.as_array().cloned().unwrap_or_default();
        let last = read.len() < 100;
        comments.extend(read);
        if last {
            break;
        }
    }
    issue["comments_read"] = json!(comments);
    Ok(issue)
}

fn cache_file(repo: &Repo, number: u64) -> std::path::PathBuf {
    paths::cache_dir()
        .join("ticket-view")
        .join("issues")
        .join(&repo.owner)
        .join(&repo.name)
        .join(format!("{number}.json"))
}

/// The issue view: the last copy at once, then the issue read again.
pub fn main(repo: Repo, number: u64) -> ExitCode {
    // Its words may be private, and this may run before any server has
    // made the cache folder yours alone.
    util::make_private(&[paths::cache_dir()]);
    let cache = cache_file(&repo, number);
    let cached = util::read_json(&cache);
    let initial = match &cached {
        Some(issue) => content(issue),
        None => Content::Loading(format!("Reading {repo}#{number}...")),
    };
    let read = move || util::block_on(fetch(&repo, number));
    let file = move || Some(cache.clone());
    // An issue's body and comments are anyone's words, on an issue no PR
    // stands beside, so their images aren't fetched as it opens.
    pager::run_following_without_pictures(initial, follow::start(cached, file, read, content))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue() -> Value {
        json!({
            "number": 12, "title": "Make the widget blue", "state": "open",
            "body": "It is red.", "user": { "login": "sam" },
            "created_at": "2026-10-01T09:00:00Z", "updated_at": "2026-10-03T10:00:00Z",
            "assignees": [{ "login": "jo" }], "labels": [{ "name": "autopilot" }],
            "milestone": { "title": "v1" },
            "comments_read": [
                { "body": "Found it.", "created_at": "2026-10-03T00:00:00Z", "user": { "login": "sam" } },
                { "body": "Which widget?", "created_at": "2026-10-02T00:00:00Z", "user": { "login": "jo" } }
            ]
        })
    }

    #[test]
    fn the_board_has_the_body_the_comments_and_the_facts_beside() {
        let now = "2026-10-04T12:00:00Z".parse().unwrap();
        let mut closed = issue();
        closed["state"] = json!("closed");
        closed["state_reason"] = json!("not_planned");
        let pager = pager::Pager::new(Content::Board(board(&closed, now)), 130, 40);
        let screen = pager.screen().join("\n");
        for want in [
            "#12  Make the widget blue",
            "It is red.",
            "COMMENTS",
            "Which widget?",
            "Found it.",
            "Closed: not planned",
            "Assignees   jo",
            "Labels      autopilot",
            "Milestone   v1",
        ] {
            assert!(screen.contains(want), "{want:?} in:\n{screen}");
        }
        assert!(pager.frame().contains("M-l"), "its keys on the footer");
    }
}
