//! Slack with a token of your own, for the two things Claude's Slack
//! connector can't do for the Slack view: list your reminders, which arrive
//! in your DM with Slackbot where the connector neither searches nor reads,
//! and mark a conversation read in Slack once you open it here. New with
//! the Rust version, with no tmux-version file behind it.
//!
//! The token is a user token (`xoxp-…`) of an app of yours, the relay's
//! with the user scopes `SCOPES` names added, which `switchboard setup
//! slack-user` walks through and stores as a secret (`TOKEN_SERVICE`: the
//! Keychain, or Linux's keyring, and where that can't take it, the
//! `secrets.json` only you can read, as every secret of Switchboard's), in no
//! file of its own; `SWITCHBOARD_SLACK_USER_TOKEN` is read before it, as
//! `LINEAR_API_KEY` is for Linear. Storing it writes `slack-user.json` in
//! the state directory (your team, its address and your member ID, as
//! `auth.test` gave them), which is how the server knows there is a sign-in
//! without asking the Keychain, a process, on its tick: the token itself is
//! read only in the tasks that send it. Every request goes through
//! `integrations::slack`'s client, with this token.
//!
//! Reminders: `look` reads `reminders.list` and keeps the ones due and not
//! complete in `slack-reminders.json`, with the ones you have seen in the
//! view and the ones you dismissed, each let go once Slack no longer lists
//! it as due. A recurring reminder has no time of its own in Slack's
//! answer, so it is never listed.
//!
//! Read marks: `mark_read` asks `conversations.info` how far you have read
//! first, since `conversations.mark` sets the mark wherever it is told,
//! backward too, which would turn what you read in Slack unread again.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use crate::integrations::slack::Client;
use crate::integrations::slack::{self, Send};
use crate::util;

/// The secret the token is stored as, and the variable read before it.
pub const TOKEN_SERVICE: &str = "slack-user-token";
pub const TOKEN_ENV: &str = "SWITCHBOARD_SLACK_USER_TOKEN";
/// How the token is set up, as the view and the errors say it.
pub const SETUP: &str = "switchboard setup slack-user";
/// The user scopes the token needs: reading and completing reminders,
/// reading how far a conversation is read and marking it, and the Slack
/// view's reads (`slack_api`): its messages, threads, people, search and
/// pictures, and the workspace's own emoji (`slack_emoji`).
pub const SCOPES: [&str; 18] = [
    "reminders:read",
    "reminders:write",
    "channels:read",
    "channels:write",
    "groups:read",
    "groups:write",
    "im:read",
    "im:write",
    "mpim:read",
    "mpim:write",
    "channels:history",
    "groups:history",
    "im:history",
    "mpim:history",
    "search:read",
    "users:read",
    "files:read",
    "emoji:read",
];
/// How often the reminders are read while you're at the computer.
/// How a read Slack refused for its rate limit starts, Slack's
/// `Retry-After` after it when it gave one (`retry_after_ms`).
pub const RATE_LIMIT: &str = "Slack's rate limit is spent for now";

/// How long Slack asked a read refused for its rate limit (`why`, as
/// `api` says it) to wait, a minute when it didn't say; `None` for any
/// other failure. A wait over an hour is taken as an hour.
pub fn retry_after_ms(why: &str) -> Option<i64> {
    let rest = why.strip_prefix(RATE_LIMIT)?;
    let said = rest
        .strip_prefix(": Slack asked to wait ")
        .and_then(|r| r.strip_suffix(" seconds"))
        .and_then(|n| n.parse::<i64>().ok())
        .filter(|n| *n > 0);
    Some(said.map_or(60_000, |s| s.min(3_600) * 1000))
}

pub const REMINDERS_MS: i64 = 2 * 60_000;
/// What the view says with no sign-in.
pub const NO_SIGN_IN: &str =
    "Slack reminders and read marks need your Slack sign-in: switchboard setup slack-user";

/// Whose token is stored, as `auth.test` said when it was.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignIn {
    #[serde(default)]
    pub team: String,
    /// The workspace's address, `https://<name>.slack.com/`.
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub user: String,
}

