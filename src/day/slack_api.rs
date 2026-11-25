//! The Slack view's reads through your own token rather than Claude's
//! connector: the look's search, a thread, a thread's files and the search
//! view's Slack results, each a request to Slack's Web API instead of a
//! headless Haiku call. New with the Rust version, with no tmux-version file
//! behind it.
//!
//! It uses the sign-in `switchboard setup slack-user` stores
//! (`slack_user`), read only in the tasks that send it, never on the
//! server's task, and only while Settings' `slack_token_reads` is on
//! (`client`). Each read gives back what the connector's reader gives
//! (`slack_look::Found`, `slack_thread::Thread`, a file's bytes), so the
//! view doesn't know which way it was read. A read that fails (the token
//! gone or refused, a scope missing, the rate limit spent, Slack out of
//! reach) is read through the connector instead, and why is kept in
//! `slack-token.json` in the state directory (`failed`), which the view says
//! under its header until a read through the token works again (`worked`):
//! a file's part apart, since a picture needs the `files:read` scope the
//! rest doesn't.
//!
//! People's names come from `users.info`, kept for the server's life, as
//! the connector answers with them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::slack_look::{Found, plain};
use super::slack_thread::{Said, SlackFile, Thread};
use super::slack_user::{self, Client, api};
use crate::net::{self, Audit};
use crate::{settings, util};

/// How many results a page of the look's search holds (Slack's most).
pub const SEARCH_PAGE: u32 = 100;
/// How many messages a page of a thread holds, and how many pages are read.
const THREAD_PAGE: u32 = 200;
const THREAD_PAGES: usize = 25;
/// How long the token read from the Keychain is used before it is read
/// again, so one stored anew is taken up.
const TOKEN_KEPT: Duration = Duration::from_secs(15 * 60);
/// How long a file may take to arrive.
const FILE_TIMEOUT: Duration = Duration::from_secs(60);

/// The token's client, while Settings' `slack_token_reads` is on and there
/// is a sign-in: `None` reads through the connector as before, `Some(Err)`
/// says why the stored token couldn't be read. Asks the Keychain, so never
/// on the server's task.
///
/// The token is read once a sign-in and kept `TOKEN_KEPT` while
/// `slack-user.json` is unchanged, so a look a minute doesn't start a
/// process each time to ask the Keychain.
pub async fn client(state: &Path) -> Option<Result<Client, String>> {
    type Kept = (Option<std::time::SystemTime>, std::time::Instant, Client);
    static KEPT: Mutex<Option<Kept>> = Mutex::new(None);
    if !settings::is_on("slack_token_reads") || slack_user::signed_in(state).is_none() {
        return None;
    }
    let stamp = std::fs::metadata(slack_user::sign_in_file(state))
        .and_then(|m| m.modified())
        .ok();
    if let Some((at, read, client)) = KEPT.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
        && *at == stamp
        && read.elapsed() < TOKEN_KEPT
    {
        return Some(Ok(client.clone()));
    }
    let client = slack_user::stored().await;
    if let Ok(c) = &client {
        *KEPT.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((stamp, std::time::Instant::now(), c.clone()));
    }
    Some(client)
}

/// `client`'s answer as a client to read with: a token that couldn't be
/// read is said (`failed`) and read through the connector instead.
pub fn usable(state: &Path, token: Option<Result<Client, String>>) -> Option<Client> {
    match token? {
        Ok(client) => {
            worked(state, Part::Token);
            Some(client)
        }
        Err(why) => {
            failed(state, Part::Token, &why);
            None
        }
    }
}

/// Lets go of what was said of threads no longer kept (`threads`, by
/// key) and of files no kept thread holds (`files`, by ID), which nothing
/// will read again to clear it, so the file stays as small as the view.
pub fn forget_gone(
    state: &Path,
    threads: &std::collections::HashSet<String>,
    files: &std::collections::HashSet<String>,
) {
    change_note(state, |kept| {
        for (part, keys) in [(Part::Threads, threads), (Part::Files, files)] {
            let now_empty = kept
                .get_mut(part.key())
                .and_then(Value::as_object_mut)
                .map(|each| {
                    each.retain(|k, _| keys.contains(k));
                    each.is_empty()
                });
            if now_empty == Some(true) {
                kept.remove(part.key());
            }
        }
    });
}

