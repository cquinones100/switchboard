//! Linear as a ticket source: its tickets by their identifiers (`ENG-42`)
//! and its projects (`P-ENG-4`), found in a branch, a tab's name or any
//! text, the tickets and projects of yours to choose a tab's from, and a
//! ticket's link. Linear's API is `integrations::linear`, behind the
//! `linear` setting.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use super::{Lists, Ticket, Yours};
use crate::integrations::linear;
use crate::util;

static ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z][A-Za-z0-9]*-[0-9]+$").unwrap());
static IN_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Z][A-Z0-9]*-[0-9]+").unwrap());
/// A Linear project's ID in a branch, in any case.
static PROJECT_IN_BRANCH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^A-Za-z0-9])(P-[A-Z][A-Z0-9]*-[0-9]+)(?:$|[^A-Za-z0-9])").unwrap()
});
/// A ticket's ID in a branch, in any case, as `linear::ticket_in` finds it.
static TICKET_IN_BRANCH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)[a-z][a-z0-9]*-[0-9]+").unwrap());
/// A ticket's ID at a branch's start.
static LEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^([a-z][a-z0-9]*-\d+)").unwrap());
/// A Linear project's ID, `P-<TEAM>-<n>`.
static PROJECT_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?i)P-[A-Z][A-Z0-9]*-[0-9]+$").unwrap());
static PROJECT_IN_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[^A-Za-z0-9])([Pp]-[A-Z][A-Z0-9]*-[0-9]+)(?:$|[^A-Za-z0-9])").unwrap()
});

/// A ticket's identifier, in capitals, when `text` is one (`eng-42`).
pub fn id_of(text: &str) -> Option<String> {
    ID.is_match(text.trim())
        .then(|| text.trim().to_ascii_uppercase())
}

/// A project as `Ticket::arg` writes it (`project:<id>`), or its bare
/// `P-<TEAM>-<n>`.
pub(super) fn project_arg(text: &str) -> Option<Ticket> {
    if let Some(id) = text.strip_prefix("project:").filter(|id| !id.is_empty()) {
        return Some(Ticket::Project(id.to_string()));
    }
    PROJECT_ID
        .is_match(text)
        .then(|| Ticket::Project(text.to_ascii_uppercase()))
}

/// The ticket a directory's branch names (linear-ticket.sh's match).
pub fn id_for_dir(dir: &Path) -> Option<String> {
    let place = crate::tabs::git::locate(dir).filter(|p| p.on_branch())?;
    id_in_branch(&place.branch)
}

/// The ticket ID a branch's name starts with, in capitals.
pub fn leading_in_branch(branch: &str) -> Option<String> {
    LEADING.captures(branch).map(|m| m[1].to_uppercase())
}

/// A branch's name without the ticket ID it starts with.
pub fn without_leading(branch: &str) -> std::borrow::Cow<'_, str> {
    LEADING.replace(branch, "")
}

/// The Linear ticket a branch names, in capitals, never the end of a
/// project's ID: what the tab's name and M-i read from a branch alike.
pub fn id_in_branch(branch: &str) -> Option<String> {
    branch_ticket(branch).map(|(_, id)| id)
}

/// The first ticket ID a branch names, in capitals, and where: as
/// `linear::ticket_in` finds one, but never the end of a project's ID
/// (`p-abc-4-eng-7` gives ENG-7).
fn branch_ticket(branch: &str) -> Option<(usize, String)> {
    TICKET_IN_BRANCH
        .find_iter(branch)
        .find(|m| {
            let before = &branch[..m.start()];
            let alnum = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
            let whole = !alnum(before.chars().last()) && !alnum(branch[m.end()..].chars().next());
            let project_end = (before.ends_with("p-") || before.ends_with("P-"))
                && !before[..before.len() - 2]
                    .chars()
                    .last()
                    .is_some_and(|c| c.is_ascii_alphanumeric());
            whole && !project_end
        })
        .map(|m| (m.start(), m.as_str().to_ascii_uppercase()))
}

/// The ticket a tab's name gives, in capitals as Linear writes them
/// ("Shaping ABC-123: ..."): a whole word, never the end of a project's ID
/// (`P-ABC-4` names no ticket `ABC-4`).
pub fn id_in_name(name: &str) -> Option<String> {
    ticket_word(name).map(|(_, id)| id)
}