impl SignIn {
    /// Where Slack lists your reminders, for "Open it in Slack".
    pub fn later(&self) -> Option<String> {
        let team_ok = !self.team.is_empty()
            && self
                .team
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
        team_ok.then(|| format!("https://app.slack.com/client/{}/later", self.team))
    }
}

pub(super) fn sign_in_file(state: &Path) -> PathBuf {
    state.join("slack-user.json")
}

/// The sign-in `switchboard setup slack-user` kept, or `None` with none.
pub fn signed_in(state: &Path) -> Option<SignIn> {
    serde_json::from_value(util::read_json(sign_in_file(state))?).ok()
}

/// Keeps who the stored token is for. The reminders kept are let go, since
/// they may be another account's: the next look reads this one's; and so is
/// what was said of reads that failed through the token before
/// (`slack_api`'s `slack-token.json`), which may be fixed now.
pub fn save_sign_in(state: &Path, who: &SignIn) -> std::io::Result<()> {
    std::fs::create_dir_all(state)?;
    for file in [reminders_file(state), state.join("slack-token.json")] {
        match std::fs::remove_file(file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    util::write_atomic(
        sign_in_file(state),
        util::json_compact(&serde_json::to_value(who).unwrap_or_default()),
    )
}

/// The token: `SWITCHBOARD_SLACK_USER_TOKEN`, else the stored secret. It
/// asks the Keychain, a process, so never on the server's task.
pub fn token() -> Option<String> {
    std::env::var(TOKEN_ENV)
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .or_else(|| crate::relay::slack::token(TOKEN_SERVICE))
}

/// With the stored token, read off the task that asks.
pub(super) async fn stored() -> Result<Client, String> {
    let token = tokio::task::spawn_blocking(token)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| format!("your Slack sign-in isn't stored any more: run {SETUP}"))?;
    Ok(Client::new(slack::api_base(), token))
}

/// One call to Slack's Web API: a GET with its arguments in the query
/// (`reminders.list` takes no JSON), else a POST of them as JSON. Slack's
/// refusal (`ok: false`) is an error in words that say what to do about it
/// where something can be done.
pub(super) async fn api(
    client: &Client,
    get: bool,
    method: &str,
    params: &Value,
    source: &str,
) -> Result<Value, String> {
    let send = if get { Send::Get } else { Send::Json };
    client
        .call(send, method, params, source)
        .await
        .map_err(|e| match e {
            slack::Error::Unreachable { error, .. } => error.to_string(),
            slack::Error::Refused {
                status: 429,
                retry_after,
                ..
            } => match retry_after {
                Some(wait) => format!("{RATE_LIMIT}: Slack asked to wait {wait} seconds"),
                None => RATE_LIMIT.to_string(),
            },
            slack::Error::Refused {
                error,
                needed,
                status,
                ..
            } => match error.as_str() {
                "missing_scope" => {
                    let needed = needed.as_deref().unwrap_or("a");
                    format!(
                        "your Slack sign-in lacks the {needed} scope: add it and run {SETUP} again"
                    )
                }
                "invalid_auth" | "not_authed" | "token_revoked" | "token_expired"
                | "account_inactive" => {
                    format!("Slack refused your sign-in ({error}): run {SETUP} again")
                }
                "" => format!("{method}: Slack answered {status}"),
                other => format!("{method}: {other}"),
            },
        })
}

/// Who `token` is for, which also says whether Slack takes it.
pub async fn auth_test(token: &str) -> Result<SignIn, String> {
    let client = Client::new(slack::api_base(), token.to_string());
    let said = api(&client, false, "auth.test", &json!({}), "setup: slack-user").await?;
    let get = |k: &str| {
        said.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    Ok(SignIn {
        team: get("team_id"),
        url: get("url"),
        user: get("user_id"),
    })
}

/// A reminder that is due: its ID, what it says, and when it was due, in
/// seconds since the epoch.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reminder {
    pub id: String,
    #[serde(default)]
    pub text: String,
    pub time: i64,
}

/// A reminder's ID as Slack writes one (`Rm0123ABC`), so nothing else is
/// kept or sent back.
fn is_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The reminders of `reminders.list`'s answer that are due at `now`
/// (seconds) and not complete, the one due longest ago first. A recurring
/// one carries no time, and is left out.
pub fn due(answer: &Value, now: i64) -> Vec<Reminder> {
    let int = |v: Option<&Value>| {
        v.and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
            .unwrap_or(0)
    };
    let mut out: Vec<Reminder> = answer
        .get("reminders")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|r| r.get("recurring") != Some(&Value::Bool(true)))
        .filter(|r| int(r.get("complete_ts")) == 0)
        .filter_map(|r| {
            let id = r.get("id")?.as_str()?;
            let time = int(r.get("time"));
            (is_id(id) && time > 0 && time <= now).then(|| Reminder {
                id: id.to_string(),
                text: super::slack_look::plain(r.get("text").and_then(Value::as_str).unwrap_or(""))
                    .chars()
                    .take(2000)
                    .collect(),
                time,
            })
        })
        .collect();
    out.sort_by(|a, b| a.time.cmp(&b.time).then(a.id.cmp(&b.id)));
    out
}

