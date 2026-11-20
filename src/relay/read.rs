//! Catching up (lib/slack-relay/read.js). Slack pushes each message as it
//! is sent (Socket Mode), so reading the DM is only needed for what was
//! sent while the relay couldn't hear: before it started again after a
//! crash, or while its connection was down. Then the DM is read from the
//! cursor, and each watched thread from its last handled reply. Only your
//! messages come back, by the same rules as events (`slack::message_from`, or Discord's);
//! the dispatcher drops any it has already handled.

use anyhow::Result;

use super::Relay;
use super::slack::Message;

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
    let log = |line: &str| cx.log(line);
    cx.chat
        .catch_up(&dm, cursor.as_deref(), &threads, &log)
        .await
}