/// Which read failed: the messages (the look, a thread, the search) or a
/// file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// The stored token, read from the Keychain.
    Token,
    /// The server's look, apart from the rest, since whether it works says
    /// whether the connector is still needed in the background.
    Look,
    Threads,
    Search,
    Files,
}

impl Part {
    fn key(self) -> &'static str {
        match self {
            Part::Token => "token",
            Part::Look => "look",
            Part::Threads => "threads",
            Part::Search => "search",
            Part::Files => "files",
        }
    }
}

fn note_file(state: &Path) -> PathBuf {
    state.join("slack-token.json")
}

/// Keeps why a read through the token failed, so the view says it.
pub fn failed(state: &Path, part: Part, why: &str) {
    change_note(state, |kept| {
        kept.insert(part.key().into(), json!(why));
    });
}

/// A thread (by its key) or a file (by its ID) couldn't be read through
/// the token: kept apart from the others of its part, so one that reads
/// can't clear it.
pub fn one_failed(state: &Path, part: Part, key: &str, why: &str) {
    change_note(state, |kept| {
        let each = kept.entry(part.key()).or_insert_with(|| json!({}));
        if !each.is_object() {
            *each = json!({});
        }
        each[key] = json!(why);
    });
}

/// Whether a failure is Slack saying the token lacks a scope, in the words
/// `slack_user::api` says it, which a file's name can't hold.
pub fn needs_scope(why: &str) -> bool {
    why.starts_with("your Slack sign-in lacks the ")
}

/// Whether what was said of one thread or file is a scope it lacks.
pub fn one_needs_scope(state: &Path, part: Part, key: &str) -> bool {
    util::read_json_object(note_file(state))
        .get(part.key())
        .and_then(|each| each.get(key))
        .and_then(Value::as_str)
        .is_some_and(needs_scope)
}

/// Whether a thread's last token read hit Slack's rate limit.
pub fn one_says_rate_limit(state: &Path, key: &str) -> bool {
    util::read_json_object(note_file(state))
        .get(Part::Threads.key())
        .and_then(|each| each.get(key))
        .and_then(Value::as_str)
        .is_some_and(|why| why.starts_with(slack_user::RATE_LIMIT))
}

/// A thread (by its key) or a file (by its ID) was read through the token.
pub fn one_worked(state: &Path, part: Part, key: &str) {
    change_note(state, |kept| {
        let now_empty = kept
            .get_mut(part.key())
            .and_then(Value::as_object_mut)
            .map(|each| {
                each.remove(key);
                each.is_empty()
            });
        if now_empty != Some(false) {
            kept.remove(part.key());
        }
    });
}

/// A read through the token worked: what was said of its part goes.
pub fn worked(state: &Path, part: Part) {
    change_note(state, |kept| {
        kept.remove(part.key());
        if part == Part::Look {
            kept.remove(LOOK_UNTIL);
        }
    });
}

/// Where the time Slack asked the look to wait until is kept.
const LOOK_UNTIL: &str = "look_until";

/// The look's read was refused for Slack's rate limit, which asked it to
/// wait until `until`, kept from when Slack answered.
pub fn look_waits_until(state: &Path, until: i64) {
    change_note(state, |kept| {
        kept.insert(LOOK_UNTIL.into(), json!(until));
    });
}

fn change_note(state: &Path, change: impl FnOnce(&mut serde_json::Map<String, Value>)) {
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let file = note_file(state);
    let mut kept = util::read_json_object(&file);
    let before = kept.clone();
    change(&mut kept);
    if kept == before {
        return;
    }
    if kept.is_empty() {
        let _ = std::fs::remove_file(&file);
    } else if std::fs::create_dir_all(state).is_ok() {
        let _ = util::write_atomic(&file, util::json_compact(&Value::Object(kept)));
    }
}

/// Whether the look reads through the token now: it hasn't failed since
/// it last worked.
pub fn look_works(state: &Path) -> bool {
    let kept = util::read_json_object(note_file(state));
    !kept.contains_key(Part::Look.key()) && !kept.contains_key(Part::Token.key())
}

/// When Slack asked the look to wait until, while its last read through
/// the token was refused for Slack's rate limit; `None` otherwise. A small
/// file, read off the server's task as the look ends.
pub fn look_slowed(state: &Path) -> Option<i64> {
    let kept = util::read_json_object(note_file(state));
    let refused = kept
        .get(Part::Look.key())
        .and_then(Value::as_str)
        .is_some_and(|why| why.starts_with(slack_user::RATE_LIMIT));
    kept.get(LOOK_UNTIL)
        .and_then(Value::as_i64)
        .filter(|_| refused)
}

