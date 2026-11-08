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

use std::path::Path;
use std::process::ExitCode;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use super::pager::{self, Content};
use crate::integrations::linear;
use crate::{paths, util};

const SOURCE: &str = "M-i ticket view";
/// Where a diagram that can't be drawn here is shown properly.
const PLACE: &str = "M-l in Linear";

const QUERY: &str = "query($id: String!) { issue(id: $id) { \
    identifier title url description createdAt updatedAt priorityLabel estimate \
    state { name } assignee { name } creator { name } project { name } cycle { number name } \
    labels { nodes { name } } parent { identifier title } \
    children { nodes { identifier title state { name } } } \
    relations { nodes { type relatedIssue { identifier title state { name } } } } \
    attachments { nodes { title url } } \
    comments(first: 100) { nodes { id body createdAt parent { id } user { name } botActor { name } } } } }";

static ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z][A-Za-z0-9]*-[0-9]+$").unwrap());
static IN_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Z][A-Z0-9]*-[0-9]+").unwrap());
static LINEAR_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^|[^(<])<?(https://linear\.app/[^/\s]+/issue/([A-Z][A-Z0-9]*-\d+)[^\s)>]*)>?")
        .unwrap()
});

/// A ticket's identifier, in capitals, when `text` is one (`eng-42`).
pub fn id_of(text: &str) -> Option<String> {
    ID.is_match(text.trim())
        .then(|| text.trim().to_ascii_uppercase())
}

/// The ticket a directory's branch names (linear-ticket.sh's match).
pub fn id_for_dir(dir: &Path) -> Option<String> {
    let place = crate::tabs::git::locate(dir).filter(|p| p.on_branch())?;
    linear::ticket_in(&place.branch)
}

/// The ticket a tab's name gives, in capitals as Linear writes them
/// ("Shaping ABC-123: ...").
pub fn id_in_name(name: &str) -> Option<String> {
    IN_NAME.find(name).map(|m| m.as_str().to_string())
}

/// A bare link to a Linear ticket as a link named by its identifier, which
/// then reads as the identifier, clickable (pr-view.js's `tidy`).
pub fn tidy(text: &str) -> String {
    LINEAR_URL
        .replace_all(text, "${1}[${3}](${2})")
        .into_owned()
}