/// Where the first ticket ID in `name` is, and the ID: a word of its own
/// (`feat-ABC-42-fix` gives ABC-42), never part of a longer word or the
/// end of a project's ID (`P-ABC-4`).
fn ticket_word(name: &str) -> Option<(usize, String)> {
    let alnum = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
    IN_NAME
        .find_iter(name)
        .find(|m| {
            let before = &name[..m.start()];
            let project_end = before
                .strip_suffix("P-")
                .or_else(|| before.strip_suffix("p-"))
                .is_some_and(|b| !alnum(b.chars().last()));
            !alnum(before.chars().last()) && !alnum(name[m.end()..].chars().next()) && !project_end
        })
        .map(|m| (m.start(), m.as_str().to_string()))
}

/// What a branch names in Linear: a project's ID (`p-abc-4-review`) or a
/// ticket's, whichever comes first.
pub fn in_branch(branch: &str) -> Option<Ticket> {
    let project = PROJECT_IN_BRANCH
        .captures(branch)
        .and_then(|c| c.get(1))
        .map(|m| (m.start(), Ticket::Project(m.as_str().to_ascii_uppercase())));
    let ticket = branch_ticket(branch).map(|(at, id)| (at, Ticket::Linear(id)));
    match (project, ticket) {
        (Some(p), Some(t)) => Some(if p.0 <= t.0 { p.1 } else { t.1 }),
        (p, t) => p.or(t).map(|(_, found)| found),
    }
}

/// The ticket or project a name gives, whichever comes first.
pub fn in_name(name: &str) -> Option<Ticket> {
    let project = PROJECT_IN_NAME
        .captures(name)
        .and_then(|c| c.get(1))
        .map(|m| (m.start(), Ticket::Project(m.as_str().to_ascii_uppercase())));
    let ticket = ticket_word(name).map(|(at, id)| (at, Ticket::Linear(id)));
    match (project, ticket) {
        (Some(p), Some(t)) => Some(if p.0 < t.0 { p.1 } else { t.1 }),
        (p, t) => p.or(t).map(|(_, found)| found),
    }
}

/// Every ticket ID `text` names, each once, in order.
pub fn ids_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in IN_NAME.find_iter(text) {
        if !out.iter().any(|id| id == m.as_str()) {
            out.push(m.as_str().to_string());
        }
    }
    out
}

