//! Catching up (lib/slack-relay/read.js). Slack pushes each message as it
//! is sent (Socket Mode), so reading the DM is only needed for what was
//! sent while the relay couldn't hear: before it started again after a
//! crash, or while its connection was down. Then the DM is read from the
//! cursor, and each watched thread from its last handled reply. Only your
//! messages come back, by the same rules as events (`slack::message_from`);
//! the dispatcher drops any it has already handled.

use anyhow::Result;
use serde_json::{Value, json};

use super::Relay;
use super::slack::{Message, message_from};
use super::state::number;

pub const PAGE: u64 = 200;

pub async fn catch_up(cx: &Relay) -> Result<Vec<Message>> {
    let (dm, cursor, threads) = {
        let state = cx.state.borrow();
        (
            state.dm(),
            state.cursor.clone(),
            state
                .threads
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Vec<_>>(),
        )
    };
    let mine = |messages: &Value, thread: Option<&str>| -> Vec<Message> {
        messages
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| {
                let mut event = m.clone();
                event["type"] = json!("message");
                event["channel"] = json!(dm.channel);
                if event.get("thread_ts").is_none_or(Value::is_null)
                    && let Some(t) = thread
                {
                    event["thread_ts"] = json!(t);
                }
                message_from(&dm, &event, None, |line| cx.log(line))
            })
            .collect()
    };
    let mut params = json!({ "channel": dm.channel, "limit": PAGE });
    if let Some(c) = &cursor {
        params["oldest"] = json!(c);
    }
    let top = cx.slack.api("conversations.history", params).await?;
    let from = number(cursor.as_deref().unwrap_or("0"));
    let mut messages: Vec<Message> = mine(top.get("messages").unwrap_or(&Value::Null), None)
        .into_iter()
        .filter(|m| m.thread_ts.is_none() && number(&m.ts) > from)
        .collect();
    for (thread, handled_to) in threads {
        let replies = cx
            .slack
            .api(
                "conversations.replies",
                json!({ "channel": dm.channel, "ts": thread, "oldest": handled_to, "limit": PAGE }),
            )
            .await
            .unwrap_or_else(|_| json!({ "messages": [] }));
        messages.extend(
            mine(
                replies.get("messages").unwrap_or(&Value::Null),
                Some(&thread),
            )
            .into_iter()
            .filter(|m| {
                m.thread_ts.as_deref() == Some(thread.as_str())
                    && number(&m.ts) > number(&handled_to)
            }),
        );
    }
    Ok(messages)
}