/// What the last look at your reminders kept.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reminders {
    /// When, in milliseconds.
    #[serde(default)]
    pub checked: i64,
    #[serde(default)]
    pub reminders: Vec<Reminder>,
    /// The ones seen in the view, and the ones dismissed, by ID.
    #[serde(default)]
    pub seen: Vec<String>,
    #[serde(default)]
    pub dismissed: Vec<String>,
    /// Why the last look failed, when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn reminders_file(state: &Path) -> PathBuf {
    state.join("slack-reminders.json")
}

/// `slack-reminders.json`, or `None` before a look.
pub fn read_reminders(state: &Path) -> Option<Reminders> {
    serde_json::from_value(util::read_json(reminders_file(state))?).ok()
}

fn write_reminders(state: &Path, kept: &Reminders) -> std::io::Result<()> {
    std::fs::create_dir_all(state)?;
    util::write_atomic(
        reminders_file(state),
        util::json_compact(&serde_json::to_value(kept).unwrap_or_default()),
    )
}

/// Changes what is kept, when anything is.
fn update(state: &Path, change: impl FnOnce(&mut Reminders)) {
    let Some(mut kept) = read_reminders(state) else {
        return;
    };
    let before = kept.clone();
    change(&mut kept);
    if kept != before {
        let _ = write_reminders(state, &kept);
    }
}

/// Reminders shown in the view: seen, so they fade and leave the
/// notifications.
pub fn mark_seen(state: &Path, ids: &[&str]) {
    update(state, |kept| {
        for id in ids {
            if is_id(id) && !kept.seen.iter().any(|s| s == id) {
                kept.seen.push(id.to_string());
            }
        }
    });
}

/// A reminder dismissed from its menu: gone from the view, though Slack
/// still holds it.
pub fn dismiss(state: &Path, id: &str) {
    update(state, |kept| {
        if is_id(id) && !kept.dismissed.iter().any(|s| s == id) {
            kept.dismissed.push(id.to_string());
        }
    });
}

/// A reminder marked complete in Slack: off the list at once, rather than
/// at the next look.
pub fn drop_reminder(state: &Path, id: &str) {
    update(state, |kept| kept.reminders.retain(|r| r.id != id));
}

/// Keeps what a look found (or why it failed, with what was kept before):
/// the marks of reminders no longer due are let go.
fn save(state: &Path, found: Result<Vec<Reminder>, String>, now_ms: i64) -> std::io::Result<()> {
    let mut kept = read_reminders(state).unwrap_or_default();
    match found {
        Ok(reminders) => {
            let listed = |id: &String| reminders.iter().any(|r| r.id == *id);
            kept.seen.retain(listed);
            kept.dismissed.retain(listed);
            kept.reminders = reminders;
            kept.checked = now_ms;
            kept.error = None;
        }
        Err(why) => kept.error = Some(why),
    }
    write_reminders(state, &kept)
}

