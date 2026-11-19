//! The Slack view's messages as they're posted: Socket Mode for an app of
//! your own, with its app-level token (`xapp-`, scope `connections:write`)
//! stored beside its user token by `switchboard setup slack-user`, and the
//! app subscribed on your behalf to `message.channels`, `message.groups`,
//! `message.im` and `message.mpim`. New with the Rust version, with no
//! tmux-version file behind it.
//!
//! The socket is the relay's (`relay::http::socket_loop`, which acknowledges
//! each envelope and reconnects with backoff), with neither the relay's app
//! nor its token: this one is yours, and what it brings only fills the view,
//! never controls anything, so none of the relay's rules about who may ask
//! what apply or are needed. Each `message` event is read as the look's
//! search reads a result (`slack_api::found_of_event`, with your user token
//! for people's and channels' names) and sorted and added as a look's would
//! be (`slack_look::take_live`), without marking a look, so the scheduled
//! look still reads from where it last did. Only an event Slack delivered
//! for your sign-in's member, in its workspace, is taken. A token that turns
//! out to be the relay's app's (two connections for one app would share its
//! events) ends the connection at once and is refused until another is
//! stored (`slack-live.json` in the state directory keeps its fingerprint).
//!
//! The server holds the connection while `wanted` says so (Settings'
//! `slack_live`, with "Slack without a tab" and "Read Slack through your own
//! token" on, and a sign-in with an app-level token) while you're at the
//! computer, closing it while you're away as the lookups pause. Once a
//! message for you has come through it, the scheduled look runs only every
//! `CATCH_UP_MS`, as a catch-up; when it is down the look keeps its
//! schedule and the view says why.

use std::sync::atomic::{AtomicI64, Ordering};

use serde_json::Value;

use super::{Dirs, slack_api, slack_look, slack_user};
use crate::relay::http::{Socket, socket_loop};
use crate::relay::slack::Envelope;
use crate::{settings, util};

/// The secret the app-level token is stored as, and the variable read
/// before it.
pub const APP_TOKEN_SERVICE: &str = "slack-app-token";
pub const APP_TOKEN_ENV: &str = "SWITCHBOARD_SLACK_APP_TOKEN";
/// The variable read before the relay's own stored app-level token.
pub const RELAY_TOKEN_ENV: &str = "SWITCHBOARD_SLACK_RELAY_APP_TOKEN";
/// How often the look runs while the connection is up.
pub const CATCH_UP_MS: i64 = 10 * 60_000;
/// Until when Slack asked the name lookups to wait, for the server's life,
/// so a connection opened again still waits.
static WAIT_UNTIL: AtomicI64 = AtomicI64::new(0);
/// Who holds the connection, in the request audit.
const SOURCE: &str = "Slack view: live";

/// The app-level token: `SWITCHBOARD_SLACK_APP_TOKEN`, else the stored
/// secret. It asks the Keychain, a process, so never on the server's task.
pub fn app_token() -> Option<String> {
    std::env::var(APP_TOKEN_ENV)
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .or_else(|| crate::relay::slack::token(APP_TOKEN_SERVICE))
}

/// The Slack relay's app-level token, to refuse it here:
/// `SWITCHBOARD_SLACK_RELAY_APP_TOKEN` when set (empty for none), else the
/// relay's stored secret. Asks the Keychain, so never on the server's task.
fn relay_token() -> Option<String> {
    match std::env::var(RELAY_TOKEN_ENV) {
        Ok(t) => Some(t.trim().to_string()).filter(|t| !t.is_empty()),
        Err(_) => crate::relay::slack::token(crate::relay::slack::APP_TOKEN_SERVICE),
    }
}

/// Whether the server should hold the connection: Settings' `slack_live`,
/// `slack_background` and `slack_token_reads` on, and a sign-in that
/// stored an app-level token. Reads a small file, as the look's schedule
/// does on the tick.
pub fn wanted(state: &std::path::Path) -> bool {
    ["slack_live", "slack_background", "slack_token_reads"]
        .iter()
        .all(|key| settings::is_on(key))
        && slack_user::signed_in(state).is_some_and(|who| who.live)
}

/// What the connection tells the server.
#[derive(Debug)]
pub enum Live {
    /// Connected: the look can run as a catch-up only.
    Up,
    /// Failed or dropped, and why; it is opened again by itself.
    Down(String),
    /// A message taken: how many items it added to the view, or why it
    /// couldn't be read.
    Took(Result<usize, String>),
    /// Slack refused a read for its rate limit and asked to wait until then
    /// (milliseconds), which the look waits too.
    Wait(i64),
}

