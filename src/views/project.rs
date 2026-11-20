//! A Linear project as the ticket view shows it, for a tab whose name
//! gives a project's ID (`Shaping P-ABC-4: ...`) or one chosen for it
//! (`views::ticket::Ticket::Project`): its name, its description, and its
//! tickets with their states, with its status, lead and target date beside
//! them, as UX 2.0's board for a ticket, with UX 2.0 off too.
//!
//! As a ticket's view does, it shows the copy last read at once
//! (`~/.cache/switchboard/ticket-view/`) and reads it again through
//! Linear's API in the background, on `r` and when the view takes the
//! focus after a while (`super::follow`). No tmux-version file is behind
//! it.

use std::process::ExitCode;

use serde_json::{Value, json};

use super::board::{Board, Part, Piece};
use super::follow;
use super::pager::{self, Content};
use crate::integrations::linear;
use crate::ui::kit::{Mark, Tone};
use crate::{paths, util};

const SOURCE: &str = "M-i project view";

const QUERY: &str = "query($id: String!) { project(id: $id) { \
    name url description content targetDate status { name type } lead { name } \
    issues(first: 100) { nodes { identifier title url state { name type } } } } }";

fn text(v: &Value, path: &str) -> String {
    v.pointer(path)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn tickets(project: &Value) -> &[Value] {
    project
        .pointer("/issues/nodes")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// What it is about: its content, else its short description.
fn about(project: &Value) -> String {
    let content = text(project, "/content");
    let about = if content.trim().is_empty() {
        text(project, "/description")
    } else {
        content
    };
    if about.trim().is_empty() {
        "_No description._".into()
    } else {
        about
    }
}

/// A ticket's state as the kit marks it, by the kind Linear gives it.
fn mark(ticket: &Value) -> Mark {
    match text(ticket, "/state/type").as_str() {
        "completed" => Mark::Passed,
        "started" => Mark::Pending,
        "canceled" => Mark::Failed,
        _ => Mark::Idle,
    }
}

/// The project as the ticket's board draws a ticket.
pub fn board(project: &Value) -> Board {
    let mut main = vec![
        Part::Row(vec![Piece::Name(text(project, "/name"), Tone::Accent)]),
        Part::Rule,
        Part::Blank,
        Part::Text {
            markdown: about(project),
            indent: 0,
        },
    ];
    let all = tickets(project);
    if !all.is_empty() {
        let done = all.iter().filter(|t| mark(t) == Mark::Passed).count();
        main.extend([
            Part::Blank,
            Part::Label(
                "Tickets".into(),
                // Read a hundred at most, which a count says.
                Some(if all.len() >= 100 {
                    format!("{done} of the first {} done", all.len())
                } else {
                    format!("{done} of {} done", all.len())
                }),
            ),
        ]);
        for t in all {
            main.push(Part::Row(vec![
                Piece::Mark(mark(t)),
                Piece::Text(" ".into(), Tone::Text),
                Piece::Link(text(t, "/identifier"), Tone::Dim, text(t, "/url")),
                Piece::Text(format!("  {}", text(t, "/title")), Tone::Text),
            ]));
        }
    }
    let fact = |key: &str, value: Piece| {
        Part::Row(vec![Piece::Text(format!("{key:<11}"), Tone::Dim), value])
    };
    let plain = |v: String| Piece::Text(format!(" {v}"), Tone::Text);
    let mut panel = Vec::new();
    let status = text(project, "/status/name");
    if !status.is_empty() {
        let tone = match text(project, "/status/type").as_str() {
            "completed" => Tone::Done,
            "started" => Tone::Needs,
            "canceled" => Tone::Failed,
            _ => Tone::Dim,
        };
        panel.push(fact("Status", Piece::Chip(status, tone)));
    }
    let lead = text(project, "/lead/name");
    panel.push(fact(
        "Lead",
        plain(if lead.is_empty() { "none".into() } else { lead }),
    ));
    let target = text(project, "/targetDate");
    if !target.is_empty() {
        panel.push(fact("Target", plain(target)));
    }
    Board {
        main,
        panel,
        place: super::ticket::PLACE.into(),
        keys: [
            ("j/k", "scroll"),
            ("/", "search"),
            ("r", "read again"),
            ("M-l", "in Linear"),
            ("M-c", "back to Claude"),
        ]
        .map(|(k, w)| (k.to_string(), w.to_string()))
        .to_vec(),
        actions: Vec::new(),
        title: String::new(),
    }
}

/// What the view shows of a project: its board, with or without UX 2.0,
/// so its description is read apart from the tickets listed under it.
fn content(project: &Value) -> Content {
    Content::Board(board(project))
}

/// The project from Linear, or why it couldn't be read.
pub async fn fetch(id: &str) -> Result<Value, String> {
    let data = linear::graphql(QUERY, json!({ "id": id }), SOURCE)
        .await
        .map_err(|e| format!("Couldn't read project {id} from Linear: {e}"))?;
    data.get("project")
        .filter(|p| p.is_object())
        .cloned()
        .ok_or_else(|| format!("Linear has no project {id}."))
}

/// The project's link, for M-l.
pub async fn url(id: &str) -> Result<Option<String>, String> {
    Ok(Some(text(&fetch(id).await?, "/url")).filter(|u| !u.is_empty()))
}

fn cache_file(id: &str) -> std::path::PathBuf {
    let name: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    paths::cache_dir()
        .join("ticket-view")
        .join(format!("project-{name}.json"))
}

/// The project view: the last copy at once, then the project read again.
pub fn main(id: String) -> ExitCode {
    // Its words may be private, and this may run before any server has
    // made the cache folder yours alone.
    util::make_private(&[paths::cache_dir()]);
    let cache = cache_file(&id);
    let cached = util::read_json(&cache);
    let initial = match &cached {
        Some(project) => content(project),
        None => Content::Loading(format!("Reading {id}...")),
    };
    let read = move || util::block_on(fetch(&id));
    let file = move || Some(cache.clone());
    // Anyone in the workspace writes a project's words, so their images
    // aren't fetched as it opens, as an issue's aren't.
    pager::run_following_without_pictures(initial, follow::start(cached, file, read, content))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> Value {
        json!({
            "name": "review-loop cost", "description": "Cut what a round costs.",
            "status": { "name": "Backlog", "type": "backlog" }, "lead": { "name": "Sam" },
            "targetDate": "2026-12-01",
            "issues": { "nodes": [
                { "identifier": "AB-1", "title": "Gate lint", "state": { "name": "Done", "type": "completed" } },
                { "identifier": "AB-2", "title": "Benchmark", "state": { "name": "Todo", "type": "unstarted" } }
            ] }
        })
    }

    #[test]
    fn a_project_reads_with_its_facts_and_tickets() {
        let pager = pager::Pager::new(Content::Board(board(&project())), 130, 30);
        let screen = pager.screen().join("\n");
        for want in [
            "review-loop cost",
            "1 of 2 done",
            "AB-2  Benchmark",
            "Lead        Sam",
        ] {
            assert!(screen.contains(want), "{want:?} in:\n{screen}");
        }
    }
}