/// What the view says under its header while a read through the token
/// fails, from a small file; `None` while none does.
/// `threads` is whether threads and pictures are read at all (Settings'
/// `slack_threads`), `look` whether the server looks itself
/// (`slack_background`) and `search` whether search everything asks Slack
/// (`search_slack`): off, nothing is said of theirs.
pub fn note(state: &Path, threads: bool, look: bool, search: bool) -> Option<String> {
    note_of(state, threads, look, search, false)
}

/// `note`, leaving out the look's own rate limit, which the view says as
/// Slack asking the look to slow down, so another read's reason still shows.
pub fn note_while_slowed(state: &Path, threads: bool, look: bool, search: bool) -> Option<String> {
    note_of(state, threads, look, search, true)
}

fn note_of(state: &Path, threads: bool, look: bool, search: bool, slowed: bool) -> Option<String> {
    let kept = util::read_json_object(note_file(state));
    let said = |k: &str| match kept.get(k) {
        Some(Value::String(why))
            if slowed && k == Part::Look.key() && why.starts_with(slack_user::RATE_LIMIT) =>
        {
            None
        }
        Some(Value::String(why)) => Some(why.clone()),
        // Each thread's or file's: a missing scope first, as what it needs
        // is said in a word, else any one of them.
        Some(Value::Object(by_key)) => {
            let all: Vec<&str> = by_key.values().filter_map(Value::as_str).collect();
            all.iter()
                .find(|why| needs_scope(why))
                .or(all.first())
                .map(|why| why.to_string())
        }
        _ => None,
    };
    let mut out = Vec::new();
    // A missing scope first, as what it needs is said in a word.
    let reads: Vec<String> = ["token", "look", "threads", "search"]
        .iter()
        .filter(|k| threads || **k != "threads")
        .filter(|k| look || **k != "look")
        .filter(|k| search || **k != "search")
        .filter_map(|k| said(k))
        .collect();
    if let Some(why) = reads.iter().find(|why| needs_scope(why)).or(reads.first()) {
        out.push(format!("Slack read through the connector: {why}"));
    }
    if let Some(why) = said("files").filter(|_| threads) {
        out.push(if needs_scope(&why) && why.contains("files:read") {
            format!(
                "Add files:read to your Slack app for pictures, then run {}",
                slack_user::SETUP
            )
        } else {
            format!("Pictures through the connector: {why}")
        });
    }
    // What needs a scope first, whichever read it was, so the one line
    // drawn says what to add.
    out.sort_by_key(|said| {
        !(said.contains("your Slack sign-in lacks the ") || said.starts_with("Add files:read"))
    });
    (!out.is_empty()).then(|| out.join(" · "))
}

/// What a person is called, and their handle, by member ID, as
/// `users.info` said, for the server's life. Slack refusing the token or a
/// scope (`users:read`), or out of reach, fails the read, so it goes
/// through the connector and says why; a person Slack doesn't know is
/// named by their ID.
async fn person(client: &Client, user: &str, source: &str) -> Result<(String, String), String> {
    static KNOWN: LazyLock<Mutex<HashMap<String, (String, String)>>> =
        LazyLock::new(Mutex::default);
    let key = format!("{}:{user}", client.base());
    if let Some(known) = KNOWN.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return Ok(known.clone());
    }
    if !super::slack::is_channel(user) {
        return Ok((user.to_string(), String::new()));
    }
    let said = match api(client, true, "users.info", &json!({ "user": user }), source).await {
        Ok(said) => said,
        // Slack saying it has no such person to show, not kept, so the next
        // read asks again.
        Err(why)
            if why == "users.info: user_not_found" || why == "users.info: user_not_visible" =>
        {
            return Ok((user.to_string(), String::new()));
        }
        Err(why) => return Err(why),
    };
    let text = |p: &str| {
        said.pointer(p)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let handle = text("/user/name").unwrap_or_default();
    let name = text("/user/profile/display_name")
        .or_else(|| text("/user/real_name"))
        .or_else(|| (!handle.is_empty()).then(|| handle.clone()))
        .unwrap_or_else(|| user.to_string());
    let known = (name, handle);
    KNOWN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, known.clone());
    Ok(known)
}

