//! What a remote front-end, the phone app (src/phone) or the Slack relay
//! (src/relay), shares with the other: one protocol of the server's
//! commands to reach the tabs (`protocol`, answered in `serve`), reading
//! what a Claude tab's screen shows through `agent::screen`, and one policy
//! for what reaches you and how loudly (`notify`). Neither front-end
//! reaches into the other; each keeps who it answers to (the relay's
//! `slack`, the phone's `guard`) and what it shows.

pub mod notify;
pub mod protocol;
pub mod serve;