/// Reads your reminders once and keeps the ones due; gives a line for the
/// log.
pub async fn look(state: &Path, now_ms: i64) -> Result<String, String> {
    look_with(stored().await, state, now_ms).await
}

async fn look_with(
    client: Result<Client, String>,
    state: &Path,
    now_ms: i64,
) -> Result<String, String> {
    let found = async {
        let answer = api(
            &client?,
            true,
            "reminders.list",
            &json!({}),
            "Slack view: reminders",
        )
        .await?;
        Ok::<_, String>(due(&answer, now_ms.div_euclid(1000)))
    }
    .await;
    let line = found
        .as_ref()
        .map(|r| format!("slack: {} reminders due", r.len()))
        .map_err(Clone::clone);
    save(state, found, now_ms).map_err(|e| e.to_string())?;
    line
}

/// Marks reminder `id` complete in Slack.
pub async fn complete(id: &str) -> Result<(), String> {
    complete_with(&stored().await?, id).await
}

async fn complete_with(client: &Client, id: &str) -> Result<(), String> {
    if !is_id(id) {
        return Err("that isn't a reminder Slack listed".into());
    }
    api(
        client,
        false,
        "reminders.complete",
        &json!({ "reminder": id }),
        "Slack view: a reminder completed",
    )
    .await
    .map(|_| ())
}

/// Marks conversation `channel` read in Slack up to message `ts`, unless
/// you have read that far there already; whether it moved the mark. Tried
/// once more when it fails.
pub async fn mark_read(channel: &str, ts: &str) -> Result<bool, String> {
    mark_read_with(&stored().await?, channel, ts).await
}

async fn mark_read_with(client: &Client, channel: &str, ts: &str) -> Result<bool, String> {
    if !super::slack::is_channel(channel) || !super::slack::is_ts(ts) {
        return Err("that isn't a message Slack listed".into());
    }
    match mark_once(client, channel, ts).await {
        Ok(moved) => Ok(moved),
        Err(_) => mark_once(client, channel, ts).await,
    }
}