/// A message's text with each bare mention (`<@U1>`, as the Web API writes
/// one) given its person's name (`<@U1|ada>`, as the connector writes one),
/// so it reads as a name and still says whom it mentions.
async fn labeled(client: &Client, text: &str, source: &str) -> Result<String, String> {
    static BARE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"<@([UW][A-Z0-9]+)>").unwrap());
    let mut out = String::new();
    let mut last = 0;
    for c in BARE.captures_iter(text) {
        let whole = c.get(0).expect("a match");
        let name = person(client, &c[1], source).await?.0;
        out.push_str(&text[last..whole.start()]);
        out.push_str(&format!("<@{}|{}>", &c[1], name.replace(['>', '|'], "")));
        last = whole.end();
    }
    out.push_str(&text[last..]);
    Ok(out)
}

/// A message's thread, from its permalink's `thread_ts`.
fn thread_of(link: &str) -> Option<String> {
    let query = link.split_once('?')?.1;
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == "thread_ts" && super::slack::is_ts(v)).then(|| v.to_string())
    })
}

/// How a search reads: `count` results a page, at most `pages` pages,
/// back to when (`back_to`, seconds), and whether with the messages around
/// each (`context`).
#[derive(Clone, Copy, Debug)]
pub struct How {
    pub count: u32,
    pub pages: usize,
    pub back_to: Option<f64>,
    pub context: bool,
}

/// Slack's search (`search.messages`) for `query`, newest first, up to
/// `pages` pages of `count`, each result as the connector's reader gives it: where it
/// is (`#channel`, `DM` or `Group DM`, with a DM's people), who wrote it,
/// its thread and permalink, and, with `context`, the messages Slack gives
/// around it. `me`
/// is left out of a group DM's people. With `back_to` (seconds), the
/// pages are read until one reaches a message older than it, the window
/// a look needs, and the pages running out first fails the read, since what
/// is left would be missed.
pub async fn search(
    client: &Client,
    query: &str,
    me: &str,
    how: How,
    source: &str,
) -> Result<Vec<Found>, String> {
    let How {
        count,
        pages,
        back_to,
        context,
    } = how;
    let my_handle = if me.is_empty() {
        String::new()
    } else {
        person(client, me, source).await?.1
    };
    let mut out = Vec::new();
    for page in 1..=pages.max(1) {
        let said = api(
            client,
            true,
            "search.messages",
            &json!({
                "query": query, "sort": "timestamp", "sort_dir": "desc",
                "count": count, "page": page,
            }),
            source,
        )
        .await?;
        let matches = said
            .pointer("/messages/matches")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut reached = false;
        for m in &matches {
            let at = m
                .get("ts")
                .and_then(Value::as_str)
                .and_then(|t| t.parse::<f64>().ok());
            // Newest first: what is older than the window isn't wanted, so
            // nothing more is asked about it.
            if back_to.zip(at).is_some_and(|(back, at)| at < back) {
                reached = true;
                continue;
            }
            if let Some(found) = found_of(client, m, &my_handle, context, source).await? {
                out.push(found);
            }
        }
        let last = said
            .pointer("/messages/paging/pages")
            .and_then(Value::as_u64)
            .unwrap_or(1);
        if matches.is_empty() || page as u64 >= last || reached {
            return Ok(out);
        }
    }
    if back_to.is_some() {
        return Err(format!(
            "over {} messages since the last look, more than a look reads",
            pages * count as usize
        ));
    }
    Ok(out)
}