/// Holds the connection until the task is ended, telling `send` what
/// happens. Ends at once, after saying why, when no app-level token can be
/// read.
pub async fn run(dirs: Dirs, send: impl Fn(Live) + Send + Sync + 'static) {
    let tokens = tokio::task::spawn_blocking(|| (app_token(), relay_token()))
        .await
        .ok();
    let Some((Some(app), relay_token)) = tokens else {
        send(Live::Down(format!(
            "no app-level token is stored: run {}",
            slack_user::SETUP
        )));
        return;
    };
    let refused_file = dirs.state.join("slack-live.json");
    let fingerprint = {
        use sha2::Digest;
        sha2::Sha256::digest(app.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let refused = util::read_json(&refused_file)
        .and_then(|v| v.get("refused").and_then(Value::as_str).map(str::to_string))
        .is_some_and(|f| f == fingerprint);
    if refused || relay_token.as_deref() == Some(app.as_str()) {
        send(Live::Down(RELAY_APP.to_string()));
        return;
    }
    // The relay's own app: two connections would share its events, so this
    // one closes before taking any of them, leaving it to the relay's.
    let relays = |what: &Envelope| {
        let app_id = match what {
            Envelope::Event { app, .. } => app.clone(),
            Envelope::Interactive(payload) => payload
                .get("api_app_id")
                .and_then(Value::as_str)
                .map(str::to_string),
            _ => None,
        };
        app_id.is_some() && app_id == relay_app(&dirs.relay)
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let socket = async {
        if socket_loop(
            &app,
            SOURCE,
            || false,
            relays,
            move |news| {
                let _ = tx.send(news);
            },
        )
        .await
        {
            // Refused until another token is stored.
            let _ = util::write_atomic(
                &refused_file,
                util::json_compact(&serde_json::json!({ "refused": fingerprint })),
            );
            send(Live::Down(RELAY_APP.to_string()));
        }
    };
    let take = async {
        while let Some(news) = rx.recv().await {
            match news {
                Socket::Envelope(Envelope::Hello) => send(Live::Up),
                Socket::Envelope(Envelope::Event {
                    event,
                    team,
                    authorized,
                    ..
                }) => {
                    let waited = WAIT_UNTIL.load(Ordering::SeqCst);
                    let took = take(&dirs, &event, team.as_deref(), authorized).await;
                    // The wait first, so the look it starts waits too.
                    let wait_until = WAIT_UNTIL.load(Ordering::SeqCst);
                    if wait_until != waited {
                        send(Live::Wait(wait_until));
                    }
                    if let Some(took) = took {
                        send(Live::Took(took));
                    }
                }
                Socket::Dropped { why, retry_in_ms } => send(Live::Down(format!(
                    "{why}; trying again in {}s",
                    retry_in_ms / 1000
                ))),
                Socket::Envelope(_) => {}
            }
        }
    };
    tokio::join!(socket, take);
}

/// What the view says when the token is the Slack relay's app's.
pub const RELAY_APP: &str = "the app-level token is the Slack relay's app's, whose events it would take: use an app of your own";

/// The Slack relay's app's ID, once an event has given it to the relay.
fn relay_app(relay: &std::path::Path) -> Option<String> {
    util::read_json(relay.join("state.json"))?
        .pointer("/dm/appId")?
        .as_str()
        .filter(|a| !a.is_empty())
        .map(str::to_string)
}

/// One event read and added to the view: `None` for one left out before
/// anything was read.
async fn take(
    dirs: &Dirs,
    event: &Value,
    team: Option<&str>,
    authorized: Option<(String, String)>,
) -> Option<Result<usize, String>> {
    let who = slack_user::signed_in(&dirs.state)?;
    // Delivered for you, in your workspace: not for your app's bot, nor for
    // someone else who installed it, nor in another workspace.
    let yours = authorized.is_some_and(|(t, u)| t == who.team && u == who.user);
    if !yours || who.team.is_empty() || team.is_some_and(|t| t != who.team) {
        return None;
    }
    // Only what the view would show is read further, and nothing while
    // Slack asked to wait: the look catches up on that.
    let now = util::now_ms();
    if !slack_look::would_show(dirs, &who.user, event, now) {
        return None;
    }
    if now < WAIT_UNTIL.load(Ordering::SeqCst) {
        return Some(Err("Slack asked to wait".to_string()));
    }
    let Some(client) = slack_api::usable(&dirs.state, slack_api::client(&dirs.state).await) else {
        return Some(Err("your Slack sign-in couldn't be read".to_string()));
    };
    let found = match slack_api::found_of_event(&client, event, &who, SOURCE).await {
        Ok(Some(found)) => found,
        Ok(None) => return None,
        Err(why) => {
            if let Some(wait) = slack_user::retry_after_ms(&why) {
                WAIT_UNTIL.store(util::now_ms().saturating_add(wait), Ordering::SeqCst);
            }
            return Some(Err(why));
        }
    };
    Some(slack_look::take_live(dirs, &who.user, found, util::now_ms()).map_err(|e| e.to_string()))
}