async fn mark_once(token: &Client, channel: &str, ts: &str) -> Result<bool, String> {
    const SOURCE: &str = "Slack view: marked read";
    let info = api(
        token,
        true,
        "conversations.info",
        &json!({ "channel": channel }),
        SOURCE,
    )
    .await?;
    // Without how far it is read, a mark might move back: none is made.
    let read_to = info
        .pointer("/channel/last_read")
        .and_then(Value::as_str)
        .and_then(|t| t.parse::<f64>().ok())
        .ok_or("Slack didn't say how far you have read there")?;
    if ts.parse::<f64>().unwrap_or(0.0) <= read_to {
        return Ok(false);
    }
    api(
        token,
        false,
        "conversations.mark",
        &json!({ "channel": channel, "ts": ts }),
        SOURCE,
    )
    .await
    .map(|_| true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reminders_due_and_not_complete_are_kept_oldest_first() {
        let answer = json!({ "ok": true, "reminders": [
            { "id": "Rm2", "text": "call <@U1|ada>", "time": 900, "complete_ts": 0, "recurring": false },
            { "id": "Rm1", "text": "water the plants", "time": 500, "complete_ts": 0 },
            { "id": "RmDone", "text": "done", "time": 400, "complete_ts": 450 },
            { "id": "RmLater", "text": "later", "time": 2000, "complete_ts": 0 },
            { "id": "RmWeekly", "text": "weekly", "recurring": true },
            { "id": "not an id!", "text": "forged", "time": 100, "complete_ts": 0 },
        ] });
        let got = due(&answer, 1000);
        assert_eq!(
            got.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["Rm1", "Rm2"]
        );
        assert_eq!(got[1].text, "call @ada");
    }

    #[test]
    fn a_look_lets_go_of_the_marks_of_reminders_no_longer_due() {
        let state = std::env::temp_dir().join(format!(
            "sb-slack-user-{}-{}",
            std::process::id(),
            util::now_ms()
        ));
        let r = |id: &str| Reminder {
            id: id.into(),
            text: id.into(),
            time: 5,
        };
        save(&state, Ok(vec![r("Rm1"), r("Rm2")]), 10_000).unwrap();
        mark_seen(&state, &["Rm1", "Rm2", "no id"]);
        dismiss(&state, "Rm2");
        let kept = read_reminders(&state).unwrap();
        assert_eq!(kept.seen, ["Rm1", "Rm2"]);
        assert_eq!(kept.dismissed, ["Rm2"]);
        // A failed look keeps what was there, with why.
        save(&state, Err("no network".into()), 20_000).unwrap();
        let kept = read_reminders(&state).unwrap();
        assert_eq!((kept.checked, kept.reminders.len()), (10_000, 2));
        assert_eq!(kept.error.as_deref(), Some("no network"));
        save(&state, Ok(vec![r("Rm1")]), 30_000).unwrap();
        let kept = read_reminders(&state).unwrap();
        assert_eq!((kept.seen.len(), kept.dismissed.len()), (1, 0));
        assert_eq!(kept.error, None);
        drop_reminder(&state, "Rm1");
        assert!(read_reminders(&state).unwrap().reminders.is_empty());
        let who = SignIn {
            team: "T1".into(),
            url: "https://x.slack.com/".into(),
            user: "U1".into(),
        };
        save_sign_in(&state, &who).unwrap();
        assert_eq!(signed_in(&state), Some(who.clone()));
        assert_eq!(
            read_reminders(&state),
            None,
            "a sign-in lets go of the reminders kept, which may be another account's"
        );
        assert_eq!(
            who.later().as_deref(),
            Some("https://app.slack.com/client/T1/later")
        );
        let _ = std::fs::remove_dir_all(&state);
    }

    #[cfg(feature = "net")]
    #[test]
    fn slack_is_asked_as_you_and_a_conversation_marked_only_past_where_it_is_read() {
        use crate::fake_http::{Fake, Reply, Service};
        use std::sync::atomic::{AtomicUsize, Ordering};
        static MARKS: AtomicUsize = AtomicUsize::new(0);
        let fake = Fake::start(
            Service::Slack {
                token: "xoxp-test".into(),
            },
            |request| {
                let method = request.path().trim_start_matches("/api/");
                let body = match method {
                    // A conversation Slack gives no read position for.
                    "conversations.info"
                        if request.method == "GET" && request.target.ends_with("channel=C2") =>
                    {
                        json!({ "ok": true, "channel": { "id": "C2" } })
                    }
                    "conversations.info" if request.method == "GET" => {
                        json!({ "ok": true, "channel": { "last_read": "150.000000" } })
                    }
                    // The first mark fails, so it is tried once more.
                    "conversations.mark" if request.method == "POST" => {
                        assert_eq!(
                            request.json(),
                            json!({ "channel": "C1", "ts": "200.000000" })
                        );
                        if MARKS.fetch_add(1, Ordering::SeqCst) == 0 {
                            json!({ "ok": false, "error": "internal_error" })
                        } else {
                            json!({ "ok": true })
                        }
                    }
                    "reminders.complete" if request.method == "POST" => {
                        assert_eq!(request.json(), json!({ "reminder": "Rm1" }));
                        json!({ "ok": true })
                    }
                    "reminders.list" if request.method == "GET" => {
                        json!({ "ok": true, "reminders": [
                        { "id": "Rm1", "text": "stretch", "time": 5, "complete_ts": 0 },
                    ] })
                    }
                    _ => return None,
                };
                Some(Reply::json(200, body))
            },
        );
        let client = Client::new(format!("{}/api/", fake.url), "xoxp-test".into());
        let state = std::env::temp_dir().join(format!(
            "sb-slack-user-api-{}-{}",
            std::process::id(),
            util::now_ms()
        ));
        util::block_on(async {
            // Read in Slack through 150: an older message moves nothing,
            // and asks only how far it is read.
            assert_eq!(mark_read_with(&client, "C1", "100.000000").await, Ok(false));
            assert_eq!(fake.seen().len(), 1);
            assert_eq!(mark_read_with(&client, "C1", "200.000000").await, Ok(true));
            assert_eq!(MARKS.load(Ordering::SeqCst), 2, "tried once more");
            assert!(
                mark_read_with(&client, "not a channel", "1.0")
                    .await
                    .is_err()
            );
            complete_with(&client, "Rm1").await.unwrap();
            // How far it is read unknown: no mark, which might move back.
            assert_eq!(
                mark_read_with(&client, "C2", "200.000000").await,
                Err("Slack didn't say how far you have read there".into())
            );
            assert_eq!(MARKS.load(Ordering::SeqCst), 2, "nothing marked");
            let line = look_with(
                Ok(Client::new(client.base.clone(), "xoxp-test".into())),
                &state,
                10_000,
            )
            .await
            .unwrap();
            assert_eq!(line, "slack: 1 reminders due");
            // With no token nothing is asked, and the view is told why.
            let before = fake.seen().len();
            let none = look_with(Err("no sign-in".into()), &state, 20_000).await;
            assert_eq!(none, Err("no sign-in".into()));
            assert_eq!(fake.seen().len(), before);
        });
        let kept = read_reminders(&state).unwrap();
        assert_eq!(kept.reminders[0].text, "stretch");
        assert_eq!(kept.error.as_deref(), Some("no sign-in"));
        let _ = std::fs::remove_dir_all(&state);
    }

    #[cfg(feature = "net")]
    #[test]
    fn slacks_refusals_say_what_to_do() {
        use crate::fake_http::{Fake, Reply, Service};
        let mut fake = Fake::canned(
            Service::Slack {
                token: "xoxp-test".into(),
            },
            vec![Reply::json(
                200,
                json!({ "ok": false, "error": "missing_scope", "needed": "reminders:read" }),
            )],
        );
        fake.expect_refusals();
        util::block_on(async {
            let base = format!("{}/api/", fake.url);
            let scope = api(
                &Client::new(base.clone(), "xoxp-test".into()),
                true,
                "reminders.list",
                &json!({}),
                "test",
            )
            .await
            .unwrap_err();
            assert_eq!(
                scope,
                "your Slack sign-in lacks the reminders:read scope: add it and run switchboard setup slack-user again"
            );
            let wrong = api(
                &Client::new(base, "xoxp-wrong".into()),
                true,
                "reminders.list",
                &json!({}),
                "test",
            )
            .await
            .unwrap_err();
            assert_eq!(
                wrong,
                "Slack refused your sign-in (invalid_auth): run switchboard setup slack-user again"
            );
        });
    }

    #[test]
    fn how_long_a_rate_limit_asks_to_wait_is_read_back_from_its_words() {
        assert_eq!(
            retry_after_ms("Slack's rate limit is spent for now: Slack asked to wait 30 seconds"),
            Some(30_000)
        );
        assert_eq!(retry_after_ms(RATE_LIMIT), Some(60_000));
        assert_eq!(retry_after_ms("search.messages: invalid_cursor"), None);
        assert_eq!(
            retry_after_ms(&format!(
                "{RATE_LIMIT}: Slack asked to wait {} seconds",
                i64::MAX
            )),
            Some(3_600_000)
        );
    }

    #[cfg(feature = "net")]
    #[test]
    fn a_rate_limit_says_how_long_slack_asked_to_wait() {
        use crate::fake_http::{Fake, Reply, Service};
        let fake = Fake::canned(
            Service::Slack {
                token: "xoxp-test".into(),
            },
            vec![
                Reply::json(429, json!({ "ok": false, "error": "ratelimited" }))
                    .header("Retry-After", "30"),
            ],
        );
        let why = util::block_on(api(
            &Client::new(format!("{}/api/", fake.url), "xoxp-test".into()),
            true,
            "search.messages",
            &json!({}),
            "test",
        ))
        .unwrap_err();
        assert_eq!(
            why,
            "Slack's rate limit is spent for now: Slack asked to wait 30 seconds"
        );
        assert_eq!(retry_after_ms(&why), Some(30_000));
    }
}
