//! Mission control's ticket board, ported from lib/sidebar/ticketboard.js:
//! the Linear issues assigned to you, as columns by where they stand (Todo,
//! In progress, In review (a started state whose name says review), and Done
//! in the last `DONE_DAYS`), from one GraphQL query with the key the ticket
//! scripts use (`integrations::linear`). Todo is by priority, urgent first,
//! the rest most recently changed first, and each card names the tab working
//! on its ticket, when a tab's name has it. With UX 2.0 a fifth group,
//! Closed, holds the tickets canceled in the same days, which the board
//! draws in a row under its columns; the query brings each ticket's
//! description and estimate for the card and the pane beside the board.
//!
//! Closing a ticket from its menu is here too: the state it would move to,
//! its team's first completed state by the team's own order (Done, or what
//! the team calls it), then the move.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use super::{TabInfo, in_columns, ms_of, text};
use crate::integrations::linear;
use crate::items::{Item, State, Target};

pub const COLUMNS: [&str; 4] = ["Todo", "In progress", "In review", "Done"];
/// UX 2.0's groups: the tickets canceled this week after the columns.
pub const COLUMNS_UX2: [&str; 5] = ["Todo", "In progress", "In review", "Done", "Closed"];

/// The board's groups, with UX 2.0 on or off.
pub fn columns(ux2: bool) -> &'static [&'static str] {
    if ux2 { &COLUMNS_UX2 } else { &COLUMNS }
}
pub const DONE_DAYS: i64 = 7;