/// Linear's workspace for a ticket's link: `LINEAR_WORKSPACE`, else
/// linear.json's `linearWorkspace`, as linear-ticket.sh read it.
fn workspace() -> Option<String> {
    std::env::var("LINEAR_WORKSPACE")
        .ok()
        .filter(|w| !w.trim().is_empty())
        .or_else(|| {
            util::read_json_object(crate::files::file("linear.json"))
                .get("linearWorkspace")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .map(|w| w.trim().to_string())
        .filter(|w| {
            !w.is_empty()
                && w.chars()
                    .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        })
}

/// A ticket's link at the workspace written down, with none asked.
pub(super) fn link_now(id: &str) -> Option<String> {
    workspace().map(|w| format!("https://linear.app/{w}/issue/{id}"))
}

/// A ticket's link where Linear says it is; `None` for no such ticket.
pub(super) async fn ticket_url(id: &str, source: &str) -> Result<Option<String>, String> {
    match linear::ticket(id, source).await {
        Ok(Some(t)) if !t.url.is_empty() => Ok(Some(t.url)),
        Ok(_) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
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

pub(crate) const YOUR_TICKETS: &str = "query($after: String) { viewer { \
    assignedIssues(first: 50, after: $after, orderBy: createdAt) { \
    nodes { identifier title createdAt state { name } } pageInfo { hasNextPage endCursor } } } }";
const YOUR_PROJECTS: &str = "query($after: String) { projects(first: 50, after: $after, orderBy: createdAt, \
    filter: { members: { some: { isMe: { eq: true } } } }) { \
    nodes { id name createdAt status { name } } pageInfo { hasNextPage endCursor } } }";

/// Every page of a list `query` asks for, its connection at `at`, as one
/// answer of the first page's shape: up to twenty pages (a thousand), the
/// picker filtering them on the server's task. A page after the first
/// that fails, or one past the twentieth, ends the list there, and says so.
async fn every_page(
    query: &str,
    at: &str,
    source: &str,
) -> anyhow::Result<(Value, Option<String>)> {
    let mut whole = linear::graphql(query, json!({}), source).await?;
    let mut all: Vec<Value> = nodes(&whole, &format!("{at}/nodes"))
        .into_iter()
        .cloned()
        .collect();
    let mut info = whole.pointer(&format!("{at}/pageInfo")).cloned();
    let mut stopped = None;
    for _ in 1..20 {
        let more = info.as_ref().and_then(|i| {
            (i.get("hasNextPage") == Some(&json!(true)))
                .then(|| i.get("endCursor").cloned())
                .flatten()
        });
        let Some(after) = more else {
            break;
        };
        match linear::graphql(query, json!({ "after": after }), source).await {
            Ok(page) => {
                all.extend(nodes(&page, &format!("{at}/nodes")).into_iter().cloned());
                info = page.pointer(&format!("{at}/pageInfo")).cloned();
            }
            Err(e) => {
                stopped = Some(e.to_string());
                break;
            }
        }
    }
    let more = info
        .as_ref()
        .is_some_and(|i| i.get("hasNextPage") == Some(&json!(true)));
    if stopped.is_none() && more {
        stopped = Some(format!("the first {} are listed", all.len()));
    }
    if let Some(list) = whole.pointer_mut(&format!("{at}/nodes")) {
        *list = Value::Array(all);
    }
    Ok((whole, stopped))
}

/// The tickets assigned to you and the projects you're a member of, most
/// recently created first, with what couldn't be read; an error when
/// nothing could.
pub async fn yours() -> Result<Lists, String> {
    const SOURCE: &str = "choose a tab's ticket";
    let tickets = every_page(YOUR_TICKETS, "/viewer/assignedIssues", SOURCE).await;
    let projects = every_page(YOUR_PROJECTS, "/projects", SOURCE).await;
    let found = yours_in(
        tickets.as_ref().map(|(v, _)| v).unwrap_or(&Value::Null),
        projects.as_ref().map(|(v, _)| v).unwrap_or(&Value::Null),
    );
    let missing: Vec<String> = [("tickets", &tickets), ("projects", &projects)]
        .into_iter()
        .filter_map(|(what, read)| match read {
            Err(e) => Some(format!("Couldn't list your {what}: {e}")),
            Ok((_, Some(e))) => Some(format!("Only some of your {what} are listed: {e}")),
            Ok((_, None)) => None,
        })
        .collect();
    if found.is_empty() && !missing.is_empty() {
        return Err(missing.join("\n"));
    }
    Ok(Lists { found, missing })
}

/// `yours` from Linear's answers.
pub fn yours_in(tickets: &Value, projects: &Value) -> Vec<Yours> {
    let text = |v: &Value, path: &str| name_at(v, path).unwrap_or_default();
    let mut out: Vec<Yours> = nodes(tickets, "/viewer/assignedIssues/nodes")
        .into_iter()
        .filter_map(|t| {
            let id = name_at(t, "/identifier")?;
            Some(Yours {
                row: format!("{id}  {} · {}", text(t, "/title"), text(t, "/state/name")),
                ticket: Ticket::Linear(id),
                created: text(t, "/createdAt"),
            })
        })
        .collect();
    let projects = nodes(projects, "/projects/nodes");
    // Two projects of one name are told apart by the day each was created.
    let shared_name = |name: &str| projects.iter().filter(|p| text(p, "/name") == name).count() > 1;
    out.extend(projects.iter().filter_map(|p| {
        let id = name_at(p, "/id")?;
        let name = text(p, "/name");
        let created = text(p, "/createdAt");
        let day = if shared_name(&name) {
            format!(" · created {}", created.get(..10).unwrap_or(&created))
        } else {
            String::new()
        };
        Some(Yours {
            row: format!("Project  {name} · {}{day}", text(p, "/status/name")),
            ticket: Ticket::Project(id),
            created,
        })
    }));
    out.sort_by(|a, b| b.created.cmp(&a.created));
    out
}

const ASSIGNED: &str = "query { viewer { assignedIssues(first: 100, orderBy: createdAt, \
     filter: { state: { type: { nin: [\"completed\", \"canceled\", \"duplicate\"] } } }) { \
     nodes { identifier title createdAt state { type } } } } }";

/// The open Linear tickets assigned to you, newest first: their IDs and
/// titles, the titles without control characters, since someone else
/// wrote them.
pub async fn assigned(source: &str) -> anyhow::Result<Vec<(String, String)>> {
    let data = linear::graphql(ASSIGNED, json!({}), source).await?;
    Ok(assigned_of(&data))
}

/// `assigned` from Linear's answer.
fn assigned_of(data: &Value) -> Vec<(String, String)> {
    let mut tickets: Vec<(String, String, String)> = data
        .pointer("/viewer/assignedIssues/nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|n| {
            let kind = n.pointer("/state/type").and_then(Value::as_str);
            !matches!(kind, Some("completed" | "canceled" | "duplicate"))
        })
        .filter_map(|n| {
            let text = |k: &str| n.get(k).and_then(Value::as_str).map(str::to_string);
            let title: String = text("title")?.chars().filter(|c| !c.is_control()).collect();
            Some((
                text("createdAt").unwrap_or_default(),
                text("identifier")?,
                title,
            ))
        })
        .collect();
    tickets.sort_by(|a, b| b.0.cmp(&a.0));
    tickets
        .into_iter()
        .map(|(_, id, title)| (id, title))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn your_open_tickets_newest_first_without_closed_ones_or_control_characters() {
        let data = json!({ "viewer": { "assignedIssues": { "nodes": [
            { "identifier": "ENG-1", "title": "Old", "createdAt": "2026-01-01T00:00:00Z", "state": { "type": "started" } },
            { "identifier": "ENG-2", "title": "New\u{1b}[2J", "createdAt": "2026-02-01T00:00:00Z", "state": { "type": "unstarted" } },
            { "identifier": "ENG-3", "title": "Done", "createdAt": "2026-03-01T00:00:00Z", "state": { "type": "completed" } },
        ] } } });
        assert_eq!(
            assigned_of(&data),
            vec![
                ("ENG-2".to_string(), "New[2J".to_string()),
                ("ENG-1".to_string(), "Old".to_string())
            ]
        );
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
        assert_eq!(Ticket::Linear("ENG-4".into()).short(), "ENG-4");
    }

    #[test]
    fn a_name_gives_a_ticket_as_a_whole_word_and_a_project_by_its_id() {
        assert_eq!(id_in_name("Shaping P-ABC-4: x"), None);
        assert_eq!(
            in_name("Shaping P-ABC-4: x"),
            Some(Ticket::Project("P-ABC-4".into()))
        );
        assert_eq!(in_name("ABC-12: x"), Some(Ticket::Linear("ABC-12".into())));
        assert_eq!(
            in_name("P-ABC-4-follow-up"),
            Some(Ticket::Project("P-ABC-4".into()))
        );
        assert_eq!(
            in_name("Shaping ABC-42: follow up on P-ABC-4"),
            Some(Ticket::Linear("ABC-42".into())),
            "the first the name gives"
        );
        assert_eq!(id_in_name("XABC-12x"), None);
        assert_eq!(id_in_name("ABC-42-fix-login"), Some("ABC-42".into()));
        assert_eq!(id_in_name("feat-ABC-42-fix-login"), Some("ABC-42".into()));
        assert_eq!(id_in_name("plan-P-ABC-4-review"), None);
        assert_eq!(
            in_name("Shaping p-ENG-4"),
            Some(Ticket::Project("P-ENG-4".into()))
        );
        assert_eq!(
            in_name("plan-P-ABC-4-review"),
            Some(Ticket::Project("P-ABC-4".into()))
        );
        assert_eq!(
            branch_ticket("p-abc-4-eng-7-fix"),
            Some((8, "ENG-7".into()))
        );
        assert_eq!(branch_ticket("eng-42x-fix"), None);
        assert_eq!(branch_ticket("feature/123eng-42-fix"), None);
        assert_eq!(branch_ticket("me/eng-42"), Some((3, "ENG-42".into())));
        assert_eq!(
            in_branch("p-abc-4-review-cost"),
            Some(Ticket::Project("P-ABC-4".into()))
        );
        assert_eq!(
            in_branch("eng-42-p-abc-4"),
            Some(Ticket::Linear("ENG-42".into()))
        );
    }

    #[test]
    fn your_tickets_and_projects_list_newest_first() {
        let tickets = json!({ "viewer": { "assignedIssues": { "nodes": [
            { "identifier": "AB-1", "title": "Old", "createdAt": "2026-01-01T00:00:00Z", "state": { "name": "Todo" } },
            { "identifier": "AB-2", "title": "New", "createdAt": "2026-03-01T00:00:00Z", "state": { "name": "Done" } }
        ] } } });
        let projects = json!({ "projects": { "nodes": [
            { "id": "p1", "name": "Speed", "createdAt": "2026-02-01T00:00:00Z", "status": { "name": "Backlog" } },
            { "id": "p2", "name": "Speed", "createdAt": "2025-12-01T00:00:00Z", "status": { "name": "Done" } }
        ] } });
        let rows: Vec<String> = yours_in(&tickets, &projects)
            .into_iter()
            .map(|y| format!("{} = {}", y.row, y.ticket.arg()))
            .collect();
        assert_eq!(
            rows,
            [
                "AB-2  New · Done = AB-2",
                "Project  Speed · Backlog · created 2026-02-01 = project:p1",
                "AB-1  Old · Todo = AB-1",
                "Project  Speed · Done · created 2025-12-01 = project:p2"
            ]
        );
    }
}