fn day(iso: &Value) -> String {
    iso.as_str().unwrap_or("").chars().take(10).collect()
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

/// The ticket as Markdown, as pr-view.js's `ticket` laid it out.
pub fn markdown(issue: &Value) -> String {
    let text = |path: &str| name_at(issue, path).unwrap_or_default();
    let mut facts: Vec<String> = Vec::new();
    if let Some(state) = name_at(issue, "/state/name") {
        facts.push(state);
    }
    if let Some(priority) = name_at(issue, "/priorityLabel").filter(|p| p != "No priority") {
        facts.push(priority);
    }
    facts.push(match name_at(issue, "/assignee/name") {
        Some(name) => format!("assigned to {name}"),
        None => "unassigned".into(),
    });
    if let Some(project) = name_at(issue, "/project/name") {
        facts.push(format!("project {project}"));
    }
    if let Some(cycle) = issue.get("cycle").filter(|c| c.is_object()) {
        let named = name_at(cycle, "/name").or_else(|| {
            cycle
                .get("number")
                .filter(|n| !n.is_null())
                .map(util::js_string)
        });
        facts.push(format!("cycle {}", named.unwrap_or_default()));
    }
    if let Some(estimate) = issue.get("estimate").filter(|e| !e.is_null()) {
        facts.push(format!("estimate {}", util::js_string(estimate)));
    }
    let labels: Vec<String> = nodes(issue, "/labels/nodes")
        .iter()
        .filter_map(|l| name_at(l, "/name"))
        .collect();
    if !labels.is_empty() {
        facts.push(labels.join(", "));
    }
    let list = |title: &str, rows: Vec<String>| {
        if rows.is_empty() {
            String::new()
        } else {
            format!("\n\n## {title}\n\n{}", rows.join("\n"))
        }
    };
    let link = |i: &Value| {
        let state = name_at(i, "/state/name")
            .map(|s| format!(" _({s})_"))
            .unwrap_or_default();
        format!(
            "**{}** {}{state}",
            name_at(i, "/identifier").unwrap_or_default(),
            name_at(i, "/title").unwrap_or_default()
        )
    };
    let relation = |kind: &str| match kind {
        "blocks" => "Blocks".to_string(),
        "duplicate" => "Duplicates".to_string(),
        "related" => "Related to".to_string(),
        "similar" => "Similar to".to_string(),
        other => other.to_string(),
    };
    let mut comments = nodes(issue, "/comments/nodes");
    comments.sort_by(|a, b| {
        let at = |c: &Value| {
            c.get("createdAt")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        at(a).cmp(&at(b))
    });
    let comment = |c: &Value, reply: bool| {
        let who = name_at(c, "/user/name")
            .or_else(|| name_at(c, "/botActor/name"))
            .unwrap_or_else(|| "Someone".into());
        let head = format!(
            "{}**{who}** · {}",
            if reply { "↳ " } else { "" },
            day(c.get("createdAt").unwrap_or(&Value::Null))
        );
        let body = tidy(c.get("body").and_then(Value::as_str).unwrap_or(""));
        if reply {
            let quoted: Vec<String> = body.split('\n').map(|l| format!("> {l}")).collect();
            format!("> {head}\n>\n{}", quoted.join("\n"))
        } else {
            format!("{head}\n\n{body}")
        }
    };
    let parent_of = |c: &Value| name_at(c, "/parent/id");
    let threads: Vec<String> = comments
        .iter()
        .filter(|c| parent_of(c).is_none())
        .map(|c| {
            let id = name_at(c, "/id");
            let mut parts = vec![comment(c, false)];
            parts.extend(
                comments
                    .iter()
                    .filter(|r| parent_of(r).is_some() && parent_of(r) == id)
                    .map(|r| comment(r, true)),
            );
            parts.join("\n\n")
        })
        .collect();
    let parent = match (
        name_at(issue, "/parent/identifier"),
        name_at(issue, "/parent/title"),
    ) {
        (Some(id), title) => format!(" · sub-issue of {id} {}", title.unwrap_or_default()),
        _ => String::new(),
    };
    let description = tidy(&text("/description"));
    let mut out = String::new();
    out.push_str(&format!("# {}: {}", text("/identifier"), text("/title")));
    out.push_str(&format!("\n\n_{}_", facts.join(" · ")));
    out.push_str(&format!(
        "\n\n_Opened by {} on {}, updated {}{parent}_",
        name_at(issue, "/creator/name").unwrap_or_else(|| "someone".into()),
        day(issue.get("createdAt").unwrap_or(&Value::Null)),
        day(issue.get("updatedAt").unwrap_or(&Value::Null)),
    ));
    out.push_str("\n\n---\n\n");
    out.push_str(if description.trim().is_empty() {
        "_No description._"
    } else {
        &description
    });
    out.push_str(&list(
        "Sub-issues",
        nodes(issue, "/children/nodes")
            .iter()
            .map(|c| format!("- {}", link(c)))
            .collect(),
    ));
    out.push_str(&list(
        "Related",
        nodes(issue, "/relations/nodes")
            .iter()
            .map(|r| {
                let kind = r.get("type").and_then(Value::as_str).unwrap_or("");
                format!(
                    "- {} {}",
                    relation(kind),
                    link(r.get("relatedIssue").unwrap_or(&Value::Null))
                )
            })
            .collect(),
    ));
    out.push_str(&list(
        "Links",
        nodes(issue, "/attachments/nodes")
            .iter()
            .map(|a| {
                format!(
                    "- [{}]({})",
                    name_at(a, "/title").unwrap_or_default(),
                    name_at(a, "/url").unwrap_or_default()
                )
            })
            .collect(),
    ));
    if !threads.is_empty() {
        out.push_str(&format!(
            "\n\n## Comments\n\n{}",
            threads.join("\n\n---\n\n")
        ));
    }
    out
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

fn cache_file(id: &str) -> std::path::PathBuf {
    paths::cache_dir()
        .join("ticket-view")
        .join(format!("{id}.json"))
}

/// The ticket view: the last copy at once, then the ticket read again.
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
        Some(issue) => Content::Markdown {
            text: markdown(issue),
            place: PLACE.into(),
        },
        None => Content::Loading(format!("Reading {id}...")),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || match util::block_on(fetch(&id)) {
        Ok(issue) => {
            if cached.as_ref() == Some(&issue) {
                return;
            }
            if let Some(parent) = cache.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = util::write_atomic(&cache, util::json_compact(&issue));
            let _ = tx.send(Content::Markdown {
                text: markdown(&issue),
                place: PLACE.into(),
            });
        }
        Err(why) if cached.is_none() => {
            let _ = tx.send(Content::Failed(why));
        }
        Err(_) => {}
    });
    pager::run(initial, Some(rx), None)
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
    fn a_ticket_is_laid_out_as_its_page_in_linear() {
        let md = markdown(&issue());
        let expected = "# AB-1: Make it faster\n\n\
            _In Progress · High · assigned to Jo · project Speed · cycle 7 · estimate 3 · perf, backend_\n\n\
            _Opened by Sam on 2026-10-01, updated 2026-10-03 · sub-issue of AB-0 Speed it all up_\n\n\
            ---\n\n\
            It is slow. See [AB-2](https://linear.app/x/issue/AB-2/slow-thing) for more.\n\n\
            ## Sub-issues\n\n- **AB-3** Cache it _(Todo)_\n\n\
            ## Related\n\n- Blocks **AB-4** Ship\n\n\
            ## Links\n\n- [PR #12](https://github.com/o/r/pull/12)\n\n\
            ## Comments\n\n\
            **Bot** · 2026-10-01\n\nWhere is it slow?\n\n\
            > ↳ **Jo** · 2026-10-02\n>\n> Agreed.\n\n---\n\n\
            **Sam** · 2026-10-03\n\nFound it.";
        assert_eq!(md, expected);
    }

    #[test]
    fn a_bare_ticket_with_nothing_else_still_reads() {
        let md =
            markdown(&json!({ "identifier": "AB-9", "title": "T", "labels": { "nodes": [] } }));
        assert!(md.starts_with("# AB-9: T\n\n_unassigned_\n\n_Opened by someone on , updated _"));
        assert!(md.ends_with("---\n\n_No description._"));
    }

    #[test]
    fn identifiers_are_found_in_arguments_and_tab_names() {
        assert_eq!(id_of("eng-42"), Some("ENG-42".into()));
        assert_eq!(id_of("/tmp/eng-42"), None);
        assert_eq!(
            id_in_name("Shaping ABC-123: a thing"),
            Some("ABC-123".into())
        );
        assert_eq!(id_in_name("my tab"), None);
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