const QUERY: &str = "query($since: DateTimeOrDuration!) {
  viewer {
    assignedIssues(first: 100, orderBy: updatedAt, filter: { or: [{ state: { type: { in: [\"unstarted\", \"started\"] } } }, { completedAt: { gte: $since } }] }) {
      nodes { identifier title description url priority priorityLabel estimate updatedAt completedAt canceledAt state { name type } project { name } }
    }
    canceled: assignedIssues(first: 50, orderBy: updatedAt, filter: { canceledAt: { gte: $since } }) {
      nodes { identifier title description url priority priorityLabel estimate updatedAt completedAt canceledAt state { name type } project { name } }
    }
  }
}";

fn column_of(issue: &Value) -> &'static str {
    static REVIEW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)review").unwrap());
    match text(issue, "/state/type") {
        "completed" => "Done",
        "canceled" => "Closed",
        "started" if REVIEW.is_match(text(issue, "/state/name")) => "In review",
        "started" => "In progress",
        _ => "Todo",
    }
}

// Whether a tab's name has the ticket, as tab-label.sh gives it, and not
// one whose number only starts the same (AB-3 isn't AB-30).
fn names(tab_name: &str, identifier: &str) -> bool {
    let pattern = format!("(?i)(^|[^a-z0-9]){}([^0-9]|$)", regex::escape(identifier));
    Regex::new(&pattern).is_ok_and(|re| re.is_match(tab_name))
}

/// A board card: the ticket and its title, then its state, priority and
/// project, and the tab working on it. An urgent one needs you.
pub fn item_of(issue: &Value, tabs: &[TabInfo]) -> Option<Item> {
    let identifier = issue.get("identifier").and_then(Value::as_str)?;
    issue.get("state").filter(|s| s.is_object())?;
    let priority = issue.get("priority").and_then(Value::as_u64).unwrap_or(0);
    let tab = tabs.iter().find(|t| names(&t.name, identifier));
    let marks: Vec<String> = [
        text(issue, "/state/name").to_string(),
        if priority > 0 {
            text(issue, "/priorityLabel").to_string()
        } else {
            String::new()
        },
        text(issue, "/project/name").to_string(),
        tab.map(|t| format!("tab {}", t.index)).unwrap_or_default(),
    ]
    .into_iter()
    .filter(|m| !m.is_empty())
    .collect();
    let kind = text(issue, "/state/type");
    let done = kind == "completed" || kind == "canceled";
    let url = text(issue, "/url");
    let mut item = Item::new(
        identifier,
        format!("{identifier} {}", text(issue, "/title")),
    )
    .detail(marks.join(" · "))
    .state(if done {
        State::Idle
    } else if priority == 1 {
        State::Attention
    } else {
        State::Active
    })
    .target(Target::Url(url.to_string()))
    .group(column_of(issue))
    .data(json!({
        "ticket": identifier,
        "url": url,
        "priority": if priority == 0 { 5 } else { priority },
        "priorityLabel": text(issue, "/priorityLabel"),
        "title": text(issue, "/title"),
        "state": text(issue, "/state/name"),
        "project": text(issue, "/project/name"),
        "estimate": issue.get("estimate").and_then(Value::as_f64),
        "description": text(issue, "/description"),
        "tab": tab.map(|t| t.index),
    }));
    item.time = ms_of(
        issue
            .get("completedAt")
            .and_then(Value::as_str)
            .or_else(|| issue.get("canceledAt").and_then(Value::as_str))
            .or_else(|| issue.get("updatedAt").and_then(Value::as_str)),
    );
    Some(item)
}

/// Lays out the issues as the board, with UX 2.0's groups or without.
pub fn board_from(issues: &[Value], tabs: &[TabInfo], ux2: bool) -> Vec<Item> {
    let mut items: Vec<Item> = issues.iter().filter_map(|i| item_of(i, tabs)).collect();
    // Column by column, so the order is a total one; in Todo by priority
    // first.
    items.sort_by_key(|i| {
        let column = COLUMNS_UX2
            .iter()
            .position(|c| i.group.as_deref() == Some(c))
            .unwrap_or(COLUMNS_UX2.len());
        let rank = if column == 0 {
            i.data["priority"].as_u64().unwrap_or(5)
        } else {
            0
        };
        (column, rank, -i.time.unwrap_or(0))
    });
    in_columns(columns(ux2), items)
}

/// Your assigned issues, as Linear has them.
pub async fn fetch(now_ms: i64) -> anyhow::Result<Vec<Value>> {
    let since = jiff::Timestamp::from_millisecond(now_ms - DONE_DAYS * 86_400_000)
        .map(|t| t.to_string())
        .unwrap_or_default();
    let data = linear::graphql(QUERY, json!({ "since": since }), "sidebar: tickets").await?;
    Ok(issues_of(&data))
}

/// The issues of the query's answer: the open and done ones, then the
/// canceled ones, asked for apart so they never take an open one's place
/// in the first hundred.
fn issues_of(data: &Value) -> Vec<Value> {
    ["/viewer/assignedIssues/nodes", "/viewer/canceled/nodes"]
        .iter()
        .filter_map(|at| data.pointer(at).and_then(Value::as_array))
        .flatten()
        .cloned()
        .collect()
}

/// The state closing a ticket moves it to: `(issue id, already done, state
/// id, state name)`, or `None` when Linear has no such ticket or its team no
/// completed state.
pub async fn done_state_of(
    identifier: &str,
) -> anyhow::Result<Option<(String, bool, String, String)>> {
    let data = linear::graphql(
        "query($id: String!) { issue(id: $id) { id state { type } team { states(filter: { type: { eq: \"completed\" } }) { nodes { id name position } } } } }",
        json!({ "id": identifier }),
        "sidebar: a ticket's state",
    )
    .await?;
    let Some(issue) = data.get("issue").filter(|i| i.is_object()) else {
        return Ok(None);
    };
    let mut states: Vec<&Value> = issue
        .pointer("/team/states/nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    states.sort_by(|a, b| {
        let p = |s: &Value| s.get("position").and_then(Value::as_f64).unwrap_or(0.0);
        p(a).total_cmp(&p(b))
    });
    Ok(states.first().map(|state| {
        (
            text(issue, "/id").to_string(),
            text(issue, "/state/type") == "completed",
            text(state, "/id").to_string(),
            text(state, "/name").to_string(),
        )
    }))
}

/// Moves a ticket to a state; whether Linear says it did.
pub async fn close_ticket(issue: &str, state: &str) -> anyhow::Result<bool> {
    let data = linear::graphql(
        "mutation($id: String!, $state: String!) { issueUpdate(id: $id, input: { stateId: $state }) { success } }",
        json!({ "id": issue, "state": state }),
        "sidebar: a ticket's state",
    )
    .await?;
    Ok(data
        .pointer("/issueUpdate/success")
        .and_then(Value::as_bool)
        == Some(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canceled_tickets_come_apart_from_the_open_ones() {
        let data = json!({ "viewer": {
            "assignedIssues": { "nodes": [{ "identifier": "AB-1" }] },
            "canceled": { "nodes": [{ "identifier": "AB-2" }] },
        } });
        let issues = issues_of(&data);
        let ids: Vec<&str> = issues
            .iter()
            .filter_map(|i| i["identifier"].as_str())
            .collect();
        assert_eq!(ids, ["AB-1", "AB-2"]);
        assert!(issues_of(&json!({})).is_empty());
    }
    use crate::mux::TabId;

    fn issue(id: &str, priority: u64, name: &str, kind: &str, updated: &str) -> Value {
        let labels = ["No priority", "Urgent", "High", "Medium", "Low"];
        json!({
            "identifier": id, "title": format!("Work on {id}"),
            "url": format!("https://linear.app/x/issue/{id}"), "priority": priority,
            "priorityLabel": labels[priority as usize], "updatedAt": updated,
            "completedAt": if kind == "completed" { Value::from(updated) } else { Value::Null },
            "state": { "name": name, "type": kind }, "project": null
        })
    }

    #[test]
    fn your_issues_in_columns_by_state_urgent_first_in_todo_with_the_tab_working_on_each() {
        // lib/sidebar's "the ticket board puts your Linear issues in columns
        // by state, urgent first in Todo, with the tab working on each".
        let tabs = vec![
            TabInfo {
                id: TabId(4),
                index: 4,
                name: "AB-3: Work on AB-3".into(),
                ..TabInfo::default()
            },
            TabInfo {
                id: TabId(5),
                index: 5,
                name: "AB-30: something else".into(),
                ..TabInfo::default()
            },
        ];
        let items = board_from(
            &[
                issue("AB-1", 3, "Todo", "unstarted", "2026-10-01T00:00:00Z"),
                issue("AB-2", 1, "Todo", "unstarted", "2026-09-01T00:00:00Z"),
                issue("AB-3", 0, "In Progress", "started", "2026-10-02T00:00:00Z"),
                issue(
                    "AB-4",
                    2,
                    "Ready for Review",
                    "started",
                    "2026-10-03T00:00:00Z",
                ),
                issue("AB-5", 0, "Done", "completed", "2026-10-04T00:00:00Z"),
            ],
            &tabs,
            false,
        );
        let laid: Vec<(&str, &str)> = items
            .iter()
            .map(|i| {
                (
                    i.group.as_deref().unwrap(),
                    i.title.split(' ').next().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            laid,
            vec![
                ("Todo", "AB-2"),
                ("Todo", "AB-1"),
                ("In progress", "AB-3"),
                ("In review", "AB-4"),
                ("Done", "AB-5"),
            ]
        );
        assert_eq!(items[0].state, State::Attention, "urgent needs you");
        assert_eq!(items[2].detail, "In Progress · tab 4");
        assert_eq!(items[1].detail, "Todo · Medium");
        assert_eq!(items[4].state, State::Idle);
    }

    #[test]
    fn with_ux2_the_tickets_canceled_this_week_are_closed_after_the_columns_with_their_description()
    {
        let mut canceled = issue("AB-6", 0, "Duplicate", "canceled", "2026-10-05T00:00:00Z");
        canceled["canceledAt"] = json!("2026-10-05T00:00:00Z");
        let mut todo = issue("AB-1", 2, "Todo", "unstarted", "2026-10-01T00:00:00Z");
        todo["description"] = json!("## Steps\nOpen the menu.");
        todo["estimate"] = json!(2);
        let issues = [canceled, todo];
        let items = board_from(&issues, &[], true);
        let groups: Vec<_> = items.iter().map(|i| i.group.clone().unwrap()).collect();
        assert_eq!(groups, ["Todo", "Closed"]);
        assert_eq!(items[0].data["description"], "## Steps\nOpen the menu.");
        assert_eq!(items[0].data["estimate"], 2.0);
        assert_eq!(items[0].data["title"], "Work on AB-1");
        assert_eq!(items[1].state, State::Idle);
        assert_eq!(items[1].time, ms_of(Some("2026-10-05T00:00:00Z")));
        let without = board_from(&issues, &[], false);
        assert_eq!(without.len(), 1, "UX 2.0 off leaves the canceled out");
    }
}