async fn found_of(
    client: &Client,
    m: &Value,
    my_handle: &str,
    context: bool,
    source: &str,
) -> Result<Option<Found>, String> {
    let text = |p: &str| m.pointer(p).and_then(Value::as_str).unwrap_or("");
    let flag = |p: &str| m.pointer(p) == Some(&Value::Bool(true));
    let channel = text("/channel/id");
    let ts = text("/ts");
    if !super::slack::is_channel(channel) || !super::slack::is_ts(ts) {
        return Ok(None);
    }
    let name = text("/channel/name");
    let user = text("/user");
    let bot = !text("/bot_id").is_empty() || text("/subtype") == "bot_message";
    let from = if user.is_empty() {
        text("/username").to_string()
    } else {
        person(client, user, source).await?.0
    };
    let (r#where, participants) = if flag("/channel/is_im") {
        // A DM's name is the other person's member ID.
        let other = person(client, name, source).await?.0;
        ("DM".to_string(), vec![(other, name.to_string())])
    } else if flag("/channel/is_mpim") {
        // `mpdm-ada--bo--cy-1`: the handles of everyone in it.
        let handles = name
            .strip_prefix("mpdm-")
            .and_then(|n| n.rsplit_once('-'))
            .map_or(name, |(n, _)| n);
        let people = handles
            .split("--")
            .filter(|h| !h.is_empty() && *h != my_handle)
            .map(|h| (h.to_string(), String::new()))
            .collect();
        ("Group DM".to_string(), people)
    } else {
        (format!("#{name}"), Vec::new())
    };
    let link = Some(text("/permalink").to_string()).filter(|l| l.starts_with("https://"));
    let around = |keys: &'static [&'static str]| async move {
        let mut lines = Vec::new();
        for k in keys {
            if let Some(t) = m.pointer(&format!("/{k}/text")).and_then(Value::as_str) {
                let t = labeled(client, t, source).await?;
                lines.extend(t.lines().map(str::to_string));
            }
        }
        Ok::<_, String>(lines)
    };
    // Only search everything's preview shows them; the look leaves them.
    let (before, after) = if context {
        (
            around(&["previous_2", "previous"]).await?,
            around(&["next", "next_2"]).await?,
        )
    } else {
        Default::default()
    };
    Ok(Some(Found {
        channel: channel.to_string(),
        r#where,
        user: user.to_string(),
        from,
        bot,
        participants,
        ts: ts.to_string(),
        thread: link.as_deref().and_then(thread_of),
        link,
        text: labeled(client, text("/text").trim(), source).await?,
        before,
        after,
    }))
}

/// A thread whole (`conversations.replies`), as `slack_thread` keeps one.
pub async fn thread(
    client: &Client,
    channel: &str,
    ts: &str,
    now: i64,
    source: &str,
) -> Result<Thread, String> {
    if !super::slack::is_channel(channel) || !super::slack::is_ts(ts) {
        return Err("that isn't a thread Slack listed".into());
    }
    let mut messages: Vec<Said> = Vec::new();
    let mut replies = 0;
    let mut cursor = String::new();
    for _ in 0..THREAD_PAGES {
        let mut params = json!({ "channel": channel, "ts": ts, "limit": THREAD_PAGE });
        if !cursor.is_empty() {
            params["cursor"] = json!(cursor);
        }
        let said = api(client, true, "conversations.replies", &params, source).await?;
        for m in said
            .get("messages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let text = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or("");
            let at = text("ts");
            if !super::slack::is_ts(at) || messages.iter().any(|had| had.ts == at) {
                continue;
            }
            if at == ts {
                replies = m.get("reply_count").and_then(Value::as_u64).unwrap_or(0) as u32;
            }
            let user = text("user");
            let from = if user.is_empty() {
                m.pointer("/bot_profile/name")
                    .and_then(Value::as_str)
                    .unwrap_or(text("username"))
                    .to_string()
            } else {
                person(client, user, source).await?.0
            };
            let raw = labeled(client, text("text"), source).await?;
            let raw = raw.as_str();
            let files = m
                .get("files")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|f| {
                    let get = |k: &str| f.get(k).and_then(Value::as_str).map(str::to_string);
                    Some(SlackFile {
                        id: get("id")?,
                        name: get("name").unwrap_or_default(),
                        mimetype: get("mimetype").unwrap_or_default(),
                    })
                })
                .collect();
            messages.push(Said {
                user: user.to_string(),
                from,
                text: plain(raw.trim()),
                ts: at.to_string(),
                files,
                refs: crate::tickets::linear::ids_in(raw),
            });
        }
        cursor = said
            .pointer("/response_metadata/next_cursor")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        // A cursor always goes on; with none, Slack saying there is more
        // is a thread cut short.
        let more = !cursor.is_empty() || said.get("has_more") == Some(&Value::Bool(true));
        if more && cursor.is_empty() {
            return Err("Slack said the thread goes on but not where".into());
        }
        if !more {
            cursor.clear();
            break;
        }
    }
    // Pages left past the last read: read whole through the connector.
    if !cursor.is_empty() {
        return Err(format!("the thread is longer than {THREAD_PAGES} pages"));
    }
    if messages.is_empty() {
        return Err("no messages in the thread".into());
    }
    messages.sort_by(|a, b| {
        let at = |s: &Said| s.ts.parse::<f64>().unwrap_or(0.0);
        at(a).total_cmp(&at(b))
    });
    if replies == 0 {
        replies = messages.len().saturating_sub(1) as u32;
    }
    Ok(Thread {
        messages,
        replies,
        read: now,
    })
}

