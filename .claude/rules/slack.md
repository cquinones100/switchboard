---
paths:
  - "src/relay/**"
  - "src/day/**"
  - "src/connector.rs"
---

# The Slack relay and Claude's connectors

**The Slack relay answers to one person.** `src/relay/slack.rs` holds the rules every Slack call goes through: an incoming message is acted on only if `from_owner` (the owner named in `slack-relay.json`, in the app's DM, in that workspace, not a bot) and `typed_by_you` (Slack's `client_msg_id` and no `app_id`) pass, a button press only if `press_from` does, a form only if `submission_from` does, reactions control nothing, and `refuse` stops any call that names a conversation other than the DM. The owner never comes from a message.

**Claude's connectors.** `src/connector.rs` is the one way to reach a Claude connector: a headless Haiku allowed only the one tool, the answer taken from the tool's own result, never from what Haiku writes. A connector that isn't connected fails with `not_connected` and a message naming `/mcp`, and every call records its service's state for the views and the health light, so a feature that is on says where it shows that its connector is missing (`connector_down`) rather than failing quietly. A call nobody waits on goes through `call_in_background`, which while its connector is down asks `claude mcp get` (no model) at most every 15 minutes instead of running Haiku again. A view that is on is always in mission control, and before what it reads from is set up it says what is missing and how to set it up. The Slack view reaches Slack through the connector, and directly, with a user token of your own (`src/day/slack_user.rs`, stored by `switchboard setup slack-user` as a secret and read only off the server's task), for the two things the connector can't do: your reminders and marking what you open read in Slack. With that token stored and Settings' `slack_token_reads` on, its reads (the look, threads, files, search everything's Slack results) go through Slack's Web API with it too (`src/day/slack_api.rs`), each falling back to the connector when Slack refuses it; with none, they go through the connector. Anything else it needs of Slack goes through the connector, and anything that changes your state in Slack is off by default or asks first.
