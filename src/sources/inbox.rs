//! Mission control's Inbox view, ported from lib/sidebar/inbox.js: your
//! unread Linear notifications that need you, newest first: someone
//! mentioning you, in a ticket or a comment, a new comment on a ticket of
//! yours, and a ticket newly assigned to you. Status changes and the like,
//! most of Linear's inbox, are left out, and so are those read, archived or
//! snoozed. Each one also goes in the sidebar's notifications (the server's
//! `notices` under "inbox"). Enter offers what the Tickets board does with
//! its ticket, and marks the notification read in Linear (`mark_read`).

use serde_json::{Value, json};

use super::{ms_of, text};
use crate::integrations::linear;
use crate::items::{Item, State, Target};

const QUERY: &str = "query { notifications(first: 100) { nodes { id type createdAt readAt archivedAt snoozedUntilAt actor { name } \
  ... on IssueNotification { issue { identifier title url } comment { body url } } } } }";

fn kind_words(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "issueMention" => "mentioned you",
        "issueCommentMention" => "mentioned you in a comment",
        "issueNewComment" => "commented",
        "issueAssignedToYou" => "assigned it to you",
        _ => return None,
    })
}

/// The notifications as the view lists them.
pub fn inbox_from(nodes: &[Value], now_ms: i64) -> Vec<Item> {
    let set = |n: &Value, key: &str| n.get(key).is_some_and(|v| !v.is_null());
    let mut wanted: Vec<&Value> = nodes
        .iter()
        .filter(|n| {
            kind_words(text(n, "/type")).is_some()
                && n.get("issue").is_some_and(Value::is_object)
                && !set(n, "readAt")
                && !set(n, "archivedAt")
                && !ms_of(n.get("snoozedUntilAt").and_then(Value::as_str))
                    .is_some_and(|until| until > now_ms)
        })
        .collect();
    wanted.sort_by_key(|n| -ms_of(n.get("createdAt").and_then(Value::as_str)).unwrap_or(0));
    wanted
        .into_iter()
        .map(|n| {
            let kind = text(n, "/type");
            let actor = n
                .pointer("/actor/name")
                .and_then(Value::as_str)
                .unwrap_or("Someone");
            let mut detail = format!("{actor} {}", kind_words(kind).unwrap_or(""));
            let body: String = text(n, "/comment/body")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(160)
                .collect();
            if !body.is_empty() {
                detail.push_str(": ");
                detail.push_str(&body);
            }
            let url = match text(n, "/comment/url") {
                "" => text(n, "/issue/url"),
                comment => comment,
            };
            let ticket = text(n, "/issue/identifier");
            let mut item = Item::new(
                text(n, "/id"),
                format!("{ticket} {}", text(n, "/issue/title")),
            )
            .detail(detail)
            .state(if kind == "issueNewComment" {
                State::Active
            } else {
                State::Attention
            })
            .target(Target::Url(url.to_string()))
            .data(json!({ "ticket": ticket, "notification": text(n, "/id"), "url": url }));
            item.time = ms_of(n.get("createdAt").and_then(Value::as_str));
            item
        })
        .collect()
}

/// Your notifications, as Linear has them.
pub async fn fetch() -> anyhow::Result<Vec<Value>> {
    let data = linear::graphql(QUERY, json!({}), "sidebar: inbox").await?;
    Ok(data
        .pointer("/notifications/nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

/// Marks a notification read in Linear. The view drops it at once with
/// `Sources::drop_inbox_item`.
pub async fn mark_read(id: &str) -> anyhow::Result<()> {
    let at = jiff::Timestamp::now().to_string();
    linear::graphql(
        "mutation($id: String!, $at: DateTime!) { notificationUpdate(id: $id, input: { readAt: $at }) { success } }",
        json!({ "id": id, "at": at }),
        "sidebar: inbox",
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unread_mentions_comments_and_assignments_newest_first_leaving_out_status_changes() {
        // lib/sidebar's "the Inbox lists unread Linear mentions, comments and
        // assignments, newest first, leaving out status changes".
        let issue = json!({ "identifier": "AB-1", "title": "Do it", "url": "https://linear.app/x/issue/AB-1" });
        let nodes = vec![
            json!({ "id": "1", "type": "issueStatusChanged", "createdAt": "2026-10-28T10:00:00Z", "actor": { "name": "Ada" }, "issue": issue }),
            json!({ "id": "2", "type": "issueAssignedToYou", "createdAt": "2026-10-28T11:00:00Z", "actor": { "name": "Ada" }, "issue": issue }),
            json!({ "id": "3", "type": "issueCommentMention", "createdAt": "2026-10-28T12:00:00Z", "actor": { "name": "Lin" }, "issue": issue,
                    "comment": { "body": "can you\nlook", "url": "https://linear.app/x/issue/AB-1#comment-1" } }),
            json!({ "id": "4", "type": "issueMention", "createdAt": "2026-10-28T13:00:00Z", "readAt": "2026-10-28T13:05:00Z", "actor": { "name": "Bo" }, "issue": issue }),
            json!({ "id": "5", "type": "issueNewComment", "createdAt": "2026-10-28T14:00:00Z", "snoozedUntilAt": "2999-01-01T00:00:00Z", "actor": { "name": "Cy" }, "issue": issue }),
        ];
        let now = "2026-10-29T00:00:00Z"
            .parse::<jiff::Timestamp>()
            .unwrap()
            .as_millisecond();
        let items = inbox_from(&nodes, now);
        let rows: Vec<(&str, &str, Target, State)> = items
            .iter()
            .map(|i| (i.id.as_str(), i.detail.as_str(), i.target.clone(), i.state))
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    "3",
                    "Lin mentioned you in a comment: can you look",
                    Target::Url("https://linear.app/x/issue/AB-1#comment-1".into()),
                    State::Attention
                ),
                (
                    "2",
                    "Ada assigned it to you",
                    Target::Url("https://linear.app/x/issue/AB-1".into()),
                    State::Attention
                ),
            ],
            "not a status change, nor one read or snoozed"
        );
        assert_eq!(items[0].data["ticket"], "AB-1");
        assert_eq!(items[0].data["notification"], "3");
    }
}