/// Whether a file's address is one of Slack's over HTTPS (or, in a test,
/// on the Web API's own origin), so the token is sent nowhere else.
/// It is read as the HTTPS client reads it, so a backslash or a `@` can't
/// make it name one host here and another there.
fn slacks(url: &str, base: &str) -> bool {
    let (Ok(url), Ok(base)) = (url::Url::parse(url), url::Url::parse(base)) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    (url.scheme() == "https" && slack_host(host)) || url.origin() == base.origin()
}

fn slack_host(host: &str) -> bool {
    host == "slack.com" || host.ends_with(".slack.com")
}

/// A file's bytes (`files.info`, then its private download address), at
/// most `largest` of them. Needs the `files:read` scope.
pub async fn file(
    client: &Client,
    f: &SlackFile,
    largest: usize,
    source: &str,
) -> Result<Vec<u8>, String> {
    let said = api(client, true, "files.info", &json!({ "file": f.id }), source).await?;
    let url = ["/file/url_private_download", "/file/url_private"]
        .iter()
        .find_map(|p| said.pointer(p).and_then(Value::as_str))
        .ok_or_else(|| format!("Slack gave no address for {}", f.name))?;
    if !slacks(url, client.base()) {
        return Err(format!("{} isn't kept on Slack", f.name));
    }
    let bearer = format!("Bearer {}", client.token());
    let got = net::get_file_following(
        url,
        &[("Authorization", bearer.as_str())],
        FILE_TIMEOUT,
        Audit {
            service: "Slack",
            protocol: "REST",
            source,
        },
        slack_host,
        largest,
    )
    .await
    .map_err(|e| e.to_string())?;
    if got.status != 200 {
        return Err(format!("Slack answered {} for {}", got.status, f.name));
    }
    Ok(got.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn while_slowed_the_looks_rate_limit_leaves_room_for_another_reason() {
        let state = std::env::temp_dir().join(format!(
            "sb-slack-api-slowed-{}-{}",
            std::process::id(),
            util::now_ms()
        ));
        failed(&state, Part::Look, slack_user::RATE_LIMIT);
        look_waits_until(&state, 5_000);
        assert_eq!(look_slowed(&state), Some(5_000));
        assert_eq!(note_while_slowed(&state, true, true, true), None);
        one_failed(
            &state,
            Part::Threads,
            "C1:1.0",
            "Slack refused your sign-in",
        );
        assert_eq!(
            note_while_slowed(&state, true, true, true).as_deref(),
            Some("Slack read through the connector: Slack refused your sign-in")
        );
        worked(&state, Part::Look);
        assert_eq!(
            look_slowed(&state),
            None,
            "a look that read lets the wait go"
        );
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn what_failed_is_said_until_a_read_works_again() {
        let state = std::env::temp_dir().join(format!(
            "sb-slack-api-note-{}-{}",
            std::process::id(),
            util::now_ms()
        ));
        assert_eq!(note(&state, true, true, true), None);
        one_failed(
            &state,
            Part::Threads,
            "C1:1.0",
            "Slack refused your sign-in",
        );
        one_failed(&state, Part::Threads, "C2:1.0", "no groups:history");
        one_failed(
            &state,
            Part::Files,
            "F1",
            "your Slack sign-in lacks the files:read scope: add it",
        );
        let said = note(&state, true, true, true).unwrap();
        assert!(said.contains("Slack refused your sign-in"), "{said}");
        assert!(
            said.starts_with("Add files:read to your Slack app for pictures"),
            "{said}"
        );
        one_worked(&state, Part::Threads, "C1:1.0");
        assert!(
            note(&state, true, true, true)
                .unwrap()
                .contains("groups:history"),
            "another thread's stays"
        );
        one_worked(&state, Part::Threads, "C2:1.0");
        assert!(!note(&state, true, true, true).unwrap().contains("refused"));
        one_worked(&state, Part::Files, "F1");
        assert_eq!(note(&state, true, true, true), None);
        assert!(!note_file(&state).exists());
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn the_token_goes_only_to_slack() {
        assert!(!slacks(
            "https://evil.example\\.slack.com/a.png",
            "https://slack.com/api/"
        ));
        assert!(!slacks(
            "https://files.slack.com@evil.example/a.png",
            "https://slack.com/api/"
        ));
        assert!(slacks(
            "https://files.slack.com/files-pri/T1-F1/a.png",
            "https://slack.com/api/"
        ));
        assert!(!slacks(
            "https://files.slack.com.evil.example/a.png",
            "https://slack.com/api/"
        ));
        assert!(!slacks(
            "http://files.slack.com/a.png",
            "https://slack.com/api/"
        ));
        assert!(slacks(
            "http://127.0.0.1:9/files/F1",
            "http://127.0.0.1:9/api/"
        ));
        assert!(!slacks(
            "http://127.0.0.1:8/files/F1",
            "http://127.0.0.1:9/api/"
        ));
    }

    #[cfg(feature = "net")]
    #[test]
    fn the_search_a_thread_and_a_file_are_read_through_the_token() {
        use crate::fake_http::{Fake, Reply, Service};
        let fake = Fake::start(
            Service::Slack {
                token: "xoxp-test".into(),
            },
            |request| {
                let method = request.path().trim_start_matches("/api/");
                let q = request.target.clone();
                let body = match method {
                    "users.info" if q.contains("user=U0ME") => {
                        json!({ "ok": true, "user": { "name": "me", "real_name": "Me" } })
                    }
                    "users.info" if q.contains("user=UDEE") => {
                        json!({ "ok": true, "user": { "name": "dee", "real_name": "Dee Dee", "profile": { "display_name": "Dee" } } })
                    }
                    "search.messages" => json!({ "ok": true, "messages": {
                        "paging": { "pages": 1 },
                        "matches": [
                            { "channel": { "id": "DDEE", "is_im": true, "name": "UDEE" },
                              "user": "UDEE", "ts": "1793282136.000100", "text": "did you see <@U0ME>?",
                              "permalink": "https://x.slack.com/archives/DDEE/p1793282136000100?thread_ts=1793282000.000100&cid=DDEE" },
                            { "channel": { "id": "GMP", "is_mpim": true, "name": "mpdm-me--dee--bo-1" },
                              "user": "UDEE", "ts": "1793282137.000100", "text": "all of us",
                              "permalink": "https://x.slack.com/archives/GMP/p1793282137000100" },
                            { "channel": { "id": "CGEN", "name": "general" },
                              "username": "deploybot", "bot_id": "B1", "ts": "1793282138.000100", "text": "deployed",
                              "permalink": "https://x.slack.com/archives/CGEN/p1793282138000100" },
                            { "channel": { "id": "C;rm", "name": "forged" }, "ts": "1.0", "text": "left out" },
                        ] } }),
                    "conversations.replies" => json!({ "ok": true, "has_more": false, "messages": [
                        { "user": "UDEE", "ts": "1793282000.000100", "reply_count": 1,
                          "text": "see <https://linear.app/x/issue/ABC-7/a|the bug>",
                          "files": [{ "id": "F1", "name": "shot.png", "mimetype": "image/png" }] },
                        { "bot_profile": { "name": "Relay" }, "ts": "1793282136.000100", "text": "on it" },
                    ] }),
                    "files.info" if q.contains("file=F1") => json!({ "ok": true, "file": {
                        "url_private_download": format!("http://{}/api/files/F1", request.header("host").unwrap_or("")) } }),
                    "files.info" => {
                        json!({ "ok": false, "error": "missing_scope", "needed": "files:read" })
                    }
                    "files/F1" => {
                        assert_eq!(request.header("authorization"), Some("Bearer xoxp-test"));
                        return Some(Reply::bytes(
                            200,
                            "image/png",
                            b"\x89PNG\r\n\x1a\nbytes".to_vec(),
                        ));
                    }
                    _ => return None,
                };
                Some(Reply::json(200, body))
            },
        );
        let client = Client::new(format!("{}/api/", fake.url), "xoxp-test".into());
        util::block_on(async {
            let found = search(
                &client,
                "after:2026-11-01",
                "U0ME",
                How {
                    count: 100,
                    pages: 3,
                    back_to: None,
                    context: true,
                },
                "test",
            )
            .await
            .unwrap();
            assert_eq!(found.len(), 3, "{found:?}");
            assert_eq!(
                (found[0].r#where.as_str(), found[0].from.as_str()),
                ("DM", "Dee")
            );
            assert_eq!(
                found[0].participants,
                [("Dee".to_string(), "UDEE".to_string())]
            );
            assert_eq!(found[0].thread.as_deref(), Some("1793282000.000100"));
            assert_eq!(
                found[0].text, "did you see <@U0ME|Me>?",
                "a bare mention named"
            );
            assert_eq!(found[1].r#where, "Group DM");
            assert_eq!(
                found[1]
                    .participants
                    .iter()
                    .map(|p| p.0.as_str())
                    .collect::<Vec<_>>(),
                ["dee", "bo"],
                "your own handle left out"
            );
            assert_eq!(
                (
                    found[2].r#where.as_str(),
                    found[2].from.as_str(),
                    found[2].bot
                ),
                ("#general", "deploybot", true)
            );
            let t = thread(&client, "DDEE", "1793282000.000100", 7, "test")
                .await
                .unwrap();
            assert_eq!((t.replies, t.read, t.messages.len()), (1, 7, 2));
            assert_eq!(t.messages[0].text, "see the bug");
            assert_eq!(t.messages[0].refs, ["ABC-7"]);
            assert_eq!(t.messages[1].from, "Relay");
            let f = &t.messages[0].files[0];
            assert_eq!(
                file(&client, f, 1024, "test").await.unwrap(),
                b"\x89PNG\r\n\x1a\nbytes"
            );
            let other = SlackFile {
                id: "F2".into(),
                name: "b.png".into(),
                mimetype: "image/png".into(),
            };
            let why = file(&client, &other, 1024, "test").await.unwrap_err();
            assert!(why.contains("files:read"), "{why}");
        });
        // Each name asked once, however often it comes.
        let asked = fake
            .seen()
            .iter()
            .filter(|r| r.path() == "/api/users.info")
            .count();
        assert_eq!(asked, 2);
    }

    #[cfg(feature = "net")]
    #[test]
    fn a_name_slack_refuses_or_a_look_past_its_pages_fails_the_read() {
        use crate::fake_http::{Fake, Reply, Service};
        let fake = Fake::start(
            Service::Slack {
                token: "xoxp-test".into(),
            },
            |request| {
                let body = match request.path().trim_start_matches("/api/") {
                    "users.info" if request.target.contains("user=UGONE") => {
                        json!({ "ok": false, "error": "user_not_found" })
                    }
                    "users.info" => {
                        json!({ "ok": false, "error": "missing_scope", "needed": "users:read" })
                    }
                    "search.messages" => json!({ "ok": true, "messages": {
                        "paging": { "pages": 9 },
                        "matches": [{ "channel": { "id": "C1", "name": "dev" }, "user": "UGONE",
                                      "ts": "1.0", "text": "hi" }] } }),
                    _ => return None,
                };
                Some(Reply::json(200, body))
            },
        );
        let client = Client::new(format!("{}/api/", fake.url), "xoxp-test".into());
        util::block_on(async {
            let why = search(
                &client,
                "x",
                "UME",
                How {
                    count: 10,
                    pages: 2,
                    back_to: None,
                    context: false,
                },
                "test",
            )
            .await
            .unwrap_err();
            assert!(why.contains("users:read"), "{why}");
            let found = search(
                &client,
                "x",
                "",
                How {
                    count: 10,
                    pages: 2,
                    back_to: None,
                    context: false,
                },
                "test",
            )
            .await
            .unwrap();
            assert_eq!(found[0].from, "UGONE", "someone Slack doesn't know, by ID");
            let why = search(
                &client,
                "x",
                "",
                How {
                    count: 10,
                    pages: 2,
                    back_to: Some(0.5),
                    context: false,
                },
                "test",
            )
            .await
            .unwrap_err();
            assert!(why.contains("over 20 messages"), "{why}");
            let found = search(
                &client,
                "x",
                "",
                How {
                    count: 10,
                    pages: 2,
                    back_to: Some(2.0),
                    context: false,
                },
                "test",
            )
            .await
            .unwrap();
            assert!(
                found.is_empty(),
                "a page reaching the window's start ends it, what is older left out"
            );
        });
        let searches = fake
            .seen()
            .iter()
            .filter(|r| r.path() == "/api/search.messages")
            .count();
        assert_eq!(
            searches,
            2 + 2 + 1,
            "a refused name fails before any search"
        );
    }
}
